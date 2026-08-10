//! `alien-gui` — the desktop control centre.
//!
//! A phosphor-green instrument panel: five screens, a live telemetry poll, and
//! controls that are enabled only where the machine in front of you actually
//! supports them. Every pixel is drawn from primitives; none of Acer's
//! artwork is used or redistributed.
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

use alien_core::profile::Profile;
use alien_core::wmi::OverclockTarget;
use alien_core::{BacklightState, Capabilities, Colour, Device, Direction, Effect, Fan, Sensors, Support};

mod gauge;
mod glyph;
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

/// Flavour, not data: a plate number for the panel, in the manner of the
/// equipment this thing is pretending to be.
const PLATE: &str = "WY-ALN/02 · REV C";

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
    cpu_turbo: u8,
    gpu_turbo: u8,
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
    cpu_turbo: u8,
    gpu_turbo: u8,
    backlight: Option<BacklightState>,
    caps: Capabilities,
    cpu_hist: Vec<Option<u16>>,
    gpu_hist: Vec<Option<u16>>,
    cpu_rpm_hist: Vec<Option<u16>>,
    gpu_rpm_hist: Vec<Option<u16>>,
    fan_readback: [Option<u8>; 2],
    link: LinkState,
    interface: String,
}

/// Append one sample, keeping the last [`HISTORY`] of them.
///
/// The absence is preserved rather than flattened to zero. Storing a missing
/// reading as 0 put cliffs to the floor in the temperature history — see
/// [`gauge::plot_into`].
fn push(v: &mut Vec<Option<u16>>, x: Option<u16>) {
    v.push(x);
    if v.len() > HISTORY {
        v.remove(0);
    }
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
            let wait = if was == Link::Never { RETRY_NEW } else { RETRY_LOST };

            match Device::open() {
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

        let s = dev.sensors();
        let cpu = dev.overclock(OverclockTarget::Cpu).unwrap_or(0);
        let gpu = dev.overclock(OverclockTarget::Gpu).unwrap_or(0);
        let duty = if want_duty.load(Ordering::Relaxed) {
            [dev.fan_percent(Fan::Cpu).ok(), dev.fan_percent(Fan::Gpu).ok()]
        } else {
            [None, None]
        };

        if let Ok(mut sh) = shared.lock() {
            push(&mut sh.cpu_hist, s.cpu_temp_c);
            push(&mut sh.gpu_hist, s.gpu_temp_c);
            push(&mut sh.cpu_rpm_hist, s.cpu_fan_rpm);
            push(&mut sh.gpu_rpm_hist, s.gpu_fan_rpm);
            sh.sensors = s;
            sh.cpu_turbo = cpu;
            sh.gpu_turbo = gpu;
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

    per_effect: [EffectSettings; 7],
    zone_colours: [[f32; 3]; 4],
    brightness: u8,
    effect: Effect,
    profile: Option<String>,
    seeded: bool,
    mem: alien_core::Lighting,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FanMode {
    Max,
    Auto,
    Manual,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, tab: Tab) -> Self {
        theme::apply(&cc.egui_ctx);

        let mem = alien_core::Lighting::load();
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
            fan_mode: FanMode::Auto,
            duty_sent: [None, None],
            per_effect: [EffectSettings { colour: [0.0, 0.68, 0.78], speed: 5, left_to_right: true }; 7],
            zone_colours: [[0.0, 0.68, 0.78]; 4],
            brightness: 100,
            effect: Effect::Static,
            profile: None,
            seeded: false,
            mem,
        }
    }

    fn device(&self) -> Option<Arc<Device>> {
        self.conn.lock().ok().and_then(|g| g.clone())
    }

    fn act<T>(&mut self, r: alien_core::Result<T>, ok: &str) {
        match r {
            Ok(_) => {
                self.status = ok.to_owned();
                self.status_bad = false;
            }
            Err(e) => {
                self.status = format!("failed: {e}");
                self.status_bad = true;
            }
        }
    }

    /// Push the current lighting selection at the hardware and remember it.
    fn apply_lighting(&mut self, zones: bool) {
        let Some(dev) = self.device() else { return };
        let idx = self.effect as usize;

        if zones || self.effect == Effect::Static {
            // Static colour lives in the per-zone registers, not in the effect
            // payload — so the mode/brightness call and the four zone writes
            // have to go together, in that order.
            let cs = self.zone_colours.map(to_colour);
            let r = dev.set_zone_colours(cs, self.brightness);
            self.act(r, "zone colours applied");
            self.mem.set_zone_colours(cs);
            self.mem.set_colour(Effect::Static, cs[0]);
        } else {
            let set = self.per_effect[idx];
            let c = to_colour(set.colour);
            // Clamp here as well as in the UI: a slot seeded from a hand-edited
            // file could still hold 0 for an animated effect.
            let speed = set.speed.max(1);
            let dir = if set.left_to_right { Direction::LeftToRight } else { Direction::RightToLeft };
            let r = dev.set_effect(self.effect, speed, self.brightness, dir, c);
            self.act(r, &format!("lighting → {}", self.effect.name()));
            self.mem.set_colour(self.effect, c);
            self.mem.set_speed(self.effect, speed);
            self.mem.set_direction(self.effect, dir);
        }
        self.mem.brightness = self.brightness;
        let _ = self.mem.save();
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
        self.want_duty
            .store(self.tab == Tab::Fans && self.fan_mode == FanMode::Manual, Ordering::Relaxed);

        let snap = {
            let sh = self.shared.lock().ok();
            match sh {
                Some(s) => Snapshot {
                    sensors: s.sensors,
                    cpu_turbo: s.cpu_turbo,
                    gpu_turbo: s.gpu_turbo,
                    backlight: s.backlight,
                    caps: s.caps.clone().unwrap_or_default(),
                    cpu_hist: s.cpu_hist.clone(),
                    gpu_hist: s.gpu_hist.clone(),
                    cpu_rpm_hist: s.cpu_rpm_hist.clone(),
                    gpu_rpm_hist: s.gpu_rpm_hist.clone(),
                    fan_readback: s.fan_readback,
                    link: s.link.clone().unwrap_or_default(),
                    interface: s.interface.clone(),
                },
                None => return,
            }
        };

        // Seed the editable controls from the hardware exactly once, so the
        // controls open where the machine is rather than snapping it to a
        // default the moment the user touches anything.
        if !self.seeded {
            if let Some(b) = snap.backlight {
                // Colours come from the shared store, not from firmware: in
                // static mode the firmware's RGB field is not what is on the
                // keyboard (the per-zone registers are), and it holds only one
                // colour where we remember seven.
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
                self.brightness = self.mem.brightness;
                // The live effect DOES come from firmware — machine state, not
                // a preference.
                self.effect = b.effect;
                self.seeded = true;
            }
        }

        // The nav rail numbers its entries 01–05; the number keys follow it.
        let pressed = ctx.input(|i| {
            [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5]
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
                p.hline(rect.x_range(), rect.bottom() - 0.5, theme::hair(theme::LINE));

                let y = rect.center().y;
                glyph::logo(p, Pos2::new(rect.left() + 26.0, y), 20.0);

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

                let model = if narrow { product() } else { model_name().to_uppercase() };
                p.text(
                    Pos2::new(x, y),
                    egui::Align2::LEFT_CENTER,
                    model,
                    theme::mono(11.0),
                    theme::MUTED,
                );

                // Right-hand furniture, laid out right to left.
                let turbo = snap.cpu_turbo == 2 || snap.gpu_turbo == 2;
                let dead = snap.link.link != Link::Up;
                let (mark, label, colour) = if dead {
                    (None, "—", theme::DIM)
                } else if turbo {
                    (Some(Mark::TriUp), "TURBO", theme::AMBER)
                } else {
                    (Some(Mark::Dot), "NORMAL", theme::MUTED)
                };

                let font = theme::mono_b(10.0);
                let tw = theme::tracked_width(ctx, label, &font, 0.0);
                let pill_w = tw + if mark.is_some() { 32.0 } else { 20.0 };
                let pill = egui::Rect::from_min_size(
                    Pos2::new(rect.right() - 16.0 - pill_w, y - 11.0),
                    egui::vec2(pill_w, 22.0),
                );
                p.rect_stroke(pill, 0.0, theme::hair(if dead { theme::LINE } else { colour.gamma_multiply(0.6) }), StrokeKind::Inside);
                let mut tx = pill.left() + 10.0;
                if let Some(m) = mark {
                    glyph::draw(p, Pos2::new(tx + 4.0, y), m, 8.0, colour);
                    tx += 14.0;
                }
                p.text(Pos2::new(tx, y), egui::Align2::LEFT_CENTER, label, font, colour);

                if !narrow {
                    p.text(
                        Pos2::new(pill.left() - 14.0, y),
                        egui::Align2::RIGHT_CENTER,
                        PLATE,
                        theme::mono(9.0),
                        theme::DIM,
                    );
                }
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
                ui.painter().vline(
                    full.right() - 0.5,
                    full.y_range(),
                    theme::hair(theme::LINE),
                );

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
                            egui::Rect::from_min_size(rect.left_top(), egui::vec2(3.0, rect.height())),
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
                        if selected { theme::sans(13.0) } else { theme::sans(13.0) },
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
                glyph::draw(p, Pos2::new(foot.left() + 66.0, foot.top() + 15.0), mark, 7.0, colour);
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
                let right = if snap.interface.is_empty() { socket_path() } else { snap.interface.clone() };
                let right_w = theme::tracked_width(ui.ctx(), &right, &font, 0.0);
                let room = (rect.width() - 30.0 - 14.0 - right_w - 18.0).max(80.0);
                let text = elide(ui.ctx(), &text, &font, room);

                p.text(Pos2::new(rect.left() + 30.0, y), egui::Align2::LEFT_CENTER, text, font.clone(), theme::MUTED);
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
        theme::section(ui, "TELEMETRY", Some("ALN-TH-01 · 1 Hz"));
        ui.add_space(10.0);

        // Gauge plate.
        let s = &snap.sensors;
        let gsize = if narrow { 104.0 } else { 128.0 };
        let plate_h = gsize + 20.0;
        let plate = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(ui.available_width(), plate_h),
        );
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

        // History and state, side by side. Reserve the profiles block first so
        // the middle row takes exactly what is left.
        let profiles_h = 24.0 + 29.0 + 12.0;
        let mid_h = (ui.available_height() - profiles_h).max(96.0);
        let total_w = ui.available_width();
        let gap = 12.0;
        let hist_w = if narrow { total_w } else { (total_w - gap) * 0.63 };

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
                            reading_colour: theme::temp_colour(snap.sensors.cpu_temp_c.unwrap_or(0)),
                            mark: snap.sensors.cpu_temp_c.map(|_| Mark::Dot),
                            min_span: 10.0,
                        }
                        .show(ui, egui::vec2(pw, plots_h));
                        gauge::SparkCard {
                            title: "GPU °C",
                            values: &snap.gpu_hist,
                            colour: theme::GREEN,
                            reading: fmt_opt(snap.sensors.gpu_temp_c),
                            reading_colour: theme::temp_colour(snap.sensors.gpu_temp_c.unwrap_or(0)),
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
        ui.horizontal(|ui| {
            for p in Profile::builtins() {
                let active = self.profile.as_deref() == Some(p.name.as_str());
                let style = if active {
                    theme::ChipStyle::Active
                } else if p.name == "turbo" {
                    theme::ChipStyle::Warn
                } else {
                    theme::ChipStyle::Idle
                };
                let label = if p.name == "turbo" { "TURBO".to_owned() } else { p.name.to_uppercase() };
                if theme::chip(ui, &label, style, 11.0).on_hover_text(&p.description).clicked() {
                    if let Some(dev) = self.device() {
                        let r = p.apply(&dev);
                        let ok = r.is_ok();
                        self.act(r, &format!("applied profile: {}", p.name));
                        if ok {
                            self.profile = Some(p.name.clone());
                        }
                    }
                }
            }
            if !narrow {
                ui.add_space(6.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                ui.label(
                    RichText::new("fans max + turbo off — captures the whole measured gain")
                        .font(theme::mono(10.0))
                        .color(theme::DIM),
                );
            }
        });
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
            p.text(Pos2::new(lx, y), egui::Align2::LEFT_CENTER, k, font.clone(), theme::MUTED);
            p.text(Pos2::new(rx, y), egui::Align2::RIGHT_CENTER, v, font.clone(), c);
        };

        put(
            p,
            y,
            "turbo",
            &format!("cpu {} · gpu {}", flag_name(snap.cpu_turbo), flag_name(snap.gpu_turbo)),
            theme::TEXT,
        );
        y += step;

        // Backlight: a swatch of the live colour, then the mode and level.
        p.text(Pos2::new(lx, y), egui::Align2::LEFT_CENTER, "backlight", font.clone(), theme::MUTED);
        match snap.backlight {
            Some(b) => {
                let text = format!("{} · {}%", b.effect.name(), b.brightness);
                let tw = theme::tracked_width(ui.ctx(), &text, &font, 0.0);
                p.text(Pos2::new(rx, y), egui::Align2::RIGHT_CENTER, &text, font.clone(), theme::TEXT);
                let sw = egui::Rect::from_center_size(
                    Pos2::new(rx - tw - 12.0, y),
                    egui::vec2(14.0, 9.0),
                );
                p.rect_filled(sw, 0.0, Color32::from_rgb(b.colour.r, b.colour.g, b.colour.b));
            }
            None => {
                p.text(Pos2::new(rx, y), egui::Align2::RIGHT_CENTER, "—", font.clone(), theme::DIM);
            }
        }
        y += step;

        put(
            p,
            y,
            "profile",
            self.profile.as_deref().unwrap_or("—"),
            if self.profile.is_some() { theme::BRIGHT } else { theme::DIM },
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

    fn fans(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        theme::section(ui, "FAN MODE", Some("ALN-FN-04"));
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
                        if mode == FanMode::Manual { theme::ChipStyle::Outline } else { theme::ChipStyle::Active }
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
                            if self.fan_mode == mode { theme::BRIGHT } else { theme::MUTED },
                        );
                    }
                    if resp.clicked() {
                        self.fan_mode = mode;
                        let Some(dev) = self.device() else { return };
                        match mode {
                            FanMode::Max => {
                                let r = dev.fans_max();
                                self.act(r, "fans at maximum");
                            }
                            FanMode::Auto => {
                                let r = dev.fans_auto();
                                self.act(r, "fans on the EC curve");
                            }
                            FanMode::Manual => {
                                let r = dev
                                    .set_fan_percent(Fan::Cpu, self.cpu_pct)
                                    .and_then(|_| dev.set_fan_percent(Fan::Gpu, self.gpu_pct));
                                self.duty_sent = [Some(Instant::now()); 2];
                                self.act(r, "manual duty");
                            }
                        }
                    }
                }
            });
            ui.add_space(8.0);
            runs(
                ui,
                11.0,
                &[
                    ("Maximum is worth roughly ", theme::MUTED),
                    ("+48% sustained CPU throughput", theme::BRIGHT),
                    (" on this chassis: the stock EC curve holds the processor in thermal throttle.", theme::MUTED),
                ],
            );
        });

        ui.add_space(12.0);
        theme::section(ui, "MANUAL DUTY", None);
        ui.add_space(10.0);

        let manual = self.fan_mode == FanMode::Manual && snap.caps.manual_fan_duty != Support::No;
        card_body(ui, cw, |ui| {
            for (i, (name, fan)) in [("CPU", Fan::Cpu), ("GPU", Fan::Gpu)].into_iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).font(theme::mono(11.0)).color(if manual { theme::MUTED } else { theme::DIM }));
                    ui.add_space(6.0);
                    let want = if i == 0 { &mut self.cpu_pct } else { &mut self.gpu_pct };
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
                        RichText::new(format!("{:>3} %", if i == 0 { self.cpu_pct } else { self.gpu_pct }))
                            .font(theme::mono_b(12.0))
                            .color(if manual { theme::TEXT } else { theme::DIM }),
                    );
                    ui.add_space(10.0);
                    self.readback_line(ui, snap, i, manual);
                });
                ui.add_space(6.0);
            }
            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "Duty is not linear in RPM. Sent on release, confirmed by reading it back from \
                     firmware — the fans take 8–10 s to settle, so the readback lags the slider honestly.",
                )
                .font(theme::mono(10.0))
                .color(theme::DIM),
            );
        });

        ui.add_space(12.0);
        theme::section(ui, "NOW", None);
        ui.add_space(10.0);

        let h = ui.available_height().max(120.0);
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), h));
        theme::card(ui.painter(), rect);
        let inner = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(rect.width() - 32.0, (h - 16.0).min(128.0)),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.horizontal_top(|ui| {
                let g = ui.available_height().min(128.0);
                gauge::Gauge::fan("CPU FAN", snap.sensors.cpu_fan_rpm, 6000.0).show(ui, g);
                ui.add_space(10.0);
                gauge::Gauge::fan("GPU FAN", snap.sensors.gpu_fan_rpm, 6500.0).show(ui, g);
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
            });
        });
        ui.advance_cursor_after_rect(rect);
    }

    /// The honest readback column: what firmware says the duty is, and whether
    /// that has caught up with what we asked for.
    fn readback_line(&mut self, ui: &mut egui::Ui, snap: &Snapshot, i: usize, manual: bool) {
        let want = if i == 0 { self.cpu_pct } else { self.gpu_pct };
        let got = snap.fan_readback[i];
        let settling = self
            .duty_sent[i]
            .map(|t| t.elapsed() < Duration::from_secs(10))
            .unwrap_or(false);

        if !manual {
            ui.label(RichText::new("").font(theme::mono(10.0)));
            return;
        }
        match got {
            None => {
                ui.label(RichText::new("readback —").font(theme::mono(10.0)).color(theme::DIM));
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

    fn lighting_left(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        let can = snap.caps.backlight_effects != Support::No;
        theme::section(ui, "EFFECT", Some("ALN-KB-16"));
        ui.add_space(10.0);

        let mut changed = false;
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
                    can,
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
                RichText::new(
                    "= uses the firmware's own palette — your colour choice has no effect there, so none is offered",
                )
                .font(theme::mono(10.0))
                .color(theme::DIM),
            );
        });

        ui.add_space(12.0);
        let mut zones_changed = false;
        if self.effect == Effect::Static {
            theme::section(ui, "ZONE COLOURS", Some("4 INDEPENDENT REGISTERS"));
            ui.add_space(10.0);
            card_body(ui, cw, |ui| {
                ui.horizontal(|ui| {
                    for (i, z) in self.zone_colours.iter_mut().enumerate() {
                        ui.vertical(|ui| {
                            ui.spacing_mut().interact_size = egui::vec2(52.0, 30.0);
                            // `.changed()` only. An earlier version also
                            // compared the value before and after, to catch
                            // edits the widget reported late — but the picker
                            // round-trips rgb through Color32 and comes back a
                            // float or two different, so that comparison fired
                            // on the very first frame and wrote the keyboard on
                            // every launch without anyone asking.
                            if ui.color_edit_button_rgb(z).changed() {
                                zones_changed = true;
                            }
                            ui.label(
                                RichText::new(format!("Z{}", i + 1))
                                    .font(theme::mono(9.0))
                                    .color(theme::MUTED),
                            );
                        });
                    }
                    ui.add_space(6.0);
                    if theme::tag(ui, "ALL ALIKE", false, true, false).clicked() {
                        self.zone_colours = [self.zone_colours[0]; 4];
                        zones_changed = true;
                    }
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "the keyboard genuinely has four independently addressable zones — one picker each, left to right",
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
                    if ui.color_edit_button_rgb(&mut self.per_effect[self.effect as usize].colour).changed() {
                        changed = true;
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("remembered for {} only", self.effect.name()))
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
                ui.label(RichText::new("BRIGHTNESS").font(theme::mono(11.0)).color(theme::MUTED));
                ui.add_space(lab_w - 78.0);
                let sw = (ui.available_width() - 60.0).max(80.0);
                if theme::slider(ui, &mut self.brightness, 0..=100, sw, can).drag_stopped() {
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
                if theme::slider(ui, &mut sp, 1..=9, sw, animated).drag_stopped() {
                    self.per_effect[idx].speed = sp;
                    changed = true;
                }
                ui.label(
                    RichText::new(if animated { format!("{sp:>4}") } else { "   —".to_owned() })
                        .font(theme::mono_b(12.0))
                        .color(if animated { theme::TEXT } else { theme::DIM }),
                );
            });
            ui.add_space(8.0);

            // Direction.
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("DIRECTION")
                        .font(theme::mono(11.0))
                        .color(if animated { theme::MUTED } else { theme::DIM }),
                );
                ui.add_space(lab_w - 74.0);
                let idx = self.effect as usize;
                let ltr = self.per_effect[idx].left_to_right;
                ui.spacing_mut().item_spacing.x = 0.0;
                if theme::tag(ui, "L-R", animated && ltr, animated, false).clicked() {
                    self.per_effect[idx].left_to_right = true;
                    changed = true;
                }
                if theme::tag(ui, "R-L", animated && !ltr, animated, false).clicked() {
                    self.per_effect[idx].left_to_right = false;
                    changed = true;
                }
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.add_space(10.0);
                if !animated {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.label(
                        RichText::new("static does not animate — speed and direction wake with an animated effect")
                            .font(theme::mono(10.0))
                            .color(theme::DIM),
                    );
                }
            });
        });

        if zones_changed {
            self.apply_lighting(true);
        } else if changed {
            self.apply_lighting(false);
        }
    }

    fn lighting_right(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        theme::section(ui, "PER-KEY", None);
        ui.add_space(10.0);

        let supported = snap.caps.per_key == Support::Yes;
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
            } else {
                theme::badge(ui, Mark::Cross, "UNSUPPORTED", theme::DIM, true);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "This keyboard is four-zone: keys are wired into four banks that share LEDs, so an \
                         individual key cannot be addressed by any software. Per-key needs an ITE 8291 \
                         controller (Triton 500 SE, Helios 16/18).",
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
                ui.painter().rect_filled(
                    r,
                    0.0,
                    Color32::from_rgb(col.r, col.g, col.b).gamma_multiply(a),
                );
                ui.painter().text(
                    Pos2::new(r.center().x, rect.bottom() + 9.0),
                    egui::Align2::CENTER_CENTER,
                    format!("Z{}", i + 1),
                    theme::mono(9.0),
                    theme::DIM,
                );
            }
            ui.add_space(14.0);
            ui.label(
                RichText::new(format!(
                    "{} · brightness {}% · verified by looking at the keyboard, not by readback",
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
                let r = dev.backlight_off();
                self.act(r, "backlight off");
            }
        }
    }

    fn performance(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        theme::section(ui, "TURBO FLAGS", Some("ALN-OC-22"));
        ui.add_space(10.0);

        card_body(ui, cw, |ui| {
            // GPU — this one demonstrably goes through the interface.
            let gpu_on = snap.gpu_turbo == 2;
            ui.horizontal(|ui| {
                ui.label(RichText::new("GPU").font(theme::mono(11.0)).color(theme::MUTED));
                ui.add_space(8.0);
                let live = snap.caps.gpu_overclock != Support::No;
                if toggle(ui, gpu_on, live).clicked() {
                    if let Some(dev) = self.device() {
                        let r = dev.set_overclock(OverclockTarget::Gpu, !gpu_on);
                        self.act(r, if gpu_on { "gpu turbo off" } else { "gpu turbo on" });
                    }
                }
                ui.add_space(8.0);
                if gpu_on {
                    glyph::labelled(ui, Mark::TriUp, "TURBO", theme::mono(11.0), theme::AMBER);
                } else {
                    ui.label(RichText::new("off").font(theme::mono(11.0)).color(theme::MUTED));
                }
                ui.add_space(8.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                ui.label(
                    RichText::new("goes through function 22 — flag readback 2 (turbo)")
                        .font(theme::mono(10.0))
                        .color(theme::MUTED),
                );
            });

            ui.add_space(10.0);
            let r = ui.max_rect();
            ui.painter().hline(r.x_range(), ui.cursor().min.y, theme::hair(theme::LINE));
            ui.add_space(10.0);

            // CPU — accepted, inert on this model. Say so.
            let cpu_on = snap.cpu_turbo == 2;
            let unverified = snap.caps.cpu_overclock == Support::Unverified;
            ui.horizontal(|ui| {
                ui.label(RichText::new("CPU").font(theme::mono(11.0)).color(theme::DIM));
                ui.add_space(8.0);
                if toggle(ui, cpu_on, !unverified).clicked() {
                    if let Some(dev) = self.device() {
                        let r = dev.set_overclock(OverclockTarget::Cpu, !cpu_on);
                        self.act(r, if cpu_on { "cpu turbo off" } else { "cpu turbo on" });
                    }
                }
                ui.add_space(8.0);
                if unverified {
                    theme::badge(ui, Mark::Query, "ACCEPTED, UNVERIFIED", theme::AMBER, true);
                    ui.add_space(8.0);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.label(
                        RichText::new("write accepted; inert here — Feature.ini gates it off")
                            .font(theme::mono(10.0))
                            .color(theme::MUTED),
                    );
                } else {
                    ui.label(
                        RichText::new(if cpu_on { "turbo" } else { "off" })
                            .font(theme::mono(11.0))
                            .color(theme::MUTED),
                    );
                }
            });

            ui.add_space(10.0);
            runs(
                ui,
                10.5,
                &[
                    ("PredatorSense's \"CPU turbo\" on this model is Intel XTU power limits, not this \
                      interface. Measured with fans pinned at maximum, these flags produced ", theme::MUTED),
                    ("no benchmark change here", theme::TEXT),
                    (" — the fan curve is what moves the numbers on this chassis.", theme::MUTED),
                ],
            );
        });

        ui.add_space(12.0);
        theme::section(ui, "TEMPERATURES", Some("UNDER LOAD"));
        ui.add_space(10.0);

        let h = ui.available_height().max(140.0);
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), h));
        theme::card(ui.painter(), rect);
        let inner = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(rect.width() - 20.0, (h - 20.0).min(140.0)),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.horizontal(|ui| {
                let g = ui.available_height().min(140.0);
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
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(left_w, 40.0), egui::Sense::hover());
        let p = ui.painter();
        let y = rect.center().y;
        glyph::logo(p, Pos2::new(rect.left() + 17.0, y), 34.0);
        let w = theme::tracked(
            ui.ctx(),
            p,
            Pos2::new(rect.left() + 46.0, y),
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
                "Fan, lighting, turbo and telemetry control for Acer Predator and Nitro \
                 laptops on Linux, with no vendor software. A clean-room implementation of \
                 the gaming WMI protocol, verified against real firmware.",
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
                ("Interface", snap.interface.clone(), theme::TEXT),
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
                            if c == theme::OK { Mark::Dot } else { Mark::Cross },
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
        ui.label(
            RichText::new("GPL-2.0-or-later · alien.hartle.tech · not affiliated with Acer")
                .font(theme::mono(10.0))
                .color(theme::DIM),
        );
    }

    fn about_right(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cw = ui.max_rect().width();
        theme::section(ui, "WHAT THIS MACHINE SUPPORTS", Some("alien doctor"));
        ui.add_space(8.0);
        card_body(ui, cw, |ui| {
            for (name, sup) in snap.caps.rows() {
                let (mark, text, colour) = match sup {
                    Support::Yes => (Mark::Dot, "yes", theme::OK),
                    Support::No => (Mark::Cross, "no", theme::DIM),
                    Support::Unverified => {
                        (Mark::Query, "accepted, unverified", theme::AMBER)
                    }
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
                    theme::mono(11.0),
                    theme::MUTED,
                );
                let tw = theme::tracked_width(ui.ctx(), text, &theme::mono(11.0), 0.0);
                glyph::draw(p, Pos2::new(r.right() - tw - 11.0, y), mark, 7.0, colour);
                p.text(
                    Pos2::new(r.right(), y),
                    egui::Align2::RIGHT_CENTER,
                    text,
                    theme::mono(11.0),
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

                glyph::logo(p, Pos2::new(lx + 15.0, y), 30.0);
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
                            if theme::chip(ui, "RUN DOCTOR", theme::ChipStyle::Idle, 11.0).clicked() {
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
        if enabled { egui::Sense::click() } else { egui::Sense::hover() },
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
        egui::Rect::from_min_size(Pos2::new(rect.right() - 16.0, rect.top() + 3.0), egui::vec2(13.0, 14.0))
    } else {
        egui::Rect::from_min_size(Pos2::new(rect.left() + 3.0, rect.top() + 3.0), egui::vec2(13.0, 14.0))
    };
    p.rect_filled(knob, 0.0, if on { theme::BG } else { theme::DIM });
    resp
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
        0 => "off",
        2 => "turbo",
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
alien-gui — desktop control centre for Acer Predator/Nitro laptops

usage: alien-gui [--tab <screen>]

  --tab <screen>   open on a given screen instead of the dashboard.
                   one of: dashboard, fans, lighting, performance, about
  -h, --help       this text

The Predator key launches this through `alien-launch`, which is where a
different starting screen is worth setting: the key is next to the fan vents,
and `--tab fans` is what most people press it for.
";

fn main() -> eframe::Result<()> {
    let mut tab = Tab::Dashboard;
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
            // The Wayland app_id / X11 WM_CLASS. Without it the window has an
            // EMPTY class, which quietly breaks everything that identifies a
            // window: compositor rules, taskbar grouping, .desktop matching,
            // and any "focus it instead of opening a second one" logic. It
            // matches the desktop file's basename, which associates the two.
            .with_app_id("tech.hartle.Alien"),
        ..Default::default()
    };

    // Note there is no `Device::open()` here, and no early exit if it fails.
    // The daemon is a separate unit that may not be enabled yet; dying with a
    // console message nobody sees is useless, so the window opens either way
    // and the poller owns connecting, reconnecting, and saying which it is.
    eframe::run_native("Alien", options, Box::new(move |cc| Ok(Box::new(App::new(cc, tab)))))
}
