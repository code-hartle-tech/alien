//! The high-level device handle — what the CLI, TUI and GUI all talk to.
//!
//! Everything below is a thin, honest wrapper: it encodes the payload, issues
//! the call, and reports what the firmware said. It deliberately does **not**
//! verify that the hardware obeyed, because verification differs per control
//! (fans are checked by RPM, RGB by eye) and a wrapper that guessed would be
//! the third layer of "returned success, changed nothing" in this stack.
//! Callers that need proof use [`Device::sensors`] and compare.

use crate::performance::{GpuMode, GpuModeOptIn, GpuModeState};
use crate::rgb::{self, Colour, Direction, Effect, Zone};
#[cfg(unix)]
use crate::socket::SocketClient;
#[cfg(unix)]
use crate::transport::AcpiCall;
use crate::transport::{sensor_u16, KeyboardTimeoutState, Transport, TransportError};
use crate::wmi::{
    fan_speed_word, misc_word, Fan, FanBehaviour, FanMode, Function, OverclockTarget, Sensor,
    Status, OC_OFF, OC_TURBO,
};

pub struct Device {
    transport: Box<dyn Transport>,
    /// PredatorSense primes the Covini lighting path once at startup with the
    /// saved function-2 zone mask, while suppressing effect/colour writes.
    /// Keep that process-lifetime prerequisite explicit and idempotent.
    lighting_prepared: std::sync::atomic::AtomicBool,
    lighting_prepare_lock: std::sync::Mutex<()>,
    /// Per-key state is cumulative: the controller takes whole frames, so
    /// colouring one key means resending every key. Held here so a caller can
    /// set keys one at a time without each call wiping the previous one.
    key_frame: std::sync::Mutex<crate::perkey::KeyFrame>,
}

/// Backlight state as the firmware reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BacklightState {
    pub effect: Effect,
    pub speed: u8,
    pub brightness: u8,
    pub reverse: bool,
    pub colour: Colour,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sensors {
    pub cpu_temp_c: Option<u16>,
    pub gpu_temp_c: Option<u16>,
    /// Board / chassis sensor, not either processor.
    pub system_temp_c: Option<u16>,
    pub cpu_fan_rpm: Option<u16>,
    pub gpu_fan_rpm: Option<u16>,
}

#[derive(Debug)]
pub enum Error {
    Transport(TransportError),
    /// The firmware accepted the call and returned a rejection status.
    Rejected(Status),
    /// The caller asked for something the firmware has no way to express.
    Unsupported(&'static str),
    /// User-authored state failed validation before any hardware call.
    Invalid(String),
    /// Hardware changed, but related local state could not be persisted.
    State(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(e) => write!(f, "{e}"),
            Error::Rejected(s) => write!(f, "firmware {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Invalid(s) => write!(f, "invalid input: {s}"),
            Error::State(s) => write!(f, "state error: {s}"),
        }
    }
}

impl Error {
    /// Whether this means "the transport went away" rather than "the firmware
    /// said no".
    ///
    /// Only a long-running client cares. A one-shot CLI invocation prints the
    /// message and exits either way, but a GUI polling once a second has to
    /// tell the two apart: one calls for reconnecting, the other for reporting
    /// and carrying on.
    pub fn is_link_lost(&self) -> bool {
        matches!(self, Error::Transport(TransportError::Io(_)))
    }
}

impl std::error::Error for Error {}

impl From<TransportError> for Error {
    fn from(e: TransportError) -> Self {
        Error::Transport(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn require_socket(value: Option<&std::ffi::OsStr>) -> std::result::Result<bool, String> {
    match value {
        None => Ok(false),
        Some(value) if value == "0" => Ok(false),
        Some(value) if value == "1" => Ok(true),
        Some(value) => Err(format!(
            "ALIEN_REQUIRE_SOCKET must be unset, 0, or 1 (got {value:?})"
        )),
    }
}

impl Device {
    /// Open the machine's gaming interface, preferring the daemon.
    ///
    /// Daemon first, direct second, and in that order for a reason: if
    /// `alien-daemon` is running it owns `/proc/acpi/call`, and a second
    /// process writing the same global buffer would interleave with it — both
    /// sides then read each other's answers with no error anywhere. Falling
    /// back to direct access is only correct when nothing else is using it.
    ///
    /// `ALIEN_REQUIRE_SOCKET=1` is an audit/QA guard: if the daemon connection
    /// fails, return that socket error immediately and never probe or open the
    /// direct ACPI transport. Unset or `0` retains normal fallback; any other
    /// value is rejected so a typo cannot silently weaken the guard.
    #[cfg(unix)]
    pub fn open() -> Result<Device> {
        let socket_only = require_socket(std::env::var_os("ALIEN_REQUIRE_SOCKET").as_deref())
            .map_err(Error::Invalid)?;
        match SocketClient::connect() {
            Ok(c) => Ok(Device::with_transport(Box::new(c))),
            Err(sock_err) if socket_only => Err(Error::Transport(sock_err)),
            Err(sock_err) => match AcpiCall::detect() {
                Ok(direct) => Ok(Device::with_transport(Box::new(direct))),
                // Report the direct-access failure, not the socket one: if the
                // daemon is not installed, "no such file /run/alien/alien.sock"
                // is noise, whereas "acpi_call not loaded" is the real problem.
                // The socket error is appended only when it is not a plain
                // absence, because a *running but broken* daemon is worth
                // saying out loud.
                Err(direct_err) => Err(Error::Transport(match sock_err {
                    TransportError::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
                        direct_err
                    }
                    other => TransportError::AcpiFailure(format!(
                        "daemon unusable ({other}); direct access also failed ({direct_err})"
                    )),
                })),
            },
        }
    }

    /// Not available on this platform.
    ///
    /// `open` exists to choose between the broker socket and direct
    /// `/proc/acpi/call`, and Windows has neither: firmware there is reached by
    /// invoking the same ACPI-WMI methods through COM, which needs no broker
    /// because the AML declares them `Serialized`. A Windows build constructs
    /// its transport explicitly and passes it to [`Device::with_transport`].
    #[cfg(not(unix))]
    pub fn open() -> Result<Device> {
        Err(Error::Unsupported(
            "Device::open is POSIX-only; construct a platform transport and use \
             Device::with_transport",
        ))
    }

    /// Drive a specific transport. For tests, and for a client that must not
    /// silently fall back to direct access.
    pub fn with_transport(t: Box<dyn Transport>) -> Device {
        Device {
            transport: t,
            lighting_prepared: std::sync::atomic::AtomicBool::new(false),
            lighting_prepare_lock: std::sync::Mutex::new(()),
            key_frame: std::sync::Mutex::new(Default::default()),
        }
    }

    /// What this machine can actually do.
    ///
    /// Getter-only probing, so it is safe to call at startup. A UI should
    /// drive its enabled/disabled states from this rather than assuming the
    /// reference model's feature set.
    pub fn capabilities(&self) -> crate::capability::Capabilities {
        crate::capability::probe(self.transport.as_ref())
    }

    /// Experimental source-mapped per-key transport.
    ///
    /// Returns [`Error::Unsupported`] on four-zone keyboards, which is most
    /// Predators — their keys are wired into four banks with no per-key
    /// addressing, so this is a hardware limit and no software can work
    /// around it.
    pub fn set_key(&self, key: &str, colour: Colour) -> Result<()> {
        if crate::perkey::detect().is_none() {
            return Err(Error::Unsupported(
                "no per-key controller (ITE 8291) on this machine — this keyboard is \
                 four-zone, so individual keys cannot be addressed",
            ));
        }
        let (row, col) = crate::perkey::key_position(key)
            .ok_or(Error::Unsupported("unknown key name; try `alien rgb keys`"))?;
        let mut frame = self.key_frame.lock().unwrap_or_else(|p| p.into_inner());
        frame.set(row, col, colour);
        crate::perkey::send(&frame).map_err(|error| {
            Error::State(format!(
                "experimental per-key ITE transport failed: {error}; this path is \
                 hardware-unverified and packaged frontends intentionally have no hidraw access"
            ))
        })
    }

    /// How this handle reaches the firmware.
    pub fn method_path(&self) -> String {
        self.transport.describe()
    }

    // ── Telemetry ───────────────────────────────────────────────────────────

    fn read_sensor(&self, s: Sensor) -> Result<Option<u16>> {
        let resp = self
            .transport
            .call_bytes(Function::GetSysInfo as u32, &[0x01, s as u8])?;
        // A sensor that is not populated reads back as zero rather than
        // failing, so treat zero as absent — no fan on this chassis idles at
        // 0 RPM while the machine is powered, and 0 °C is not a real reading.
        Ok(sensor_u16(&resp).filter(|v| *v != 0))
    }

    /// Read the complete sensor batch, preserving transport failure.
    ///
    /// Long-running clients must be able to distinguish an unsupported sensor
    /// (`Ok(None)`) from a dead daemon connection (`Err`). The old `sensors()`
    /// API flattened both to `None`, which made reliable reconnect logic
    /// impossible.
    pub fn try_sensors(&self) -> Result<Sensors> {
        Ok(Sensors {
            cpu_temp_c: self.read_sensor(Sensor::CpuTemp)?,
            gpu_temp_c: self.read_sensor(Sensor::GpuTemp)?,
            system_temp_c: self.read_sensor(Sensor::SystemTemp)?,
            cpu_fan_rpm: self.read_sensor(Sensor::CpuFanRpm)?,
            gpu_fan_rpm: self.read_sensor(Sensor::GpuFanRpm)?,
        })
    }

    /// Best-effort sensor snapshot for one-shot callers.
    pub fn sensors(&self) -> Sensors {
        self.try_sensors().unwrap_or_default()
    }

    // ── Fans ────────────────────────────────────────────────────────────────

    /// Set fan behaviour. This is the control that actually matters — on the
    /// reference machine it is worth +61.8% sustained CPU throughput.
    pub fn set_fan_behaviour(&self, b: FanBehaviour) -> Result<()> {
        let resp = self
            .transport
            .call_u64(Function::SetFanBehaviour as u32, b.to_word())?;
        check(&resp)
    }

    /// Put a fan into manual mode and set its duty cycle.
    ///
    /// Two calls, in this order, because the percentage is ignored unless the
    /// fan is already in manual mode. Setting only the target fan's bit is
    /// enough — the other fan keeps whatever mode it had.
    ///
    /// # Verified
    ///
    /// This works, and an earlier version of this comment said it did not.
    /// From a clean automatic state (4687/5454 RPM), CPU-only manual at 100%
    /// took the CPU fan to 5882 RPM and left the GPU fan alone; 45% and 75%
    /// read back through [`Device::fan_percent`] as exactly 45 and 75.
    ///
    /// The original "the firmware accepts it and the EC ignores it" conclusion
    /// was a measurement artifact: the RPM was sampled 600 ms after the call,
    /// and these fans take eight to ten seconds to settle. Every check landed
    /// mid-ramp. If you are testing fan control, wait for a steady state — see
    /// the settling loop in the CLI.
    ///
    /// Duty is not linear in RPM: 30% ≈ 2100–3300, 45% ≈ 4700, 90% ≈ 5450,
    /// 100% = 5882 on the reference CPU fan. Monotonic, but do not present the
    /// percentage to users as a fraction of maximum RPM.
    pub fn set_fan_percent(&self, fan: Fan, percent: u8) -> Result<()> {
        self.set_fan_behaviour(FanBehaviour::Single {
            fan,
            mode: FanMode::Manual,
        })?;
        let resp = self
            .transport
            .call_u64(Function::SetFanSpeed as u32, fan_speed_word(fan, percent))?;
        check(&resp)
    }

    /// Read a fan's manual duty setting back from the firmware.
    ///
    /// This is the *requested* percentage, not a measurement — it reports what
    /// manual mode was told to do even while the fan is still ramping, which
    /// makes it the right way to confirm a `set_fan_percent` landed.
    /// [`Device::sensors`] is the one that tells you what the fan is doing.
    pub fn fan_percent(&self, fan: Fan) -> Result<u8> {
        let resp = self.transport.call_bytes(
            Function::GetFanSpeed as u32,
            &[fan as u8, 0, 0, 0, 0, 0, 0, 0],
        )?;
        check(&resp)?;
        // Byte 1 is the percentage. Byte 2 reads 0x17 regardless of fan or
        // value, so it is some fixed field rather than part of the number —
        // decoding both as a u16 would turn 45% into 5933.
        value_byte(&resp, 1, "fan-duty getter")
    }

    /// Hand both fans back to the EC's own curve.
    ///
    /// Worth calling on exit from anything that forced them, so a crashed or
    /// killed process does not leave the machine screaming.
    pub fn fans_auto(&self) -> Result<()> {
        self.set_fan_behaviour(FanBehaviour::Auto)
    }

    /// Both fans to maximum.
    pub fn fans_max(&self) -> Result<()> {
        self.set_fan_behaviour(FanBehaviour::Max)
    }

    // ── Exact PH315-53 advanced controls ───────────────────────────────────

    /// APGe CoolBoost state. This confirms the firmware field only; a thermal
    /// or fan effect still requires separate physical measurement.
    pub fn coolboost(&self) -> Result<bool> {
        self.transport.coolboost().map_err(Error::from)
    }

    /// Set CoolBoost with getter readback and rollback on mismatch.
    pub fn set_coolboost(&self, enabled: bool) -> Result<bool> {
        self.transport.set_coolboost(enabled).map_err(Error::from)
    }

    /// Keyboard inactivity timeout plus the brightness byte that a setter must
    /// preserve. Only the exact native fallback hotkey index 0 is probed.
    pub fn keyboard_timeout(&self) -> Result<KeyboardTimeoutState> {
        self.transport.keyboard_timeout().map_err(Error::from)
    }

    /// Set the exact 0/30-second timeout while preserving firmware brightness.
    pub fn set_keyboard_timeout(&self, seconds: u8) -> Result<KeyboardTimeoutState> {
        self.transport
            .set_keyboard_timeout(seconds)
            .map_err(Error::from)
    }

    /// Conditional LCD-overdrive state. `None` is a getter-confirmed absence.
    pub fn lcd_overdrive(&self) -> Result<Option<bool>> {
        self.transport.lcd_overdrive().map_err(Error::from)
    }

    /// Set LCD overdrive only when its exact getter reports support, with
    /// immediate readback and rollback on mismatch.
    pub fn set_lcd_overdrive(&self, enabled: bool) -> Result<Option<bool>> {
        self.transport
            .set_lcd_overdrive(enabled)
            .map_err(Error::from)
    }

    // ── Raw firmware flags ──────────────────────────────────────────────────

    /// Read a raw misc-setting flag. Returns the firmware value, normally 0 or 2.
    ///
    /// On the PH315-53 GPU sub-index 5 is not the OEM Normal/Faster/Turbo
    /// implementation: PredatorSense command 45 applies NVIDIA clock offsets
    /// first and only then writes its related firmware field. GPU reads route
    /// through the typed compound getter so daemon clients inherit its
    /// notification rate limit; the raw socket policy does not expose fn23/5.
    pub fn overclock(&self, target: OverclockTarget) -> Result<u8> {
        if target == OverclockTarget::Gpu {
            return self.gpu_mode().map(|state| state.gpoc);
        }
        let resp = self
            .transport
            .call_bytes(Function::GetMiscSetting as u32, &[target as u8, 0])?;
        value_byte(&resp, 1, "misc-setting getter")
    }

    /// Set a raw misc flag.
    ///
    /// Independent GPU writes are rejected because changing only GPOC would
    /// split the guarded NVML/fan-table/GPOC transaction. Use [`Self::set_gpu_mode`]
    /// for GPU state. The legacy CPU flag remains available for compatibility
    /// but has no measured effect on this CPU-OC-disabled target.
    pub fn set_overclock(&self, target: OverclockTarget, on: bool) -> Result<()> {
        if target == OverclockTarget::Gpu {
            return Err(Error::Unsupported(
                "raw GPU-flag writes are disabled because they split OEM GPU-mode state; use set_gpu_mode",
            ));
        }
        let v = if on { OC_TURBO } else { OC_OFF };
        let resp = self
            .transport
            .call_u64(Function::SetMiscSetting as u32, misc_word(target as u8, v))?;
        check(&resp)
    }

    /// Read getter-confirmed OEM GPU-mode state across NVML and Acer firmware.
    ///
    /// This is an explicit/manual operation: the Acer GPOC getter sends an OEM
    /// discrete-GPU notification even though it does not write the stored field.
    pub fn gpu_mode(&self) -> Result<GpuModeState> {
        self.transport.gpu_mode().map_err(Error::from)
    }

    /// Apply PH315-53 Normal/Faster/Turbo with exact-device gating, readback
    /// and reverse-order rollback. The opt-in cannot be constructed without
    /// explicitly acknowledging NVIDIA's unsupported clock-control boundary.
    pub fn set_gpu_mode(&self, mode: GpuMode, opt_in: GpuModeOptIn) -> Result<GpuModeState> {
        self.transport
            .set_gpu_mode(mode, opt_in)
            .map_err(Error::from)
    }

    // ── Keyboard backlight ──────────────────────────────────────────────────

    /// Colour one keyboard zone.
    ///
    /// Verified working: four zones set to four distinct colours read back
    /// individually and light the keyboard accordingly. This was previously
    /// documented as unsupported — the payload encoding was simply wrong, see
    /// [`crate::rgb::zone_word`].
    ///
    /// The zone colours are only visible in **static** mode; an effect
    /// overwrites them. Use [`Device::set_zone_colours`] to set mode and
    /// colours together.
    pub fn set_zone_colour(&self, zone: Zone, c: Colour) -> Result<()> {
        let resp = self
            .transport
            .call_u64(Function::SetStaticLed as u32, rgb::zone_word(zone, c))?;
        check(&resp)
    }

    /// Enable or disable each static keyboard zone, left to right.
    ///
    /// PredatorSense performs this function-2 SMI call before the static
    /// mode/brightness write. The daemon accepts only this exact recovered
    /// bit-mask shape; other function-2 behaviours remain blocked.
    pub fn set_zone_enabled(&self, enabled: [bool; 4]) -> Result<()> {
        let resp = self.transport.call_u64(
            Function::SetGamingLed as u32,
            rgb::zone_enable_word(enabled),
        )?;
        check(&resp)?;
        self.lighting_prepared
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Perform Covini's one startup lighting write, at most once per handle.
    ///
    /// The exact PH315-53 application loads the saved profile while its global
    /// `initial_flag` suppresses function 20 and function 6, but still sends the
    /// function-2 zone mask. No managed call or AML method supplies a later
    /// companion trigger. Frontends call this immediately before their first
    /// requested lighting mutation so opening Alien for read-only status does
    /// not unexpectedly change hardware.
    pub fn prepare_lighting(&self, enabled: [bool; 4]) -> Result<()> {
        if self
            .lighting_prepared
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(());
        }
        let _guard = self
            .lighting_prepare_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self
            .lighting_prepared
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(());
        }
        self.set_zone_enabled(enabled)
    }

    /// Read one zone's colour back.
    pub fn zone_colour(&self, zone: Zone) -> Result<Colour> {
        let r = self.transport.call_bytes(
            Function::GetStaticLed as u32,
            &[zone as u8, 0, 0, 0, 0, 0, 0, 0],
        )?;
        check(&r)?;
        if r.len() < 4 {
            return Err(Error::Transport(TransportError::AcpiFailure(format!(
                "static-zone getter returned {} byte(s), expected at least 4",
                r.len()
            ))));
        }
        // Reply is [status, R, G, B].
        Ok(Colour::new(r[1], r[2], r[3]))
    }

    /// Set all four zones, each to its own colour.
    ///
    /// Order matters: the zone-enable mask goes first, then static mode and
    /// brightness, then one call per zone. Setting the colours before the
    /// mode loses them because switching mode reinitialises the zones.
    pub fn set_zone_colours(&self, colours: [Colour; 4], brightness: u8) -> Result<()> {
        self.set_zone_colours_enabled(colours, [true; 4], brightness)
    }

    /// Set the complete Covini static-lighting state.
    ///
    /// The order is copied from PredatorSense 3.00.3152: zone enable mask,
    /// static mode plus brightness, then one colour write for each enabled
    /// zone. Disabled zones are deliberately not recoloured.
    pub fn set_zone_colours_enabled(
        &self,
        colours: [Colour; 4],
        enabled: [bool; 4],
        brightness: u8,
    ) -> Result<()> {
        self.set_zone_enabled(enabled)?;
        self.set_effect(
            Effect::Static,
            0,
            brightness,
            Direction::LeftToRight,
            Colour::OFF,
        )?;
        for ((zone, c), on) in Zone::ALL.iter().zip(colours).zip(enabled) {
            if on {
                self.set_zone_colour(*zone, c)?;
            }
        }
        Ok(())
    }

    /// Reapply static brightness without needlessly resending the zone mask.
    ///
    /// This is the exact Covini brightness-change path: after the one startup
    /// preparation, function 20 selects static/brightness and function 6
    /// restores each enabled colour. The mask is not part of this event.
    pub fn set_static_brightness_and_colours(
        &self,
        colours: [Colour; 4],
        enabled: [bool; 4],
        brightness: u8,
    ) -> Result<()> {
        self.prepare_lighting(enabled)?;
        self.set_effect(
            Effect::Static,
            0,
            brightness,
            Direction::LeftToRight,
            Colour::OFF,
        )?;
        for ((zone, colour), on) in Zone::ALL.iter().zip(colours).zip(enabled) {
            if on {
                self.set_zone_colour(*zone, colour)?;
            }
        }
        Ok(())
    }

    /// Apply a checkbox-style static-zone update.
    ///
    /// Covini sends the complete function-2 mask for either edge. It follows an
    /// off->on edge with only the newly enabled zone's function-6 colour; an
    /// on->off edge has no colour or function-20 write.
    pub fn update_zone_enabled(
        &self,
        colours: [Colour; 4],
        previous: [bool; 4],
        enabled: [bool; 4],
    ) -> Result<()> {
        self.set_zone_enabled(enabled)?;
        for (((zone, colour), was_on), is_on) in
            Zone::ALL.iter().zip(colours).zip(previous).zip(enabled)
        {
            if !was_on && is_on {
                self.set_zone_colour(*zone, colour)?;
            }
        }
        Ok(())
    }

    /// Read the backlight state back out of the firmware.
    ///
    /// Reliable for mode, speed and brightness. Note what it does NOT tell
    /// you: in static mode the visible colour comes from the per-zone values
    /// (function 6), not from the RGB field here — so this reporting a colour
    /// says nothing about what the keyboard looks like.
    pub fn backlight(&self) -> Result<BacklightState> {
        // Input is u64 = 1, per the vendor protocol; zeros happened to work
        // but are not what the firmware documents.
        let r = self
            .transport
            .call_u64(Function::GetKbBacklight as u32, 1)?;
        check(&r)?;
        if r.len() < 9 {
            return Err(Error::Transport(TransportError::AcpiFailure(format!(
                "backlight getter returned {} byte(s), expected at least 9",
                r.len()
            ))));
        }
        // Layout after the status byte: effect, speed, brightness, unused,
        // direction, r, g, b.
        let effect = Effect::ALL
            .iter()
            .copied()
            .find(|effect| *effect as u8 == r[1])
            .ok_or_else(|| {
                Error::Transport(TransportError::AcpiFailure(format!(
                    "backlight getter returned unknown Covini mode {}",
                    r[1]
                )))
            })?;
        Ok(BacklightState {
            effect,
            speed: r[2],
            brightness: r[3],
            reverse: r[5] == Direction::RightToLeft as u8,
            colour: Colour::new(r[6], r[7], r[8]),
        })
    }

    /// Paint all four zones one colour.
    pub fn set_all_zones(&self, c: Colour) -> Result<()> {
        let b = self.backlight()?.brightness;
        self.set_zone_colours([c; 4], b)
    }

    pub fn set_effect(
        &self,
        effect: Effect,
        speed: u8,
        brightness: u8,
        dir: Direction,
        c: Colour,
    ) -> Result<()> {
        let resp = self.transport.call_bytes(
            Function::SetKbBacklight as u32,
            &rgb::effect_payload(effect, rgb::covini_speed(effect, speed), brightness, dir, c),
        )?;
        check(&resp)
    }

    /// Backlight off while retaining the selected static/dynamic mode.
    ///
    /// Covini has no dedicated off operation: the first brightness tick emits
    /// zero in byte 2 and otherwise resends the current mode record. Forcing
    /// Static here used to lose the active effect and made the next brightness
    /// adjustment restore a different state from PredatorSense. Static follows
    /// the normal brightness path and therefore replays every enabled zone
    /// colour after the function-20 write.
    pub fn backlight_off(&self, colours: [Colour; 4], enabled: [bool; 4]) -> Result<()> {
        let current = self.backlight()?;
        self.prepare_lighting(enabled)?;
        if current.effect == Effect::Static {
            return self.set_static_brightness_and_colours(colours, enabled, 0);
        }
        let speed = rgb::covini_speed(current.effect, current.speed);
        let direction = if current.reverse {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        };
        self.set_effect(current.effect, speed, 0, direction, current.colour)
    }
}

fn check(resp: &[u8]) -> Result<()> {
    let s = crate::transport::status(resp);
    if s.is_ok() {
        Ok(())
    } else {
        Err(Error::Rejected(s))
    }
}

fn value_byte(resp: &[u8], index: usize, operation: &str) -> Result<u8> {
    check(resp)?;
    resp.get(index).copied().ok_or_else(|| {
        Error::Transport(TransportError::AcpiFailure(format!(
            "{operation} returned {} byte(s), expected at least {}",
            resp.len(),
            index + 1
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportError;
    use std::sync::{Arc, Mutex};

    #[test]
    fn require_socket_guard_is_exact_and_rejects_typos() {
        assert_eq!(require_socket(None), Ok(false));
        assert_eq!(require_socket(Some(std::ffi::OsStr::new("0"))), Ok(false));
        assert_eq!(require_socket(Some(std::ffi::OsStr::new("1"))), Ok(true));
        assert!(require_socket(Some(std::ffi::OsStr::new("true"))).is_err());
        assert!(require_socket(Some(std::ffi::OsStr::new(""))).is_err());
    }

    #[test]
    fn raw_gpu_flag_write_is_rejected_before_transport() {
        let (dev, recorder) = recorder();
        let error = dev.set_overclock(OverclockTarget::Gpu, true).unwrap_err();
        assert!(matches!(error, Error::Unsupported(_)));
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[test]
    fn raw_gpu_flag_read_routes_through_typed_compound_getter() {
        struct GpuModeOnly(Mutex<usize>);

        impl Transport for Arc<GpuModeOnly> {
            fn call_bytes(
                &self,
                _function: u32,
                _buf: &[u8],
            ) -> std::result::Result<Vec<u8>, TransportError> {
                panic!("raw CALL must not be used for the notifying GPU getter")
            }

            fn describe(&self) -> String {
                "typed GPU-mode recorder".into()
            }

            fn gpu_mode(
                &self,
            ) -> std::result::Result<crate::performance::GpuModeState, TransportError> {
                *self.0.lock().unwrap() += 1;
                Ok(crate::performance::GpuModeState {
                    graphics: crate::performance::GpuOffsetRange {
                        current_mhz: 100,
                        min_mhz: -1000,
                        max_mhz: 1000,
                    },
                    memory: crate::performance::GpuOffsetRange {
                        current_mhz: 60,
                        min_mhz: -2000,
                        max_mhz: 6000,
                    },
                    fan_table: 3,
                    gpoc: 2,
                })
            }
        }

        let recorder = Arc::new(GpuModeOnly(Mutex::new(0)));
        let dev = Device::with_transport(Box::new(Arc::clone(&recorder)));
        assert_eq!(dev.overclock(OverclockTarget::Gpu).unwrap(), 2);
        assert_eq!(*recorder.0.lock().unwrap(), 1);
    }

    /// Records what actually went on the wire.
    ///
    /// Note the fully-qualified `std::result::Result`: this module imports
    /// `super::*`, which brings in the crate's own one-parameter `Result`
    /// alias and makes the trait signature fail to match.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(u32, Vec<u8>)>>);

    impl Transport for Arc<Recorder> {
        fn call_bytes(
            &self,
            function: u32,
            buf: &[u8],
        ) -> std::result::Result<Vec<u8>, TransportError> {
            self.0.lock().unwrap().push((function, buf.to_vec()));
            Ok(vec![0u8; 9])
        }
        fn describe(&self) -> String {
            "recorder".into()
        }
    }

    struct ReplyRecorder {
        calls: Mutex<Vec<(u32, Vec<u8>)>>,
        backlight_reply: Vec<u8>,
    }

    impl Transport for Arc<ReplyRecorder> {
        fn call_bytes(
            &self,
            function: u32,
            buf: &[u8],
        ) -> std::result::Result<Vec<u8>, TransportError> {
            self.calls.lock().unwrap().push((function, buf.to_vec()));
            if function == Function::GetKbBacklight as u32 {
                Ok(self.backlight_reply.clone())
            } else {
                Ok(vec![0u8; 9])
            }
        }

        fn describe(&self) -> String {
            "reply-recorder".into()
        }
    }

    fn reply_recorder(reply: Vec<u8>) -> (Device, Arc<ReplyRecorder>) {
        let recorder = Arc::new(ReplyRecorder {
            calls: Mutex::new(Vec::new()),
            backlight_reply: reply,
        });
        (Device::with_transport(Box::new(recorder.clone())), recorder)
    }

    fn recorder() -> (Device, Arc<Recorder>) {
        let r = Arc::new(Recorder::default());
        (Device::with_transport(Box::new(r.clone())), r)
    }

    fn last_payload(r: &Arc<Recorder>) -> Vec<u8> {
        r.0.lock()
            .unwrap()
            .last()
            .expect("a call was made")
            .1
            .clone()
    }

    #[test]
    fn an_animated_effect_never_goes_out_at_speed_zero() {
        // Speed 0 on an animation is accepted by firmware, reads back fine,
        // and does nothing — the exact shape of a bug that looks like broken
        // hardware. A UI carrying the speed over from Static produces it.
        let (dev, rec) = recorder();
        dev.set_effect(Effect::Zoom, 0, 100, Direction::LeftToRight, Colour::OFF)
            .unwrap();
        let p = last_payload(&rec);
        assert_eq!(p[0], Effect::Zoom as u8);
        assert_eq!(p[1], 1, "speed must be raised to 1 for an animation");
        assert_eq!(p.len(), 8, "PH315-53 Covini setter is exactly one u64");
    }

    #[test]
    fn static_keeps_speed_zero() {
        // Static has nothing to animate; 0 is correct there and must not be
        // rewritten.
        let (dev, rec) = recorder();
        dev.set_effect(Effect::Static, 0, 50, Direction::LeftToRight, Colour::OFF)
            .unwrap();
        assert_eq!(last_payload(&rec)[1], 0);
    }

    #[test]
    fn an_explicit_speed_is_passed_through_untouched() {
        let (dev, rec) = recorder();
        dev.set_effect(Effect::Wave, 7, 80, Direction::LeftToRight, Colour::OFF)
            .unwrap();
        assert_eq!(last_payload(&rec)[1], 7);
        assert_eq!(
            last_payload(&rec)[2],
            75,
            "brightness snaps to a Covini step"
        );
    }

    #[test]
    fn static_sequence_matches_predatorsense_covini_order() {
        let (dev, rec) = recorder();
        let colours = [
            Colour::new(1, 2, 3),
            Colour::new(4, 5, 6),
            Colour::new(7, 8, 9),
            Colour::new(10, 11, 12),
        ];
        dev.set_zone_colours_enabled(colours, [true, false, true, false], 76)
            .unwrap();
        let calls = rec.0.lock().unwrap();
        assert_eq!(
            calls.len(),
            4,
            "mask, mode/brightness, and two enabled colours"
        );
        assert_eq!(
            calls[0],
            (
                Function::SetGamingLed as u32,
                rgb::zone_enable_word([true, false, true, false])
                    .to_le_bytes()
                    .to_vec(),
            )
        );
        assert_eq!(calls[1].0, Function::SetKbBacklight as u32);
        assert_eq!(calls[1].1, vec![0, 0, 75, 0, 0, 0, 0, 0]);
        assert_eq!(
            calls[2],
            (
                Function::SetStaticLed as u32,
                rgb::zone_word(Zone::One, colours[0]).to_le_bytes().to_vec()
            )
        );
        assert_eq!(
            calls[3],
            (
                Function::SetStaticLed as u32,
                rgb::zone_word(Zone::Three, colours[2])
                    .to_le_bytes()
                    .to_vec()
            )
        );
    }

    #[test]
    fn startup_zone_mask_is_sent_once_before_dynamic_writes() {
        let (dev, rec) = recorder();
        let enabled = [true, false, true, true];

        dev.prepare_lighting(enabled).unwrap();
        dev.set_effect(
            Effect::Breath,
            5,
            75,
            Direction::LeftToRight,
            Colour::new(0x12, 0x34, 0x56),
        )
        .unwrap();
        dev.prepare_lighting([false; 4]).unwrap();
        dev.set_effect(
            Effect::Neon,
            9,
            25,
            Direction::RightToLeft,
            Colour::new(1, 2, 3),
        )
        .unwrap();

        let calls = rec.0.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls[0],
            (
                Function::SetGamingLed as u32,
                rgb::zone_enable_word(enabled).to_le_bytes().to_vec()
            )
        );
        assert_eq!(
            calls[1],
            (
                Function::SetKbBacklight as u32,
                vec![1, 5, 75, 0, 0, 0x12, 0x34, 0x56]
            )
        );
        assert_eq!(
            calls[2],
            (
                Function::SetKbBacklight as u32,
                vec![2, 9, 25, 0, 0, 0, 0, 0]
            )
        );
    }

    #[test]
    fn static_brightness_event_omits_mask_after_startup() {
        let (dev, rec) = recorder();
        let colours = [
            Colour::new(1, 2, 3),
            Colour::new(4, 5, 6),
            Colour::new(7, 8, 9),
            Colour::new(10, 11, 12),
        ];
        let enabled = [true, false, true, false];
        dev.prepare_lighting(enabled).unwrap();
        rec.0.lock().unwrap().clear();

        dev.set_static_brightness_and_colours(colours, enabled, 49)
            .unwrap();

        let calls = rec.0.lock().unwrap();
        assert_eq!(calls.len(), 3, "mode/brightness plus two enabled colours");
        assert_eq!(
            calls[0],
            (
                Function::SetKbBacklight as u32,
                vec![0, 0, 50, 0, 0, 0, 0, 0]
            )
        );
        assert_eq!(calls[1].0, Function::SetStaticLed as u32);
        assert_eq!(
            calls[1].1,
            rgb::zone_word(Zone::One, colours[0]).to_le_bytes()
        );
        assert_eq!(calls[2].0, Function::SetStaticLed as u32);
        assert_eq!(
            calls[2].1,
            rgb::zone_word(Zone::Three, colours[2]).to_le_bytes()
        );
    }

    #[test]
    fn zone_checkbox_enable_restores_only_new_zone_and_disable_is_mask_only() {
        let (dev, rec) = recorder();
        let colours = [
            Colour::new(1, 2, 3),
            Colour::new(4, 5, 6),
            Colour::new(7, 8, 9),
            Colour::new(10, 11, 12),
        ];
        let before = [true, false, true, false];
        let enabled = [true, true, true, false];
        dev.update_zone_enabled(colours, before, enabled).unwrap();
        dev.update_zone_enabled(colours, enabled, before).unwrap();

        let calls = rec.0.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, Function::SetGamingLed as u32);
        assert_eq!(calls[0].1, rgb::zone_enable_word(enabled).to_le_bytes());
        assert_eq!(calls[1].0, Function::SetStaticLed as u32);
        assert_eq!(
            calls[1].1,
            rgb::zone_word(Zone::Two, colours[1]).to_le_bytes()
        );
        assert_eq!(calls[2].0, Function::SetGamingLed as u32);
        assert_eq!(calls[2].1, rgb::zone_enable_word(before).to_le_bytes());
    }

    #[test]
    fn off_retains_the_active_dynamic_record_and_changes_only_brightness() {
        // Getter result is status followed by the eight EC fields.
        let (dev, rec) = reply_recorder(vec![0, 4, 7, 75, 0, 1, 1, 2, 3]);
        let enabled = [true, false, true, false];
        dev.backlight_off([Colour::OFF; 4], enabled).unwrap();

        let calls = rec.calls.lock().unwrap();
        assert_eq!(calls.len(), 3, "one getter, startup mask, one setter");
        assert_eq!(
            calls[0],
            (Function::GetKbBacklight as u32, 1u64.to_le_bytes().to_vec())
        );
        assert_eq!(
            calls[1],
            (
                Function::SetGamingLed as u32,
                rgb::zone_enable_word(enabled).to_le_bytes().to_vec()
            )
        );
        assert_eq!(
            calls[2],
            (
                Function::SetKbBacklight as u32,
                vec![4, 7, 0, 0, 1, 1, 2, 3]
            )
        );
    }

    #[test]
    fn static_off_replays_enabled_colours_after_brightness_zero() {
        // Older Alien builds left direction/RGB in the static EC record. The
        // exact Covini static/off write zeros those irrelevant bytes.
        let (dev, rec) = reply_recorder(vec![0, 0, 0, 70, 0, 2, 0xff, 0xb0, 0]);
        let colours = [
            Colour::new(1, 2, 3),
            Colour::new(4, 5, 6),
            Colour::new(7, 8, 9),
            Colour::new(10, 11, 12),
        ];
        let enabled = [true, false, true, false];
        dev.backlight_off(colours, enabled).unwrap();
        let calls = rec.calls.lock().unwrap();
        assert_eq!(calls.len(), 5, "getter, mask, brightness, two colours");
        assert_eq!(calls[0].0, Function::GetKbBacklight as u32);
        assert_eq!(calls[1].0, Function::SetGamingLed as u32);
        assert_eq!(calls[2].1, vec![0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            calls[3].1,
            rgb::zone_word(Zone::One, colours[0]).to_le_bytes()
        );
        assert_eq!(
            calls[4].1,
            rgb::zone_word(Zone::Three, colours[2]).to_le_bytes()
        );
    }

    #[test]
    fn backlight_getter_rejects_short_and_unknown_responses() {
        let (short, _) = reply_recorder(vec![0, 1, 5]);
        assert!(matches!(short.backlight(), Err(Error::Transport(_))));

        let (unknown, _) = reply_recorder(vec![0, 99, 5, 75, 0, 0, 0, 0, 0]);
        assert!(matches!(unknown.backlight(), Err(Error::Transport(_))));
    }

    #[test]
    fn off_and_all_zones_do_not_write_when_the_backlight_getter_fails() {
        let (off, off_rec) = reply_recorder(vec![0, 1, 5]);
        assert!(matches!(
            off.backlight_off([Colour::OFF; 4], [true; 4]),
            Err(Error::Transport(_))
        ));
        assert_eq!(
            off_rec.calls.lock().unwrap().as_slice(),
            &[(Function::GetKbBacklight as u32, 1u64.to_le_bytes().to_vec())]
        );

        let (zones, zone_rec) = reply_recorder(vec![0, 1, 5]);
        assert!(matches!(
            zones.set_all_zones(Colour::new(1, 2, 3)),
            Err(Error::Transport(_))
        ));
        assert_eq!(
            zone_rec.calls.lock().unwrap().as_slice(),
            &[(Function::GetKbBacklight as u32, 1u64.to_le_bytes().to_vec())]
        );
    }
}
