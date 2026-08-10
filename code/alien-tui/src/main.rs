//! `alien-tui` — the terminal control centre.
//!
//! Live telemetry with sparkline history, fan and profile control, and RGB,
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
use alien_core::{Colour, Device, Direction, Effect, Fan, Sensors};

mod term;
use term::*;

/// How many samples the sparklines keep. Chosen to fit a narrow terminal
/// without wrapping; the history is cosmetic, so dropping the oldest is fine.
const HISTORY: usize = 60;

#[derive(Default)]
struct History {
    cpu_temp: Vec<u16>,
    gpu_temp: Vec<u16>,
    cpu_rpm: Vec<u16>,
    gpu_rpm: Vec<u16>,
}

impl History {
    fn push(&mut self, s: &Sensors) {
        let add = |v: &mut Vec<u16>, x: Option<u16>| {
            v.push(x.unwrap_or(0));
            if v.len() > HISTORY {
                v.remove(0);
            }
        };
        add(&mut self.cpu_temp, s.cpu_temp_c);
        add(&mut self.gpu_temp, s.gpu_temp_c);
        add(&mut self.cpu_rpm, s.cpu_fan_rpm);
        add(&mut self.gpu_rpm, s.gpu_fan_rpm);
    }
}

struct State {
    sensors: Sensors,
    history: History,
    cpu_turbo: u8,
    gpu_turbo: u8,
    backlight: Option<alien_core::BacklightState>,
    /// Last thing we did, shown in the status line. The whole project's habit:
    /// say what the firmware said, never assume it worked.
    message: String,
    interface: String,
}

fn main() -> std::process::ExitCode {
    let dev = match Device::open() {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!("alien-tui: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let state = Arc::new(Mutex::new(State {
        sensors: Sensors::default(),
        history: History::default(),
        cpu_turbo: 0,
        gpu_turbo: 0,
        backlight: None,
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
                let cpu = dev.overclock(OverclockTarget::Cpu).unwrap_or(0);
                let gpu = dev.overclock(OverclockTarget::Gpu).unwrap_or(0);
                let bl = dev.backlight().ok();
                if let Ok(mut st) = state.lock() {
                    st.history.push(&s);
                    st.sensors = s;
                    st.cpu_turbo = cpu;
                    st.gpu_turbo = gpu;
                    st.backlight = bl;
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

    while running.load(Ordering::Relaxed) {
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

        match key {
            b'q' | 3 /* ctrl-c */ => running.store(false, Ordering::Relaxed),
            b'm' => msg = Some(act(&dev.fans_max(), "fans -> maximum")),
            b'a' => msg = Some(act(&dev.fans_auto(), "fans -> EC automatic curve")),
            b'1' => msg = Some(apply(&dev, "silent")),
            b'2' => msg = Some(apply(&dev, "performance")),
            b'3' => msg = Some(apply(&dev, "turbo")),
            b't' => {
                let on = state.lock().map(|s| s.cpu_turbo == 0).unwrap_or(true);
                let r = dev
                    .set_overclock(OverclockTarget::Cpu, on)
                    .and_then(|_| dev.set_overclock(OverclockTarget::Gpu, on));
                msg = Some(act(&r, if on { "turbo flags on" } else { "turbo flags off" }));
            }
            b'+' | b'=' | b'-' | b'_' => {
                let cur = state.lock().ok().and_then(|s| s.backlight).map(|b| b.brightness).unwrap_or(50);
                let step: i16 = if key == b'+' || key == b'=' { 10 } else { -10 };
                let next = (cur as i16 + step).clamp(0, 100) as u8;
                {
                    let mut m = alien_core::Lighting::load();
                    m.brightness = next;
                    let _ = m.save();
                }
                let b = state.lock().ok().and_then(|s| s.backlight);
                let (e, sp, dir, c) = b
                    .map(|b| (b.effect, b.speed, if b.reverse { Direction::RightToLeft } else { Direction::LeftToRight }, b.colour))
                    .unwrap_or((Effect::Static, 0, Direction::LeftToRight, Colour::new(0, 174, 199)));
                msg = Some(act(&dev.set_effect(e, sp, next, dir, c), &format!("backlight brightness {next}")));
            }
            b'c' => {
                // Step through the effects. Each one's colour and speed come
                // from the store shared with the CLI and GUI, so cycling here
                // restores what that mode was last set to rather than dragging
                // the previous mode's colour along.
                let cur = state.lock().ok().and_then(|s| s.backlight);
                let idx = cur
                    .and_then(|b| Effect::ALL.iter().position(|e| *e == b.effect))
                    .unwrap_or(0);
                let next = Effect::ALL[(idx + 1) % Effect::ALL.len()];
                let mut mem = alien_core::Lighting::load();
                let c = mem.colour(next);
                let sp = mem.speed(next);
                let br = mem.brightness;
                let r = if next == Effect::Static {
                    // Static colour lives in the per-zone registers.
                    dev.set_zone_colours(mem.zone_colours(), br)
                } else {
                    dev.set_effect(next, sp, br, Direction::LeftToRight, c)
                };
                mem.set_colour(next, c);
                let _ = mem.save();
                msg = Some(act(&r, &format!("effect -> {}", next.name())));
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
        Some(p) => act(&p.apply(dev), &format!("profile -> {name}")),
        None => format!("no profile {name}"),
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// Accent teal. Matches PredatorSense's own primary (`#00AEC7`), so the TUI and
/// the GUI read as the same product.
const ACCENT: Rgb = Rgb(0x00, 0xAE, 0xC7);
const DIM: Rgb = Rgb(0x6a, 0x77, 0x7f);
const WARN: Rgb = Rgb(0xff, 0xb0, 0x00);
const HOT: Rgb = Rgb(0xff, 0x4d, 0x3a);

fn temp_colour(c: u16) -> Rgb {
    match c {
        0..=69 => Rgb(0x5a, 0xd8, 0x9a),
        70..=84 => WARN,
        _ => HOT,
    }
}

/// Columns consumed by "  label      value unit  " before the sparkline.
const GUTTER: usize = 2;
const LABEL_W: usize = 9;
const VALUE_W: usize = 10;

fn draw(t: &mut Terminal, st: &State) {
    let mut o = String::with_capacity(4096);
    o.push_str(CLEAR_HOME);

    // Re-read every frame rather than caching from startup: the window
    // manager tiles this window after it opens, so the size at startup is
    // usually not the size it ends up.
    t.refresh_size();
    let w = t.width.max(38) as usize;

    // Header: a title line and a rule. Deliberately not a box — a bordered
    // box needs every content line padded to an exact width, and the first
    // version got that wrong the moment the interface path was long enough to
    // wrap, leaving a torn frame on screen.
    let mut title = String::new();
    title.push_str(&bold("ALIEN"));
    title.push_str(&fg(DIM, "  ·  Acer Predator control"));
    o.push_str(&format!("  {}\r\n", title));
    // The socket path is the first thing to sacrifice when space is tight.
    let iface = ellipsise(&st.interface, w.saturating_sub(4));
    o.push_str(&format!("  {}\r\n", fg(DIM, &iface)));
    o.push_str(&fg(ACCENT, &format!("  {}\r\n", "─".repeat(w.saturating_sub(4)))));

    // Width left for a sparkline once label and value have taken their share.
    let spark_w = w.saturating_sub(GUTTER + LABEL_W + VALUE_W + 2);

    // ── Temperatures ────────────────────────────────────────────────────────
    o.push_str(&section("THERMAL"));
    row(&mut o, "CPU", st.sensors.cpu_temp_c, "°C", &st.history.cpu_temp, spark_w, temp_colour);
    row(&mut o, "GPU", st.sensors.gpu_temp_c, "°C", &st.history.gpu_temp, spark_w, temp_colour);
    row(&mut o, "Board", st.sensors.system_temp_c, "°C", &[], spark_w, temp_colour);

    // ── Fans ────────────────────────────────────────────────────────────────
    o.push_str(&section("FANS"));
    row(&mut o, "CPU fan", st.sensors.cpu_fan_rpm, "RPM", &st.history.cpu_rpm, spark_w, |_| ACCENT);
    row(&mut o, "GPU fan", st.sensors.gpu_fan_rpm, "RPM", &st.history.gpu_rpm, spark_w, |_| ACCENT);

    // ── Turbo + backlight ───────────────────────────────────────────────────
    o.push_str(&section("STATE"));
    let flag = |v: u8| match v {
        0 => fg(DIM, "off"),
        2 => fg(WARN, "TURBO"),
        n => fg(HOT, &format!("? {n}")),
    };
    o.push_str(&format!(
        "  {:<w$} cpu {}  gpu {}\r\n",
        "turbo",
        flag(st.cpu_turbo),
        flag(st.gpu_turbo),
        w = LABEL_W
    ));
    match st.backlight {
        Some(b) => {
            // Measure the plain text and colour afterwards. Ellipsising the
            // composed string would cut through an escape sequence and leave
            // the rest of the screen tinted — and the swatch is two columns
            // wide but eleven bytes long, so byte length is meaningless here.
            let detail = format!(
                "{}  brightness {}  speed {}",
                b.effect.name(),
                b.brightness,
                b.speed
            );
            let avail = w.saturating_sub(GUTTER + LABEL_W + 4);
            o.push_str(&format!(
                "  {:<lw$} {} {}\r\n",
                "backlight",
                fg(b.colour_rgb(), "██"),
                ellipsise(&detail, avail),
                lw = LABEL_W
            ));
        }
        None => o.push_str(&format!("  {:<w$} {}\r\n", "backlight", fg(DIM, "unavailable"), w = LABEL_W)),
    }

    // ── Keys ────────────────────────────────────────────────────────────────
    //
    // Packed to the real width rather than hardcoded into two lines. The fixed
    // version wrapped mid-word ("+/- bri / ghtness") in a tiled window, which
    // is the sort of thing that looks broken rather than merely tight.
    o.push_str(&section("KEYS"));
    const HINTS: &[&str] = &[
        "m fans max",
        "a fans auto",
        "C cpu 60%",
        "G gpu 60%",
        "t turbo",
        "1 silent",
        "2 performance",
        "3 turbo",
        "c effect",
        "+/- bright",
        "q quit",
    ];
    for line in pack(HINTS, w.saturating_sub(4), "   ") {
        o.push_str(&format!("  {}\r\n", fg(DIM, &line)));
    }

    o.push_str("\r\n  ");
    o.push_str(&fg(ACCENT, "› "));
    o.push_str(&ellipsise(&st.message, w.saturating_sub(6)));
    o.push_str("\r\n");

    let _ = t.out.write_all(o.as_bytes());
    let _ = t.out.flush();
}

fn section(name: &str) -> String {
    format!("\r\n  {}\r\n", fg(DIM, name))
}

fn row(
    o: &mut String,
    label: &str,
    value: Option<u16>,
    unit: &str,
    history: &[u16],
    spark_w: usize,
    colour: impl Fn(u16) -> Rgb,
) {
    match value {
        Some(v) => {
            o.push_str(&format!(
                "  {:<w$} {}  {}\r\n",
                label,
                fg(colour(v), &format!("{v:>5} {unit:<3}")),
                spark(history, spark_w, &colour),
                w = LABEL_W
            ));
        }
        // An absent sensor says so rather than showing a zero that reads as a
        // real measurement.
        None => o.push_str(&format!("  {:<w$} {}\r\n", label, fg(DIM, "    — "), w = LABEL_W)),
    }
}

/// Braille-free sparkline over the most recent `width` samples.
///
/// Uses the eighth-block run, which every terminal font that can draw a box
/// can also draw. Takes only the tail that fits: the history buffer is longer
/// than any terminal, and rendering all of it is what pushed the line past the
/// window edge and made each row wrap onto the next.
fn spark(v: &[u16], width: usize, colour: &impl Fn(u16) -> Rgb) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if v.len() < 2 || width < 2 {
        return String::new();
    }
    let v = &v[v.len().saturating_sub(width)..];
    let lo = *v.iter().min().unwrap_or(&0);
    let hi = *v.iter().max().unwrap_or(&1);
    // A flat series would divide by zero and, worse, render as a jagged line
    // built from rounding noise. Draw it flat and low, which is the truth.
    if hi == lo {
        return fg(DIM, &BLOCKS[0].to_string().repeat(v.len()));
    }
    let s: String = v
        .iter()
        .map(|x| {
            let idx = ((x - lo) as usize * (BLOCKS.len() - 1)) / (hi - lo) as usize;
            BLOCKS[idx]
        })
        .collect();
    fg(colour(*v.last().unwrap_or(&0)), &s)
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

/// Greedily pack items into lines no wider than `width`.
fn pack(items: &[&str], width: usize, sep: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for it in items {
        let need = if cur.is_empty() { it.chars().count() } else { cur.chars().count() + sep.len() + it.chars().count() };
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
                assert!(line.chars().count() <= w.max(items.iter().map(|i| i.chars().count()).max().unwrap()),
                    "line {line:?} too wide for {w}");
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
    fn spark_fits_the_given_width() {
        let v: Vec<u16> = (0..200).map(|i| (i % 50) as u16).collect();
        let s = spark(&v, 20, &|_| ACCENT);
        // Strip the SGR wrapper before counting glyphs.
        let glyphs = s.trim_start_matches(|c| c != 'm').trim_start_matches('m').trim_end_matches("\x1b[0m");
        assert_eq!(glyphs.chars().count(), 20);
    }

    #[test]
    fn spark_on_a_flat_series_does_not_divide_by_zero() {
        let v = vec![74u16; 30];
        let s = spark(&v, 10, &|_| ACCENT);
        assert!(s.contains('\u{2581}'), "a flat series should render flat and low");
    }
}
