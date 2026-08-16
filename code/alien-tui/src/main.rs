//! `alien-tui` — the terminal control centre.
//!
//! Live telemetry with dense Braille history, fan and profile control, and RGB,
//! all over the same [`alien_core::Device`] the GUI uses.
//!
//! # Why this is hand-rolled rather than ratatui
//!
//! It is about 400 lines of ANSI against roughly the same in glue plus a
//! dependency tree of a dozen crates, in a project that gets vendored into six
//! packaging formats where every crate is a licence review. The layout here is
//! a fixed set of panels, not a resizable widget tree, so almost nothing that
//! ratatui provides would get used.
//!
//! # Threading
//!
//! Telemetry polling runs on its own thread and publishes through a mutex, so
//! a firmware call that takes 40 ms never stalls keyboard input. The render
//! loop only ever reads the latest snapshot.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alien_core::profile::Profile;
use alien_core::wmi::OverclockTarget;
use alien_core::{
    covini_brightness, Colour, Device, Direction, Effect, Fan, GpuMode, GpuModeOptIn, GpuModeState,
    KeyboardTimeoutState, Sensors, Support, GPU_MODE_ACKNOWLEDGEMENT,
};

mod telemetry_graph;
mod term;
use telemetry_graph::{GlyphMode, GraphSpec};
use term::*;

/// Two minutes at the one-second polling cadence. Braille cells carry two
/// samples each, so all 120 seconds fit in a 60-column graph.
const HISTORY: usize = 120;

fn graph_glyph_mode() -> GlyphMode {
    parse_graph_glyph_mode(std::env::var("ALIEN_TUI_GRAPH_SYMBOLS").ok().as_deref())
}

fn rich_graph_rows(height: usize) -> usize {
    // The 96x32 support boundary fits two plot rows exactly. Above it, every
    // extra row in the three panel bands costs three terminal lines (thermal
    // pair, full-width board, fan pair). Grow into the available canvas so a
    // high-resolution TUI looks like a monitor rather than a few sparklines,
    // while capping the plots before they crowd the control/state surface.
    (2 + height.saturating_sub(32) / 3).min(8)
}

fn parse_graph_glyph_mode(value: Option<&str>) -> GlyphMode {
    match value {
        Some(value) if value.eq_ignore_ascii_case("block") => GlyphMode::Block,
        _ => GlyphMode::Braille,
    }
}

#[derive(Default)]
struct History {
    cpu_temp: Vec<Option<u16>>,
    gpu_temp: Vec<Option<u16>>,
    system_temp: Vec<Option<u16>>,
    cpu_rpm: Vec<Option<u16>>,
    gpu_rpm: Vec<Option<u16>>,
}

impl History {
    fn push(&mut self, s: &Sensors) {
        let add = |v: &mut Vec<Option<u16>>, x: Option<u16>| {
            v.push(x);
            if v.len() > HISTORY {
                v.remove(0);
            }
        };
        add(&mut self.cpu_temp, s.cpu_temp_c);
        add(&mut self.gpu_temp, s.gpu_temp_c);
        add(&mut self.system_temp, s.system_temp_c);
        add(&mut self.cpu_rpm, s.cpu_fan_rpm);
        add(&mut self.gpu_rpm, s.gpu_fan_rpm);
    }
}

struct State {
    sensors: Sensors,
    history: History,
    cpu_turbo: Option<u8>,
    gpu_turbo: Option<u8>,
    gpu_mode: Option<GpuModeState>,
    gpu_mode_error: Option<String>,
    coolboost: Option<bool>,
    keyboard_timeout: Option<KeyboardTimeoutState>,
    lcd_overdrive: Option<bool>,
    coolboost_support: Support,
    keyboard_timeout_support: Support,
    lcd_overdrive_support: Support,
    backlight: Option<alien_core::BacklightState>,
    static_colour: Colour,
    /// Last thing we did, shown in the status line. The whole project's habit:
    /// say what the firmware said, never assume it worked.
    message: String,
    interface: String,
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

#[derive(Clone, Copy)]
enum GpuModeOperation {
    Refresh,
    Apply(GpuMode),
}

struct GpuModeJob {
    operation: GpuModeOperation,
    result: std::sync::mpsc::Receiver<Result<GpuModeState, String>>,
}

fn gpu_mode_confirmation_message(mode: GpuMode) -> String {
    let offsets = mode.offsets();
    format!(
        "UNSUPPORTED clock control: heat/crash/data-loss risk · {}: graphics {:+} MHz, memory {:+} MHz; fan table {}; GPOC {} · uppercase Y required · Esc cancels",
        mode.label(),
        offsets.graphics_mhz,
        offsets.memory_mhz,
        mode.fan_table(),
        mode as u8,
    )
}

fn start_gpu_mode_job(
    dev: &Arc<Device>,
    operation: GpuModeOperation,
) -> Result<GpuModeJob, String> {
    let dev = Arc::clone(dev);
    let (send, result) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("alien-tui-gpu-mode".into())
        .spawn(move || {
            let outcome = match operation {
                GpuModeOperation::Refresh => dev.gpu_mode(),
                GpuModeOperation::Apply(mode) => {
                    let opt_in = GpuModeOptIn::acknowledge(GPU_MODE_ACKNOWLEDGEMENT)
                        .expect("TUI confirmation maps to the exact acknowledgement");
                    dev.set_gpu_mode(mode, opt_in)
                }
            }
            .map_err(|error| error.to_string());
            let _ = send.send(outcome);
        })
        .map_err(|error| format!("failed to start GPU-mode worker: {error}"))?;
    Ok(GpuModeJob { operation, result })
}

fn main() -> std::process::ExitCode {
    let dev = match Device::open() {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!("alien-tui: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let caps = dev.capabilities();
    let state = Arc::new(Mutex::new(State {
        sensors: Sensors::default(),
        history: History::default(),
        cpu_turbo: None,
        gpu_turbo: None,
        gpu_mode: None,
        gpu_mode_error: None,
        coolboost: None,
        keyboard_timeout: None,
        lcd_overdrive: None,
        coolboost_support: caps.coolboost,
        keyboard_timeout_support: caps.keyboard_timeout,
        lcd_overdrive_support: caps.lcd_overdrive,
        backlight: None,
        static_colour: alien_core::Lighting::load().zone_colours()[0],
        message: "ready".into(),
        interface: dev.method_path(),
    }));

    let running = Arc::new(AtomicBool::new(true));

    // Poller. Separate thread so a slow firmware call cannot make the UI feel
    // stuck — the render loop always has a recent snapshot to draw.
    {
        let dev = Arc::clone(&dev);
        let state = Arc::clone(&state);
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                let s = dev.sensors();
                let cpu = dev.overclock(OverclockTarget::Cpu).ok();
                // Acer's GPU GPOC getter sends a discrete-GPU notification.
                // Never issue it from this one-second telemetry loop; `r`
                // performs a clearly labelled manual compound-mode refresh.
                let (coolboost_support, keyboard_timeout_support, lcd_overdrive_support) = state
                    .lock()
                    .ok()
                    .map(|state| {
                        (
                            state.coolboost_support,
                            state.keyboard_timeout_support,
                            state.lcd_overdrive_support,
                        )
                    })
                    .unwrap_or((Support::Unknown, Support::Unknown, Support::Unknown));
                let (coolboost, coolboost_support) = if coolboost_support == Support::No {
                    (None, Support::No)
                } else {
                    match dev.coolboost() {
                        Ok(value) => (Some(value), Support::Unverified),
                        Err(error) => (None, advanced_error_support(&error)),
                    }
                };
                let (keyboard_timeout, keyboard_timeout_support) =
                    if keyboard_timeout_support == Support::No {
                        (None, Support::No)
                    } else {
                        match dev.keyboard_timeout() {
                            Ok(value) => (Some(value), Support::Unverified),
                            Err(error) => (None, advanced_error_support(&error)),
                        }
                    };
                let (lcd_overdrive, lcd_overdrive_support) = if lcd_overdrive_support == Support::No
                {
                    (None, Support::No)
                } else {
                    match dev.lcd_overdrive() {
                        Ok(Some(value)) => (Some(value), Support::Unverified),
                        Ok(None) => (None, Support::No),
                        Err(error) => (None, advanced_error_support(&error)),
                    }
                };
                let bl = dev.backlight().ok();
                let static_colour = alien_core::Lighting::load().zone_colours()[0];
                if let Ok(mut st) = state.lock() {
                    st.history.push(&s);
                    st.sensors = s;
                    st.cpu_turbo = cpu;
                    st.coolboost = coolboost;
                    st.keyboard_timeout = keyboard_timeout;
                    st.lcd_overdrive = lcd_overdrive;
                    st.coolboost_support = coolboost_support;
                    st.keyboard_timeout_support = keyboard_timeout_support;
                    st.lcd_overdrive_support = lcd_overdrive_support;
                    st.backlight = bl;
                    st.static_colour = static_colour;
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
        });
    }

    let mut term = match Terminal::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("alien-tui: cannot set up the terminal: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 8];
    let mut gpu_mode_job: Option<GpuModeJob> = None;
    let mut pending_gpu_mode: Option<GpuMode> = None;

    while running.load(Ordering::Relaxed) {
        let job_outcome = gpu_mode_job
            .as_ref()
            .and_then(|job| match job.result.try_recv() {
                Ok(result) => Some((job.operation, result)),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((
                    job.operation,
                    Err("GPU-mode worker exited without a result".into()),
                )),
            });
        if let Some((operation, outcome)) = job_outcome {
            gpu_mode_job = None;
            if let Ok(mut st) = state.lock() {
                match outcome {
                    Ok(confirmed) => {
                        st.gpu_mode = Some(confirmed);
                        st.gpu_mode_error = None;
                        st.gpu_turbo = Some(confirmed.gpoc);
                        st.message = match operation {
                            GpuModeOperation::Refresh => {
                                "OEM GPU snapshot refreshed; getter sent one Acer GPU notification"
                                    .into()
                            }
                            GpuModeOperation::Apply(mode) => format!(
                                "OEM GPU {} getter-confirmed across offsets, fan table and GPOC",
                                mode.label()
                            ),
                        };
                    }
                    Err(error) => {
                        st.gpu_mode = None;
                        st.gpu_mode_error = Some(error.clone());
                        st.gpu_turbo = None;
                        st.message = format!("FAILED: {error}");
                    }
                }
            }
            last_draw = Instant::now() - Duration::from_secs(1);
        }

        if last_draw.elapsed() >= Duration::from_millis(250) {
            if let Ok(st) = state.lock() {
                draw(&mut term, &st);
            }
            last_draw = Instant::now();
        }

        // Non-blocking-ish read: the terminal is in raw mode with VMIN=0 and
        // VTIME=1, so this returns after 100 ms with nothing rather than
        // blocking the redraw.
        let n = stdin.read(&mut buf).unwrap_or(0);
        if n == 0 {
            continue;
        }
        let key = buf[0];
        let mut msg: Option<String> = None;
        if pending_gpu_mode.is_some() && !matches!(key, b'Y' | 27 | b'n' | b'f' | b'u') {
            pending_gpu_mode = None;
        }

        match key {
            b'q' | 3 /* ctrl-c */ => running.store(false, Ordering::Relaxed),
            b'm' => msg = Some(act(&dev.fans_max(), "fans -> maximum")),
            b'a' => msg = Some(act(&dev.fans_auto(), "fans -> EC automatic curve")),
            b'1' => msg = Some(apply(&dev, "silent")),
            b'2' => msg = Some(apply(&dev, "performance")),
            b'3' => msg = Some(apply(&dev, "turbo")),
            b'r' => {
                if gpu_mode_job.is_some() {
                    msg = Some("GPU-mode worker already in progress".into());
                } else {
                    match start_gpu_mode_job(&dev, GpuModeOperation::Refresh) {
                        Ok(job) => {
                            gpu_mode_job = Some(job);
                            msg = Some(
                                "reading OEM GPU mode; Acer getter sends one GPU notification…"
                                    .into(),
                            );
                        }
                        Err(error) => msg = Some(format!("FAILED: {error}")),
                    }
                }
            }
            b'n' | b'f' | b'u' => {
                let mode = match key {
                    b'n' => GpuMode::Normal,
                    b'f' => GpuMode::Faster,
                    b'u' => GpuMode::Turbo,
                    _ => unreachable!("GPU-mode key match"),
                };
                // A new selection always replaces the previous pending one,
                // including when this target is range-disabled.
                pending_gpu_mode = None;
                if gpu_mode_job.is_some() {
                    msg = Some("GPU-mode worker already in progress".into());
                } else {
                    match state.lock().ok().and_then(|st| st.gpu_mode) {
                        None => {
                            msg = Some(
                                "press r first for a manual OEM GPU snapshot (getter sends one notification)"
                                    .into(),
                            );
                        }
                        Some(snapshot) if !snapshot.target_fits(mode) => {
                            let target = mode.offsets();
                            msg = Some(format!(
                                "{} unavailable: live NVML ranges reject {:+}/{:+} MHz",
                                mode.label(), target.graphics_mhz, target.memory_mhz
                            ));
                        }
                        Some(_) => {
                            pending_gpu_mode = Some(mode);
                            msg = Some(gpu_mode_confirmation_message(mode));
                        }
                    }
                }
            }
            b'Y' => {
                if let Some(mode) = pending_gpu_mode.take() {
                    match start_gpu_mode_job(&dev, GpuModeOperation::Apply(mode)) {
                        Ok(job) => {
                            gpu_mode_job = Some(job);
                            msg = Some(format!(
                                "applying OEM GPU {} with readback/rollback…",
                                mode.label()
                            ));
                        }
                        Err(error) => msg = Some(format!("FAILED: {error}")),
                    }
                }
            }
            27 if pending_gpu_mode.take().is_some() => {
                msg = Some("OEM GPU mode cancelled; no write sent".into());
            }
            b't' => {
                msg = Some(
                    "raw GPOC write disabled here; use n/f/u for a coherent OEM GPU mode".into(),
                );
            }
            b'b' => {
                let (current, support) = state
                    .lock()
                    .ok()
                    .map(|s| (s.coolboost, s.coolboost_support))
                    .unwrap_or((None, Support::Unknown));
                if let Some(current) = current {
                    match dev.set_coolboost(!current) {
                        Ok(confirmed) => {
                            if let Ok(mut st) = state.lock() {
                                st.coolboost = Some(confirmed);
                            }
                            msg = Some(format!(
                                "CoolBoost {} · PH315-53 reinit transient confirmed; no sustained A/B/A cooling lift",
                                if confirmed { "on" } else { "off" }
                            ));
                        }
                        Err(error) => msg = Some(format!("FAILED: {error}")),
                    }
                } else {
                    msg = Some(format!(
                        "CoolBoost {}; no write sent",
                        if support == Support::No {
                            "unsupported"
                        } else {
                            "unavailable"
                        }
                    ));
                }
            }
            b'k' => {
                let (current, support) = state
                    .lock()
                    .ok()
                    .map(|s| (s.keyboard_timeout, s.keyboard_timeout_support))
                    .unwrap_or((None, Support::Unknown));
                if let Some(current) = current {
                    let seconds = if current.seconds == 30 { 0 } else { 30 };
                    match dev.set_keyboard_timeout(seconds) {
                        Ok(confirmed) => {
                            if let Ok(mut st) = state.lock() {
                                st.keyboard_timeout = Some(confirmed);
                            }
                            msg = Some(format!(
                                "keyboard timeout {} s · getter confirmed; optical effect unverified",
                                confirmed.seconds
                            ));
                        }
                        Err(error) => msg = Some(format!("FAILED: {error}")),
                    }
                } else {
                    msg = Some(format!(
                        "keyboard timeout {}; no write sent",
                        if support == Support::No {
                            "unsupported"
                        } else {
                            "unavailable"
                        }
                    ));
                }
            }
            b'o' => {
                let (current, support) = state
                    .lock()
                    .ok()
                    .map(|s| (s.lcd_overdrive, s.lcd_overdrive_support))
                    .unwrap_or((None, Support::Unknown));
                if let Some(current) = current {
                    match dev.set_lcd_overdrive(!current) {
                        Ok(Some(confirmed)) => {
                            if let Ok(mut st) = state.lock() {
                                st.lcd_overdrive = Some(confirmed);
                            }
                            msg = Some(format!(
                                "LCD overdrive {} · getter confirmed; panel effect unverified",
                                if confirmed { "on" } else { "off" }
                            ));
                        }
                        Ok(None) => msg = Some("LCD overdrive unsupported; no write sent".into()),
                        Err(error) => msg = Some(format!("FAILED: {error}")),
                    }
                } else {
                    msg = Some(format!(
                        "LCD overdrive {}; no write sent",
                        if support == Support::No {
                            "unsupported"
                        } else {
                            "unavailable"
                        }
                    ));
                }
            }
            b'+' | b'=' | b'-' | b'_' => {
                if let Some(backlight) = state.lock().ok().and_then(|s| s.backlight) {
                    let cur = covini_brightness(backlight.brightness);
                    let step: i16 = if key == b'+' || key == b'=' { 25 } else { -25 };
                    let next = (cur as i16 + step).clamp(0, 100) as u8;
                    let mut mem = alien_core::Lighting::load();
                    let direction = if backlight.reverse {
                        Direction::RightToLeft
                    } else {
                        Direction::LeftToRight
                    };
                    let r = if backlight.effect == Effect::Static {
                        dev.set_static_brightness_and_colours(
                            mem.zone_colours(),
                            mem.zone_enabled,
                            next,
                        )
                    } else {
                        dev.prepare_lighting(mem.zone_enabled).and_then(|()| {
                            dev.set_effect(
                                backlight.effect,
                                backlight.speed,
                                next,
                                direction,
                                backlight.colour,
                            )
                        })
                    };
                    let r = r.and_then(|()| {
                        mem.set_brightness(next);
                        mem.save().map_err(|error| {
                            alien_core::Error::State(format!(
                                "hardware changed, but lighting settings were not saved: {error}"
                            ))
                        })
                    });
                    let success = if next == 0 {
                        "backlight brightness 0 requested · optical effect unverified".into()
                    } else {
                        format!("backlight brightness {next} requested")
                    };
                    msg = Some(act(&r, &success));
                } else {
                    msg = Some("backlight unavailable; no write sent".into());
                }
            }
            b'c' => {
                // Step through the effects. Speed/direction are per-pattern;
                // colour is Covini's one shared Pattern colour.
                if let Some(backlight) = state.lock().ok().and_then(|s| s.backlight) {
                    let idx = Effect::ALL
                        .iter()
                        .position(|effect| *effect == backlight.effect)
                        .expect("live backlight effect is a Covini enum");
                    let next = Effect::ALL[(idx + 1) % Effect::ALL.len()];
                    let mut mem = alien_core::Lighting::load();
                    let colour = mem.colour(next);
                    let speed = mem.speed(next);
                    let brightness = mem.brightness;
                    let direction = mem.direction(next);
                    let r = if next == Effect::Static {
                        // Static colour lives in the per-zone registers.
                        dev.set_zone_colours_enabled(
                            mem.zone_colours(),
                            mem.zone_enabled,
                            brightness,
                        )
                    } else {
                        dev.prepare_lighting(mem.zone_enabled).and_then(|()| {
                            dev.set_effect(next, speed, brightness, direction, colour)
                        })
                    };
                    let r = r.and_then(|()| {
                        mem.set_colour(next, colour);
                        mem.save().map_err(|error| {
                            alien_core::Error::State(format!(
                                "hardware changed, but lighting settings were not saved: {error}"
                            ))
                        })
                    });
                    let success = if next == Effect::Static {
                        "effect -> static".into()
                    } else {
                        format!(
                            "{} request accepted · optical effect unverified",
                            next.name()
                        )
                    };
                    msg = Some(act(&r, &success));
                } else {
                    msg = Some("backlight unavailable; no write sent".into());
                }
            }
            b'C' => msg = Some(act(&dev.set_fan_percent(Fan::Cpu, 60), "cpu fan -> 60%")),
            b'G' => msg = Some(act(&dev.set_fan_percent(Fan::Gpu, 60), "gpu fan -> 60%")),
            _ => {}
        }

        if let Some(m) = msg {
            if let Ok(mut st) = state.lock() {
                st.message = m;
            }
            // Redraw immediately so the action feels instant even though the
            // poller is on a one-second cadence.
            last_draw = Instant::now() - Duration::from_secs(1);
        }
    }

    drop(term);
    std::process::ExitCode::SUCCESS
}

fn act<T>(r: &alien_core::Result<T>, ok: &str) -> String {
    match r {
        Ok(_) => ok.to_string(),
        Err(e) => format!("FAILED: {e}"),
    }
}

fn apply(dev: &Device, name: &str) -> String {
    match Profile::builtins().into_iter().find(|p| p.name == name) {
        Some(p) => {
            let ignored = p.deprecated_gpu_flag_ignored();
            let success = if ignored {
                format!("profile -> {name}; deprecated raw GPU flag ignored")
            } else {
                format!("profile -> {name}; GPU mode unchanged")
            };
            act(&p.apply(dev), &success)
        }
        None => format!("no profile {name}"),
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// Alien's phosphor-green instrument palette. The RGB lighting swatch remains
/// the actual configured colour and is deliberately never recoloured here.
const ACCENT: Rgb = Rgb(0x3f, 0xe8, 0x6c);
const BRIGHT: Rgb = Rgb(0xa6, 0xff, 0xc0);
const DIM: Rgb = Rgb(0x64, 0x7a, 0x6b);
const WARN: Rgb = Rgb(0xff, 0xb0, 0x00);
const HOT: Rgb = Rgb(0xff, 0x4d, 0x3a);

fn temp_colour(c: u16) -> Rgb {
    match c {
        0..=69 => Rgb(0x5a, 0xd8, 0x9a),
        70..=84 => WARN,
        _ => HOT,
    }
}

/// Columns consumed by the compact fallback telemetry row.
const GUTTER: usize = 2;
const LABEL_W: usize = 9;
const VALUE_W: usize = 10;

fn draw(t: &mut Terminal, st: &State) {
    // Re-read every frame rather than caching from startup: the window
    // manager tiles this window after it opens, so the size at startup is
    // usually not the size it ends up.
    t.refresh_size();
    let o = render(st, t.width, t.height);

    let _ = t.out.write_all(o.as_bytes());
    let _ = t.out.flush();
}

fn render(st: &State, width: u16, height: u16) -> String {
    let mut o = String::with_capacity(4096);
    o.push_str(CLEAR_HOME);

    let w = width.max(38) as usize;
    let h = usize::from(height);
    // A 760x520 kitty window is about 25 rows tall. The full layout used to
    // emit more lines than that, which made the terminal scroll the title and
    // CPU row off the top after a resize. At roughly 520x380, only about 17
    // rows and 46 columns remain; that needs summaries as well as removing
    // ornamental blank lines.
    let compact = height < 28;
    // The regular compact composition fits the actual 76x22 cell budget of
    // our 760x520 QA window. Reserve the summary composition for genuinely
    // short or narrow terminals; otherwise the wide window wastes most of its
    // useful space.
    let tight = height < 21 || w < 56;

    if tight {
        render_tight(&mut o, st, w, h);
        return finish_frame(o);
    }

    // The regular 760x520 composition has room for three status rows when
    // the secondary proof note yields its row. The GPU confirmation uses all
    // three so none of its safety contract is ellipsised.
    let status_lines = bounded_status_lines(&st.message, w.saturating_sub(4), 3);
    let show_proof = status_lines.len() <= 2;

    // Header: a title line and a rule. Deliberately not a box — a bordered
    // box needs every content line padded to an exact width, and the first
    // version got that wrong the moment the interface path was long enough to
    // wrap, leaving a torn frame on screen.
    let iface = ellipsise(&st.interface, w.saturating_sub(4));
    if compact {
        let iface = ellipsise(&iface, w.saturating_sub(14));
        o.push_str(&format!("  {}  {}\r\n", bold("ALIEN"), fg(DIM, &iface)));
    } else {
        let mut title = String::new();
        title.push_str(&bold("ALIEN"));
        title.push_str(&fg(DIM, "  ·  Acer Predator control"));
        o.push_str(&format!("  {title}\r\n"));
        o.push_str(&format!("  {}\r\n", fg(DIM, &iface)));
    }
    o.push_str(&fg(
        ACCENT,
        &format!("  {}\r\n", "─".repeat(w.saturating_sub(4))),
    ));

    // At the rich capture size, spend the available rows on actual history:
    // paired thermal and fan panels with current/min/max/scale metadata. The
    // default Braille view carries two samples per cell; setting
    // ALIEN_TUI_GRAPH_SYMBOLS=block selects the one-sample block renderer for
    // terminal fonts without Braille. The 76x22 compact layout retains the
    // one-row fallback so its safety/status contract still fits.
    if !compact && w >= 96 && h >= 32 {
        render_telemetry_panels(&mut o, st, w, rich_graph_rows(h));
    } else {
        let spark_w = w.saturating_sub(GUTTER + LABEL_W + VALUE_W + 2);
        o.push_str(&section("THERMAL", compact));
        row(
            &mut o,
            "CPU",
            st.sensors.cpu_temp_c,
            "°C",
            &st.history.cpu_temp,
            spark_w,
            temp_colour,
        );
        row(
            &mut o,
            "GPU",
            st.sensors.gpu_temp_c,
            "°C",
            &st.history.gpu_temp,
            spark_w,
            temp_colour,
        );
        row(
            &mut o,
            "Board",
            st.sensors.system_temp_c,
            "°C",
            &[],
            spark_w,
            temp_colour,
        );

        o.push_str(&section("FANS", compact));
        row(
            &mut o,
            "CPU fan",
            st.sensors.cpu_fan_rpm,
            "RPM",
            &st.history.cpu_rpm,
            spark_w,
            |_| ACCENT,
        );
        row(
            &mut o,
            "GPU fan",
            st.sensors.gpu_fan_rpm,
            "RPM",
            &st.history.gpu_rpm,
            spark_w,
            |_| ACCENT,
        );
    }

    // ── Firmware flags + backlight ──────────────────────────────────────────
    o.push_str(&section("STATE", compact));
    let flag = |v: Option<u8>| match v {
        Some(0) => fg(DIM, "clear"),
        Some(2) => fg(WARN, "set"),
        Some(n) => fg(HOT, &format!("? {n}")),
        None => fg(DIM, "unavailable"),
    };
    let gpu_mode = match st.gpu_mode {
        Some(state) => format!(
            "{} snapshot {:+}/{:+} MHz · fan {} · GPOC {}",
            state
                .confirmed_mode()
                .map(GpuMode::label)
                .unwrap_or("split"),
            state.graphics.current_mhz,
            state.memory.current_mhz,
            state.fan_table,
            state.gpoc,
        ),
        None => st
            .gpu_mode_error
            .as_deref()
            .map(|error| format!("unavailable: {error}"))
            .unwrap_or_else(|| "manual refresh required: r sends one GPU notification".into()),
    };
    o.push_str(&format!(
        "  {:<w$} {}\r\n",
        "OEM GPU",
        ellipsise(&gpu_mode, w.saturating_sub(GUTTER + LABEL_W)),
        w = LABEL_W,
    ));
    o.push_str(&format!(
        "  {:<w$} cpu {}  gpu {}  {}\r\n",
        "raw flags",
        flag(st.cpu_turbo),
        flag(st.gpu_turbo),
        fg(DIM, "(not OEM GPU modes)"),
        w = LABEL_W
    ));
    match st.backlight {
        Some(b) => {
            // Measure the plain text and colour afterwards. Ellipsising the
            // composed string would cut through an escape sequence and leave
            // the rest of the screen tinted — and the swatch is two columns
            // wide but eleven bytes long, so byte length is meaningless here.
            let detail = if b.effect == Effect::Static {
                format!(
                    "static (Z1 swatch)  brightness {}  speed {}",
                    b.brightness, b.speed
                )
            } else {
                format!(
                    "{}  brightness {}  speed {}",
                    b.effect.name(),
                    b.brightness,
                    b.speed
                )
            };
            let avail = w.saturating_sub(GUTTER + LABEL_W + 4);
            o.push_str(&format!(
                "  {:<lw$} {} {}\r\n",
                "backlight",
                fg(
                    if b.effect == Effect::Static {
                        Rgb(st.static_colour.r, st.static_colour.g, st.static_colour.b)
                    } else {
                        b.colour_rgb()
                    },
                    "██",
                ),
                ellipsise(&detail, avail),
                lw = LABEL_W
            ));
        }
        None => o.push_str(&format!(
            "  {:<w$} {}\r\n",
            "backlight",
            fg(DIM, "unavailable"),
            w = LABEL_W
        )),
    }

    // ── Getter-gated advanced controls ─────────────────────────────────────
    o.push_str(&section("ADVANCED · GETTER-GATED", compact));
    let on_off = |state: Option<bool>, support: Support| match state {
        Some(true) => "on",
        Some(false) => "off",
        None if support == Support::No => "unsupported",
        None => "unavailable",
    };
    let timeout = st
        .keyboard_timeout
        .map(|state| format!("{} s", state.seconds))
        .unwrap_or_else(|| {
            if st.keyboard_timeout_support == Support::No {
                "unsupported".into()
            } else {
                "unavailable".into()
            }
        });
    let advanced = format!(
        "CoolBoost {} · LCD OD {} · KB timeout {}",
        on_off(st.coolboost, st.coolboost_support),
        on_off(st.lcd_overdrive, st.lcd_overdrive_support),
        timeout
    );
    o.push_str(&format!(
        "  {}\r\n",
        fg(DIM, &ellipsise(&advanced, w.saturating_sub(4)))
    ));
    if show_proof {
        o.push_str(&format!(
            "  {}\r\n",
            fg(
                DIM,
                &ellipsise(
                    "CB PH315-53: reinit transient, no sustained A/B/A lift · OD/KB effects unverified",
                    w.saturating_sub(4),
                ),
            )
        ));
    }

    // ── Keys ────────────────────────────────────────────────────────────────
    //
    // Packed to the real width rather than hardcoded into two lines. The fixed
    // version wrapped mid-word ("+/- bri / ghtness") in a tiled window, which
    // is the sort of thing that looks broken rather than merely tight.
    o.push_str(&section("KEYS", compact));
    const HINTS: &[&str] = &[
        "m fans max",
        "a fans auto",
        "C cpu 60%",
        "G gpu 60%",
        "r GPU read",
        "n normal",
        "f faster",
        "u turbo",
        "b CoolBoost",
        "o LCD OD",
        "k kb timeout",
        "1 silent",
        "2 performance",
        "3 max+red",
        "c effect",
        "+/- bright",
        "q quit",
    ];
    for line in pack(HINTS, w.saturating_sub(4), "   ") {
        o.push_str(&format!("  {}\r\n", fg(DIM, &line)));
    }

    push_status_lines(&mut o, &status_lines);

    finish_frame(o)
}

/// Leave the cursor on the final rendered row instead of advancing past it.
///
/// A CRLF after row `height` makes the terminal scroll immediately, even
/// though the frame itself has exactly `height` rows. The next frame starts
/// with `CLEAR_HOME`, so there is no need for a trailing line ending here.
fn finish_frame(mut frame: String) -> String {
    if frame.ends_with("\r\n") {
        frame.truncate(frame.len() - 2);
    }
    frame
}

/// Render a complete control surface inside the row/column budget of the
/// smallest window used in visual QA (about 46x17 terminal cells).
///
/// This is deliberately a different composition, not the full layout with
/// arbitrary rows hidden. Telemetry is grouped into labelled summaries, all
/// getter-gated state retains an explicit value, and every write key remains
/// discoverable.
fn render_tight(o: &mut String, st: &State, w: usize, height: usize) {
    const TIGHT_HINTS: &[&str] = &[
        "m max",
        "a auto",
        "C CPU60",
        "G GPU60",
        "r GPU read",
        "n normal",
        "f faster",
        "u turbo",
        "b CoolBoost",
        "o LCD OD",
        "k KB timeout",
        "1 silent",
        "2 performance",
        "3 legacy",
        "c effect",
        "+/- light",
        "q quit",
    ];
    let hint_lines = pack(TIGHT_HINTS, w.saturating_sub(4), "  ");
    // Header + THERMAL/FANS/GPU/LIGHT/ADV are the six indispensable rows.
    // Status is safety-critical; optional labels yield before it at the
    // 38-column supported minimum.
    let max_status_rows = height.saturating_sub(6 + hint_lines.len()).max(1);
    let status_lines = bounded_status_lines(&st.message, w.saturating_sub(4), max_status_rows);
    let spare_rows = height.saturating_sub(6 + hint_lines.len() + status_lines.len());
    let show_keys_label = spare_rows >= 1;
    let show_proof = spare_rows >= 2;

    let iface = ellipsise(&st.interface, w.saturating_sub(11));
    o.push_str(&format!("  {}  {}\r\n", bold("ALIEN"), fg(DIM, &iface)));

    let reading = |value: Option<u16>, unit: &str| {
        value
            .map(|value| format!("{value}{unit}"))
            .unwrap_or_else(|| "—".into())
    };
    let summary = |label: &str, detail: String| {
        format!(
            "  {}\r\n",
            ellipsise(&format!("{label:<8}{detail}"), w.saturating_sub(2))
        )
    };

    o.push_str(&summary(
        "THERMAL",
        format!(
            "CPU {} · GPU {} · board {}",
            reading(st.sensors.cpu_temp_c, "°C"),
            reading(st.sensors.gpu_temp_c, "°C"),
            reading(st.sensors.system_temp_c, "°C"),
        ),
    ));
    o.push_str(&summary(
        "FANS",
        format!(
            "CPU {} · GPU {}",
            reading(st.sensors.cpu_fan_rpm, " RPM"),
            reading(st.sensors.gpu_fan_rpm, " RPM"),
        ),
    ));

    let flag = |value: Option<u8>| match value {
        Some(0) => "clear".into(),
        Some(2) => "set".into(),
        Some(value) => format!("?{value}"),
        None => "?".into(),
    };
    let gpu = match st.gpu_mode {
        Some(state) => format!(
            "{} snap {:+}/{:+} · raw C{}/G{}",
            state
                .confirmed_mode()
                .map(GpuMode::label)
                .unwrap_or("split"),
            state.graphics.current_mhz,
            state.memory.current_mhz,
            flag(st.cpu_turbo),
            flag(st.gpu_turbo),
        ),
        None => format!("refresh r (notify) · raw C{}/G?", flag(st.cpu_turbo)),
    };
    o.push_str(&summary("GPU", gpu));

    match st.backlight {
        Some(backlight) => {
            let colour = if backlight.effect == Effect::Static {
                Rgb(st.static_colour.r, st.static_colour.g, st.static_colour.b)
            } else {
                backlight.colour_rgb()
            };
            let detail = format!(
                "{} · bright {} · speed {}",
                backlight.effect.name(),
                backlight.brightness,
                backlight.speed,
            );
            // Two leading spaces + LIGHT + two spaces + swatch + one space =
            // twelve visible cells before the detail begins.
            o.push_str(&format!(
                "  {}  {} {}\r\n",
                fg(DIM, "LIGHT"),
                fg(colour, "██"),
                ellipsise(&detail, w.saturating_sub(12)),
            ));
        }
        None => o.push_str(&summary("LIGHT", "unavailable".into())),
    }

    // Compact tokens keep all three states visible on one narrow line. `n/a`
    // means the getter established that the endpoint is unsupported; `?`
    // means there is no trustworthy readback, so the associated key still
    // refuses to write.
    let advanced = |state: Option<bool>, support: Support| match state {
        Some(true) => "on",
        Some(false) => "off",
        None if support == Support::No => "n/a",
        None => "?",
    };
    let timeout = st
        .keyboard_timeout
        .map(|state| format!("{}s", state.seconds))
        .unwrap_or_else(|| {
            if st.keyboard_timeout_support == Support::No {
                "n/a".into()
            } else {
                "?".into()
            }
        });
    o.push_str(&summary(
        "ADV",
        format!(
            "CB {} · OD {} · KB {}",
            advanced(st.coolboost, st.coolboost_support),
            advanced(st.lcd_overdrive, st.lcd_overdrive_support),
            timeout,
        ),
    ));
    if show_proof {
        o.push_str(&summary(
            "PROOF",
            "CB no sustained PH315 A/B/A lift; OD/KB unverified".into(),
        ));
    }

    if show_keys_label {
        o.push_str(&format!("  {}\r\n", fg(DIM, "KEYS")));
    }
    for line in hint_lines {
        o.push_str(&format!("  {}\r\n", fg(DIM, &line)));
    }

    push_status_lines(o, &status_lines);
}

fn section(name: &str, compact: bool) -> String {
    format!(
        "{}  {}\r\n",
        if compact { "" } else { "\r\n" },
        fg(DIM, name)
    )
}

/// Dense telemetry cluster for the rich TUI composition.
///
/// Thermal and fan panels are paired horizontally; the board-temperature
/// panel spans the full width so the richer monitor view never drops a sensor
/// that the compact layout exposes. The title reports the exact visible
/// history capacity at the current terminal width instead of promising 120 s
/// when a narrower panel cannot physically carry all 120 one-second samples.
fn render_telemetry_panels(o: &mut String, st: &State, width: usize, rows: usize) {
    let available = width.saturating_sub(4);
    let left_width = (available.saturating_sub(1)) / 2;
    let right_width = available.saturating_sub(left_width + 1);

    let with_current = |history: &[Option<u16>], current: Option<u16>| {
        let mut samples = history.to_vec();
        if samples.last().copied() != Some(current) {
            samples.push(current);
        }
        samples
    };

    let cpu_temp = with_current(&st.history.cpu_temp, st.sensors.cpu_temp_c);
    let gpu_temp = with_current(&st.history.gpu_temp, st.sensors.gpu_temp_c);
    let system_temp = with_current(&st.history.system_temp, st.sensors.system_temp_c);
    let cpu_fan = with_current(&st.history.cpu_rpm, st.sensors.cpu_fan_rpm);
    let gpu_fan = with_current(&st.history.gpu_rpm, st.sensors.gpu_fan_rpm);

    let mode = graph_glyph_mode();
    let panel = |label: &str,
                 unit: &str,
                 samples: &[Option<u16>],
                 width: usize,
                 minimum_span: u16,
                 colour: Rgb| {
        telemetry_graph::panel(GraphSpec {
            label,
            unit,
            samples,
            width,
            rows,
            minimum_span,
            colour,
            dim: DIM,
            mode,
        })
    };

    let history_label = |name: &str, panel_width: usize| {
        let samples_per_cell = match mode {
            GlyphMode::Braille => 2,
            GlyphMode::Block => 1,
        };
        let seconds = panel_width
            .saturating_sub(2)
            .saturating_mul(samples_per_cell)
            .min(HISTORY);
        format!("{name} · {seconds} s")
    };
    let cpu_temp_label = history_label("CPU THERMAL", left_width);
    let gpu_temp_label = history_label("GPU THERMAL", right_width);
    let board_temp_label = history_label("BOARD THERMAL", available);
    let cpu_fan_label = history_label("CPU FAN", left_width);
    let gpu_fan_label = history_label("GPU FAN", right_width);

    let cpu_temp_panel = panel(
        &cpu_temp_label,
        "°C",
        &cpu_temp,
        left_width,
        20,
        st.sensors.cpu_temp_c.map_or(DIM, temp_colour),
    );
    let gpu_temp_panel = panel(
        &gpu_temp_label,
        "°C",
        &gpu_temp,
        right_width,
        20,
        st.sensors.gpu_temp_c.map_or(DIM, temp_colour),
    );
    let board_temp_panel = panel(
        &board_temp_label,
        "°C",
        &system_temp,
        available,
        20,
        st.sensors.system_temp_c.map_or(DIM, temp_colour),
    );
    let cpu_fan_panel = panel(&cpu_fan_label, "RPM", &cpu_fan, left_width, 1200, ACCENT);
    let gpu_fan_panel = panel(&gpu_fan_label, "RPM", &gpu_fan, right_width, 1200, BRIGHT);

    o.push_str(&section("LIVE MONITORS", false));
    for (left, right) in cpu_temp_panel.iter().zip(gpu_temp_panel.iter()) {
        o.push_str(&format!("  {left} {right}\r\n"));
    }
    for board in board_temp_panel {
        o.push_str(&format!("  {board}\r\n"));
    }
    for (left, right) in cpu_fan_panel.iter().zip(gpu_fan_panel.iter()) {
        o.push_str(&format!("  {left} {right}\r\n"));
    }
}

fn row(
    o: &mut String,
    label: &str,
    value: Option<u16>,
    unit: &str,
    history: &[Option<u16>],
    spark_w: usize,
    colour: impl Fn(u16) -> Rgb,
) {
    match value {
        Some(v) => {
            o.push_str(&format!(
                "  {:<w$} {}  {}\r\n",
                label,
                fg(colour(v), &format!("{v:>5} {unit:<3}")),
                compact_history(history, spark_w, &colour),
                w = LABEL_W
            ));
        }
        // An absent sensor says so rather than showing a zero that reads as a
        // real measurement.
        None => o.push_str(&format!(
            "  {:<w$} {}\r\n",
            label,
            fg(DIM, "    — "),
            w = LABEL_W
        )),
    }
}

/// One-row block fallback for terminals too small for bordered Braille panels.
fn compact_history(v: &[Option<u16>], width: usize, colour: &impl Fn(u16) -> Rgb) -> String {
    if v.len() < 2 || width < 2 {
        return String::new();
    }
    let scale = telemetry_graph::Scale::fitted(v, 1);
    let row = telemetry_graph::graph_rows(v, width, 1, scale, GlyphMode::Block)
        .into_iter()
        .next()
        .unwrap_or_default();
    fg(v.last().copied().flatten().map_or(DIM, colour), &row)
}

/// Truncate with an ellipsis, counting characters rather than bytes.
///
/// Byte slicing would panic on the degree sign and the box characters used
/// throughout this UI.
fn ellipsise(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".into();
    }
    s.chars().take(max - 1).collect::<String>() + "…"
}

/// Wrap status copy without losing words or splitting UTF-8 code points.
///
/// Long unbroken driver/path tokens are hard-wrapped by character count so a
/// malformed error can never force the terminal itself to wrap a row.
fn wrap_status_words(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in s.split_whitespace() {
        let mut remainder = word.to_string();
        loop {
            let separator = usize::from(!current.is_empty());
            let available = width.saturating_sub(current.chars().count() + separator);
            if remainder.chars().count() <= available {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(&remainder);
                break;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                continue;
            }

            let chunk = remainder.chars().take(width).collect();
            remainder = remainder.chars().skip(width).collect();
            lines.push(chunk);
            if remainder.is_empty() {
                break;
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn bounded_status_lines(s: &str, width: usize, max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }
    let mut lines = wrap_status_words(s, width);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            let mut marked: String = last.chars().take(width.saturating_sub(1)).collect();
            marked.push('…');
            *last = marked;
        }
    }
    lines
}

fn push_status_lines(o: &mut String, lines: &[String]) {
    for (index, line) in lines.iter().enumerate() {
        if index == 0 {
            o.push_str("  ");
            o.push_str(&fg(ACCENT, "› "));
        } else {
            o.push_str("    ");
        }
        o.push_str(line);
        o.push_str("\r\n");
    }
}

/// Greedily pack items into lines no wider than `width`.
fn pack(items: &[&str], width: usize, sep: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for it in items {
        let need = if cur.is_empty() {
            it.chars().count()
        } else {
            cur.chars().count() + sep.len() + it.chars().count()
        };
        if !cur.is_empty() && need > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str(sep);
        }
        cur.push_str(it);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

trait ColourExt {
    fn colour_rgb(&self) -> Rgb;
}

impl ColourExt for alien_core::BacklightState {
    fn colour_rgb(&self) -> Rgb {
        Rgb(self.colour.r, self.colour.g, self.colour.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> State {
        State {
            sensors: Sensors {
                cpu_temp_c: Some(72),
                gpu_temp_c: Some(70),
                system_temp_c: Some(72),
                cpu_fan_rpm: Some(5882),
                gpu_fan_rpm: Some(6122),
            },
            history: History::default(),
            cpu_turbo: Some(0),
            gpu_turbo: Some(0),
            gpu_mode: Some(GpuModeState {
                graphics: alien_core::GpuOffsetRange {
                    current_mhz: 0,
                    min_mhz: -1000,
                    max_mhz: 1000,
                },
                memory: alien_core::GpuOffsetRange {
                    current_mhz: 0,
                    min_mhz: -2000,
                    max_mhz: 6000,
                },
                fan_table: 1,
                gpoc: 0,
            }),
            gpu_mode_error: None,
            coolboost: Some(false),
            keyboard_timeout: None,
            lcd_overdrive: Some(false),
            coolboost_support: Support::Unverified,
            keyboard_timeout_support: Support::No,
            lcd_overdrive_support: Support::Unverified,
            backlight: Some(alien_core::BacklightState {
                effect: Effect::Static,
                speed: 0,
                brightness: 75,
                reverse: false,
                colour: Colour::new(0xff, 0xb0, 0x00),
            }),
            static_colour: Colour::new(0xff, 0xb0, 0x00),
            message: "ready".into(),
            interface: "daemon:/run/alien/daemon.sock".into(),
        }
    }

    /// Remove CSI sequences so tests can assert terminal-cell widths rather
    /// than counting the bytes used to colour them.
    fn visible_text(s: &str) -> String {
        let mut visible = String::new();
        let mut chars = s.chars();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                visible.push(ch);
                continue;
            }
            if chars.next() == Some('[') {
                for csi in chars.by_ref() {
                    if csi.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        }
        visible
    }

    fn assert_render_budget(screen: &str, width: u16, height: u16) {
        assert!(
            !screen.ends_with(['\r', '\n']),
            "frame must not advance past its final row"
        );
        let payload = screen
            .strip_prefix(CLEAR_HOME)
            .expect("rendered frame starts by clearing and homing");
        let row_count = usize::from(!payload.is_empty()) + payload.matches("\r\n").count();
        assert!(
            row_count <= usize::from(height),
            "{width}x{height} layout emitted {row_count} rows"
        );
        for row in screen.split("\r\n") {
            let visible = visible_text(row);
            assert!(
                visible.chars().count() <= usize::from(width),
                "{width}-column row wrapped: {visible:?}"
            );
        }
    }

    fn assert_gpu_confirmation_semantics(screen: &str) {
        let visible = visible_text(screen);
        let normalized = visible.split_whitespace().collect::<Vec<_>>().join(" ");
        for token in [
            "ALIEN",
            "UNSUPPORTED clock control",
            "heat/crash/data-loss risk",
            "turbo",
            "graphics +100 MHz",
            "memory +60 MHz",
            "fan table 3",
            "GPOC 2",
            "uppercase Y required",
            "Esc cancels",
        ] {
            assert!(
                normalized.contains(token),
                "GPU confirmation lost {token:?}: {normalized:?}"
            );
        }
    }

    #[test]
    fn ellipsise_counts_characters_not_bytes() {
        // Byte slicing here would panic: the degree sign is two bytes.
        assert_eq!(ellipsise("90 °C ok", 5), "90 °…");
        assert_eq!(ellipsise("short", 20), "short");
        assert_eq!(ellipsise("abc", 1), "…");
    }

    #[test]
    fn pack_never_exceeds_the_width() {
        let items = ["m fans max", "a fans auto", "C cpu 60%", "q quit"];
        for w in [12usize, 20, 40, 80] {
            for line in pack(&items, w, "   ") {
                assert!(
                    line.chars().count()
                        <= w.max(items.iter().map(|i| i.chars().count()).max().unwrap()),
                    "line {line:?} too wide for {w}"
                );
            }
        }
    }

    #[test]
    fn pack_keeps_every_item() {
        let items = ["one", "two", "three", "four"];
        let joined = pack(&items, 9, " ").join(" ");
        for i in items {
            assert!(joined.contains(i), "lost {i}");
        }
    }

    #[test]
    fn compact_history_fits_the_given_width() {
        let v: Vec<Option<u16>> = (0..200).map(|i| Some((i % 50) as u16)).collect();
        let s = compact_history(&v, 20, &|_| ACCENT);
        // Strip the SGR wrapper before counting glyphs.
        let glyphs = s
            .trim_start_matches(|c| c != 'm')
            .trim_start_matches('m')
            .trim_end_matches("\x1b[0m");
        assert_eq!(glyphs.chars().count(), 20);
    }

    #[test]
    fn compact_history_on_a_flat_series_does_not_divide_by_zero() {
        let v = vec![Some(74u16); 30];
        let s = compact_history(&v, 10, &|_| ACCENT);
        assert!(
            s.contains('\u{2588}'),
            "a flat series should render flat and low"
        );
    }

    #[test]
    fn history_preserves_unavailable_samples_as_gaps() {
        let mut history = History::default();
        history.push(&Sensors {
            cpu_temp_c: Some(64),
            gpu_temp_c: None,
            system_temp_c: None,
            cpu_fan_rpm: None,
            gpu_fan_rpm: Some(1800),
        });
        assert_eq!(history.cpu_temp, vec![Some(64)]);
        assert_eq!(history.gpu_temp, vec![None]);
        assert_eq!(history.system_temp, vec![None]);
        assert_eq!(history.cpu_rpm, vec![None]);
        assert_eq!(history.gpu_rpm, vec![Some(1800)]);
    }

    #[test]
    fn rich_render_uses_five_dense_monitor_panels() {
        let mut state = sample_state();
        for i in 0..120u16 {
            state.history.cpu_temp.push(Some(58 + i % 13));
            state.history.gpu_temp.push(Some(52 + i % 17));
            state.history.cpu_rpm.push(Some(2200 + (i * 37) % 1800));
            state.history.gpu_rpm.push(Some(1800 + (i * 29) % 1600));
        }
        let screen = render(&state, 150, 46);
        let visible = visible_text(&screen);
        for label in [
            "LIVE MONITORS",
            "CPU THERMAL",
            "GPU THERMAL",
            "BOARD THERMAL",
            "CPU FAN",
            "GPU FAN",
            "min ",
            "max ",
            "scale ",
        ] {
            assert!(visible.contains(label), "rich layout lost {label:?}");
        }
        assert!(
            visible
                .chars()
                .any(|glyph| ('\u{2800}'..='\u{28ff}').contains(&glyph)),
            "rich layout must use high-density Braille graph cells"
        );
        assert_render_budget(&screen, 150, 46);
    }

    #[test]
    fn block_graph_mode_is_an_explicit_font_compatibility_fallback() {
        assert_eq!(parse_graph_glyph_mode(Some("block")), GlyphMode::Block);
        assert_eq!(parse_graph_glyph_mode(Some("BLOCK")), GlyphMode::Block);
        assert_eq!(parse_graph_glyph_mode(None), GlyphMode::Braille);
    }

    #[test]
    fn rich_monitor_threshold_never_wraps_or_scrolls() {
        let mut state = sample_state();
        for i in 0..120u16 {
            state.history.cpu_temp.push(Some(58 + i % 13));
            state.history.gpu_temp.push(Some(52 + i % 17));
            state.history.cpu_rpm.push(Some(2200 + (i * 37) % 1800));
            state.history.gpu_rpm.push(Some(1800 + (i * 29) % 1600));
        }
        for (width, height) in [
            (96, 32),
            (100, 32),
            (120, 32),
            (96, 36),
            (120, 40),
            (150, 46),
            (180, 60),
        ] {
            let screen = render(&state, width, height);
            assert_render_budget(&screen, width, height);
        }
    }

    #[test]
    fn rich_graphs_expand_into_high_resolution_terminal_space() {
        assert_eq!(rich_graph_rows(32), 2);
        assert_eq!(rich_graph_rows(46), 6);
        assert_eq!(rich_graph_rows(60), 8);
        assert_eq!(rich_graph_rows(200), 8);
    }

    #[test]
    fn tight_render_fits_small_terminal_without_wrapping_or_scrolling() {
        let state = sample_state();

        // The 520x380 kitty capture exposed about 46x17 terminal cells. Check
        // that nearby widths, including the renderer's supported minimum, fit
        // both dimensions without relying on a terminal to clip anything.
        for width in [38u16, 46, 52] {
            let screen = render(&state, width, 17);
            assert_render_budget(&screen, width, 17);
        }
    }

    #[test]
    fn tight_render_retains_status_and_every_control_key() {
        let screen = visible_text(&render(&sample_state(), 46, 17));
        for text in [
            "ALIEN",
            "THERMAL CPU",
            "FANS",
            "GPU",
            "LIGHT",
            "ADV",
            "PROOF",
            "KEYS",
            "m max",
            "a auto",
            "C CPU60",
            "G GPU60",
            "r GPU read",
            "n normal",
            "f faster",
            "u turbo",
            "b CoolBoost",
            "o LCD OD",
            "k KB timeout",
            "1 silent",
            "2 performance",
            "3 legacy",
            "c effect",
            "+/- light",
            "q quit",
            "ready",
        ] {
            assert!(screen.contains(text), "tight layout lost {text:?}");
        }
    }

    #[test]
    fn standard_760x520_budget_keeps_the_richer_layout() {
        let width = 76u16;
        let height = 22u16;
        let screen = render(&sample_state(), width, height);
        let visible = visible_text(&screen);

        assert!(visible.contains("raw flags"));
        assert!(visible.contains("OEM GPU"));
        assert!(visible.contains("backlight"));
        assert_render_budget(&screen, width, height);
    }

    #[test]
    fn gpu_confirmation_fits_760x520_with_full_safety_contract() {
        let width = 76u16;
        let height = 22u16;
        let mut state = sample_state();
        state.message = gpu_mode_confirmation_message(GpuMode::Turbo);

        let screen = render(&state, width, height);
        assert_render_budget(&screen, width, height);
        assert_eq!(screen.matches("\r\n").count() + 1, usize::from(height));
        assert_gpu_confirmation_semantics(&screen);
    }

    #[test]
    fn gpu_confirmation_fits_520x380_with_full_safety_contract() {
        let width = 46u16;
        let height = 17u16;
        let mut state = sample_state();
        state.message = gpu_mode_confirmation_message(GpuMode::Turbo);

        let screen = render(&state, width, height);
        assert_render_budget(&screen, width, height);
        assert_eq!(screen.matches("\r\n").count() + 1, usize::from(height));
        assert_gpu_confirmation_semantics(&screen);
    }
}
