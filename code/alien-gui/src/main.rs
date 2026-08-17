//! `alien-gui` — the desktop control centre.
//!
//! A phosphor-green instrument panel: five screens, a live telemetry poll, and
//! controls that are enabled only where the machine in front of you actually
//! supports them. The project logo is embedded from Alien's own static artwork;
//! none of Acer's artwork is used or redistributed.
//!
//! # Why egui rather than GTK
//!
//! The whole point is a fully skinned look. Adwaita fights that at every step
//! — the platform theme is the thing we are deliberately not using. egui draws
//! every pixel itself, produces a single static binary, and drops into
//! flatpak, snap, Docker and six distro packages without a toolkit runtime to
//! chase.
//!
//! # Talking to the hardware
//!
//! Through `alien-daemon` over its socket, never directly. A GUI cannot run as
//! root on Wayland in any sane way, and a sandboxed build has no path to
//! `/proc/acpi/call` at all. The daemon exists precisely so this process can
//! stay unprivileged — which also means the daemon can be absent, or go away
//! mid-session, and this program has to have something honest to show for
//! both. See [`Link`].

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, RichText, StrokeKind};

use alien_core::performance::Performance;
use alien_core::profile::{self, Backlight as ProfileBacklight, FanPolicy, FanSide, Profile};
#[cfg(unix)]
use alien_core::socket::SocketClient;
use alien_core::wmi::OverclockTarget;
use alien_core::{
    covini_brightness, BacklightState, Capabilities, Colour, Device, Direction, Effect, Fan,
    GpuMode, GpuModeOptIn, GpuModeState, KeyboardTimeoutState, Sensors, Support, Zone,
    GPU_MODE_ACKNOWLEDGEMENT,
};

mod compatible_models;
mod gauge;
mod glyph;
mod splash;
mod theme;

use glyph::Mark;

const HISTORY: usize = 120;
/// Below this width the chrome sheds its optional furniture. 900 sits between
/// the design's two sizes — a 980 window keeps everything, a half-tiled 760
/// one drops the part code and the third history plot.
const NARROW: f32 = 900.0;

const TOP_H: f32 = 52.0;
const NAV_W: f32 = 168.0;
const FOOT_H: f32 = 28.0;
const DASHBOARD_MIDDLE_MIN_H: f32 = 96.0;
const DASHBOARD_SECTION_H: f32 = 14.0;
const DASHBOARD_PROFILE_CHIP_H: f32 = 29.0;
const LOGO_PNG: &[u8] = include_bytes!("../assets/tech.hartle.Alien.png");

fn logo_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(LOGO_PNG)
        .expect("the embedded Alien logo is a build-validated PNG")
}

fn load_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let icon = logo_icon();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    ctx.load_texture("alien-canonical-logo", image, egui::TextureOptions::LINEAR)
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Dashboard,
    Fans,
    Lighting,
    Performance,
    About,
}

impl Tab {
    const ALL: [(Tab, &'static str, &'static str); 5] = [
        (Tab::Dashboard, "Dashboard", "01"),
        (Tab::Fans, "Fan Control", "02"),
        (Tab::Lighting, "Lighting", "03"),
        (Tab::Performance, "Performance", "04"),
        (Tab::About, "About", "05"),
    ];

    fn parse(s: &str) -> Option<Tab> {
        Some(match s.to_ascii_lowercase().as_str() {
            "dashboard" | "1" => Tab::Dashboard,
            "fans" | "fan" | "2" => Tab::Fans,
            "lighting" | "rgb" | "3" => Tab::Lighting,
            "performance" | "turbo" | "4" => Tab::Performance,
            "about" | "5" => Tab::About,
            _ => return None,
        })
    }
}

/// State of the connection to `alien-daemon`.
///
/// This is a first-class part of the UI rather than an error path. The daemon
/// is a separate process under systemd: it can be not installed yet, not
/// permitted to this user, or stopped while the window is open. Each of those
/// has a different remedy, and a window that renders an empty dashboard for
/// all three looks like idle hardware.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Link {
    /// Never reached it. Show the setup screen.
    Never,
    Up,
    /// Was up, then went away. Freeze the readings and keep trying.
    Lost,
}

#[derive(Default)]
struct Shared {
    sensors: Sensors,
    performance: Performance,
    cpu_turbo: Option<u8>,
    gpu_turbo: Option<u8>,
    gpu_mode: Option<GpuModeState>,
    gpu_mode_error: Option<String>,
    coolboost: Option<bool>,
    keyboard_timeout: Option<KeyboardTimeoutState>,
    lcd_overdrive: Option<bool>,
    backlight: Option<BacklightState>,
    caps: Option<Capabilities>,
    cpu_hist: Vec<Option<u16>>,
    gpu_hist: Vec<Option<u16>>,
    cpu_rpm_hist: Vec<Option<u16>>,
    gpu_rpm_hist: Vec<Option<u16>>,
    /// What the firmware says the duty is, which lags what we asked for.
    fan_readback: [Option<u8>; 2],
    link: Option<LinkState>,
    interface: String,
}

#[derive(Clone)]
struct LinkState {
    link: Link,
    attempts: u32,
    error: String,
    /// When the last good sample landed, for the "frozen NN ago" readout.
    last_good: Option<Instant>,
    /// When the next reconnection attempt is due.
    next_try: Option<Instant>,
}

impl Default for LinkState {
    fn default() -> Self {
        LinkState {
            link: Link::Never,
            attempts: 0,
            error: String::new(),
            last_good: None,
            next_try: None,
        }
    }
}

/// Everything the render thread reads, copied once per frame.
#[derive(Clone)]
struct Snapshot {
    sensors: Sensors,
    performance: Performance,
    cpu_turbo: Option<u8>,
    gpu_turbo: Option<u8>,
    gpu_mode: Option<GpuModeState>,
    gpu_mode_error: Option<String>,
    coolboost: Option<bool>,
    keyboard_timeout: Option<KeyboardTimeoutState>,
    lcd_overdrive: Option<bool>,
    backlight: Option<BacklightState>,
    caps: Capabilities,
    cpu_hist: Vec<Option<u16>>,
    gpu_hist: Vec<Option<u16>>,
    cpu_rpm_hist: Vec<Option<u16>>,
    gpu_rpm_hist: Vec<Option<u16>>,
    fan_readback: [Option<u8>; 2],
    /// `false` only while the background poller is still inside its very first
    /// daemon connection attempt. This drives the splash without adding any
    /// socket work to the render thread.
    daemon_attempt_finished: bool,
    link: LinkState,
    interface: String,
}

/// Append one sample, keeping the last [`HISTORY`] of them.
///
/// The absence is preserved rather than flattened to zero. Storing a missing
/// reading as 0 put cliffs to the floor in the temperature history; the
/// monitor renderer treats `None` as a visible gap.
fn push(v: &mut Vec<Option<u16>>, x: Option<u16>) {
    v.push(x);
    if v.len() > HISTORY {
        v.remove(0);
    }
}

fn advanced_error_support(error: &alien_core::Error) -> Support {
    match error {
        alien_core::Error::Transport(
            alien_core::transport::TransportError::FirmwareStatus { .. }
            | alien_core::transport::TransportError::UnsupportedEndpoint(_),
        ) => Support::No,
        _ => Support::Unknown,
    }
}

/// The desktop frontend is always an unprivileged daemon client. Do not use
/// `Device::open()` here: its trusted-root direct-ACPI fallback is useful to
/// the CLI, but would contradict this GUI's security boundary and turn a
/// missing socket into an irrelevant `/proc/acpi/call` error.
#[cfg(unix)]
fn open_daemon_device() -> alien_core::Result<Device> {
    SocketClient::connect()
        .map(|client| Device::with_transport(Box::new(client)))
        .map_err(alien_core::Error::from)
}

/// Where a Windows build would construct its transport.
///
/// The GUI itself is already portable — it type-checks for
/// `x86_64-pc-windows-msvc` unmodified, and its winit/glow dependency graph
/// contains no X11 or Wayland crates on that target. What is missing is only
/// the transport: Windows reaches the same ACPI-WMI methods through COM, with
/// no broker, because the AML declares them `Serialized`.
#[cfg(not(unix))]
fn open_daemon_device() -> alien_core::Result<Device> {
    Err(alien_core::Error::Unsupported(
        "the daemon socket is POSIX-only; a Windows build constructs a WMI \
         transport here",
    ))
}

/// Owns the connection for its whole life: connects, polls, notices death,
/// reconnects.
///
/// Firmware calls take tens of milliseconds and there are ten per cycle, so
/// none of this can happen on the UI thread.
fn poller(
    shared: Arc<Mutex<Shared>>,
    conn: Arc<Mutex<Option<Arc<Device>>>>,
    running: Arc<AtomicBool>,
    want_duty: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    // Reconnect cadence. The design says 5 s before we have ever connected
    // (you are probably in another terminal running systemctl) and 3 s after a
    // live link drops (a restart should be picked up promptly).
    const RETRY_NEW: Duration = Duration::from_secs(5);
    const RETRY_LOST: Duration = Duration::from_secs(3);

    while running.load(Ordering::Relaxed) {
        let dev = conn.lock().ok().and_then(|g| g.clone());

        let Some(dev) = dev else {
            // Disconnected: try to (re)establish.
            let (was, attempts) = {
                let sh = shared.lock().ok();
                let st = sh.as_ref().and_then(|s| s.link.clone());
                (
                    st.as_ref().map(|s| s.link).unwrap_or(Link::Never),
                    st.as_ref().map(|s| s.attempts).unwrap_or(0),
                )
            };
            let wait = if was == Link::Never {
                RETRY_NEW
            } else {
                RETRY_LOST
            };

            match open_daemon_device() {
                Ok(d) => {
                    let d = Arc::new(d);
                    let iface = d.method_path();
                    let caps = d.capabilities();
                    if let Ok(mut g) = conn.lock() {
                        *g = Some(Arc::clone(&d));
                    }
                    if let Ok(mut sh) = shared.lock() {
                        sh.caps = Some(caps);
                        sh.interface = iface;
                        sh.link = Some(LinkState {
                            link: Link::Up,
                            attempts: 0,
                            error: String::new(),
                            last_good: Some(Instant::now()),
                            next_try: None,
                        });
                    }
                    // A successful connection has no retry deadline. Enter
                    // the polling branch immediately; falling through to the
                    // retry wait would keep replacing `None` with
                    // `Instant::now() + wait`, an ever-receding deadline that
                    // freezes telemetry before its first sample.
                    ctx.request_repaint();
                    continue;
                }
                Err(e) => {
                    if let Ok(mut sh) = shared.lock() {
                        sh.link = Some(LinkState {
                            link: was,
                            attempts: attempts + 1,
                            error: e.to_string(),
                            last_good: sh.link.as_ref().and_then(|s| s.last_good),
                            next_try: Some(Instant::now() + wait),
                        });
                    }
                }
            }
            ctx.request_repaint();
            // Sleep in slices so quitting stays responsive and the countdown
            // in the UI keeps ticking.
            //
            // The deadline is re-read from the shared state each slice rather
            // than being fixed here, because RETRY works by pulling `next_try`
            // forward. With a local deadline the button only shortened the
            // displayed countdown while the actual retry still waited out the
            // full interval — a control that appears to do something and
            // does not, which is the exact thing the About screen promises
            // this program will not do.
            while running.load(Ordering::Relaxed) {
                // Falling back to the full interval, not to "now": a missing
                // deadline must not turn this into a spin that reconnects as
                // fast as the socket can refuse.
                let due = shared
                    .lock()
                    .ok()
                    .and_then(|s| s.link.as_ref().and_then(|l| l.next_try))
                    .unwrap_or_else(|| Instant::now() + wait);
                if Instant::now() >= due {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
                ctx.request_repaint();
            }
            continue;
        };

        // Probe with a call that returns a Result, before touching anything
        // that swallows errors. `sensors()` reports a dead socket as five
        // Nones, which is indistinguishable from a machine with no sensors —
        // so the liveness question has to be asked separately, and first.
        let bl = dev.backlight();
        if let Err(ref e) = bl {
            if e.is_link_lost() {
                if let Ok(mut g) = conn.lock() {
                    *g = None;
                }
                if let Ok(mut sh) = shared.lock() {
                    let last_good = sh.link.as_ref().and_then(|s| s.last_good);
                    sh.link = Some(LinkState {
                        link: Link::Lost,
                        attempts: 1,
                        error: e.to_string(),
                        last_good,
                        next_try: Some(Instant::now() + RETRY_LOST),
                    });
                }
                ctx.request_repaint();
                continue;
            }
        }

        let s = match dev.try_sensors() {
            Ok(s) => s,
            Err(e) if e.is_link_lost() => {
                if let Ok(mut g) = conn.lock() {
                    *g = None;
                }
                if let Ok(mut sh) = shared.lock() {
                    let last_good = sh.link.as_ref().and_then(|state| state.last_good);
                    sh.link = Some(LinkState {
                        link: Link::Lost,
                        attempts: 1,
                        error: e.to_string(),
                        last_good,
                        next_try: Some(Instant::now() + RETRY_LOST),
                    });
                }
                ctx.request_repaint();
                continue;
            }
            Err(_) => Sensors::default(),
        };
        let performance = Performance::sample();
        let cpu = dev.overclock(OverclockTarget::Cpu).ok();
        // Do not query GPU GPOC here: Acer's nominal fn23 getter sends a GPU
        // notification as a side effect. GPU mode/raw GPOC is refreshed only
        // by an explicit action on the Performance page or around a confirmed
        // mode mutation.
        let caps = shared
            .lock()
            .ok()
            .and_then(|state| state.caps.clone())
            .unwrap_or_default();
        // Only poll typed endpoints whose getter passed the startup probe.
        // This avoids hammering a known-rejected APGe timeout selector once a
        // second on machines such as the PH315-53, while keeping supported
        // controls in sync with changes made by the CLI or firmware.
        let (coolboost, coolboost_support) = if caps.coolboost == Support::No {
            (None, Support::No)
        } else {
            match dev.coolboost() {
                Ok(state) => (Some(state), Support::Unverified),
                Err(error) => (None, advanced_error_support(&error)),
            }
        };
        let (keyboard_timeout, keyboard_timeout_support) = if caps.keyboard_timeout == Support::No {
            (None, Support::No)
        } else {
            match dev.keyboard_timeout() {
                Ok(state) => (Some(state), Support::Unverified),
                Err(error) => (None, advanced_error_support(&error)),
            }
        };
        let (lcd_overdrive, lcd_overdrive_support) = if caps.lcd_overdrive == Support::No {
            (None, Support::No)
        } else {
            match dev.lcd_overdrive() {
                Ok(Some(state)) => (Some(state), Support::Unverified),
                Ok(None) => (None, Support::No),
                Err(error) => (None, advanced_error_support(&error)),
            }
        };
        let duty = if want_duty.load(Ordering::Relaxed) {
            [
                dev.fan_percent(Fan::Cpu).ok(),
                dev.fan_percent(Fan::Gpu).ok(),
            ]
        } else {
            [None, None]
        };

        if let Ok(mut sh) = shared.lock() {
            push(&mut sh.cpu_hist, s.cpu_temp_c);
            push(&mut sh.gpu_hist, s.gpu_temp_c);
            push(&mut sh.cpu_rpm_hist, s.cpu_fan_rpm);
            push(&mut sh.gpu_rpm_hist, s.gpu_fan_rpm);
            sh.sensors = s;
            sh.performance = performance;
            sh.cpu_turbo = cpu;
            sh.coolboost = coolboost;
            sh.keyboard_timeout = keyboard_timeout;
            sh.lcd_overdrive = lcd_overdrive;
            if let Some(caps) = sh.caps.as_mut() {
                caps.coolboost = coolboost_support;
                caps.keyboard_timeout = keyboard_timeout_support;
                caps.lcd_overdrive = lcd_overdrive_support;
            }
            sh.backlight = bl.ok();
            sh.fan_readback = duty;
            sh.link = Some(LinkState {
                link: Link::Up,
                attempts: 0,
                error: String::new(),
                last_good: Some(Instant::now()),
                next_try: None,
            });
        }
        ctx.request_repaint();
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[derive(Clone, Copy)]
struct EffectSettings {
    colour: [f32; 3],
    /// 0 is a valid speed only for Static. For an animation it means "do not
    /// animate", which looks exactly like a broken effect.
    speed: u8,
    left_to_right: bool,
}

#[derive(Clone, Copy)]
enum LightingUpdate {
    /// Static tab/profile apply: mask -> function 20 -> enabled colours.
    CompleteStatic,
    /// User edits while Static is already selected. Covini has distinct
    /// checkbox, colour and brightness paths; do not turn each into a full
    /// profile reapply.
    StaticIncremental {
        previous_enabled: [bool; 4],
        colour_changed: [bool; 4],
        brightness_changed: bool,
    },
    /// One complete function-20 pattern record after startup preparation.
    Dynamic,
}

struct App {
    conn: Arc<Mutex<Option<Arc<Device>>>>,
    shared: Arc<Mutex<Shared>>,
    running: Arc<AtomicBool>,
    want_duty: Arc<AtomicBool>,
    tab: Tab,
    status: String,
    status_bad: bool,

    cpu_pct: u8,
    gpu_pct: u8,
    /// Which of the three fan modes the user last chose. Firmware has no
    /// getter for it, so this is our own record, not a reading.
    fan_mode: FanMode,
    /// Set on release, so the readback line can say "settling" honestly.
    duty_sent: [Option<Instant>; 2],

    per_effect: [EffectSettings; 6],
    zone_colours: [[f32; 3]; 4],
    zone_enabled: [bool; 4],
    brightness: u8,
    effect: Effect,
    profile: Option<String>,
    profiles: Vec<Profile>,
    profile_editor: bool,
    profile_editor_focus: bool,
    profile_name: String,
    seeded: bool,
    mem: alien_core::Lighting,
    logo: egui::TextureHandle,
    splash: splash::Splash,
    gpu_mode_confirm: Option<GpuMode>,
    gpu_mode_job: Option<GpuModeJob>,
    compatible_models_open: bool,
    compatible_models_search: String,
}

#[derive(Clone, Copy)]
enum GpuModeOperation {
    Refresh,
    Apply(GpuMode),
}

struct GpuModeJob {
    operation: GpuModeOperation,
    result: std::sync::mpsc::Receiver<Result<GpuModeState, String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FanMode {
    Unknown,
    Max,
    Auto,
    Manual,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, tab: Tab, reduced_motion: bool) -> Self {
        theme::apply(&cc.egui_ctx);

        let mem = alien_core::Lighting::load();
        let logo = load_logo_texture(&cc.egui_ctx);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let conn: Arc<Mutex<Option<Arc<Device>>>> = Arc::new(Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let want_duty = Arc::new(AtomicBool::new(false));

        std::thread::spawn({
            let shared = Arc::clone(&shared);
            let conn = Arc::clone(&conn);
            let running = Arc::clone(&running);
            let want_duty = Arc::clone(&want_duty);
            let ctx = cc.egui_ctx.clone();
            move || poller(shared, conn, running, want_duty, ctx)
        });

        App {
            conn,
            shared,
            running,
            want_duty,
            tab,
            status: "ready".into(),
            status_bad: false,
            cpu_pct: 60,
            gpu_pct: 60,
            fan_mode: FanMode::Unknown,
            duty_sent: [None, None],
            per_effect: [EffectSettings {
                colour: [0.0, 0.68, 0.78],
                speed: 5,
                left_to_right: true,
            }; 6],
            zone_colours: [[0.0, 0.68, 0.78]; 4],
            zone_enabled: [true; 4],
            brightness: 100,
            effect: Effect::Static,
            profile: None,
            profiles: profile::list(),
            profile_editor: false,
            profile_editor_focus: false,
            profile_name: String::new(),
            seeded: false,
            mem,
            logo,
            splash: splash::Splash::new(reduced_motion),
            gpu_mode_confirm: None,
            gpu_mode_job: None,
            compatible_models_open: false,
            compatible_models_search: String::new(),
        }
    }

    fn device(&self) -> Option<Arc<Device>> {
        self.conn.lock().ok().and_then(|g| g.clone())
    }

    fn reload_lighting_memory(&mut self) {
        self.mem = alien_core::Lighting::load();
        for e in Effect::ALL {
            let c = self.mem.colour(e);
            self.per_effect[e as usize] = EffectSettings {
                colour: [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0],
                speed: self.mem.speed(e),
                left_to_right: self.mem.direction(e) == Direction::LeftToRight,
            };
        }
        for (slot, c) in self.zone_colours.iter_mut().zip(self.mem.zone_colours()) {
            *slot = [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0];
        }
        self.zone_enabled = self.mem.zone_enabled;
        self.brightness = self.mem.brightness;
    }

    fn act<T>(&mut self, r: alien_core::Result<T>, ok: &str) {
        match r {
            Ok(_) => {
                self.status = ok.to_owned();
                self.status_bad = false;
                // A direct mutation means any previously applied profile is
                // no longer a truthful description of the complete state.
                self.profile = None;
            }
            Err(e) => {
                self.status = format!("failed: {e}");
                self.status_bad = true;
            }
        }
    }

    fn apply_coolboost(&mut self, enabled: bool) {
        let Some(dev) = self.device() else { return };
        match dev.set_coolboost(enabled) {
            Ok(confirmed) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared.coolboost = Some(confirmed);
                }
                self.act::<()>(
                    Ok(()),
                    if confirmed {
                        "CoolBoost on · PH315-53 reinit transient confirmed; no sustained A/B/A cooling lift"
                    } else {
                        "CoolBoost off · PH315-53 reinit transient confirmed; no sustained A/B/A cooling lift"
                    },
                );
            }
            Err(error) => self.act::<()>(Err(error), ""),
        }
    }

    fn apply_keyboard_timeout(&mut self, seconds: u8) {
        let Some(dev) = self.device() else { return };
        match dev.set_keyboard_timeout(seconds) {
            Ok(confirmed) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared.keyboard_timeout = Some(confirmed);
                }
                self.act::<()>(
                    Ok(()),
                    if confirmed.seconds == 30 {
                        "keyboard timeout 30 s · getter confirmed, optical effect unverified"
                    } else {
                        "keyboard timeout disabled · getter confirmed"
                    },
                );
            }
            Err(error) => self.act::<()>(Err(error), ""),
        }
    }

    fn apply_lcd_overdrive(&mut self, enabled: bool) {
        let Some(dev) = self.device() else { return };
        match dev.set_lcd_overdrive(enabled) {
            Ok(Some(confirmed)) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared.lcd_overdrive = Some(confirmed);
                }
                self.act::<()>(
                    Ok(()),
                    if confirmed {
                        "LCD overdrive on · getter confirmed, panel effect unverified"
                    } else {
                        "LCD overdrive off · getter confirmed, panel effect unverified"
                    },
                );
            }
            Ok(None) => self.act::<()>(
                Err(alien_core::Error::Unsupported(
                    "LCD-overdrive getter reports no panel support",
                )),
                "",
            ),
            Err(error) => self.act::<()>(Err(error), ""),
        }
    }

    fn start_gpu_mode_job(&mut self, operation: GpuModeOperation) {
        if self.gpu_mode_job.is_some() {
            return;
        }
        let Some(dev) = self.device() else { return };
        let (send, result) = std::sync::mpsc::sync_channel(1);
        let spawn = std::thread::Builder::new()
            .name("alien-gpu-mode".into())
            .spawn(move || {
                let outcome = match operation {
                    GpuModeOperation::Refresh => dev.gpu_mode(),
                    GpuModeOperation::Apply(mode) => {
                        let opt_in = GpuModeOptIn::acknowledge(GPU_MODE_ACKNOWLEDGEMENT)
                            .expect("confirmation dialog maps to the exact acknowledgement");
                        dev.set_gpu_mode(mode, opt_in)
                    }
                }
                .map_err(|error| error.to_string());
                let _ = send.send(outcome);
            });
        match spawn {
            Ok(_) => {
                self.gpu_mode_job = Some(GpuModeJob { operation, result });
                self.status_bad = false;
                self.status = match operation {
                    GpuModeOperation::Refresh => {
                        "reading OEM GPU mode · Acer getter sends one GPU notification…".into()
                    }
                    GpuModeOperation::Apply(mode) => {
                        format!("applying OEM GPU {} with readback/rollback…", mode.label())
                    }
                };
            }
            Err(error) => {
                self.status = format!("failed to start GPU-mode worker: {error}");
                self.status_bad = true;
            }
        }
    }

    fn poll_gpu_mode_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.gpu_mode_job else {
            return;
        };
        let outcome = match job.result.try_recv() {
            Ok(result) => Some((job.operation, result)),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((
                job.operation,
                Err("GPU-mode worker exited without a result".into()),
            )),
        };
        let Some((operation, outcome)) = outcome else {
            return;
        };
        self.gpu_mode_job = None;
        match outcome {
            Ok(confirmed) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared.gpu_mode = Some(confirmed);
                    shared.gpu_mode_error = None;
                    shared.gpu_turbo = Some(confirmed.gpoc);
                }
                self.status_bad = false;
                self.status = match operation {
                    GpuModeOperation::Refresh => {
                        "OEM GPU snapshot refreshed · getter sent one Acer GPU notification".into()
                    }
                    GpuModeOperation::Apply(mode) => format!(
                        "OEM GPU {} · offsets, fan table and GPOC getter-confirmed",
                        mode.label()
                    ),
                };
            }
            Err(error) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared.gpu_mode = None;
                    shared.gpu_mode_error = Some(error.clone());
                    shared.gpu_turbo = None;
                }
                self.status = format!("failed: {error}");
                self.status_bad = true;
            }
        }
    }

    /// Push the current lighting event using the corresponding Covini call path.
    fn apply_lighting(&mut self, update: LightingUpdate) {
        let Some(dev) = self.device() else { return };
        let idx = self.effect as usize;
        let colours = self.zone_colours.map(to_colour);
        let hardware = (|| -> alien_core::Result<String> {
            Ok(match update {
                LightingUpdate::CompleteStatic => {
                    dev.set_zone_colours_enabled(colours, self.zone_enabled, self.brightness)?;
                    "static lighting applied".to_owned()
                }
                LightingUpdate::StaticIncremental {
                    previous_enabled,
                    colour_changed,
                    brightness_changed,
                } => {
                    if previous_enabled != self.zone_enabled {
                        dev.update_zone_enabled(colours, previous_enabled, self.zone_enabled)?;
                    } else {
                        dev.prepare_lighting(self.zone_enabled)?;
                    }

                    // A newly enabled zone was already restored immediately
                    // after the mask, just as Zone_Checkbox_Checked does.
                    for (index, changed) in colour_changed.into_iter().enumerate() {
                        if changed
                            && self.zone_enabled[index]
                            && previous_enabled[index] == self.zone_enabled[index]
                        {
                            dev.set_zone_colour(Zone::ALL[index], colours[index])?;
                        }
                    }
                    if brightness_changed {
                        dev.set_static_brightness_and_colours(
                            colours,
                            self.zone_enabled,
                            self.brightness,
                        )?;
                    }
                    "static lighting updated".to_owned()
                }
                LightingUpdate::Dynamic => {
                    let set = self.per_effect[idx];
                    let colour = to_colour(set.colour);
                    let speed = set.speed.max(1);
                    let direction = if set.left_to_right {
                        Direction::LeftToRight
                    } else {
                        Direction::RightToLeft
                    };
                    dev.prepare_lighting(self.zone_enabled)?;
                    dev.set_effect(self.effect, speed, self.brightness, direction, colour)?;
                    format!(
                        "{} request accepted · optical effect unverified",
                        self.effect.name()
                    )
                }
            })
        })();
        let result = match hardware {
            Ok(result) => result,
            Err(error) => {
                self.status = format!("lighting may be partially changed: {error}");
                self.status_bad = true;
                self.reload_lighting_memory();
                return;
            }
        };

        if self.effect == Effect::Static {
            self.mem.set_zone_colours(colours);
            self.mem.set_zone_enabled(self.zone_enabled);
            self.mem.set_colour(Effect::Static, colours[0]);
        } else {
            let set = self.per_effect[idx];
            let colour = to_colour(set.colour);
            let direction = if set.left_to_right {
                Direction::LeftToRight
            } else {
                Direction::RightToLeft
            };
            self.mem.set_colour(self.effect, colour);
            self.mem.set_speed(self.effect, set.speed.max(1));
            self.mem.set_direction(self.effect, direction);
        }
        self.mem.set_brightness(self.brightness);
        match self.mem.save() {
            Ok(()) => {
                self.status = result;
                self.status_bad = false;
                self.profile = None;
            }
            Err(error) => {
                self.status = format!("hardware changed, but settings were not saved: {error}");
                self.status_bad = true;
            }
        }
    }
}

fn to_colour(v: [f32; 3]) -> Colour {
    Colour::new(
        (v[0] * 255.0).round() as u8,
        (v[1] * 255.0).round() as u8,
        (v[2] * 255.0).round() as u8,
    )
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.running.store(false, Ordering::Relaxed);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_gpu_mode_job(ctx);
        self.want_duty.store(
            self.tab == Tab::Fans && self.fan_mode == FanMode::Manual,
            Ordering::Relaxed,
        );

        let snap = {
            let sh = self.shared.lock().ok();
            match sh {
                Some(s) => Snapshot {
                    sensors: s.sensors,
                    performance: s.performance,
                    cpu_turbo: s.cpu_turbo,
                    gpu_turbo: s.gpu_turbo,
                    gpu_mode: s.gpu_mode,
                    gpu_mode_error: s.gpu_mode_error.clone(),
                    coolboost: s.coolboost,
                    keyboard_timeout: s.keyboard_timeout,
                    lcd_overdrive: s.lcd_overdrive,
                    backlight: s.backlight,
                    caps: s.caps.clone().unwrap_or_default(),
                    cpu_hist: s.cpu_hist.clone(),
                    gpu_hist: s.gpu_hist.clone(),
                    cpu_rpm_hist: s.cpu_rpm_hist.clone(),
                    gpu_rpm_hist: s.gpu_rpm_hist.clone(),
                    fan_readback: s.fan_readback,
                    daemon_attempt_finished: s.link.is_some(),
                    link: s.link.clone().unwrap_or_default(),
                    interface: s.interface.clone(),
                },
                None => return,
            }
        };

        // The splash is a view of the poller's first connection attempt, not
        // a sleep and not a second connection path. Every socket and firmware
        // call remains on the background thread above.
        if let Some(frame) = self
            .splash
            .frame(Instant::now(), snap.daemon_attempt_finished)
        {
            let outcome = if !snap.daemon_attempt_finished {
                splash::Outcome::Contacting
            } else {
                match snap.link.link {
                    Link::Never => splash::Outcome::Unreachable,
                    Link::Up => splash::Outcome::Connected,
                    Link::Lost => splash::Outcome::LinkLost,
                }
            };
            splash::show(ctx, self.logo.id(), frame, outcome);
            ctx.request_repaint_after(if frame.reduced_motion {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(16)
            });
            return;
        }

        // Seed the editable controls from the hardware exactly once, so the
        // controls open where the machine is rather than snapping it to a
        // default the moment the user touches anything.
        if !self.seeded {
            if let Some(b) = snap.backlight {
                // Seed every inactive effect from the user's remembered
                // settings. Static zone colours must also come from that
                // store: the firmware's single RGB field is not the four zone
                // registers that are actually on the keyboard.
                self.reload_lighting_memory();

                // The active effect and its controls come from firmware. This
                // matters after another client (the CLI, TUI, or firmware)
                // changed the keyboard: showing remembered values here would
                // make the first render disagree with the machine.
                self.effect = b.effect;
                self.brightness = covini_brightness(b.brightness);
                if b.effect != Effect::Static {
                    let active = &mut self.per_effect[b.effect as usize];
                    active.speed = b.speed.max(1);
                    active.left_to_right = !b.reverse;
                    if b.effect.honours_colour() {
                        let pattern_colour = [
                            b.colour.r as f32 / 255.0,
                            b.colour.g as f32 / 255.0,
                            b.colour.b as f32 / 255.0,
                        ];
                        for effect in Effect::ALL {
                            if effect != Effect::Static {
                                self.per_effect[effect as usize].colour = pattern_colour;
                            }
                        }
                    }
                }
                self.seeded = true;
            }
        }

        // The nav rail numbers its entries 01–05; the number keys follow it.
        let pressed = ctx.input(|i| {
            [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
            ]
            .iter()
            .position(|k| i.key_pressed(*k))
        });
        if let Some(i) = pressed {
            self.tab = Tab::ALL[i].0;
        }

        let narrow = ctx.screen_rect().width() < NARROW;
        self.top_bar(ctx, &snap, narrow);

        if snap.link.link == Link::Never {
            self.status_bar(ctx, &snap);
            self.first_run(ctx, &snap);
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        }

        if snap.link.link == Link::Lost {
            self.lost_banner(ctx, &snap);
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        self.side_nav(ctx, &snap);
        self.status_bar(ctx, &snap);

        let live = snap.link.link == Link::Up;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(20, 16)),
            )
            .show(ctx, |ui| {
                theme::scanlines(ui.painter(), ui.max_rect().expand(20.0));
                // A dropped link disables every control while leaving the last
                // readings on screen. Blanking them would lose the very thing
                // you want to look at when something has just gone wrong.
                ui.add_enabled_ui(live, |ui| {
                    ui.set_opacity(if live { 1.0 } else { 0.55 });
                    match self.tab {
                        Tab::Dashboard => self.dashboard(ui, &snap, narrow),
                        Tab::Fans => self.fans(ui, &snap),
                        Tab::Lighting => self.lighting(ui, &snap),
                        Tab::Performance => self.performance(ui, &snap),
                        Tab::About => self.about(ui, &snap),
                    }
                });
            });

        if self.profile_editor {
            self.profile_editor(ctx, &snap);
        }
        if self.gpu_mode_confirm.is_some() {
            self.gpu_mode_confirmation(ctx);
        }
        if self.compatible_models_open {
            self.compatible_models_dialog(ctx);
        }
    }
}

// ── Chrome ──────────────────────────────────────────────────────────────────

impl App {
    fn top_bar(&mut self, ctx: &egui::Context, snap: &Snapshot, narrow: bool) {
        egui::TopBottomPanel::top("top")
            .exact_height(TOP_H)
            .frame(egui::Frame::NONE.fill(theme::PANEL))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let p = ui.painter();
                p.hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    theme::hair(theme::LINE),
                );

                let y = rect.center().y;
                glyph::logo(p, self.logo.id(), Pos2::new(rect.left() + 26.0, y), 30.0);

                let mut x = rect.left() + 44.0;
                x += theme::tracked(
                    ctx,
                    p,
                    Pos2::new(x, y),
                    "ALIEN",
                    theme::sans_b(15.0),
                    theme::BRIGHT,
                    6.0,
                ) + 18.0;

                let model = if narrow {
                    product()
                } else {
                    model_name().to_uppercase()
                };
                p.text(
                    Pos2::new(x, y),
                    egui::Align2::LEFT_CENTER,
                    model,
                    theme::mono(11.0),
                    theme::MUTED,
                );

                // Right-hand furniture, laid out right to left. Only call an
                // OEM mode selected when NVML graphics+memory and both Acer
                // firmware legs agree; the raw GPOC flag alone is not a mode.
                let dead = snap.link.link != Link::Up;
                let confirmed = snap.gpu_mode.and_then(GpuModeState::confirmed_mode);
                let (mark, label, colour) = if dead {
                    (None, "—", theme::DIM)
                } else if confirmed == Some(GpuMode::Turbo) {
                    (Some(Mark::Bar), "GPU TURBO SNAP", theme::AMBER)
                } else if confirmed == Some(GpuMode::Faster) {
                    (Some(Mark::Bar), "GPU FASTER SNAP", theme::AMBER)
                } else if confirmed == Some(GpuMode::Normal) {
                    (Some(Mark::Dot), "GPU NORMAL SNAP", theme::MUTED)
                } else if snap.gpu_mode.is_some() {
                    (Some(Mark::Bar), "GPU SPLIT SNAP", theme::RED)
                } else {
                    (Some(Mark::Dot), "GPU MODE N/A", theme::DIM)
                };

                let font = theme::mono_b(10.0);
                let tw = theme::tracked_width(ctx, label, &font, 0.0);
                let pill_w = tw + if mark.is_some() { 32.0 } else { 20.0 };
                let pill = egui::Rect::from_min_size(
                    Pos2::new(rect.right() - 16.0 - pill_w, y - 11.0),
                    egui::vec2(pill_w, 22.0),
                );
                p.rect_stroke(
                    pill,
                    0.0,
                    theme::hair(if dead {
                        theme::LINE
                    } else {
                        colour.gamma_multiply(0.6)
                    }),
                    StrokeKind::Inside,
                );
                let mut tx = pill.left() + 10.0;
                if let Some(m) = mark {
                    glyph::draw(p, Pos2::new(tx + 4.0, y), m, 8.0, colour);
                    tx += 14.0;
                }
                p.text(
                    Pos2::new(tx, y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    font,
                    colour,
                );
            });
    }

    /// The amber strip that appears when a live link drops.
    fn lost_banner(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::TopBottomPanel::top("lost")
            .exact_height(34.0)
            .frame(egui::Frame::NONE.fill(theme::AMBER_BG))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let p = ui.painter();
                p.hline(rect.x_range(), rect.bottom() - 0.5, theme::hair(theme::AMBER));
                let y = rect.center().y;
                glyph::draw(p, Pos2::new(rect.left() + 20.0, y), Mark::TriUp, 9.0, theme::AMBER);
                p.text(
                    Pos2::new(rect.left() + 32.0, y),
                    egui::Align2::LEFT_CENTER,
                    "LINK LOST",
                    theme::mono(11.0),
                    theme::AMBER,
                );
                let secs = snap
                    .link
                    .next_try
                    .map(|t| t.saturating_duration_since(Instant::now()).as_secs() + 1)
                    .unwrap_or(0);
                p.text(
                    Pos2::new(rect.left() + 108.0, y),
                    egui::Align2::LEFT_CENTER,
                    format!(
                        "socket closed by peer · reconnecting in {secs} s (attempt {}) · controls disabled, last readings frozen",
                        snap.link.attempts
                    ),
                    theme::mono(11.0),
                    theme::MUTED,
                );
            });
    }

    fn side_nav(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::SidePanel::left("nav")
            .exact_width(NAV_W)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(theme::PANEL))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter()
                    .vline(full.right() - 0.5, full.y_range(), theme::hair(theme::LINE));

                let live = snap.link.link == Link::Up;
                ui.add_space(10.0);
                for (tab, label, num) in Tab::ALL {
                    let selected = self.tab == tab;
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(NAV_W, 34.0), egui::Sense::click());
                    if resp.clicked() {
                        self.tab = tab;
                    }
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let p = ui.painter();
                    let (fg, num_fg) = match (selected, live) {
                        (true, true) => (theme::BRIGHT, theme::GREEN),
                        (true, false) => (theme::MUTED, theme::DIM),
                        (false, true) => (theme::TEXT, theme::DIM),
                        (false, false) => (theme::DIM, theme::DIM),
                    };
                    if selected {
                        p.rect_filled(rect, 0.0, theme::RAISED);
                        p.rect_filled(
                            egui::Rect::from_min_size(
                                rect.left_top(),
                                egui::vec2(3.0, rect.height()),
                            ),
                            0.0,
                            if live { theme::GREEN } else { theme::DIM },
                        );
                    } else if resp.hovered() {
                        p.rect_filled(rect, 0.0, theme::RAISED.gamma_multiply(0.6));
                    }
                    let y = rect.center().y;
                    p.text(
                        Pos2::new(rect.left() + 14.0, y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        theme::sans(13.0),
                        fg,
                    );
                    p.text(
                        Pos2::new(rect.right() - 14.0, y),
                        egui::Align2::RIGHT_CENTER,
                        num,
                        theme::mono(9.0),
                        num_fg,
                    );
                }

                // Daemon footer, pinned to the bottom.
                let foot_h = 46.0;
                let foot = egui::Rect::from_min_max(
                    Pos2::new(full.left(), full.bottom() - foot_h),
                    full.right_bottom(),
                );
                let p = ui.painter();
                p.hline(foot.x_range(), foot.top(), theme::hair(theme::LINE));
                let (mark, word, colour) = match snap.link.link {
                    Link::Up => (Mark::Dot, "LINK 1 Hz", theme::OK),
                    _ => (Mark::Cross, "NO LINK", theme::RED),
                };
                p.text(
                    Pos2::new(foot.left() + 14.0, foot.top() + 15.0),
                    egui::Align2::LEFT_CENTER,
                    "DAEMON",
                    theme::mono(9.0),
                    theme::DIM,
                );
                glyph::draw(
                    p,
                    Pos2::new(foot.left() + 66.0, foot.top() + 15.0),
                    mark,
                    7.0,
                    colour,
                );
                p.text(
                    Pos2::new(foot.left() + 74.0, foot.top() + 15.0),
                    egui::Align2::LEFT_CENTER,
                    word,
                    theme::mono(9.0),
                    colour,
                );
                p.text(
                    Pos2::new(foot.left() + 14.0, foot.top() + 30.0),
                    egui::Align2::LEFT_CENTER,
                    socket_path(),
                    theme::mono(9.0),
                    theme::DIM,
                );
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(FOOT_H)
            .frame(egui::Frame::NONE.fill(theme::PANEL))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let p = ui.painter();
                p.hline(rect.x_range(), rect.top(), theme::hair(theme::LINE));
                let y = rect.center().y;

                let down = snap.link.link != Link::Up;
                let bad = self.status_bad || down;
                let text = if down {
                    if snap.link.link == Link::Never {
                        format!("cannot connect: {}", snap.link.error)
                    } else {
                        "socket closed by peer — is alien-daemon running?".to_owned()
                    }
                } else {
                    self.status.clone()
                };

                glyph::draw(
                    p,
                    Pos2::new(rect.left() + 18.0, y),
                    if bad { Mark::Cross } else { Mark::Chevron },
                    9.0,
                    if bad { theme::RED } else { theme::GREEN },
                );
                // The right-hand path is fixed furniture; the message on the
                // left is whatever the last error said and can be long. Give
                // the message the space that is left and elide it there, so a
                // verbose transport error does not print straight through the
                // path — which is exactly what the first-run message did.
                let font = theme::mono(11.0);
                let right = if snap.interface.is_empty() {
                    socket_path()
                } else {
                    snap.interface.clone()
                };
                let right_w = theme::tracked_width(ui.ctx(), &right, &font, 0.0);
                let room = (rect.width() - 30.0 - 14.0 - right_w - 18.0).max(80.0);
                let text = elide(ui.ctx(), &text, &font, room);

                p.text(
                    Pos2::new(rect.left() + 30.0, y),
                    egui::Align2::LEFT_CENTER,
                    text,
                    font.clone(),
                    theme::MUTED,
                );
                p.text(
                    Pos2::new(rect.right() - 14.0, y),
                    egui::Align2::RIGHT_CENTER,
                    right,
                    font,
                    theme::DIM,
                );
            });
    }
}

// ── Screens ─────────────────────────────────────────────────────────────────

impl App {
    fn dashboard(&mut self, ui: &mut egui::Ui, snap: &Snapshot, narrow: bool) {
        let viewport_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt("dashboard-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Preserve the dashboard's full-width geometry when no scroll
                // bar is needed. If the compositor forces a shorter viewport,
                // this same area clips above the pinned status panel and makes
                // the profiles reachable by scrolling instead of painting them
                // through that panel.
                let content_width = ui.available_width();
                ui.set_min_width(content_width);
                self.dashboard_content(ui, snap, narrow, viewport_height);
            });
    }

    fn dashboard_content(
        &mut self,
        ui: &mut egui::Ui,
        snap: &Snapshot,
        narrow: bool,
        viewport_height: f32,
    ) {
        let content_top = ui.cursor().top();
        theme::section(ui, "TELEMETRY", Some("1 Hz"));
        ui.add_space(10.0);

        // Gauge plate.
        let s = &snap.sensors;
        let gsize = if narrow { 104.0 } else { 128.0 };
        let plate_h = gsize + 20.0;
        let plate =
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), plate_h));
        theme::card(ui.painter(), plate);
        theme::corner_ticks(ui.painter(), plate, theme::GREEN);
        ui.scope_builder(egui::UiBuilder::new().max_rect(plate.shrink(10.0)), |ui| {
            ui.horizontal(|ui| {
                // Space-around by hand: five gauges and a divider with equal
                // air in the seven gaps around them. egui has no such layout,
                // and item_spacing fights any attempt to fake one — so the
                // spacing is zeroed and every gap is placed explicitly.
                ui.spacing_mut().item_spacing.x = 0.0;
                let pad = ((ui.available_width() - 5.0 * gsize - 1.0) / 7.0).max(2.0);
                ui.add_space(pad);
                gauge::Gauge::temp("CPU", s.cpu_temp_c).show(ui, gsize);
                ui.add_space(pad);
                gauge::Gauge::temp("GPU", s.gpu_temp_c).show(ui, gsize);
                ui.add_space(pad);
                gauge::Gauge::temp("BOARD", s.system_temp_c).show(ui, gsize);
                ui.add_space(pad);
                // Temperatures left of the rule, fan speeds right: different
                // kinds of number, and the eye should not have to read the
                // unit to know which it is looking at.
                theme::vrule(ui, gsize * 0.86);
                ui.add_space(pad);
                gauge::Gauge::fan("CPU FAN", s.cpu_fan_rpm, 6000.0).show(ui, gsize);
                ui.add_space(pad);
                gauge::Gauge::fan("GPU FAN", s.gpu_fan_rpm, 6500.0).show(ui, gsize);
            });
        });
        ui.advance_cursor_after_rect(plate);
        ui.add_space(12.0);

        // History and state, side by side. Budget from the real viewport rather
        // than `available_height()`: inside a vertical ScrollArea that value is
        // intentionally unbounded. The old 65 px estimate omitted egui's two
        // automatic 8 px item gaps, so the chips overran the central panel by
        // 14 px and landed only 2 px above the status bar after its 16 px frame
        // margin. The plan accounts for every fixed gap and reserves a viewport-
        // proportional bottom gutter. At compositor-forced short heights the
        // middle row stays legible and the outer ScrollArea owns the overflow.
        let consumed_height = ui.cursor().top() - content_top;
        let vertical = dashboard_vertical_plan(
            viewport_height,
            consumed_height,
            ui.spacing().item_spacing.y,
        );
        let mid_h = vertical.middle_height;
        let total_w = ui.available_width();
        let gap = 12.0;
        let hist_w = if narrow {
            total_w
        } else {
            (total_w - gap) * 0.63
        };

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(hist_w, mid_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(ui, "HISTORY", Some("120 s"));
                    ui.add_space(8.0);
                    // Natural height, not the whole row: in the design only
                    // the STATE panel stretches. Letting these grow too turned
                    // three compact plots into three near-empty boxes.
                    let plots_h = ui.available_height().min(104.0);
                    // Three plots at full width, two when the window is halved
                    // — the third would be too narrow to read anything from.
                    let cols = if narrow { 2 } else { 3 };
                    let pw = (hist_w - (cols as f32 - 1.0) * 10.0) / cols as f32;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        gauge::SparkCard {
                            title: "CPU °C",
                            values: &snap.cpu_hist,
                            colour: theme::GREEN,
                            reading: fmt_opt(snap.sensors.cpu_temp_c),
                            reading_colour: theme::temp_colour(
                                snap.sensors.cpu_temp_c.unwrap_or(0),
                            ),
                            mark: snap.sensors.cpu_temp_c.map(|_| Mark::Dot),
                            min_span: 10.0,
                        }
                        .show(ui, egui::vec2(pw, plots_h));
                        gauge::SparkCard {
                            title: "GPU °C",
                            values: &snap.gpu_hist,
                            colour: theme::GREEN,
                            reading: fmt_opt(snap.sensors.gpu_temp_c),
                            reading_colour: theme::temp_colour(
                                snap.sensors.gpu_temp_c.unwrap_or(0),
                            ),
                            mark: snap.sensors.gpu_temp_c.map(|_| Mark::Dot),
                            min_span: 10.0,
                        }
                        .show(ui, egui::vec2(pw, plots_h));
                        if cols == 3 {
                            gauge::SparkCard {
                                title: "CPU FAN RPM",
                                values: &snap.cpu_rpm_hist,
                                colour: theme::BRIGHT,
                                reading: fmt_opt(snap.sensors.cpu_fan_rpm),
                                reading_colour: theme::TEXT,
                                mark: None,
                                min_span: 600.0,
                            }
                            .show(ui, egui::vec2(pw, plots_h));
                        }
                    });
                },
            );

            if !narrow {
                ui.add_space(gap);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), mid_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        theme::section(ui, "STATE", None);
                        ui.add_space(8.0);
                        let h = ui.available_height();
                        let rect = egui::Rect::from_min_size(
                            ui.cursor().min,
                            egui::vec2(ui.available_width(), h),
                        );
                        theme::card(ui.painter(), rect);
                        self.state_rows(ui, snap, rect);
                        ui.advance_cursor_after_rect(rect);
                    },
                );
            }
        });

        ui.add_space(12.0);
        theme::section(ui, "PROFILES", None);
        ui.add_space(8.0);
        dashboard_profile_strip(ui, |ui| {
            for p in self.profiles.clone() {
                let active = self.profile.as_deref() == Some(p.name.as_str());
                let style = if active {
                    theme::ChipStyle::Active
                } else if p.name == "turbo" {
                    theme::ChipStyle::Warn
                } else {
                    theme::ChipStyle::Idle
                };
                let label = if p.name == "turbo" {
                    "MAX + RED".to_owned()
                } else {
                    p.name.to_uppercase()
                };
                let response = theme::chip(ui, &label, style, 11.0).on_hover_text(&p.description);
                if response.gained_focus() {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                if response.clicked() {
                    if let Some(dev) = self.device() {
                        let ignored_gpu_flag = p.deprecated_gpu_flag_ignored();
                        let r = p.apply(&dev);
                        let ok = r.is_ok();
                        let success = if ignored_gpu_flag {
                            format!(
                                "applied profile: {}; deprecated raw GPU flag ignored",
                                p.name
                            )
                        } else {
                            format!("applied profile: {}; GPU mode unchanged", p.name)
                        };
                        self.act(r, &success);
                        if ok {
                            self.profile = Some(p.name.clone());
                            if let Some(fans) = &p.fans {
                                self.fan_mode = match fans {
                                    FanPolicy::Auto => FanMode::Auto,
                                    FanPolicy::Max => FanMode::Max,
                                    FanPolicy::Manual { cpu, gpu } => {
                                        self.cpu_pct = *cpu;
                                        self.gpu_pct = *gpu;
                                        FanMode::Manual
                                    }
                                    // A split policy sets each fan
                                    // independently. The three chips cannot
                                    // draw "CPU on the EC curve, GPU at 70%",
                                    // so only the all-manual case maps onto
                                    // them; a genuinely mixed profile falls
                                    // back to the existing "MODE UNKNOWN"
                                    // caption rather than a chip that lies
                                    // about what the hardware is doing.
                                    FanPolicy::Split { cpu, gpu } => {
                                        if let FanSide::Manual { percent } = cpu {
                                            self.cpu_pct = *percent;
                                        }
                                        if let FanSide::Manual { percent } = gpu {
                                            self.gpu_pct = *percent;
                                        }
                                        match (cpu, gpu) {
                                            (
                                                FanSide::Manual { .. },
                                                FanSide::Manual { .. },
                                            ) => FanMode::Manual,
                                            _ => FanMode::Unknown,
                                        }
                                    }
                                };
                            }
                            self.reload_lighting_memory();
                            if let Some(backlight) = &p.backlight {
                                if let Some(effect) = Effect::parse(&backlight.effect) {
                                    self.effect = effect;
                                }
                            }
                        }
                    }
                }
            }
            let new_profile = theme::chip(ui, "+ NEW", theme::ChipStyle::Outline, 11.0);
            if new_profile.gained_focus() {
                new_profile.scroll_to_me(Some(egui::Align::Center));
            }
            if new_profile.clicked() {
                self.profile_name.clear();
                self.profile_editor = true;
                self.profile_editor_focus = true;
            }
            if !narrow {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("performance + lighting")
                        .font(theme::mono(10.0))
                        .color(theme::DIM),
                );
            }
        });
        ui.add_space(vertical.bottom_padding);
    }

    fn state_rows(&mut self, ui: &mut egui::Ui, snap: &Snapshot, rect: egui::Rect) {
        let p = ui.painter();
        let font = theme::mono(11.0);
        let rows = 4;
        let step = rect.height() / rows as f32;
        let mut y = rect.top() + step / 2.0;
        let lx = rect.left() + 14.0;
        let rx = rect.right() - 14.0;

        let put = |p: &egui::Painter, y: f32, k: &str, v: &str, c: Color32| {
            p.text(
                Pos2::new(lx, y),
                egui::Align2::LEFT_CENTER,
                k,
                font.clone(),
                theme::MUTED,
            );
            let key_width = theme::tracked_width(ui.ctx(), k, &font, 0.0);
            let room = (rx - lx - key_width - 16.0).max(0.0);
            let value = elide(ui.ctx(), v, &font, room);
            p.text(
                Pos2::new(rx, y),
                egui::Align2::RIGHT_CENTER,
                value,
                font.clone(),
                c,
            );
        };

        let cpu_state = match snap.caps.cpu_overclock {
            Support::Yes => snap.cpu_turbo.map(flag_name).unwrap_or("unavailable"),
            Support::No => "unavailable",
            Support::Unverified => match snap.cpu_turbo {
                Some(2) => "flag set",
                Some(0) => "flag clear",
                Some(_) => "flag unknown",
                None => "unavailable",
            },
            Support::Unknown => "unknown",
        };
        put(
            p,
            y,
            "raw flags",
            &format!(
                "cpu {cpu_state} · gpu {}",
                snap.gpu_turbo.map(flag_name).unwrap_or("unavailable")
            ),
            theme::TEXT,
        );
        y += step;

        // Backlight: a swatch of the live colour, then the mode and level.
        p.text(
            Pos2::new(lx, y),
            egui::Align2::LEFT_CENTER,
            "backlight",
            font.clone(),
            theme::MUTED,
        );
        match snap.backlight {
            Some(b) => {
                let text = format!("{} · {}%", b.effect.name(), b.brightness);
                let tw = theme::tracked_width(ui.ctx(), &text, &font, 0.0);
                p.text(
                    Pos2::new(rx, y),
                    egui::Align2::RIGHT_CENTER,
                    &text,
                    font.clone(),
                    theme::TEXT,
                );
                // Static uses four independent function-7 zone registers and
                // has no single live colour. Do not substitute a remembered
                // zone-one preference and paint it as current hardware state.
                if b.effect != Effect::Static {
                    let sw = egui::Rect::from_center_size(
                        Pos2::new(rx - tw - 12.0, y),
                        egui::vec2(14.0, 9.0),
                    );
                    p.rect_filled(
                        sw,
                        0.0,
                        Color32::from_rgb(b.colour.r, b.colour.g, b.colour.b),
                    );
                }
            }
            None => {
                p.text(
                    Pos2::new(rx, y),
                    egui::Align2::RIGHT_CENTER,
                    "—",
                    font.clone(),
                    theme::DIM,
                );
            }
        }
        y += step;

        put(
            p,
            y,
            "profile",
            self.profile.as_deref().unwrap_or("—"),
            if self.profile.is_some() {
                theme::BRIGHT
            } else {
                theme::DIM
            },
        );
        y += step;
        put(
            p,
            y,
            "samples",
            &format!("{} / {}", snap.cpu_hist.len(), HISTORY),
            theme::TEXT,
        );
    }

    fn captured_profile(&self, _snap: &Snapshot, name: String) -> Profile {
        let fans = match self.fan_mode {
            FanMode::Unknown => None,
            FanMode::Auto => Some(FanPolicy::Auto),
            FanMode::Max => Some(FanPolicy::Max),
            FanMode::Manual => Some(FanPolicy::Manual {
                cpu: self.cpu_pct,
                gpu: self.gpu_pct,
            }),
        };
        let setting = self.per_effect[self.effect as usize];
        let colour = if self.effect == Effect::Static {
            to_colour(self.zone_colours[0])
        } else {
            to_colour(setting.colour)
        };
        let zones = (self.effect == Effect::Static)
            .then(|| self.zone_colours.map(|value| to_colour(value).to_hex()));
        Profile {
            name,
            description: "User profile".into(),
            fans,
            // Raw GPOC is deliberately excluded: replaying it separately can
            // split the guarded OEM GPU-mode transaction.
            gpu_turbo: None,
            backlight: Some(ProfileBacklight {
                effect: self.effect.name().into(),
                speed: if self.effect == Effect::Static {
                    0
                } else {
                    setting.speed.max(1)
                },
                brightness: self.brightness,
                colour: colour.to_hex(),
                reverse: self.effect.honours_direction() && !setting.left_to_right,
                zones,
                zone_enabled: (self.effect == Effect::Static).then_some(self.zone_enabled),
            }),
        }
    }

    fn profile_editor(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        let mut open = self.profile_editor;
        let mut close = false;
        egui::Window::new("New profile")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(
                    RichText::new(
                        "Save the current fan and lighting state. GPU mode is unchanged.",
                    )
                    .font(theme::mono(10.5))
                    .color(theme::MUTED),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("NAME")
                            .font(theme::mono(11.0))
                            .color(theme::MUTED),
                    );
                    let name = ui.add(
                        egui::TextEdit::singleline(&mut self.profile_name).desired_width(250.0),
                    );
                    if self.profile_editor_focus {
                        name.request_focus();
                        self.profile_editor_focus = false;
                    }
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if theme::chip(ui, "SAVE CURRENT", theme::ChipStyle::Active, 11.0).clicked() {
                        let name = self.profile_name.trim().to_string();
                        let profile = self.captured_profile(snap, name.clone());
                        match profile::save(&profile) {
                            Ok(_) => {
                                self.profiles = profile::list();
                                self.profile = Some(name.clone());
                                self.status = format!("saved profile: {name}");
                                self.status_bad = false;
                                close = true;
                            }
                            Err(error) => {
                                self.status = format!("profile not saved: {error}");
                                self.status_bad = true;
                            }
                        }
                    }
                    if theme::chip(ui, "CANCEL", theme::ChipStyle::Idle, 11.0).clicked() {
                        close = true;
                    }
                });
            });
        self.profile_editor = open && !close;
    }

    fn fans(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        theme::section(ui, "FAN MODE", None);
        ui.add_space(10.0);

        let can = snap.caps.fan_control != Support::No;
        card_body(ui, cw, |ui| {
            ui.horizontal(|ui| {
                for (mode, label) in [
                    (FanMode::Max, "MAXIMUM"),
                    (FanMode::Auto, "AUTOMATIC"),
                    (FanMode::Manual, "MANUAL"),
                ] {
                    let style = if !can {
                        theme::ChipStyle::Disabled
                    } else if self.fan_mode == mode {
                        if mode == FanMode::Manual {
                            theme::ChipStyle::Outline
                        } else {
                            theme::ChipStyle::Active
                        }
                    } else {
                        theme::ChipStyle::Idle
                    };
                    let resp = theme::chip(ui, label, style, 11.0);
                    if mode == FanMode::Manual {
                        // The caret says this one opens something: choosing it
                        // reveals the duty sliders rather than acting at once.
                        glyph::draw(
                            ui.painter(),
                            resp.rect.right_center() - egui::vec2(9.0, 0.0),
                            Mark::TriDown,
                            7.0,
                            if self.fan_mode == mode {
                                theme::BRIGHT
                            } else {
                                theme::MUTED
                            },
                        );
                    }
                    if resp.clicked() {
                        let Some(dev) = self.device() else { return };
                        let r = match mode {
                            FanMode::Max => dev.fans_max(),
                            FanMode::Auto => dev.fans_auto(),
                            FanMode::Manual => {
                                let r = dev
                                    .set_fan_percent(Fan::Cpu, self.cpu_pct)
                                    .and_then(|_| dev.set_fan_percent(Fan::Gpu, self.gpu_pct));
                                if r.is_ok() {
                                    self.duty_sent = [Some(Instant::now()); 2];
                                }
                                r
                            }
                            FanMode::Unknown => unreachable!(),
                        };
                        let ok = r.is_ok();
                        let success = match mode {
                            FanMode::Max => "fans at maximum",
                            FanMode::Auto => "fans on the EC curve",
                            FanMode::Manual => "manual duty",
                            FanMode::Unknown => unreachable!(),
                        };
                        self.act(r, success);
                        if ok {
                            self.fan_mode = mode;
                        }
                    }
                }
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new(if self.fan_mode == FanMode::Unknown {
                    "MODE UNKNOWN — firmware has no getter; choose a mode to establish it"
                } else {
                    "MODE HIGHLIGHT = last command confirmed from this window"
                })
                .font(theme::mono(9.5))
                .color(theme::DIM),
            );
            ui.add_space(6.0);
            runs(
                ui,
                11.0,
                &[
                    ("Maximum is worth roughly ", theme::MUTED),
                    ("+61.8% sustained CPU throughput", theme::BRIGHT),
                    (" on this chassis: the stock EC curve holds the processor in thermal throttle.", theme::MUTED),
                ],
            );
        });

        ui.add_space(12.0);
        theme::section(ui, "MANUAL DUTY", None);
        ui.add_space(10.0);

        let manual = self.fan_mode == FanMode::Manual && snap.caps.manual_fan_duty != Support::No;
        card_body(ui, cw, |ui| {
            for (i, (name, fan)) in [("CPU", Fan::Cpu), ("GPU", Fan::Gpu)]
                .into_iter()
                .enumerate()
            {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(name)
                            .font(theme::mono(11.0))
                            .color(if manual { theme::MUTED } else { theme::DIM }),
                    );
                    ui.add_space(6.0);
                    let want = if i == 0 {
                        &mut self.cpu_pct
                    } else {
                        &mut self.gpu_pct
                    };
                    let target = *want;
                    let sw = (ui.available_width() - 270.0).max(120.0);
                    let r = theme::slider(ui, want, 0..=100, sw, manual);
                    let now = *want;
                    // Send on release, not on every pixel of drag: each change
                    // is a firmware call, and streaming them while the mouse
                    // moves would hammer the EC with values the user is only
                    // passing through.
                    if r.drag_stopped() || (r.clicked() && now != target) {
                        if let Some(dev) = self.device() {
                            let v = now;
                            let res = dev.set_fan_percent(fan, v);
                            self.duty_sent[i] = Some(Instant::now());
                            self.act(res, &format!("{} fan → {}%", name.to_lowercase(), v));
                        }
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "{:>3} %",
                            if i == 0 { self.cpu_pct } else { self.gpu_pct }
                        ))
                        .font(theme::mono_b(12.0))
                        .color(if manual {
                            theme::TEXT
                        } else {
                            theme::DIM
                        }),
                    );
                    ui.add_space(10.0);
                    self.readback_line(ui, snap, i, manual);
                });
                ui.add_space(6.0);
            }
            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "Sent when released and confirmed by firmware readback. Fans take 8–10 s to settle.",
                )
                .font(theme::mono(10.0))
                .color(theme::DIM),
            );
        });

        ui.add_space(12.0);
        theme::section(ui, "NOW", None);
        ui.add_space(10.0);

        // Respect the space left by the two control cards. Any artificial
        // minimum pushes the gauges underneath the status bar at 760x520.
        let h = ui.available_height();
        if h < 24.0 {
            return;
        }
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), h));
        theme::card(ui.painter(), rect);
        let inner = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(rect.width() - 32.0, (h - 8.0).clamp(1.0, 128.0)),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.horizontal_top(|ui| {
                let g = ui.available_height().min(128.0);
                gauge::Gauge::fan("CPU FAN", snap.sensors.cpu_fan_rpm, 6000.0).show(ui, g);
                ui.add_space(10.0);
                gauge::Gauge::fan("GPU FAN", snap.sensors.gpu_fan_rpm, 6500.0).show(ui, g);
                // Two stacked plots need enough height to be legible. The
                // compact view already shows both live RPM readings in the
                // gauges, so omit the plots instead of clipping the second one.
                if g >= 80.0 {
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        let w = ui.available_width();
                        let ph = ((g - 8.0) / 2.0).max(34.0);
                        gauge::SparkCard {
                            title: "CPU FAN · 120 s",
                            values: &snap.cpu_rpm_hist,
                            colour: theme::GREEN,
                            reading: fmt_opt(snap.sensors.cpu_fan_rpm),
                            reading_colour: theme::TEXT,
                            mark: None,
                            min_span: 600.0,
                        }
                        .show(ui, egui::vec2(w, ph));
                        ui.add_space(8.0);
                        gauge::SparkCard {
                            title: "GPU FAN · 120 s",
                            values: &snap.gpu_rpm_hist,
                            colour: theme::BRIGHT,
                            reading: fmt_opt(snap.sensors.gpu_fan_rpm),
                            reading_colour: theme::TEXT,
                            mark: None,
                            min_span: 600.0,
                        }
                        .show(ui, egui::vec2(w, ph));
                    });
                }
            });
        });
        ui.advance_cursor_after_rect(rect);
    }

    /// The honest readback column: what firmware says the duty is, and whether
    /// that has caught up with what we asked for.
    fn readback_line(&mut self, ui: &mut egui::Ui, snap: &Snapshot, i: usize, manual: bool) {
        let want = if i == 0 { self.cpu_pct } else { self.gpu_pct };
        let got = snap.fan_readback[i];
        let settling = self.duty_sent[i]
            .map(|t| t.elapsed() < Duration::from_secs(10))
            .unwrap_or(false);

        if !manual {
            ui.label(RichText::new("").font(theme::mono(10.0)));
            return;
        }
        match got {
            None => {
                ui.label(
                    RichText::new("readback —")
                        .font(theme::mono(10.0))
                        .color(theme::DIM),
                );
            }
            Some(v) if settling && v != want => {
                let blink = ui.input(|s| s.time) % 1.1 < 0.55;
                ui.label(
                    RichText::new(format!("readback {v}%"))
                        .font(theme::mono(10.0))
                        .color(theme::AMBER),
                );
                let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 12.0), egui::Sense::hover());
                if blink {
                    glyph::draw(ui.painter(), r.center(), Mark::Bar, 8.0, theme::AMBER);
                }
                ui.label(
                    RichText::new("SETTLING ~8 s")
                        .font(theme::mono(10.0))
                        .color(theme::AMBER),
                );
                ui.ctx().request_repaint_after(Duration::from_millis(120));
            }
            Some(v) => {
                glyph::labelled(
                    ui,
                    Mark::Dot,
                    &format!("readback {v}% CONFIRMED"),
                    theme::mono(10.0),
                    theme::OK,
                );
            }
        }
    }

    fn lighting(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        // Below this width the left-hand control cards cannot honestly fit
        // beside the preview: the four zone pickers plus their enable actions
        // would paint into the right column. Stack both halves and make the
        // page scroll instead. This is the layout reached by the declared
        // 760x520 minimum once the navigation rail and panel margins are gone.
        if ui.available_width() < 720.0 {
            egui::ScrollArea::vertical()
                .id_salt("lighting-narrow")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let w = ui.available_width();
                    ui.set_width(w);
                    self.lighting_left(ui, snap);
                    ui.add_space(16.0);
                    self.lighting_right(ui, snap);
                });
        } else {
            // 1.5 : 1, as the design has it.
            let (lr, rr) = split_columns(ui, 0.6, 16.0);
            ui.scope_builder(egui::UiBuilder::new().max_rect(lr), |ui| {
                ui.set_max_width(lr.width());
                self.lighting_left(ui, snap);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(rr), |ui| {
                ui.set_max_width(rr.width());
                self.lighting_right(ui, snap);
            });
        }
    }

    fn lighting_left(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        let can = snap.caps.backlight_effects != Support::No;
        // Covini's logical brightness level 1 is the off state. PredatorSense
        // leaves the brightness slider enabled but gates every effect, colour,
        // speed, direction and zone control until a non-zero tick is selected.
        let controls_enabled = can && self.brightness != 0;
        let previous_effect = self.effect;
        let previous_enabled = self.zone_enabled;
        theme::section(ui, "EFFECT", None);
        ui.add_space(10.0);

        let mut changed = false;
        let mut brightness_changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            for e in Effect::ALL {
                // The diamond marks effects the firmware drives from its own
                // palette. Marking them is what lets the colour picker
                // disappear without the disappearance looking like a bug.
                let r = theme::tag(
                    ui,
                    &e.name().to_uppercase(),
                    self.effect == e,
                    controls_enabled,
                    !e.honours_colour(),
                );
                if r.clicked() && self.effect != e {
                    self.effect = e;
                    changed = true;
                }
            }
        });
        ui.add_space(8.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, 12.0), egui::Sense::hover());
            glyph::draw(ui.painter(), r.center(), Mark::Diamond, 7.0, theme::DIM);
            // A label in a horizontal row defaults to Extend, not Wrap: this
            // one ran straight off the column and under the next one.
            ui.set_max_width(cw - 14.0);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.label(
                RichText::new("= firmware palette")
                    .font(theme::mono(10.0))
                    .color(theme::DIM),
            );
        });

        ui.add_space(12.0);
        let mut zone_colours_changed = [false; 4];
        if self.effect == Effect::Static {
            theme::section(ui, "STATIC ZONES", Some("COLOUR + ENABLE MASK"));
            ui.add_space(10.0);
            card_body(ui, cw, |ui| {
                ui.horizontal(|ui| {
                    for (i, colour_changed) in zone_colours_changed.iter_mut().enumerate() {
                        ui.vertical(|ui| {
                            ui.spacing_mut().interact_size = egui::vec2(52.0, 30.0);
                            // `.changed()` only. An earlier version also
                            // compared the value before and after, to catch
                            // edits the widget reported late — but the picker
                            // round-trips rgb through Color32 and comes back a
                            // float or two different, so that comparison fired
                            // on the very first frame and wrote the keyboard on
                            // every launch without anyone asking.
                            let colour_response = ui
                                .add_enabled_ui(self.zone_enabled[i] && controls_enabled, |ui| {
                                    ui.color_edit_button_rgb(&mut self.zone_colours[i])
                                });
                            if colour_response.inner.changed() {
                                *colour_changed = true;
                            }
                            if toggle(ui, self.zone_enabled[i], controls_enabled).clicked() {
                                self.zone_enabled[i] = !self.zone_enabled[i];
                            }
                            ui.label(
                                RichText::new(format!(
                                    "Z{} {}",
                                    i + 1,
                                    if self.zone_enabled[i] { "ON" } else { "OFF" }
                                ))
                                .font(theme::mono(9.0))
                                .color(if self.zone_enabled[i] {
                                    theme::MUTED
                                } else {
                                    theme::DIM
                                }),
                            );
                        });
                    }
                    ui.add_space(6.0);
                    if theme::tag(ui, "ALL ALIKE", false, controls_enabled, false).clicked() {
                        self.zone_colours = [self.zone_colours[0]; 4];
                        zone_colours_changed = [true; 4];
                    }
                    if theme::tag(ui, "ALL ON", false, controls_enabled, false).clicked() {
                        self.zone_enabled = [true; 4];
                    }
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Four keyboard zones, left to right. Disabled zones keep their saved colour.",
                    )
                    .font(theme::mono(10.0))
                    .color(theme::DIM),
                );
            });
        } else if self.effect.honours_colour() {
            theme::section(ui, "COLOUR", None);
            ui.add_space(10.0);
            card_body(ui, cw, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().interact_size = egui::vec2(52.0, 30.0);
                    let colour_response = ui.add_enabled_ui(controls_enabled, |ui| {
                        ui.color_edit_button_rgb(&mut self.per_effect[self.effect as usize].colour)
                    });
                    if colour_response.inner.changed() {
                        let pattern_colour = self.per_effect[self.effect as usize].colour;
                        for effect in Effect::ALL {
                            if effect != Effect::Static {
                                self.per_effect[effect as usize].colour = pattern_colour;
                            }
                        }
                        changed = true;
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("shared by Breathing, Zoom and Shifting")
                            .font(theme::mono(10.0))
                            .color(theme::DIM),
                    );
                });
            });
        }

        ui.add_space(12.0);
        let animated = self.effect != Effect::Static;
        card_body(ui, cw, |ui| {
            let lab_w = 86.0;
            // Brightness.
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("BRIGHTNESS")
                        .font(theme::mono(11.0))
                        .color(theme::MUTED),
                );
                ui.add_space(lab_w - 78.0);
                let sw = (ui.available_width() - 60.0).max(80.0);
                // PredatorSense's Covini control is five logical ticks. Its
                // managed code converts 1..=5 to wire values 0,25,..,100.
                let mut level = covini_brightness(self.brightness) / 25;
                if theme::slider(ui, &mut level, 0..=4, sw, can).drag_stopped() {
                    self.brightness = level * 25;
                    brightness_changed = true;
                    changed = true;
                }
                ui.label(
                    RichText::new(format!("{:>3} %", self.brightness))
                        .font(theme::mono_b(12.0))
                        .color(theme::TEXT),
                );
            });
            ui.add_space(8.0);

            // Speed — present but inert for static, with the reason beside it.
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("SPEED")
                        .font(theme::mono(11.0))
                        .color(if animated { theme::MUTED } else { theme::DIM }),
                );
                ui.add_space(lab_w - 42.0);
                let sw = (ui.available_width() - 60.0).max(80.0);
                let idx = self.effect as usize;
                let mut sp = self.per_effect[idx].speed.max(1);
                if theme::slider(ui, &mut sp, 1..=9, sw, animated && controls_enabled)
                    .drag_stopped()
                {
                    self.per_effect[idx].speed = sp;
                    changed = true;
                }
                ui.label(
                    RichText::new(if animated {
                        format!("{sp:>4}")
                    } else {
                        "   —".to_owned()
                    })
                    .font(theme::mono_b(12.0))
                    .color(if animated { theme::TEXT } else { theme::DIM }),
                );
            });
            ui.add_space(8.0);

            // Covini exposes direction for Wave and Shifting only.
            if self.effect.honours_direction() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("DIRECTION")
                            .font(theme::mono(11.0))
                            .color(theme::MUTED),
                    );
                    ui.add_space(lab_w - 74.0);
                    let idx = self.effect as usize;
                    let ltr = self.per_effect[idx].left_to_right;
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if theme::tag(ui, "L-R", ltr, controls_enabled, false).clicked() {
                        self.per_effect[idx].left_to_right = true;
                        changed = true;
                    }
                    if theme::tag(ui, "R-L", !ltr, controls_enabled, false).clicked() {
                        self.per_effect[idx].left_to_right = false;
                        changed = true;
                    }
                });
            } else if !animated {
                ui.horizontal(|ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.label(
                        RichText::new("Static has no speed or direction.")
                            .font(theme::mono(10.0))
                            .color(theme::DIM),
                    );
                });
            }
        });

        if previous_effect != self.effect {
            self.apply_lighting(if self.effect == Effect::Static {
                LightingUpdate::CompleteStatic
            } else {
                LightingUpdate::Dynamic
            });
        } else if self.effect == Effect::Static
            && (previous_enabled != self.zone_enabled
                || zone_colours_changed.into_iter().any(|value| value)
                || brightness_changed)
        {
            self.apply_lighting(LightingUpdate::StaticIncremental {
                previous_enabled,
                colour_changed: zone_colours_changed,
                brightness_changed,
            });
        } else if changed {
            self.apply_lighting(LightingUpdate::Dynamic);
        }
    }

    fn lighting_right(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        theme::section(ui, "PER-KEY", None);
        ui.add_space(10.0);

        let supported = snap.caps.per_key == Support::Yes;
        let detected_unverified = snap.caps.per_key == Support::Unverified;
        let top = ui.cursor().min;
        card_body(ui, cw, |ui| {
            ui.add_space(2.0);
            if supported {
                theme::badge(ui, Mark::Dot, "AVAILABLE", theme::OK, false);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "This machine has an ITE per-key controller. Use `alien rgb key <name> <colour>` \
                         to colour individual keys.",
                    )
                    .font(theme::mono(10.5))
                    .color(theme::MUTED),
                );
            } else if detected_unverified {
                theme::badge(ui, Mark::Query, "EXPERIMENTAL", theme::AMBER, false);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "An ITE per-key USB id is present, but this source-mapped transport is \
                         hardware-unverified and packaged frontends have no hidraw permission. \
                         Per-key writes remain unavailable.",
                    )
                    .font(theme::mono(10.5))
                    .color(theme::MUTED),
                );
            } else {
                theme::badge(ui, Mark::Cross, "UNSUPPORTED", theme::DIM, true);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "This keyboard has four shared lighting zones and no per-key controller.",
                    )
                    .font(theme::mono(10.5))
                    .color(theme::MUTED),
                );
            }
        });
        // Hazard stripe across the head of the panel.
        if !supported {
            let w = ui.min_rect().right() - top.x;
            let strip = egui::Rect::from_min_size(top, egui::vec2(w, 4.0));
            ui.painter().rect_filled(strip, 0.0, theme::PANEL);
            theme::hazard(ui.painter(), strip, theme::DIM);
        }

        ui.add_space(12.0);
        theme::section(ui, "PREVIEW", None);
        ui.add_space(10.0);
        card_body(ui, cw, |ui| {
            let w = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 34.0), egui::Sense::hover());
            // What the four zones are about to be — or, for an animated
            // effect, the single colour it will run in.
            let cs: [[f32; 3]; 4] = if self.effect == Effect::Static {
                self.zone_colours
            } else if !self.effect.honours_colour() {
                [
                    [0.0, 0.68, 0.78],
                    [1.0, 0.0, 0.35],
                    [1.0, 0.64, 0.0],
                    [0.58, 0.0, 1.0],
                ]
            } else {
                [self.per_effect[self.effect as usize].colour; 4]
            };
            let cw = rect.width() / 4.0;
            for (i, c) in cs.iter().enumerate() {
                let col = to_colour(*c);
                let r = egui::Rect::from_min_size(
                    Pos2::new(rect.left() + i as f32 * cw + 2.0, rect.top()),
                    egui::vec2(cw - 4.0, rect.height()),
                );
                let a = (self.brightness as f32 / 100.0).clamp(0.15, 1.0);
                let enabled = self.effect != Effect::Static || self.zone_enabled[i];
                let fill = if enabled {
                    Color32::from_rgb(col.r, col.g, col.b).gamma_multiply(a)
                } else {
                    theme::BG
                };
                ui.painter().rect_filled(r, 0.0, fill);
                if !enabled {
                    ui.painter().rect_stroke(
                        r,
                        0.0,
                        theme::hair(theme::DIM),
                        egui::StrokeKind::Inside,
                    );
                }
                ui.painter().text(
                    Pos2::new(r.center().x, rect.bottom() + 9.0),
                    egui::Align2::CENTER_CENTER,
                    if enabled {
                        format!("Z{}", i + 1)
                    } else {
                        format!("Z{} OFF", i + 1)
                    },
                    theme::mono(9.0),
                    theme::DIM,
                );
            }
            ui.add_space(14.0);
            ui.label(
                RichText::new(format!(
                    "{} · brightness {}%",
                    self.effect.name(),
                    self.brightness
                ))
                .font(theme::mono(10.0))
                .color(theme::MUTED),
            );
        });

        ui.add_space(12.0);
        if theme::chip(ui, "BACKLIGHT OFF", theme::ChipStyle::Idle, 11.0).clicked() {
            if let Some(dev) = self.device() {
                let result = dev.backlight_off(self.zone_colours.map(to_colour), self.zone_enabled);
                match result {
                    Ok(()) => {
                        self.brightness = 0;
                        self.mem.set_brightness(0);
                        match self.mem.save() {
                            Ok(()) => self.act(
                                Ok(()),
                                "backlight brightness 0 requested · active mode retained · optical effect unverified",
                            ),
                            Err(error) => {
                                self.status = format!(
                                    "brightness-0 request was accepted, but settings were not saved: {error}"
                                );
                                self.status_bad = true;
                                self.profile = None;
                            }
                        }
                    }
                    Err(error) => self.act::<()>(Err(error), ""),
                }
            }
        }
    }

    fn advanced_controls(&mut self, ui: &mut egui::Ui, snap: &Snapshot, width: f32) {
        theme::section(ui, "ADVANCED", Some("GETTER-GATED"));
        ui.add_space(10.0);
        card_body(ui, width, |ui| {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [78.0, 20.0],
                    egui::Label::new(
                        RichText::new("COOLBOOST")
                            .font(theme::mono(10.0))
                            .color(theme::MUTED),
                    ),
                );
                let on = snap.coolboost.unwrap_or(false);
                let writable = snap.caps.coolboost != Support::No && snap.coolboost.is_some();
                if toggle(ui, on, writable).clicked() {
                    self.apply_coolboost(!on);
                }
                ui.add_space(7.0);
                let state = match snap.coolboost {
                    Some(true) => "ON",
                    Some(false) => "OFF",
                    None if snap.caps.coolboost == Support::No => "UNSUPPORTED",
                    None => "UNAVAILABLE",
                };
                ui.label(RichText::new(state).font(theme::mono_b(10.0)).color(if on {
                    theme::AMBER
                } else {
                    theme::MUTED
                }));
            });

            ui.add_space(7.0);
            let y = ui.cursor().min.y;
            ui.painter()
                .hline(ui.max_rect().x_range(), y, theme::hair(theme::LINE));
            ui.add_space(7.0);

            ui.horizontal(|ui| {
                ui.add_sized(
                    [78.0, 20.0],
                    egui::Label::new(
                        RichText::new("KB TIMEOUT")
                            .font(theme::mono(10.0))
                            .color(theme::MUTED),
                    ),
                );
                if let Some(state) = snap.keyboard_timeout {
                    if theme::tag(ui, "OFF", state.seconds == 0, true, false).clicked() {
                        self.apply_keyboard_timeout(0);
                    }
                    if theme::tag(ui, "30 S", state.seconds == 30, true, false).clicked() {
                        self.apply_keyboard_timeout(30);
                    }
                } else {
                    let unsupported = snap.caps.keyboard_timeout == Support::No;
                    theme::badge(
                        ui,
                        if unsupported {
                            Mark::Cross
                        } else {
                            Mark::Query
                        },
                        if unsupported {
                            "UNSUPPORTED"
                        } else {
                            "UNAVAILABLE"
                        },
                        if unsupported {
                            theme::DIM
                        } else {
                            theme::AMBER
                        },
                        true,
                    );
                }
            });

            ui.add_space(7.0);
            let y = ui.cursor().min.y;
            ui.painter()
                .hline(ui.max_rect().x_range(), y, theme::hair(theme::LINE));
            ui.add_space(7.0);

            ui.horizontal(|ui| {
                ui.add_sized(
                    [78.0, 20.0],
                    egui::Label::new(
                        RichText::new("LCD OD")
                            .font(theme::mono(10.0))
                            .color(theme::MUTED),
                    ),
                );
                let on = snap.lcd_overdrive.unwrap_or(false);
                let writable =
                    snap.caps.lcd_overdrive != Support::No && snap.lcd_overdrive.is_some();
                if toggle(ui, on, writable).clicked() {
                    self.apply_lcd_overdrive(!on);
                }
                ui.add_space(7.0);
                let state = match snap.lcd_overdrive {
                    Some(true) => "ON",
                    Some(false) => "OFF",
                    None if snap.caps.lcd_overdrive == Support::No => "UNSUPPORTED",
                    None => "UNAVAILABLE",
                };
                ui.label(RichText::new(state).font(theme::mono_b(10.0)).color(if on {
                    theme::AMBER
                } else {
                    theme::MUTED
                }));
            });

            ui.add_space(7.0);
            ui.label(
                RichText::new(
                    "CoolBoost on PH315-53: setter reinit transient confirmed; no sustained cooling lift in controlled A/B/A. LCD/timeout retain their shown physical-verification boundary.",
                )
                .font(theme::mono(8.5))
                .color(theme::DIM),
            );
        });
    }

    fn raw_firmware_flags(&mut self, ui: &mut egui::Ui, snap: &Snapshot, width: f32) {
        theme::section(ui, "RAW FLAGS", Some("NOT OEM MODES"));
        ui.add_space(10.0);
        card_body(ui, width, |ui| {
            // WMBH misc sub-index 5 is a real readable bit, but PredatorSense
            // Normal/Faster/Turbo is command 45 and a different mechanism.
            let gpu_on = snap.gpu_turbo == Some(2);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [42.0, 20.0],
                    egui::Label::new(
                        RichText::new("GPU")
                            .font(theme::mono(10.0))
                            .color(theme::MUTED),
                    ),
                );
                toggle(ui, gpu_on, false).on_hover_text(
                    "read-only: use a complete OEM GPU mode so offsets, fan table and GPOC stay coherent",
                );
                ui.add_space(7.0);
                ui.label(
                    RichText::new(if gpu_on { "FLAG SET" } else { "FLAG CLEAR" })
                        .font(theme::mono_b(10.0))
                        .color(if gpu_on { theme::AMBER } else { theme::MUTED }),
                );
            });
            ui.label(
                RichText::new("WMBH sub-index 5 · manual snapshot · writes disabled")
                    .font(theme::mono(8.5))
                    .color(theme::DIM),
            );

            ui.add_space(7.0);
            let y = ui.cursor().min.y;
            ui.painter()
                .hline(ui.max_rect().x_range(), y, theme::hair(theme::LINE));
            ui.add_space(7.0);

            // The target's real CPU policy is XTU power-limit data. Keep the
            // stored WMI flag visible as evidence, but do not offer an inert
            // switch where its model-specific gain is unproven.
            let cpu_on = snap.cpu_turbo == Some(2);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [42.0, 20.0],
                    egui::Label::new(
                        RichText::new("CPU")
                            .font(theme::mono(10.0))
                            .color(theme::MUTED),
                    ),
                );
                toggle(ui, cpu_on, false);
                ui.add_space(7.0);
                ui.label(
                    RichText::new(match snap.cpu_turbo {
                        Some(2) => "FLAG SET",
                        Some(0) => "FLAG CLEAR",
                        Some(_) => "UNKNOWN",
                        None => "UNAVAILABLE",
                    })
                    .font(theme::mono_b(10.0))
                    .color(theme::MUTED),
                );
            });
            ui.label(
                RichText::new("XTU power policy is separate · writes disabled")
                    .font(theme::mono(8.5))
                    .color(theme::DIM),
            );
        });
    }

    fn oem_gpu_modes(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        theme::section(ui, "OEM GPU MODE", Some("NVML P0 + ACER POLICY"));
        ui.add_space(10.0);
        let width = ui.available_width();
        let busy = self.gpu_mode_job.is_some();
        card_body(ui, width, |ui| {
            let confirmed = snap.gpu_mode.and_then(GpuModeState::confirmed_mode);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(if busy { "WORKING…" } else { "REFRESH" }),
                    )
                    .on_hover_text(
                        "Reads all mode legs; Acer's GPOC getter sends one OEM GPU notification",
                    )
                    .clicked()
                {
                    self.start_gpu_mode_job(GpuModeOperation::Refresh);
                }
                ui.add_space(8.0);
                for mode in [GpuMode::Normal, GpuMode::Faster, GpuMode::Turbo] {
                    let label = mode.label().to_ascii_uppercase();
                    let target_fits = snap.gpu_mode.is_some_and(|state| state.target_fits(mode));
                    let response = theme::tag(
                        ui,
                        &label,
                        confirmed == Some(mode),
                        target_fits && !busy,
                        false,
                    );
                    let response = if snap.gpu_mode.is_some() && !target_fits {
                        let target = mode.offsets();
                        response.on_hover_text(format!(
                            "Unavailable: live NVML ranges do not admit {:+}/{:+} MHz",
                            target.graphics_mhz, target.memory_mhz
                        ))
                    } else {
                        response
                    };
                    if response.clicked() {
                        self.gpu_mode_confirm = Some(mode);
                    }
                }
            });

            ui.add_space(7.0);
            match snap.gpu_mode {
                Some(state) => {
                    ui.label(
                        RichText::new(format!(
                            "P0 {:+}/{:+} MHz  ·  FAN {}  ·  GPOC {}  ·  {}",
                            state.graphics.current_mhz,
                            state.memory.current_mhz,
                            state.fan_table,
                            state.gpoc,
                            confirmed.map(GpuMode::label).unwrap_or("SPLIT STATE")
                        ))
                        .font(theme::mono_b(10.0))
                        .color(if confirmed.is_some() {
                            theme::OK
                        } else {
                            theme::AMBER
                        }),
                    );
                }
                None => {
                    ui.label(
                        RichText::new("UNAVAILABLE")
                            .font(theme::mono_b(10.0))
                            .color(theme::DIM),
                    );
                }
            }
            ui.add_space(5.0);
            let detail = match snap.gpu_mode {
                Some(state) => format!(
                    "Driver ranges: graphics {:+}..{:+} MHz · memory {:+}..{:+} MHz",
                    state.graphics.min_mhz,
                    state.graphics.max_mhz,
                    state.memory.min_mhz,
                    state.memory.max_mhz
                ),
                None => snap
                    .gpu_mode_error
                    .as_deref()
                    .unwrap_or("exact target or privileged daemon endpoint unavailable")
                    .to_owned(),
            };
            ui.label(
                RichText::new(detail)
                    .font(theme::mono(8.5))
                    .color(theme::DIM),
            );
            ui.label(
                RichText::new(
                    "Manual snapshot: refresh sends one Acer GPU notification · unsupported privileged clock control · offsets reset with the Nvidia driver",
                )
                .font(theme::mono(8.5))
                .color(theme::AMBER),
            );
        });
    }

    fn gpu_mode_confirmation(&mut self, ctx: &egui::Context) {
        let Some(mode) = self.gpu_mode_confirm else {
            return;
        };
        let target = mode.offsets();
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new("Unsupported GPU clock control")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(430.0);
                ui.label(
                    RichText::new(format!("Apply OEM {} mode?", mode.label()))
                        .font(theme::sans_b(16.0))
                        .color(theme::BRIGHT),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "P0 graphics {:+} MHz · memory {:+} MHz · Acer fan table {} · GPOC {}",
                        target.graphics_mhz,
                        target.memory_mhz,
                        mode.fan_table(),
                        mode as u8
                    ))
                    .font(theme::mono(10.0))
                    .color(theme::TEXT),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Nvidia classifies clock manipulation as unsupported. It may cause instability, excessive heat or data loss. Alien will read every leg back and attempt reverse-order rollback on failure, but cannot guarantee stability.",
                    )
                    .font(theme::sans(11.0))
                    .color(theme::AMBER),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .button(format!("I ACCEPT — APPLY {}", mode.label().to_ascii_uppercase()))
                        .clicked()
                    {
                        apply = true;
                    }
                });
            });
        if apply {
            self.gpu_mode_confirm = None;
            self.start_gpu_mode_job(GpuModeOperation::Apply(mode));
        } else if cancel || !open {
            self.gpu_mode_confirm = None;
        }
    }

    fn performance(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let width = ui.available_width();
        egui::ScrollArea::vertical()
            .id_salt("performance-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(width);
                self.performance_content(ui, snap);
            });
    }

    fn performance_content(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        self.oem_gpu_modes(ui, snap);
        ui.add_space(12.0);
        if cw < 720.0 {
            // At a compositor-forced sub-minimum width, two fixed columns make
            // the wrapped evidence copy collide with the next section. Stack
            // the cards and let their real content height drive scrolling.
            self.advanced_controls(ui, snap, cw);
            ui.add_space(12.0);
            self.raw_firmware_flags(ui, snap, cw);
        } else {
            let gap = 14.0;
            let panel_w = (cw - gap) / 2.0;
            let top_h = 190.0;
            let top = ui.cursor().min;
            let left = egui::Rect::from_min_size(top, egui::vec2(panel_w, top_h));
            let right = egui::Rect::from_min_size(
                Pos2::new(top.x + panel_w + gap, top.y),
                egui::vec2(panel_w, top_h),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                ui.set_max_width(panel_w);
                self.advanced_controls(ui, snap, panel_w);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                ui.set_max_width(panel_w);
                self.raw_firmware_flags(ui, snap, panel_w);
            });
            ui.advance_cursor_after_rect(egui::Rect::from_min_size(top, egui::vec2(cw, top_h)));
        }

        ui.add_space(12.0);
        theme::section(ui, "LIVE CLOCKS", Some("MEASURED"));
        ui.add_space(10.0);
        if cw < 600.0 {
            performance_metric(
                ui,
                "CPU FREQUENCY",
                snap.performance.cpu_mhz,
                snap.performance.cpu_max_mhz,
                None,
                cw,
                snap.cpu_turbo == Some(2) && snap.caps.cpu_overclock == Support::Yes,
            );
            ui.add_space(10.0);
            performance_metric(
                ui,
                "GPU CORE CLOCK",
                snap.performance.gpu_mhz,
                snap.performance.gpu_max_mhz,
                snap.performance.gpu_usage_pct,
                cw,
                snap.gpu_mode.and_then(GpuModeState::confirmed_mode) == Some(GpuMode::Turbo),
            );
        } else {
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 10.0) / 2.0;
                performance_metric(
                    ui,
                    "CPU FREQUENCY",
                    snap.performance.cpu_mhz,
                    snap.performance.cpu_max_mhz,
                    None,
                    width,
                    snap.cpu_turbo == Some(2) && snap.caps.cpu_overclock == Support::Yes,
                );
                ui.add_space(10.0);
                performance_metric(
                    ui,
                    "GPU CORE CLOCK",
                    snap.performance.gpu_mhz,
                    snap.performance.gpu_max_mhz,
                    snap.performance.gpu_usage_pct,
                    width,
                    snap.gpu_mode.and_then(GpuModeState::confirmed_mode) == Some(GpuMode::Turbo),
                );
            });
        }

        ui.add_space(12.0);
        theme::section(ui, "TEMPERATURES", Some("UNDER LOAD"));
        ui.add_space(10.0);

        // Fixed content height: this screen lives in a ScrollArea so a
        // constrained window scrolls rather than inflating this final card
        // beyond its viewport and clipping the controls above it.
        let h = 140.0;
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), h));
        theme::card(ui.painter(), rect);
        let inner = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(rect.width() - 20.0, (h - 20.0).min(140.0)),
        );
        // `Ui::horizontal` starts with an interaction-row height, so asking
        // the child for its available height shrinks these gauges to text
        // height even though the card has a full 120 px to offer. Size from
        // the card itself, and retain width as the limiter in narrow windows.
        let g = inner.height().min(inner.width() / 3.0).min(140.0);
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.horizontal(|ui| {
                let pad = ((ui.available_width() - 3.0 * g) / 4.0).max(0.0);
                let s = &snap.sensors;
                ui.add_space(pad);
                gauge::Gauge::temp("CPU", s.cpu_temp_c).show(ui, g);
                ui.add_space(pad - ui.spacing().item_spacing.x);
                gauge::Gauge::temp("GPU", s.gpu_temp_c).show(ui, g);
                ui.add_space(pad - ui.spacing().item_spacing.x);
                gauge::Gauge::temp("BOARD", s.system_temp_c).show(ui, g);
            });
        });
        ui.advance_cursor_after_rect(rect);
    }

    fn about(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let (lr, rr) = split_columns(ui, 0.52, 20.0);
        ui.scope_builder(egui::UiBuilder::new().max_rect(lr), |ui| {
            ui.set_max_width(lr.width());
            self.about_left(ui, snap, lr.width());
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(rr), |ui| {
            ui.set_max_width(rr.width());
            self.about_right(ui, snap);
        });
    }

    fn about_left(&mut self, ui: &mut egui::Ui, snap: &Snapshot, left_w: f32) {
        let cw = ui.max_rect().width();
        // Wordmark block.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(left_w, 40.0), egui::Sense::hover());
        let p = ui.painter();
        let y = rect.center().y;
        glyph::logo(p, self.logo.id(), Pos2::new(rect.left() + 21.0, y), 42.0);
        let w = theme::tracked(
            ui.ctx(),
            p,
            Pos2::new(rect.left() + 50.0, y),
            "ALIEN",
            theme::sans_b(24.0),
            theme::BRIGHT,
            9.0,
        );
        p.text(
            Pos2::new(rect.left() + 58.0 + w, y + 6.0),
            egui::Align2::LEFT_CENTER,
            format!("v{}", env!("CARGO_PKG_VERSION")),
            theme::mono(11.0),
            theme::MUTED,
        );

        ui.add_space(10.0);
        ui.label(
            RichText::new(
                "Fan, lighting, hardware-control and telemetry support for Acer Predator systems \
                 on Linux, with no vendor software. An independent, from-scratch \
                 interoperability implementation of the recovered WMI protocols, verified \
                 against real firmware.",
            )
            .font(theme::sans(12.0))
            .color(theme::TEXT),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Where a control is accepted by the firmware but has no observable effect on \
                 a model, this software says so rather than implying it works.",
            )
            .font(theme::mono(11.0))
            .color(theme::MUTED),
        );

        ui.add_space(12.0);
        theme::section(ui, "THIS MACHINE", None);
        ui.add_space(8.0);
        card_body(ui, cw, |ui| {
            let rows: [(&str, String, Color32); 4] = [
                ("Model", model_name(), theme::TEXT),
                ("Firmware", dmi("bios_version"), theme::TEXT),
                (
                    "Interface",
                    if left_w < 380.0 {
                        snap.interface
                            .rsplit_once(" at ")
                            .map(|(_, socket)| socket.to_owned())
                            .unwrap_or_else(|| snap.interface.clone())
                    } else {
                        snap.interface.clone()
                    },
                    theme::TEXT,
                ),
                (
                    "Daemon",
                    match snap.link.link {
                        Link::Up => "reachable · group alien".to_owned(),
                        _ => "unreachable".to_owned(),
                    },
                    match snap.link.link {
                        Link::Up => theme::OK,
                        _ => theme::RED,
                    },
                ),
            ];
            for (k, v, c) in rows {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (r, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 20.0),
                        egui::Sense::hover(),
                    );
                    let p = ui.painter();
                    p.text(
                        Pos2::new(r.left(), r.center().y),
                        egui::Align2::LEFT_CENTER,
                        k,
                        theme::mono(11.0),
                        theme::MUTED,
                    );
                    let mut x = r.left() + 100.0;
                    if k == "Daemon" {
                        glyph::draw(
                            p,
                            Pos2::new(x + 4.0, r.center().y),
                            if c == theme::OK {
                                Mark::Dot
                            } else {
                                Mark::Cross
                            },
                            7.0,
                            c,
                        );
                        x += 13.0;
                    }
                    p.text(
                        Pos2::new(x, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        v,
                        theme::mono(11.0),
                        c,
                    );
                });
            }
        });

        ui.add_space(10.0);
        theme::section(
            ui,
            "MODEL EVIDENCE CATALOG",
            Some("18 mapped · 36 candidates"),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Browse the recovered OEM plug-ins and a separate Acer-documented ecosystem \
                 tier. Only the PH315-53 reference is live-verified; catalog membership never enables a \
                 model-gated control.",
            )
            .font(theme::mono(10.0))
            .color(theme::MUTED),
        );
        ui.add_space(8.0);
        if theme::chip(ui, "BROWSE MODEL EVIDENCE", theme::ChipStyle::Outline, 10.0).clicked() {
            self.compatible_models_open = true;
        }

        ui.add_space(10.0);
        ui.label(
            RichText::new("GPL-2.0-or-later · alien.hartle.tech · not affiliated with Acer")
                .font(theme::mono(10.0))
                .color(theme::DIM),
        );
    }

    fn about_right(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        let row_font = if cw < 330.0 {
            theme::mono(9.0)
        } else {
            theme::mono(11.0)
        };
        theme::section(ui, "WHAT THIS MACHINE SUPPORTS", Some("alien doctor"));
        ui.add_space(8.0);
        card_body(ui, cw, |ui| {
            for (name, sup) in snap.caps.rows() {
                let name = if cw < 330.0 {
                    match name {
                        "cpu overclock flag" => "cpu firmware flag",
                        "gpu overclock flag" => "gpu turbo flag",
                        other => other,
                    }
                } else {
                    name
                };
                let (mark, text, colour) = match sup {
                    Support::Yes => (Mark::Dot, "yes", theme::OK),
                    Support::No => (Mark::Cross, "no", theme::DIM),
                    Support::Unverified if name == "CoolBoost protocol" => {
                        (Mark::Query, "getter; PH315 A/B/A", theme::AMBER)
                    }
                    Support::Unverified
                        if matches!(
                            name,
                            "30-second keyboard timeout" | "LCD overdrive protocol"
                        ) =>
                    {
                        (Mark::Query, "getter confirmed", theme::AMBER)
                    }
                    Support::Unverified => (Mark::Query, "accepted, unverified", theme::AMBER),
                    Support::Unknown => (Mark::Query, "unknown", theme::AMBER),
                };
                let (r, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 19.0),
                    egui::Sense::hover(),
                );
                let p = ui.painter();
                let y = r.center().y;
                p.text(
                    Pos2::new(r.left(), y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    row_font.clone(),
                    theme::MUTED,
                );
                let tw = theme::tracked_width(ui.ctx(), text, &row_font, 0.0);
                glyph::draw(p, Pos2::new(r.right() - tw - 11.0, y), mark, 7.0, colour);
                p.text(
                    Pos2::new(r.right(), y),
                    egui::Align2::RIGHT_CENTER,
                    text,
                    row_font.clone(),
                    colour,
                );
            }
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Probed at startup with getters only — reads cannot change machine state. \
                 Paste this block into a bug report.",
            )
            .font(theme::mono(10.0))
            .color(theme::DIM),
        );
    }

    fn compatible_models_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.compatible_models_open;
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            open = false;
        }

        let screen = ctx.available_rect().size();
        let content_width = (screen.x - 72.0).clamp(300.0, 720.0);
        let scroll_height = (screen.y - 310.0).clamp(180.0, 500.0);

        egui::Window::new("Model evidence catalog")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(content_width)
            .min_width(300.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(content_width.min(ui.available_width()));
                ui.label(
                    RichText::new("ACER MODEL EVIDENCE CATALOG")
                        .font(theme::sans_b(16.0))
                        .color(theme::BRIGHT),
                );
                ui.add_space(3.0);
                ui.label(
                    RichText::new(format!(
                        "1 LIVE  ·  {} PACKAGE-MAPPED  ·  {} ECOSYSTEM CANDIDATES",
                        compatible_models::MODEL_COUNT,
                        compatible_models::ECOSYSTEM_CANDIDATE_COUNT,
                    ))
                    .font(theme::mono(9.5))
                    .color(theme::GREEN),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Evidence is tiered: PH315-53 is live-verified; recovered OEM plug-ins \
                         identify six distinct PredatorSense protocol families; official Acer \
                         sources supply a wider candidate tier. Candidate or package presence \
                         never proves that an Alien control works or makes a setter safe.",
                    )
                    .font(theme::sans(11.0))
                    .color(theme::TEXT),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("SEARCH")
                            .font(theme::mono(10.0))
                            .color(theme::MUTED),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.compatible_models_search)
                            .hint_text("model, product, package or OEM family")
                            .desired_width(f32::INFINITY),
                    );
                    if !self.compatible_models_search.is_empty()
                        && theme::chip(ui, "CLEAR", theme::ChipStyle::Idle, 9.0).clicked()
                    {
                        self.compatible_models_search.clear();
                    }
                });

                let visible_mapped = compatible_models::MODELS
                    .iter()
                    .filter(|model| {
                        compatible_models::matches(model, &self.compatible_models_search)
                    })
                    .count();
                let visible_candidates = compatible_models::ECOSYSTEM_CANDIDATES
                    .iter()
                    .filter(|candidate| {
                        compatible_models::candidate_matches(
                            candidate,
                            &self.compatible_models_search,
                        )
                    })
                    .count();
                ui.add_space(5.0);
                ui.label(
                    RichText::new(format!(
                        "{visible_mapped} PACKAGE-MAPPED MODELS  ·  {visible_candidates} CANDIDATE GROUPS"
                    ))
                    .font(theme::mono(9.0))
                    .color(theme::DIM),
                );
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .id_salt("compatible-models-scroll")
                    .auto_shrink([false, false])
                    .max_height(scroll_height)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        if visible_mapped > 0 {
                            theme::section(
                                ui,
                                "LIVE + PACKAGE-MAPPED",
                                Some("recovered OEM plug-ins"),
                            );
                            ui.add_space(3.0);
                            ui.label(
                                RichText::new(
                                    "Only the green PH315-53 entry has live Alien validation.",
                                )
                                .font(theme::mono(9.0))
                                .color(theme::DIM),
                            );
                        }
                        for series in [
                            "PH315", "PH317", "PH517", "PH717", "PT314", "PT315", "PT316",
                            "PT515", "PT516",
                        ] {
                            let models: Vec<_> = compatible_models::MODELS
                                .iter()
                                .filter(|model| {
                                    model.series == series
                                        && compatible_models::matches(
                                            model,
                                            &self.compatible_models_search,
                                        )
                                })
                                .collect();
                            if models.is_empty() {
                                continue;
                            }

                            ui.add_space(6.0);
                            let count = models.len();
                            theme::section(
                                ui,
                                &format!("{series} SERIES"),
                                Some(&format!(
                                    "{count} model{}",
                                    if count == 1 { "" } else { "s" }
                                )),
                            );
                            ui.add_space(6.0);

                            for model in models {
                                egui::Frame::NONE
                                    .fill(theme::PANEL)
                                    .stroke(theme::hair(theme::LINE))
                                    .inner_margin(egui::Margin::symmetric(12, 9))
                                    .show(ui, |ui| {
                                        ui.set_width((ui.available_width() - 24.0).max(100.0));
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                RichText::new(model.model)
                                                    .font(theme::sans_b(12.0))
                                                    .color(theme::BRIGHT),
                                            );
                                            if model.live_reference {
                                                ui.label(
                                                    RichText::new("● LIVE-VERIFIED REFERENCE")
                                                        .font(theme::mono(8.5))
                                                        .color(theme::OK),
                                                );
                                            }
                                        });
                                        for package in model.packages {
                                            ui.label(
                                                RichText::new(format!(
                                                    "PredatorSense {}  ·  M{} {} / L{} / F{}  ·  PerKey {}  ·  GPU OC flag {}",
                                                    package.version,
                                                    package.machine_type,
                                                    package.family_name(),
                                                    package.lighting_type,
                                                    package.fan_type,
                                                    u8::from(package.per_key),
                                                    u8::from(package.gpu_overclock),
                                                ))
                                                .font(theme::mono(9.5))
                                                .color(theme::MUTED),
                                            );
                                        }
                                    });
                                ui.add_space(5.0);
                            }
                        }

                        if visible_candidates > 0 {
                            ui.add_space(12.0);
                            theme::section(
                                ui,
                                "OTHER PREDATORSENSE MODELS",
                                Some("Alien compatibility unverified"),
                            );
                            ui.add_space(3.0);
                            ui.label(
                                RichText::new(
                                    "No extracted plug-in or Alien protocol mapping yet. These \
                                     entries are research targets, not supported-model claims.",
                                )
                                .font(theme::mono(9.0))
                                .color(theme::AMBER),
                            );
                            ui.add_space(7.0);

                            for candidate in compatible_models::ECOSYSTEM_CANDIDATES
                                .iter()
                                .filter(|candidate| {
                                    compatible_models::candidate_matches(
                                        candidate,
                                        &self.compatible_models_search,
                                    )
                                })
                            {
                                egui::Frame::NONE
                                    .fill(theme::PANEL)
                                    .stroke(theme::hair(theme::LINE))
                                    .inner_margin(egui::Margin::symmetric(12, 9))
                                    .show(ui, |ui| {
                                        ui.set_width((ui.available_width() - 24.0).max(100.0));
                                        ui.label(
                                            RichText::new(candidate.product)
                                                .font(theme::sans_b(11.0))
                                                .color(theme::TEXT),
                                        );
                                        ui.label(
                                            RichText::new(candidate.models.join("  ·  "))
                                                .font(theme::mono(9.5))
                                                .color(theme::MUTED),
                                        );
                                    });
                                ui.add_space(5.0);
                            }
                        }

                        if visible_mapped == 0 && visible_candidates == 0 {
                            ui.add_space(24.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("NO MODEL EVIDENCE MATCHES THIS SEARCH")
                                        .font(theme::mono(10.0))
                                        .color(theme::AMBER),
                                );
                            });
                        }
                    });

                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "M / L / F are Acer's MachineType, LightingType and FanDtail Type. \
                         Package revisions stay separate where Acer changed a model profile. \
                         Runtime getters and model gates remain authoritative.",
                    )
                    .font(theme::mono(9.0))
                    .color(theme::DIM),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to("Acer GPU table", compatible_models::ACER_GPU_SPEC_URL);
                    ui.label("·");
                    ui.hyperlink_to(
                        "Acer mobile list",
                        compatible_models::ACER_MOBILE_COMPAT_URL,
                    );
                    ui.label("·");
                    ui.hyperlink_to(
                        "Acer Helios 18P spec",
                        compatible_models::ACER_HELIOS_18P_URL,
                    );
                });
            });

        self.compatible_models_open = open;
    }

    /// Shown instead of a dashboard when we have never reached the daemon.
    ///
    /// The three commands are the whole remedy, and the third one is the step
    /// people miss: a group change does not reach an already-running session.
    fn first_run(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG))
            .show(ctx, |ui| {
                theme::scanlines(ui.painter(), ui.max_rect());
                let full = ui.max_rect();
                let w = 520.0_f32.min(full.width() - 40.0);
                let h = 268.0;
                let card = egui::Rect::from_center_size(full.center(), egui::vec2(w, h));
                theme::card(ui.painter(), card);
                let strip = egui::Rect::from_min_size(card.left_top(), egui::vec2(w, 5.0));
                theme::hazard(ui.painter(), strip, theme::AMBER);

                let p = ui.painter();
                let lx = card.left() + 28.0;
                let mut y = card.top() + 34.0;

                glyph::logo(p, self.logo.id(), Pos2::new(lx + 18.0, y), 38.0);
                theme::tracked(
                    ctx,
                    p,
                    Pos2::new(lx + 42.0, y),
                    "DAEMON NOT REACHABLE",
                    theme::sans_b(15.0),
                    theme::TEXT,
                    3.0,
                );
                p.text(
                    Pos2::new(card.right() - 28.0, y),
                    egui::Align2::RIGHT_CENTER,
                    "ALN-SETUP-01",
                    theme::mono(9.0),
                    theme::DIM,
                );
                y += 30.0;

                p.text(
                    Pos2::new(lx, y),
                    egui::Align2::LEFT_CENTER,
                    "The GUI talks to alien-daemon over its socket and never to the",
                    theme::mono(11.0),
                    theme::MUTED,
                );
                y += 16.0;
                p.text(
                    Pos2::new(lx, y),
                    egui::Align2::LEFT_CENTER,
                    "firmware directly. One-time setup:",
                    theme::mono(11.0),
                    theme::MUTED,
                );
                y += 22.0;

                for (i, step) in [
                    "sudo systemctl enable --now alien-daemon",
                    "sudo gpasswd -a $USER alien",
                    "",
                ]
                .into_iter()
                .enumerate()
                {
                    p.text(
                        Pos2::new(lx, y + 15.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{}", i + 1),
                        theme::mono_b(10.0),
                        theme::GREEN,
                    );
                    if step.is_empty() {
                        p.text(
                            Pos2::new(lx + 24.0, y + 15.0),
                            egui::Align2::LEFT_CENTER,
                            "log out and back in — group changes do not reach a running session",
                            theme::mono(11.0),
                            theme::MUTED,
                        );
                    } else {
                        let box_r = egui::Rect::from_min_size(
                            Pos2::new(lx + 24.0, y + 3.0),
                            egui::vec2(card.right() - 28.0 - (lx + 24.0), 25.0),
                        );
                        p.rect_filled(box_r, 0.0, theme::BG);
                        p.rect_stroke(box_r, 0.0, theme::hair(theme::LINE), StrokeKind::Inside);
                        p.text(
                            Pos2::new(box_r.left() + 12.0, box_r.center().y),
                            egui::Align2::LEFT_CENTER,
                            step,
                            theme::mono(11.0),
                            theme::TEXT,
                        );
                    }
                    y += 32.0;
                }

                y += 6.0;
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                        Pos2::new(lx, y),
                        egui::vec2(w - 56.0, 34.0),
                    )),
                    |ui| {
                        ui.horizontal(|ui| {
                            if theme::chip(ui, "RETRY", theme::ChipStyle::Active, 11.0).clicked() {
                                // The poller retries on its own schedule; this
                                // just skips the remaining wait.
                                if let Ok(mut sh) = self.shared.lock() {
                                    if let Some(st) = sh.link.as_mut() {
                                        st.next_try = Some(Instant::now());
                                    }
                                }
                            }
                            if theme::chip(ui, "RUN DOCTOR", theme::ChipStyle::Idle, 11.0).clicked()
                            {
                                self.status = "run `alien doctor` in a terminal".into();
                            }
                            ui.add_space(6.0);
                            let secs = snap
                                .link
                                .next_try
                                .map(|t| t.saturating_duration_since(Instant::now()).as_secs() + 1)
                                .unwrap_or(5);
                            ui.label(
                                RichText::new(format!("checking again in {secs} s"))
                                    .font(theme::mono(10.0))
                                    .color(theme::DIM),
                            );
                            let blink = ui.input(|s| s.time) % 1.1 < 0.55;
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 12.0), egui::Sense::hover());
                            if blink {
                                glyph::draw(ui.painter(), r.center(), Mark::Bar, 8.0, theme::GREEN);
                            }
                        });
                    },
                );
            });
    }
}

// ── Small pieces ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct DashboardVerticalPlan {
    middle_height: f32,
    /// Extra space after egui's automatic trailing item gap.
    bottom_padding: f32,
}

/// Size the stretchable dashboard row without sacrificing the profile footer.
///
/// `consumed_height` ends immediately before the HISTORY/STATE row. From that
/// point to the bottom of the profile chips the fixed cost is:
///
/// * the item gap after the middle row;
/// * 12 px before the PROFILES heading;
/// * the 14 px heading and its item gap;
/// * 8 px before the chips;
/// * the 29 px chip row.
///
/// A one-pixel slack absorbs point rounding so a content area that should fit
/// does not flash a scroll bar. When even the minimum middle row cannot fit,
/// the caller's vertical ScrollArea exposes the remaining content.
fn dashboard_vertical_plan(
    viewport_height: f32,
    consumed_height: f32,
    item_spacing_y: f32,
) -> DashboardVerticalPlan {
    const PROFILE_LEAD_SPACE: f32 = 12.0;
    const PROFILE_HEADING_TO_CHIPS: f32 = 8.0;
    const FIT_SLACK: f32 = 1.0;

    let bottom_clearance = (viewport_height * 0.02).clamp(16.0, 28.0);
    let tail_to_chip_bottom = 2.0 * item_spacing_y
        + PROFILE_LEAD_SPACE
        + DASHBOARD_SECTION_H
        + PROFILE_HEADING_TO_CHIPS
        + DASHBOARD_PROFILE_CHIP_H;
    let remaining = (viewport_height - consumed_height).max(0.0);
    let middle_height = (remaining - tail_to_chip_bottom - bottom_clearance - FIT_SLACK)
        .max(DASHBOARD_MIDDLE_MIN_H);

    DashboardVerticalPlan {
        middle_height,
        // A horizontal row has already advanced by one item gap here.
        bottom_padding: (bottom_clearance - item_spacing_y).max(0.0),
    }
}

/// Keep the dashboard's profile controls on their single 29 px instrument row.
///
/// Profiles are user-extensible and names may be 32 characters, so wrapping
/// would invalidate the dashboard's vertical budget. A horizontal-only area
/// keeps every chip reachable by wheel, trackpad or drag; keyboard focus also
/// scrolls the focused chip into view at the call site. The bar itself stays
/// hidden so it cannot paint over the chip labels inside this one-row slot.
fn dashboard_profile_strip<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<egui::InnerResponse<R>> {
    egui::ScrollArea::horizontal()
        .id_salt("dashboard-profile-strip")
        .max_height(DASHBOARD_PROFILE_CHIP_H)
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| ui.horizontal(add_contents))
}

/// Split the current area into two columns at a fixed ratio.
///
/// Returns rects rather than taking closures: `horizontal_top` +
/// `allocate_ui_with_layout` sizes children from `available_width()`, which
/// reported a width wider than the panel here and handed the left column ~85%
/// of the row — squeezing the right one into a ribbon of wrapped text that ran
/// off the edge of the window. Rects derived from `max_rect` cannot drift.
fn split_columns(ui: &egui::Ui, left_ratio: f32, gap: f32) -> (egui::Rect, egui::Rect) {
    let full = ui.max_rect();
    let lw = ((full.width() - gap) * left_ratio).floor();
    let rw = full.width() - gap - lw;
    (
        egui::Rect::from_min_size(full.min, egui::vec2(lw, full.height())),
        egui::Rect::from_min_size(
            Pos2::new(full.min.x + lw + gap, full.min.y),
            egui::vec2(rw, full.height()),
        ),
    )
}

/// A bordered card exactly `w` wide.
///
/// The width is passed in, not measured. A `Frame` sizes itself to its
/// content, and a single non-wrapping label inside it is enough to push the
/// card past its column — which is how the left column's cards ended up
/// drawing over the right column even though the column ui itself was
/// correctly 453px wide. Taking the column width from the caller also makes
/// every card on a screen line up, which is what the design shows.
fn card_body(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(theme::PANEL)
        .stroke(theme::hair(theme::LINE))
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width((w - 32.0).max(40.0));
            add(ui);
        });
}

/// A 38×20 switch.
fn toggle(ui: &mut egui::Ui, on: bool, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(38.0, 20.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    if !enabled {
        p.rect_filled(rect, 0.0, theme::PANEL);
        theme::dashed_rect(p, rect, theme::DIM);
    } else if on {
        p.rect_filled(rect, 0.0, theme::GREEN);
    } else {
        p.rect_filled(rect, 0.0, theme::PANEL);
        p.rect_stroke(rect, 0.0, theme::hair(theme::LINE), StrokeKind::Inside);
    }
    let knob = if on {
        egui::Rect::from_min_size(
            Pos2::new(rect.right() - 16.0, rect.top() + 3.0),
            egui::vec2(13.0, 14.0),
        )
    } else {
        egui::Rect::from_min_size(
            Pos2::new(rect.left() + 3.0, rect.top() + 3.0),
            egui::vec2(13.0, 14.0),
        )
    };
    p.rect_filled(knob, 0.0, if on { theme::BG } else { theme::DIM });
    resp
}

fn performance_metric(
    ui: &mut egui::Ui,
    title: &str,
    current_mhz: Option<u32>,
    max_mhz: Option<u32>,
    usage: Option<u8>,
    width: f32,
    flag_set: bool,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 94.0), egui::Sense::hover());
    theme::card(ui.painter(), rect);
    let p = ui.painter();
    p.text(
        Pos2::new(rect.left() + 14.0, rect.top() + 15.0),
        egui::Align2::LEFT_CENTER,
        title,
        theme::mono(10.0),
        theme::MUTED,
    );
    if flag_set {
        p.text(
            Pos2::new(rect.right() - 14.0, rect.top() + 15.0),
            egui::Align2::RIGHT_CENTER,
            "FLAG SET",
            theme::mono_b(9.0),
            theme::AMBER,
        );
    }
    let reading = current_mhz
        .map(|mhz| mhz.to_string())
        .unwrap_or_else(|| "—".into());
    p.text(
        Pos2::new(rect.left() + 14.0, rect.center().y + 3.0),
        egui::Align2::LEFT_CENTER,
        reading,
        theme::mono_b(27.0),
        if current_mhz.is_some() {
            theme::BRIGHT
        } else {
            theme::DIM
        },
    );
    p.text(
        Pos2::new(rect.left() + 116.0, rect.center().y + 7.0),
        egui::Align2::LEFT_CENTER,
        "MHz",
        theme::mono(11.0),
        theme::MUTED,
    );
    let max = max_mhz
        .map(|mhz| format!("max {mhz} MHz"))
        .unwrap_or_else(|| "max —".into());
    p.text(
        Pos2::new(rect.left() + 14.0, rect.bottom() - 13.0),
        egui::Align2::LEFT_CENTER,
        max,
        theme::mono(9.5),
        theme::DIM,
    );
    if let Some(pct) = usage {
        p.text(
            Pos2::new(rect.right() - 14.0, rect.bottom() - 13.0),
            egui::Align2::RIGHT_CENTER,
            format!("load {pct}%"),
            theme::mono(9.5),
            theme::MUTED,
        );
    }
}

/// Wrapped text made of differently-coloured runs.
fn runs(ui: &mut egui::Ui, size: f32, parts: &[(&str, Color32)]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (text, colour) in parts {
            ui.label(RichText::new(*text).font(theme::mono(size)).color(*colour));
        }
    });
}

/// Shorten `text` with a trailing ellipsis until it fits `max_w`.
fn elide(ctx: &egui::Context, text: &str, font: &egui::FontId, max_w: f32) -> String {
    if theme::tracked_width(ctx, text, font, 0.0) <= max_w {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if theme::tracked_width(ctx, &candidate, font, 0.0) <= max_w {
            return candidate;
        }
    }
    String::new()
}

fn fmt_opt(v: Option<u16>) -> String {
    // An absent reading is "—", never 0. A zero is a measurement.
    v.map(|x| x.to_string()).unwrap_or_else(|| "—".to_owned())
}

fn flag_name(v: u8) -> &'static str {
    match v {
        0 => "clear",
        2 => "set",
        _ => "unknown",
    }
}

fn dmi(field: &str) -> String {
    std::fs::read_to_string(format!("/sys/class/dmi/id/{field}"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn model_name() -> String {
    let v = dmi("sys_vendor");
    let p = dmi("product_name");
    format!("{v} {p}").trim().to_string()
}

fn product() -> String {
    dmi("product_name")
}

fn socket_path() -> String {
    std::env::var("ALIEN_SOCKET").unwrap_or_else(|_| "/run/alien/alien.sock".into())
}

const USAGE: &str = "\
alien-gui — desktop control centre for Acer Predator systems

usage: alien-gui [--tab <screen>]

  --tab <screen>   open on a given screen instead of the dashboard.
                   one of: dashboard, fans, lighting, performance, about
  --reduced-motion keep the startup splash static and skip its fade
  -h, --help       this text

The Predator key launches this through `alien-launch`, which is where a
different starting screen is worth setting: the key is next to the fan vents,
and `--tab fans` is what most people press it for.
";

fn main() -> eframe::Result<()> {
    let mut tab = Tab::Dashboard;
    let mut reduced_motion = matches!(
        std::env::var("ALIEN_REDUCED_MOTION")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    );
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "--tab" => match args.next().as_deref().and_then(Tab::parse) {
                Some(t) => tab = t,
                None => {
                    eprintln!("alien-gui: --tab needs one of: dashboard, fans, lighting, performance, about");
                    std::process::exit(2);
                }
            },
            "--reduced-motion" => reduced_motion = true,
            other => {
                eprintln!("alien-gui: unrecognised argument `{other}`\n");
                eprint!("{USAGE}");
                std::process::exit(2);
            }
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("Alien")
            .with_icon(logo_icon())
            // The Wayland app_id / X11 WM_CLASS. Without it the window has an
            // EMPTY class, which quietly breaks everything that identifies a
            // window: compositor rules, taskbar grouping, .desktop matching,
            // and any "focus it instead of opening a second one" logic. It
            // matches the desktop file's basename, which associates the two.
            .with_app_id("tech.hartle.Alien"),
        ..Default::default()
    };

    #[cfg(target_os = "linux")]
    let options = {
        use winit::platform::x11::EventLoopBuilderExtX11;

        let mut options = options;
        let display_available = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
        let nvidia_loaded = std::path::Path::new("/proc/driver/nvidia/version").is_file();
        let automatic_x11 = display_available && nvidia_loaded;
        let force_x11 = match std::env::var("ALIEN_GUI_BACKEND").as_deref() {
            Ok("x11") => true,
            Ok("wayland") => false,
            Ok("auto") | Err(_) => automatic_x11,
            Ok(other) => {
                eprintln!(
                    "alien-gui: ignoring invalid ALIEN_GUI_BACKEND={other:?}; expected auto, x11, or wayland"
                );
                automatic_x11
            }
        };

        if force_x11 {
            eprintln!("alien-gui: using X11 backend (native NVIDIA Wayland EGL is unsafe)");
            options.event_loop_builder = Some(Box::new(|builder| {
                builder.with_x11();
            }));
        }
        options
    };

    // Note there is no `Device::open()` here, and no early exit if it fails.
    // The daemon is a separate unit that may not be enabled yet; dying with a
    // console message nobody sees is useless, so the window opens either way
    // and the poller owns connecting, reconnecting, and saying which it is.
    eframe::run_native(
        "Alien",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, tab, reduced_motion)))),
    )
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    const ITEM_GAP: f32 = 8.0;

    fn tail_to_profile_bottom() -> f32 {
        2.0 * ITEM_GAP + 12.0 + DASHBOARD_SECTION_H + 8.0 + DASHBOARD_PROFILE_CHIP_H
    }

    fn clearance(plan: DashboardVerticalPlan) -> f32 {
        ITEM_GAP + plan.bottom_padding
    }

    #[test]
    fn roomy_dashboard_keeps_profiles_clear_of_status_bar() {
        let viewport = 1_200.0;
        let consumed = 200.0;
        let plan = dashboard_vertical_plan(viewport, consumed, ITEM_GAP);
        let chip_bottom = consumed + plan.middle_height + tail_to_profile_bottom();

        assert!(plan.middle_height > DASHBOARD_MIDDLE_MIN_H);
        assert_eq!(clearance(plan), 24.0);
        // The extra point is deliberate rounding slack.
        assert_eq!(viewport - chip_bottom, clearance(plan) + 1.0);
    }

    #[test]
    fn declared_minimum_window_fits_without_sacrificing_clearance() {
        // 760x520 leaves roughly 408 vertical points after top/status chrome
        // and the central panel's margins; the narrow telemetry plate consumes
        // 176 of them before HISTORY.
        let viewport = 408.0;
        let consumed = 176.0;
        let plan = dashboard_vertical_plan(viewport, consumed, ITEM_GAP);
        let content_bottom =
            consumed + plan.middle_height + tail_to_profile_bottom() + clearance(plan);

        assert!(plan.middle_height >= DASHBOARD_MIDDLE_MIN_H);
        assert!(content_bottom <= viewport);
        assert!(clearance(plan) >= 16.0);
    }

    #[test]
    fn forced_short_window_preserves_middle_row_and_requires_scroll() {
        let viewport = 280.0;
        let consumed = 176.0;
        let plan = dashboard_vertical_plan(viewport, consumed, ITEM_GAP);
        let content_bottom =
            consumed + plan.middle_height + tail_to_profile_bottom() + clearance(plan);

        assert_eq!(plan.middle_height, DASHBOARD_MIDDLE_MIN_H);
        assert!(content_bottom > viewport);
        assert_eq!(clearance(plan), 16.0);
    }

    #[test]
    fn clearance_scales_and_stays_bounded() {
        let small = dashboard_vertical_plan(500.0, 176.0, ITEM_GAP);
        let large = dashboard_vertical_plan(2_000.0, 200.0, ITEM_GAP);

        assert_eq!(clearance(small), 16.0);
        assert_eq!(clearance(large), 28.0);
    }

    #[test]
    fn profile_strip_keeps_one_row_and_exposes_horizontal_overflow() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = egui::Rect::from_min_size(
                    ui.cursor().min,
                    egui::vec2(552.0, DASHBOARD_PROFILE_CHIP_H),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    let output = dashboard_profile_strip(ui, |ui| {
                        for label in [
                            "SILENT",
                            "PERFORMANCE",
                            "MAX + RED",
                            "ABCDEFGHIJKLMNOPQRSTUVWXYZ123456",
                            "+ NEW",
                        ] {
                            let _ = theme::chip(ui, label, theme::ChipStyle::Idle, 11.0);
                        }
                    });

                    assert_eq!(output.inner_rect.height(), DASHBOARD_PROFILE_CHIP_H);
                    assert_eq!(output.content_size.y, DASHBOARD_PROFILE_CHIP_H);
                    assert!(
                        output.content_size.x > output.inner_rect.width() + 100.0,
                        "a maximum-length user profile must produce a real scroll extent: content {}, viewport {}",
                        output.content_size.x,
                        output.inner_rect.width()
                    );
                });
            });
        });
    }

    #[test]
    fn state_value_elision_preserves_the_label_column() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            let font = theme::mono(11.0);
            let content_width = 252.0;
            let key_width = theme::tracked_width(ctx, "profile", &font, 0.0);
            let room = (content_width - 28.0 - key_width - 16.0).max(0.0);
            let value = "ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
            let shown = elide(ctx, value, &font, room);

            assert!(shown.ends_with('…'));
            assert!(theme::tracked_width(ctx, &shown, &font, 0.0) <= room);
            assert!(
                key_width + 16.0 + theme::tracked_width(ctx, &shown, &font, 0.0)
                    <= content_width - 28.0
            );
        });
    }
}
