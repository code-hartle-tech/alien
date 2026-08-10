//! `alien-gui` — the desktop control centre.
//!
//! A reimplementation of PredatorSense's layout for Linux: the same tabs, the
//! same dark palette, the same information in the same places, drawn entirely
//! with primitives. None of Acer's artwork is used or redistributed.
//!
//! # Why egui rather than GTK
//!
//! The whole point is a fully skinned vendor look. Adwaita fights that at
//! every step — the platform theme is the thing we are deliberately not using.
//! egui draws every pixel itself, produces a single static binary, and drops
//! into flatpak, snap, Docker and six distro packages without a toolkit
//! runtime to chase.
//!
//! # Talking to the hardware
//!
//! Through `alien-daemon` over its socket, never directly. A GUI cannot run as
//! root on Wayland in any sane way, and a sandboxed build has no path to
//! `/proc/acpi/call` at all. The daemon exists precisely so this process can
//! stay unprivileged.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, RichText};

use alien_core::profile::Profile;
use alien_core::wmi::OverclockTarget;
use alien_core::{BacklightState, Colour, Device, Direction, Effect, Fan, Sensors};

mod gauge;
mod theme;

const HISTORY: usize = 120;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Dashboard,
    Fans,
    Lighting,
    Performance,
    About,
}

impl Tab {
    const ALL: [(Tab, &'static str); 5] = [
        (Tab::Dashboard, "Dashboard"),
        (Tab::Fans, "Fan Control"),
        (Tab::Lighting, "Lighting"),
        (Tab::Performance, "Performance"),
        (Tab::About, "About"),
    ];
}

#[derive(Default)]
struct Shared {
    sensors: Sensors,
    cpu_turbo: u8,
    gpu_turbo: u8,
    backlight: Option<BacklightState>,
    cpu_hist: Vec<u16>,
    gpu_hist: Vec<u16>,
    cpu_rpm_hist: Vec<u16>,
    gpu_rpm_hist: Vec<u16>,
}

struct App {
    dev: Arc<Device>,
    shared: Arc<Mutex<Shared>>,
    running: Arc<AtomicBool>,
    tab: Tab,
    status: String,
    interface: String,

    // Editable state. Seeded from the firmware on first read so the controls
    // start where the hardware actually is, not at an arbitrary default.
    cpu_pct: u8,
    gpu_pct: u8,
    manual_fans: bool,
    /// Per-effect colour and speed, indexed by `Effect as usize`.
    ///
    /// Each mode keeps its own. Switching from a teal Static to Wave and back
    /// should return the teal, not whatever Wave was last set to — the
    /// alternative is that picking a colour silently overwrites the one you
    /// chose for a different mode.
    per_effect: [EffectSettings; 7],
    /// Static mode colours the four zones independently — that is what the
    /// hardware actually does, so the UI stops pretending it is one colour.
    zone_colours: [[f32; 3]; 4],
    /// Brightness is the backlight level, not a property of the animation, so
    /// it stays global.
    brightness: u8,
    effect: Effect,
    seeded: bool,
    /// Probed once at startup: ~25 firmware calls, so never per frame.
    caps: alien_core::Capabilities,
    /// Shared with the CLI and TUI — see alien_core::lighting.
    mem: alien_core::Lighting,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, dev: Device) -> Self {
        theme::apply(&cc.egui_ctx);

        let caps = dev.capabilities();
        let mem = alien_core::Lighting::load();
        let dev = Arc::new(dev);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let running = Arc::new(AtomicBool::new(true));
        let interface = dev.method_path();

        // Poller on its own thread. Firmware calls take tens of milliseconds
        // and there are seven per cycle; doing that on the UI thread would
        // drop frames on every tick.
        {
            let dev = Arc::clone(&dev);
            let shared = Arc::clone(&shared);
            let running = Arc::clone(&running);
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                while running.load(Ordering::Relaxed) {
                    let s = dev.sensors();
                    let cpu = dev.overclock(OverclockTarget::Cpu).unwrap_or(0);
                    let gpu = dev.overclock(OverclockTarget::Gpu).unwrap_or(0);
                    let bl = dev.backlight().ok();
                    if let Ok(mut sh) = shared.lock() {
                        push(&mut sh.cpu_hist, s.cpu_temp_c);
                        push(&mut sh.gpu_hist, s.gpu_temp_c);
                        push(&mut sh.cpu_rpm_hist, s.cpu_fan_rpm);
                        push(&mut sh.gpu_rpm_hist, s.gpu_fan_rpm);
                        sh.sensors = s;
                        sh.cpu_turbo = cpu;
                        sh.gpu_turbo = gpu;
                        sh.backlight = bl;
                    }
                    // Wake the UI rather than making it spin at 60 fps for
                    // data that changes once a second.
                    ctx.request_repaint();
                    std::thread::sleep(Duration::from_millis(1000));
                }
            });
        }

        App {
            dev,
            shared,
            running,
            tab: Tab::Dashboard,
            status: "ready".into(),
            interface,
            cpu_pct: 60,
            gpu_pct: 60,
            manual_fans: false,
            per_effect: [
                EffectSettings::defaults(Effect::Static),
                EffectSettings::defaults(Effect::Breath),
                EffectSettings::defaults(Effect::Neon),
                EffectSettings::defaults(Effect::Wave),
                EffectSettings::defaults(Effect::Shifting),
                EffectSettings::defaults(Effect::Zoom),
                EffectSettings::defaults(Effect::Ripple),
            ],
            zone_colours: [[0.0, 0.68, 0.78]; 4],
            brightness: 100,
            effect: Effect::Static,
            seeded: false,
            caps,
            mem,
        }
    }

    fn act<T>(&mut self, r: alien_core::Result<T>, ok: &str) {
        self.status = match r {
            Ok(_) => ok.to_string(),
            Err(e) => format!("failed: {e}"),
        };
    }
}

fn push(v: &mut Vec<u16>, x: Option<u16>) {
    v.push(x.unwrap_or(0));
    if v.len() > HISTORY {
        v.remove(0);
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.running.store(false, Ordering::Relaxed);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let snapshot = self.shared.lock().ok().map(|s| Snapshot {
            sensors: s.sensors,
            cpu_turbo: s.cpu_turbo,
            gpu_turbo: s.gpu_turbo,
            backlight: s.backlight,
            cpu_hist: s.cpu_hist.clone(),
            gpu_hist: s.gpu_hist.clone(),
            cpu_rpm_hist: s.cpu_rpm_hist.clone(),
            gpu_rpm_hist: s.gpu_rpm_hist.clone(),
        });
        let snap = snapshot.unwrap_or_default();

        // Seed the editable controls from the hardware exactly once, so the
        // sliders open where the machine is rather than snapping it to a
        // default the moment the user touches anything.
        if !self.seeded {
            if let Some(b) = snap.backlight {
                // Colours come from the shared store, not from firmware: in
                // static mode the firmware's RGB field is not what is on the
                // keyboard (the per-zone registers are), and it holds only
                // one colour where we remember seven.
                for e in Effect::ALL {
                    let c = self.mem.colour(e);
                    self.per_effect[e as usize] = EffectSettings {
                        colour: [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0],
                        speed: self.mem.speed(e),
                    };
                }
                let z = self.mem.zone_colours();
                for (slot, c) in self.zone_colours.iter_mut().zip(z) {
                    *slot = [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0];
                }
                self.brightness = self.mem.brightness;
                // The live effect DOES come from firmware — that is machine
                // state, not a preference.
                self.effect = b.effect;
                self.seeded = true;
            }
        }

        self.top_bar(ctx, &snap);
        self.side_nav(ctx);
        self.status_bar(ctx);

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Dashboard => self.dashboard(ui, &snap),
            Tab::Fans => self.fans(ui, &snap),
            Tab::Lighting => self.lighting(ui),
            Tab::Performance => self.performance(ui, &snap),
            Tab::About => self.about(ui),
        });
    }
}

#[derive(Clone, Copy)]
struct EffectSettings {
    colour: [f32; 3],
    /// 0 is a valid speed only for Static. For an animation it means "do not
    /// animate", which looks exactly like a broken effect — see `defaults`.
    speed: u8,
}

impl EffectSettings {
    /// Sensible starting point per effect.
    ///
    /// The speed matters: an animated effect inherits the previous mode's
    /// speed otherwise, and coming from Static that is 0 — the animation is
    /// set, accepted by firmware, reads back correctly, and does nothing at
    /// all. That is indistinguishable from an unsupported effect.
    fn defaults(e: Effect) -> Self {
        EffectSettings {
            colour: [0.0, 0.68, 0.78],
            speed: if e == Effect::Static { 0 } else { 5 },
        }
    }
}

#[derive(Default, Clone)]
struct Snapshot {
    sensors: Sensors,
    cpu_turbo: u8,
    gpu_turbo: u8,
    backlight: Option<BacklightState>,
    cpu_hist: Vec<u16>,
    gpu_hist: Vec<u16>,
    cpu_rpm_hist: Vec<u16>,
    gpu_rpm_hist: Vec<u16>,
}

impl App {
    fn top_bar(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        egui::TopBottomPanel::top("top")
            .exact_height(56.0)
            .frame(egui::Frame::NONE.fill(theme::PANEL).inner_margin(egui::Margin::symmetric(18, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Wordmark, set in letter-spaced caps. Drawn as text, not
                    // an imported logo.
                    ui.label(
                        RichText::new("A L I E N")
                            .color(theme::HILITE)
                            .size(20.0)
                            .strong(),
                    );
                    ui.add_space(12.0);
                    ui.label(RichText::new(model_name()).color(theme::MUTED).size(12.0));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let turbo = snap.cpu_turbo == 2 || snap.gpu_turbo == 2;
                        ui.label(
                            RichText::new(if turbo { "TURBO" } else { "NORMAL" })
                                .color(if turbo { theme::WARN } else { theme::MUTED })
                                .size(12.0)
                                .strong(),
                        );
                    });
                });
            });
    }

    fn side_nav(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .exact_width(168.0)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(theme::PANEL).inner_margin(egui::Margin::same(10)))
            .show(ctx, |ui| {
                ui.add_space(6.0);
                for (tab, label) in Tab::ALL {
                    let selected = self.tab == tab;
                    let text = RichText::new(label)
                        .size(13.0)
                        .color(if selected { theme::HILITE } else { theme::TEXT });
                    let r = ui.add_sized(
                        [ui.available_width(), 34.0],
                        egui::Button::selectable(selected, text),
                    );
                    if r.clicked() {
                        self.tab = tab;
                    }
                    // Accent rail on the active item — the vendor's own cue for
                    // which tab you are on.
                    if selected {
                        let rect = r.rect;
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(rect.left_top(), egui::vec2(3.0, rect.height())),
                            0.0,
                            theme::HILITE,
                        );
                    }
                }
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(30.0)
            .frame(egui::Frame::NONE.fill(theme::PANEL).inner_margin(egui::Margin::symmetric(14, 7)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let failed = self.status.starts_with("failed");
                    ui.label(
                        RichText::new(&self.status)
                            .color(if failed { theme::HOT } else { theme::MUTED })
                            .size(11.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&self.interface).color(theme::MUTED).size(11.0));
                    });
                });
            });
    }

    fn dashboard(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.add_space(10.0);
        theme::heading(ui, "Thermal");
        ui.add_space(6.0);

        // Gauges size themselves to the window. The first version hardcoded
        // four 150px gauges in a horizontal row, which is fine at the default
        // 980px and clips the fourth one the moment a tiling WM halves the
        // window — which is how it was actually found.
        let gauges: [(&str, Option<u16>, &str, f32, egui::Color32); 4] = [
            ("CPU", snap.sensors.cpu_temp_c, "°C", 100.0,
             theme::temp_colour(snap.sensors.cpu_temp_c.unwrap_or(0))),
            ("GPU", snap.sensors.gpu_temp_c, "°C", 100.0,
             theme::temp_colour(snap.sensors.gpu_temp_c.unwrap_or(0))),
            ("CPU FAN", snap.sensors.cpu_fan_rpm, "RPM", 6000.0, theme::ACCENT),
            ("GPU FAN", snap.sensors.gpu_fan_rpm, "RPM", 6500.0, theme::ACCENT),
        ];
        let avail = ui.available_width();
        let spacing = ui.spacing().item_spacing.x;
        // Fit all four across if each can be at least 96px; otherwise two rows.
        let per_row = if (avail - 3.0 * spacing) / 4.0 >= 96.0 { 4 } else { 2 };
        let size = (((avail - (per_row as f32 - 1.0) * spacing) / per_row as f32) - 8.0)
            .clamp(84.0, 156.0);

        let rows = (gauges.len() + per_row - 1) / per_row;
        let plate = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(avail, rows as f32 * (size + 22.0) + 6.0),
        );
        theme::chamfered_panel(ui, plate, theme::PANEL, theme::LINE);

        for chunk in gauges.chunks(per_row) {
            ui.horizontal(|ui| {
                for (label, value, unit, max, colour) in chunk {
                    gauge::Gauge { label, value: *value, unit, max: *max, colour: *colour }
                        .show(ui, size);
                }
            });
        }

        ui.add_space(14.0);
        theme::heading(ui, "History");
        ui.add_space(6.0);
        let plots: [(&str, &Vec<u16>, egui::Color32); 3] = [
            ("cpu temperature", &snap.cpu_hist, theme::HOT),
            ("gpu temperature", &snap.gpu_hist, theme::WARN),
            ("cpu fan", &snap.cpu_rpm_hist, theme::ACCENT),
        ];
        // Same responsive rule: three across when there is room, stacked when
        // there is not, and each plot framed so a flat series still reads as a
        // plot rather than a stray rule floating between sections.
        let cols = if (avail - 2.0 * spacing) / 3.0 >= 130.0 { 3 } else { 1 };
        let pw = ((avail - (cols as f32 - 1.0) * spacing) / cols as f32) - 2.0;
        for chunk in plots.chunks(cols) {
            ui.horizontal(|ui| {
                for (label, data, colour) in chunk {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(*label).color(theme::MUTED).size(11.0));
                        let r = ui.cursor().min;
                        let rect = egui::Rect::from_min_size(r, egui::vec2(pw, 64.0));
                        ui.painter().rect_filled(rect, 2.0, theme::PANEL);
                        gauge::sparkplot(ui, data, egui::vec2(pw, 64.0), *colour);
                    });
                }
            });
        }

        ui.add_space(16.0);
        theme::heading(ui, "Profiles");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for p in Profile::builtins() {
                if ui.button(capitalise(&p.name)).on_hover_text(&p.description).clicked() {
                    let r = p.apply(&self.dev);
                    self.act(r, &format!("applied profile: {}", p.name));
                }
            }
        });
    }

    fn fans(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.add_space(10.0);
        theme::heading(ui, "Fan mode");
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if ui.button("Maximum").clicked() {
                self.manual_fans = false;
                let r = self.dev.fans_max();
                self.act(r, "fans at maximum");
            }
            if ui.button("Automatic").clicked() {
                self.manual_fans = false;
                let r = self.dev.fans_auto();
                self.act(r, "fans on the EC curve");
            }
            ui.checkbox(&mut self.manual_fans, "Manual");
        });

        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Maximum is worth roughly +48% sustained CPU throughput on this chassis: the \
                 stock EC curve holds the processor in thermal throttle.",
            )
            .color(theme::MUTED)
            .size(11.0),
        );

        if self.manual_fans {
            ui.add_space(14.0);
            theme::heading(ui, "Manual duty");
            ui.add_space(6.0);

            let mut apply_cpu = false;
            let mut apply_gpu = false;
            ui.horizontal(|ui| {
                ui.label("CPU");
                apply_cpu = ui
                    .add(egui::Slider::new(&mut self.cpu_pct, 0..=100).suffix(" %"))
                    .drag_stopped();
            });
            ui.horizontal(|ui| {
                ui.label("GPU");
                apply_gpu = ui
                    .add(egui::Slider::new(&mut self.gpu_pct, 0..=100).suffix(" %"))
                    .drag_stopped();
            });
            // Send on release, not on every pixel of drag: each change is two
            // firmware calls, and streaming them while the mouse moves would
            // hammer the EC for values the user is only passing through.
            if apply_cpu {
                let r = self.dev.set_fan_percent(Fan::Cpu, self.cpu_pct);
                self.act(r, &format!("cpu fan -> {}%", self.cpu_pct));
            }
            if apply_gpu {
                let r = self.dev.set_fan_percent(Fan::Gpu, self.gpu_pct);
                self.act(r, &format!("gpu fan -> {}%", self.gpu_pct));
            }

            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Duty is not linear in RPM, and the fans take eight to ten seconds to \
                     settle — the readings below will lag the slider.",
                )
                .color(theme::MUTED)
                .size(11.0),
            );
        }

        ui.add_space(16.0);
        theme::heading(ui, "Now");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            gauge::Gauge {
                label: "CPU FAN",
                value: snap.sensors.cpu_fan_rpm,
                unit: "RPM",
                max: 6000.0,
                colour: theme::ACCENT,
            }
            .show(ui, 140.0);
            gauge::Gauge {
                label: "GPU FAN",
                value: snap.sensors.gpu_fan_rpm,
                unit: "RPM",
                max: 6500.0,
                colour: theme::ACCENT,
            }
            .show(ui, 140.0);
            if ui.available_width() > 160.0 {
                ui.vertical(|ui| {
                    ui.add_space(24.0);
                    let w = (ui.available_width() - 6.0).max(80.0);
                    gauge::sparkplot(ui, &snap.cpu_rpm_hist, egui::vec2(w, 58.0), theme::ACCENT);
                    gauge::sparkplot(ui, &snap.gpu_rpm_hist, egui::vec2(w, 58.0), theme::HILITE);
                });
            }
        });
    }

    fn lighting(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        theme::heading(ui, "Effect");
        ui.add_space(6.0);

        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            for e in Effect::ALL {
                if ui.selectable_label(self.effect == e, capitalise(e.name())).clicked() {
                    self.effect = e;
                    changed = true;
                }
            }
        });

        let idx = self.effect as usize;
        let mut zones_changed = false;

        // ── Colour ──────────────────────────────────────────────────────────
        // Shown only where it does something. Static gets four pickers,
        // because the keyboard genuinely has four independently addressable
        // zones — anything else would be the UI lying about the hardware.
        if self.effect == Effect::Static {
            ui.add_space(14.0);
            theme::heading(ui, "Zone colours");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (i, z) in self.zone_colours.iter_mut().enumerate() {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("{}", i + 1)).color(theme::MUTED).size(10.0));
                        if ui.color_edit_button_rgb(z).changed() {
                            zones_changed = true;
                        }
                    });
                }
                ui.add_space(8.0);
                if ui.button("All alike").clicked() {
                    let first = self.zone_colours[0];
                    self.zone_colours = [first; 4];
                    zones_changed = true;
                }
            });
        } else if self.effect.honours_colour() {
            ui.add_space(14.0);
            theme::heading(ui, "Colour");
            ui.add_space(6.0);
            if ui.color_edit_button_rgb(&mut self.per_effect[idx].colour).changed() {
                changed = true;
            }
        }

        ui.add_space(14.0);
        theme::heading(ui, "Brightness");
        ui.add_space(6.0);
        if ui
            .add(egui::Slider::new(&mut self.brightness, 0..=100).suffix(" %"))
            .drag_stopped()
        {
            changed = true;
        }

        // ── Speed ───────────────────────────────────────────────────────────
        // Static has nothing to animate, so it has no speed. Everything else
        // starts at 1: zero means "do not animate", which reads as a broken
        // effect rather than a slow one.
        if self.effect != Effect::Static {
            ui.add_space(10.0);
            theme::heading(ui, "Speed");
            ui.add_space(6.0);
            if ui
                .add(egui::Slider::new(&mut self.per_effect[idx].speed, 1..=9))
                .drag_stopped()
            {
                changed = true;
            }
        }

        if zones_changed || (changed && self.effect == Effect::Static) {
            // Static colour lives in the per-zone registers, not in the effect
            // payload — so the mode/brightness call and the four zone writes
            // have to go together, in that order.
            let to_c = |v: [f32; 3]| {
                Colour::new((v[0] * 255.0) as u8, (v[1] * 255.0) as u8, (v[2] * 255.0) as u8)
            };
            let cs = [
                to_c(self.zone_colours[0]),
                to_c(self.zone_colours[1]),
                to_c(self.zone_colours[2]),
                to_c(self.zone_colours[3]),
            ];
            let r = self.dev.set_zone_colours(cs, self.brightness);
            self.act(r, "zone colours applied");
            self.mem.set_zone_colours(cs);
            self.mem.set_colour(Effect::Static, cs[0]);
            self.mem.brightness = self.brightness;
            let _ = self.mem.save();
        } else if changed {
            let set = self.per_effect[idx];
            let c = Colour::new(
                (set.colour[0] * 255.0) as u8,
                (set.colour[1] * 255.0) as u8,
                (set.colour[2] * 255.0) as u8,
            );
            // Clamp the speed here as well as in the UI: a slot seeded from
            // firmware could still hold 0 for an animated effect.
            let speed = if self.effect == Effect::Static { 0 } else { set.speed.max(1) };
            let r = self.dev.set_effect(self.effect, speed, self.brightness, Direction::LeftToRight, c);
            self.act(r, &format!("lighting -> {}", self.effect.name()));
            self.mem.set_colour(self.effect, c);
            self.mem.set_speed(self.effect, speed);
            self.mem.brightness = self.brightness;
            let _ = self.mem.save();
        }

        ui.add_space(16.0);
        theme::heading(ui, "Per-key");
        ui.add_space(6.0);
        match self.caps.per_key {
            alien_core::Support::Yes => {
                ui.label(
                    RichText::new(
                        "This machine has an ITE per-key controller. Use `alien rgb key <name> \
                         <colour>` to colour individual keys.",
                    )
                    .color(theme::TEXT)
                    .size(11.0),
                );
            }
            _ => {
                ui.label(
                    RichText::new(
                        "Not available: this keyboard is four-zone. Its keys are wired into four \
                         banks that share LEDs, so an individual key cannot be addressed by any \
                         software. Per-key needs an ITE 8291 controller, found on the Triton 500 \
                         SE and Helios 16/18.",
                    )
                    .color(theme::MUTED)
                    .size(11.0),
                );
            }
        }

        ui.add_space(16.0);
        if ui.button("Backlight off").clicked() {
            let r = self.dev.backlight_off();
            self.act(r, "backlight off");
        }
    }

    fn performance(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.add_space(10.0);
        theme::heading(ui, "Turbo flags");
        ui.add_space(6.0);

        let on = snap.cpu_turbo == 2 || snap.gpu_turbo == 2;
        ui.horizontal(|ui| {
            if ui.button(if on { "Turn off" } else { "Turn on" }).clicked() {
                let want = !on;
                let r = self
                    .dev
                    .set_overclock(OverclockTarget::Cpu, want)
                    .and_then(|_| self.dev.set_overclock(OverclockTarget::Gpu, want));
                self.act(r, if want { "turbo flags on" } else { "turbo flags off" });
            }
            ui.label(
                RichText::new(format!(
                    "cpu {}   gpu {}",
                    flag_name(snap.cpu_turbo),
                    flag_name(snap.gpu_turbo)
                ))
                .color(if on { theme::WARN } else { theme::MUTED }),
            );
        });

        ui.add_space(10.0);
        // Saying this plainly is the point. Presenting an inert switch as a
        // performance feature is what the vendor's own UI does on this model.
        ui.label(
            RichText::new(
                "On the Helios 300 PH315-53 the CPU flag is inert. PredatorSense gates CPU \
                 overclock on Feature.ini OverclockSupport CPU, which is 0 for this model, so the \
                 firmware write does nothing; its \"CPU turbo\" is actually Intel XTU power \
                 limits. The GPU flag does go through this interface. Measured with fans pinned \
                 at maximum, these flags produced no benchmark change here — the fan curve is \
                 what moves the numbers on this chassis.",
            )
            .color(theme::MUTED)
            .size(11.0),
        );

        ui.add_space(16.0);
        theme::heading(ui, "Temperatures");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            gauge::Gauge {
                label: "CPU",
                value: snap.sensors.cpu_temp_c,
                unit: "°C",
                max: 100.0,
                colour: theme::temp_colour(snap.sensors.cpu_temp_c.unwrap_or(0)),
            }
            .show(ui, 140.0);
            gauge::Gauge {
                label: "GPU",
                value: snap.sensors.gpu_temp_c,
                unit: "°C",
                max: 100.0,
                colour: theme::temp_colour(snap.sensors.gpu_temp_c.unwrap_or(0)),
            }
            .show(ui, 140.0);
            gauge::Gauge {
                label: "BOARD",
                value: snap.sensors.system_temp_c,
                unit: "°C",
                max: 100.0,
                colour: theme::temp_colour(snap.sensors.system_temp_c.unwrap_or(0)),
            }
            .show(ui, 140.0);
        });
    }

    fn about(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(RichText::new("Alien").color(theme::HILITE).size(22.0).strong());
        ui.label(
            RichText::new(format!("version {}", env!("CARGO_PKG_VERSION")))
                .color(theme::MUTED)
                .size(12.0),
        );
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "Fan, lighting, turbo and telemetry control for Acer Predator and Nitro \
                 laptops on Linux, with no vendor software.\n\n\
                 A clean-room implementation of the gaming WMI protocol, verified against real \
                 firmware. Where a control is accepted by the firmware but has no observable \
                 effect on a model, this software says so rather than implying it works.",
            )
            .color(theme::TEXT)
            .size(12.0),
        );
        ui.add_space(12.0);
        theme::heading(ui, "This machine");
        ui.add_space(4.0);
        for (k, v) in [
            ("Model", model_name()),
            ("Firmware", dmi("bios_version")),
            ("Interface", self.interface.clone()),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{k:<12}")).color(theme::MUTED).size(12.0).monospace());
                ui.label(RichText::new(v).color(theme::TEXT).size(12.0).monospace());
            });
        }
        ui.add_space(14.0);
        theme::heading(ui, "What this machine supports");
        ui.add_space(4.0);
        for (name, sup) in self.caps.rows() {
            let (text, colour) = match sup {
                alien_core::Support::Yes => ("yes", theme::OK),
                alien_core::Support::No => ("no", theme::MUTED),
                alien_core::Support::Unverified => ("accepted, unverified", theme::WARN),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{name:<26}")).color(theme::MUTED).size(11.0).monospace());
                ui.label(RichText::new(text).color(colour).size(11.0).monospace());
            });
        }
        for n in &self.caps.notes {
            ui.add_space(4.0);
            ui.label(RichText::new(n).color(theme::MUTED).size(10.0));
        }

        ui.add_space(12.0);
        ui.label(
            RichText::new("GPL-2.0-or-later · alien.hartle.tech")
                .color(theme::MUTED)
                .size(11.0),
        );
    }
}

fn flag_name(v: u8) -> &'static str {
    match v {
        0 => "off",
        2 => "turbo",
        _ => "unknown",
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
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

fn main() -> eframe::Result<()> {
    let dev = match Device::open() {
        Ok(d) => d,
        Err(e) => {
            // A GUI that dies with a console message nobody sees is useless,
            // but a window that renders an empty dashboard is worse — it looks
            // like the hardware is idle. Say what is wrong and stop.
            eprintln!("alien-gui: {e}");
            eprintln!(
                "\nThe GUI talks to alien-daemon over its socket and never to the firmware \
                 directly.\nStart it with:  sudo systemctl start alien-daemon\nand make sure you \
                 are in the `alien` group."
            );
            std::process::exit(1);
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("Alien")
            // The Wayland app_id / X11 WM_CLASS. Without it the window has an
            // EMPTY class, which quietly breaks everything that identifies a
            // window: compositor rules, taskbar grouping, .desktop matching,
            // and any "focus it instead of opening a second one" logic. It
            // matches the desktop file's basename, which is what associates
            // the two.
            .with_app_id("tech.hartle.Alien"),
        ..Default::default()
    };

    eframe::run_native(
        "Alien",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc, dev)))),
    )
}
