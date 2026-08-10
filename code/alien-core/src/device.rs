//! The high-level device handle — what the CLI, TUI and GUI all talk to.
//!
//! Everything below is a thin, honest wrapper: it encodes the payload, issues
//! the call, and reports what the firmware said. It deliberately does **not**
//! verify that the hardware obeyed, because verification differs per control
//! (fans are checked by RPM, RGB by eye) and a wrapper that guessed would be
//! the third layer of "returned success, changed nothing" in this stack.
//! Callers that need proof use [`Device::sensors`] and compare.

use crate::rgb::{self, Colour, Direction, Effect, Zone};
use crate::socket::SocketClient;
use crate::transport::{sensor_u16, AcpiCall, Transport, TransportError};
use crate::wmi::{
    fan_speed_word, misc_word, Fan, FanBehaviour, FanMode, Function, OverclockTarget, Sensor,
    Status, OC_OFF, OC_TURBO,
};

pub struct Device {
    transport: Box<dyn Transport>,
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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(e) => write!(f, "{e}"),
            Error::Rejected(s) => write!(f, "firmware {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
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

impl Device {
    /// Open the machine's gaming interface, preferring the daemon.
    ///
    /// Daemon first, direct second, and in that order for a reason: if
    /// `alien-daemon` is running it owns `/proc/acpi/call`, and a second
    /// process writing the same global buffer would interleave with it — both
    /// sides then read each other's answers with no error anywhere. Falling
    /// back to direct access is only correct when nothing else is using it.
    pub fn open() -> Result<Device> {
        match SocketClient::connect() {
            Ok(c) => Ok(Device::with_transport(Box::new(c))),
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

    /// Drive a specific transport. For tests, and for a client that must not
    /// silently fall back to direct access.
    pub fn with_transport(t: Box<dyn Transport>) -> Device {
        Device { transport: t, key_frame: std::sync::Mutex::new(Default::default()) }
    }

    /// What this machine can actually do.
    ///
    /// Getter-only probing, so it is safe to call at startup. A UI should
    /// drive its enabled/disabled states from this rather than assuming the
    /// reference model's feature set.
    pub fn capabilities(&self) -> crate::capability::Capabilities {
        crate::capability::probe(self.transport.as_ref())
    }

    /// Colour a single key, on machines that have per-key hardware.
    ///
    /// Returns [`Error::Unsupported`] on four-zone keyboards, which is most
    /// Predators — their keys are wired into four banks with no per-key
    /// addressing, so this is a hardware limit and no software can work
    /// around it.
    pub fn set_key(&self, key: &str, colour: Colour) -> Result<()> {
        let (row, col) = crate::perkey::key_position(key)
            .ok_or(Error::Unsupported("unknown key name; try `alien rgb keys`"))?;
        let mut frame = self.key_frame.lock().unwrap_or_else(|p| p.into_inner());
        frame.set(row, col, colour);
        crate::perkey::send(&frame).map_err(|_| {
            Error::Unsupported(
                "no per-key controller (ITE 8291) on this machine — this keyboard is \
                 four-zone, so individual keys cannot be addressed",
            )
        })
    }

    /// How this handle reaches the firmware.
    pub fn method_path(&self) -> String {
        self.transport.describe()
    }

    // ── Telemetry ───────────────────────────────────────────────────────────

    fn read_sensor(&self, s: Sensor) -> Option<u16> {
        let resp = self.transport.call_bytes(Function::GetSysInfo as u32, &[0x01, s as u8]).ok()?;
        // A sensor that is not populated reads back as zero rather than
        // failing, so treat zero as absent — no fan on this chassis idles at
        // 0 RPM while the machine is powered, and 0 °C is not a real reading.
        sensor_u16(&resp).filter(|v| *v != 0)
    }

    pub fn sensors(&self) -> Sensors {
        Sensors {
            cpu_temp_c: self.read_sensor(Sensor::CpuTemp),
            gpu_temp_c: self.read_sensor(Sensor::GpuTemp),
            system_temp_c: self.read_sensor(Sensor::SystemTemp),
            cpu_fan_rpm: self.read_sensor(Sensor::CpuFanRpm),
            gpu_fan_rpm: self.read_sensor(Sensor::GpuFanRpm),
        }
    }

    // ── Fans ────────────────────────────────────────────────────────────────

    /// Set fan behaviour. This is the control that actually matters — on the
    /// reference machine it is worth ~48% sustained CPU throughput.
    pub fn set_fan_behaviour(&self, b: FanBehaviour) -> Result<()> {
        let resp = self.transport.call_u64(Function::SetFanBehaviour as u32, b.to_word())?;
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
        self.set_fan_behaviour(FanBehaviour::Single { fan, mode: FanMode::Manual })?;
        let resp = self.transport.call_u64(Function::SetFanSpeed as u32, fan_speed_word(fan, percent))?;
        check(&resp)
    }

    /// Read a fan's manual duty setting back from the firmware.
    ///
    /// This is the *requested* percentage, not a measurement — it reports what
    /// manual mode was told to do even while the fan is still ramping, which
    /// makes it the right way to confirm a `set_fan_percent` landed.
    /// [`Device::sensors`] is the one that tells you what the fan is doing.
    pub fn fan_percent(&self, fan: Fan) -> Result<u8> {
        let resp = self
            .transport
            .call_bytes(Function::GetFanSpeed as u32, &[fan as u8, 0, 0, 0, 0, 0, 0, 0])?;
        check(&resp)?;
        // Byte 1 is the percentage. Byte 2 reads 0x17 regardless of fan or
        // value, so it is some fixed field rather than part of the number —
        // decoding both as a u16 would turn 45% into 5933.
        Ok(resp.get(1).copied().unwrap_or(0))
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

    // ── Turbo / overclock ───────────────────────────────────────────────────

    /// Read a turbo flag. Returns the raw value: 0 = off, 2 = turbo.
    pub fn overclock(&self, target: OverclockTarget) -> Result<u8> {
        let resp = self
            .transport
            .call_bytes(Function::GetMiscSetting as u32, &[target as u8, 0])?;
        Ok(resp.get(1).copied().unwrap_or(0))
    }

    /// Set a turbo flag.
    ///
    /// See [`crate::wmi::OverclockTarget`] — on the reference SKU this flag is
    /// real and persists, but produces no measurable clock change; the physical
    /// Turbo button's effect there is the fan curve. Do not present this as a
    /// performance gain without measuring it.
    pub fn set_overclock(&self, target: OverclockTarget, on: bool) -> Result<()> {
        let v = if on { OC_TURBO } else { OC_OFF };
        let resp = self
            .transport
            .call_u64(Function::SetMiscSetting as u32, misc_word(target as u8, v))?;
        check(&resp)
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

    /// Read one zone's colour back.
    pub fn zone_colour(&self, zone: Zone) -> Result<Colour> {
        let r = self
            .transport
            .call_bytes(Function::GetStaticLed as u32, &[zone as u8, 0, 0, 0, 0, 0, 0, 0])?;
        check(&r)?;
        // Reply is [status, R, G, B].
        Ok(Colour::new(
            r.get(1).copied().unwrap_or(0),
            r.get(2).copied().unwrap_or(0),
            r.get(3).copied().unwrap_or(0),
        ))
    }

    /// Set all four zones, each to its own colour.
    ///
    /// Order matters: static mode and brightness go first through the effect
    /// function, then one call per zone. Setting the colours first and the
    /// mode afterwards loses them, because switching mode reinitialises the
    /// zones.
    pub fn set_zone_colours(&self, colours: [Colour; 4], brightness: u8) -> Result<()> {
        self.set_effect(
            Effect::Static,
            0,
            brightness,
            Direction::LeftToRight,
            Colour::OFF,
        )?;
        for (zone, c) in Zone::ALL.iter().zip(colours) {
            self.set_zone_colour(*zone, c)?;
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
        let r = self.transport.call_u64(Function::GetKbBacklight as u32, 1)?;
        check(&r)?;
        // Layout after the status byte: effect, speed, brightness, unused,
        // direction, r, g, b.
        let at = |i: usize| r.get(i).copied().unwrap_or(0);
        Ok(BacklightState {
            effect: Effect::ALL
                .iter()
                .copied()
                .find(|e| *e as u8 == at(1))
                .unwrap_or(Effect::Static),
            speed: at(2),
            brightness: at(3),
            reverse: at(5) == Direction::RightToLeft as u8,
            colour: Colour::new(at(6), at(7), at(8)),
        })
    }

    /// Paint all four zones one colour.
    pub fn set_all_zones(&self, c: Colour) -> Result<()> {
        let b = self.backlight().map(|s| s.brightness).unwrap_or(100);
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
        let speed = if effect == Effect::Static { speed } else { speed.max(1) };
        let resp = self.transport.call_bytes(
            Function::SetKbBacklight as u32,
            &rgb::effect_payload(effect, speed, brightness, dir, c),
        )?;
        check(&resp)
    }

    /// Backlight off. Brightness 0 rather than a dedicated "off" — the
    /// firmware has no off function, and setting every zone black leaves the
    /// effect engine running.
    pub fn backlight_off(&self) -> Result<()> {
        self.set_effect(Effect::Static, 0, 0, Direction::LeftToRight, Colour::OFF)
    }
}

fn check(resp: &[u8]) -> Result<()> {
    let s = AcpiCall::status(resp);
    if s.is_ok() {
        Ok(())
    } else {
        Err(Error::Rejected(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportError;
    use std::sync::{Arc, Mutex};

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

    fn recorder() -> (Device, Arc<Recorder>) {
        let r = Arc::new(Recorder::default());
        (Device::with_transport(Box::new(r.clone())), r)
    }

    fn last_payload(r: &Arc<Recorder>) -> Vec<u8> {
        r.0.lock().unwrap().last().expect("a call was made").1.clone()
    }

    #[test]
    fn an_animated_effect_never_goes_out_at_speed_zero() {
        // Speed 0 on an animation is accepted by firmware, reads back fine,
        // and does nothing — the exact shape of a bug that looks like broken
        // hardware. A UI carrying the speed over from Static produces it.
        let (dev, rec) = recorder();
        dev.set_effect(Effect::Ripple, 0, 100, Direction::LeftToRight, Colour::OFF)
            .unwrap();
        let p = last_payload(&rec);
        assert_eq!(p[0], Effect::Ripple as u8);
        assert_eq!(p[1], 1, "speed must be raised to 1 for an animation");
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
    }
}
