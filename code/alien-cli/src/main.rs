//! `alien` — the command-line face of Alien.
//!
//! Deliberately dependency-light: the argument surface is small enough that a
//! hand-written parser is shorter than the derive macros configuring one, and
//! this binary gets vendored into six packaging formats where every crate is a
//! licence review.
//!
//! Every mutating subcommand prints what the firmware said. That is the whole
//! point — this interface is full of calls that succeed without doing anything,
//! so a silent success would be indistinguishable from a no-op.

use std::process::ExitCode;

use alien_core::profile::{self, Profile};
use alien_core::wmi::OverclockTarget;
use alien_core::{Colour, Device, Direction, Effect, Fan, Zone};

const USAGE: &str = "\
alien — Acer Predator / Nitro control for Linux

USAGE
    alien <command> [args]

COMMANDS
    status                    temperatures, fan RPM and turbo state
    watch [interval]          live telemetry (default 1s); Ctrl-C to stop

    fan max                   both fans to maximum
    fan auto                  both fans back to the EC curve
    fan <cpu|gpu> <percent>   manual duty for one fan, 0-100

    turbo on|off              set the CPU and GPU overclock flags
    turbo status              read them back

    rgb <colour>              all four zones to one colour (#rrggbb)
    rgb zone <1-4> <colour>   colour one zone
    rgb zones <c1> <c2> <c3> <c4>   all four zones at once
    rgb effect <name> [speed] [brightness] [colour] [reverse]
    rgb off                   backlight off
    rgb status                read the backlight state back from firmware
    rgb key <name> <colour>   colour ONE key (per-key hardware only)
    rgb keys                  list the key names understood

    capabilities              what this specific machine can actually do

    profile list              show available profiles
    profile apply <name>      apply one

    json                      machine-readable status

    doctor                    check whether this machine is supported

EFFECTS
    static breath neon wave shifting zoom ripple

    Colour applies to: static breath wave zoom
    The others run the firmware's own palette and take no colour.
    Everything except static animates, so speed 1-9 (0 would not move).

NOTES
    Requires root and the acpi_call kernel module.
    Fan control is the control that matters: on a Helios 300 PH315-53 it is
    worth roughly +48% sustained CPU throughput, because the stock EC curve
    holds the chip in thermal throttle.
";

fn main() -> ExitCode {
    restore_default_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("alien: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(a: &[&str]) -> Result<(), String> {
    match a {
        [] | ["-h"] | ["--help"] | ["help"] => {
            print!("{USAGE}");
            Ok(())
        }
        ["-V"] | ["--version"] | ["version"] => {
            println!("alien {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["doctor"] => doctor(),
        ["capabilities"] | ["caps"] => capabilities(&open()?),
        ["status"] => status(&open()?),
        ["json"] => json(&open()?),
        ["watch", rest @ ..] => {
            let secs: f64 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(1.0);
            watch(&open()?, secs)
        }

        ["fan", "max"] => {
            let d = open()?;
            d.fans_max().map_err(fmt)?;
            report_fans(&d);
            Ok(())
        }
        ["fan", "auto"] => {
            let d = open()?;
            d.fans_auto().map_err(fmt)?;
            report_fans(&d);
            Ok(())
        }
        ["fan", which, pct] => {
            let fan = parse_fan(which)?;
            let pct: u8 = pct.parse().map_err(|_| format!("not a percentage: {pct}"))?;
            let d = open()?;
            d.set_fan_percent(fan, pct).map_err(fmt)?;
            // Confirm from the firmware rather than announcing success. The
            // readback is the requested duty, so it is valid immediately,
            // before the fan has finished ramping.
            match d.fan_percent(fan) {
                Ok(got) if got == pct => println!("{which} fan set to {pct}% (confirmed)"),
                Ok(got) => println!("{which} fan: asked {pct}%, firmware reports {got}%"),
                Err(e) => println!("{which} fan set to {pct}% (could not read back: {e})"),
            }
            report_fans(&d);
            Ok(())
        }

        ["turbo", "status"] => {
            let d = open()?;
            let cpu = d.overclock(OverclockTarget::Cpu).map_err(fmt)?;
            let gpu = d.overclock(OverclockTarget::Gpu).map_err(fmt)?;
            println!("cpu turbo flag: {}", flag(cpu));
            println!("gpu turbo flag: {}", flag(gpu));
            Ok(())
        }
        ["turbo", v @ ("on" | "off")] => {
            let on = *v == "on";
            let d = open()?;
            d.set_overclock(OverclockTarget::Cpu, on).map_err(fmt)?;
            d.set_overclock(OverclockTarget::Gpu, on).map_err(fmt)?;
            println!("turbo flags set to {v}");
            if on {
                eprintln!(
                    "note: on the PH315-53 the CPU flag is inert — PredatorSense itself gates CPU\n\
                     overclock on Feature.ini OverclockSupport CPU, which is 0 for this model. Its\n\
                     \"CPU turbo\" is Intel XTU power limits (PL1/PL2), not this interface. The GPU\n\
                     flag does go through here. Fans are what move the numbers on this chassis."
                );
            }
            Ok(())
        }

        ["rgb", "off"] => {
            open()?.backlight_off().map_err(fmt)?;
            println!("backlight off");
            Ok(())
        }
        ["rgb", "zone", z, colour] => {
            let idx: usize = z.parse().map_err(|_| format!("not a zone: {z}"))?;
            let zone = Zone::from_index(idx.wrapping_sub(1))
                .ok_or_else(|| format!("zone must be 1-4, got {z}"))?;
            let c = Colour::parse(colour).ok_or_else(|| format!("not a colour: {colour}"))?;
            let d = open()?;
            d.set_zone_colour(zone, c).map_err(fmt)?;
            // Deliberately NOT reported as "confirmed". A readback proves the
            // firmware stored the value; it says nothing about whether the
            // LEDs changed, and treating the two as the same is how this
            // project shipped lighting that never worked.
            println!("zone {idx} -> {}", c.to_hex());
            Ok(())
        }
        ["rgb", "effect", name, rest @ ..] => {
            let effect = Effect::parse(name).ok_or_else(|| {
                format!(
                    "unknown effect {name}; try one of: {}",
                    Effect::ALL.iter().map(|e| e.name()).collect::<Vec<_>>().join(" ")
                )
            })?;
            // Anything not given falls back to what this effect was last set
            // to, not to a global default — that is what makes each mode
            // remember its own, in a process that remembers nothing itself.
            let mut mem = alien_core::Lighting::load();
            let speed: u8 = rest.first().and_then(|s| s.parse().ok()).unwrap_or_else(|| mem.speed(effect));
            let brightness: u8 = rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(mem.brightness);
            let colour = rest.get(2).and_then(|s| Colour::parse(s)).unwrap_or_else(|| mem.colour(effect));
            let dir = if rest.contains(&"reverse") {
                Direction::RightToLeft
            } else {
                Direction::LeftToRight
            };

            let d = open()?;
            if effect == Effect::Static {
                // Static colour lives in the per-zone registers, not in the
                // effect payload.
                d.set_zone_colours([colour; 4], brightness).map_err(fmt)?;
            } else {
                d.set_effect(effect, speed, brightness, dir, colour).map_err(fmt)?;
            }

            mem.set_colour(effect, colour);
            mem.set_speed(effect, speed);
            mem.brightness = brightness;
            if effect == Effect::Static {
                mem.set_zone_colours([colour; 4]);
            }
            let _ = mem.save();

            println!("effect {} · speed {speed} · brightness {brightness}{}",
                effect.name(), if effect.honours_colour() { format!(" · {}", colour.to_hex()) } else { String::new() });
            Ok(())
        }
        ["rgb", "zones", a, b, c, d] => {
            let cs = [a, b, c, d]
                .iter()
                .map(|x| Colour::parse(x).ok_or_else(|| format!("not a colour: {x}")))
                .collect::<Result<Vec<_>, _>>()?;
            let dev = open()?;
            let bright = dev.backlight().map(|b| b.brightness).unwrap_or(100);
            dev.set_zone_colours([cs[0], cs[1], cs[2], cs[3]], bright).map_err(fmt)?;
            for (i, z) in Zone::ALL.iter().enumerate() {
                let got = dev.zone_colour(*z).map(|c| c.to_hex()).unwrap_or_else(|_| "?".into());
                println!("zone {} -> {}", i + 1, got);
            }
            Ok(())
        }
        ["rgb", "keys"] => {
            let keys = alien_core::perkey::known_keys();
            println!("{} key names understood:", keys.len());
            for chunk in keys.chunks(12) {
                println!("  {}", chunk.join(" "));
            }
            Ok(())
        }
        ["rgb", "key", name, colour] => {
            let c = Colour::parse(colour).ok_or_else(|| format!("not a colour: {colour}"))?;
            let d = open()?;
            d.set_key(name, c).map_err(fmt)?;
            println!("{name} -> {}", c.to_hex());
            Ok(())
        }
        ["rgb", "status"] => {
            let b = open()?.backlight().map_err(fmt)?;
            println!("effect     {}", b.effect.name());
            println!("speed      {}", b.speed);
            println!("brightness {}", b.brightness);
            println!("colour     {}", b.colour.to_hex());
            println!("direction  {}", if b.reverse { "right-to-left" } else { "left-to-right" });
            Ok(())
        }
        ["rgb", colour] => {
            let c = Colour::parse(colour).ok_or_else(|| format!("not a colour: {colour}"))?;
            let mut mem = alien_core::Lighting::load();
            open()?.set_zone_colours([c; 4], mem.brightness).map_err(fmt)?;
            mem.set_zone_colours([c; 4]);
            mem.set_colour(Effect::Static, c);
            let _ = mem.save();
            println!("all zones -> {}", c.to_hex());
            Ok(())
        }

        ["profile", "list"] => {
            for p in Profile::builtins() {
                println!("{:<12} {}", p.name, p.description);
            }
            let dir = profile::config_dir();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    if e.path().extension().is_some_and(|x| x == "toml") {
                        if let Some(stem) = e.path().file_stem().and_then(|s| s.to_str()) {
                            println!("{stem:<12} (user profile, {})", dir.display());
                        }
                    }
                }
            }
            Ok(())
        }
        ["profile", "apply", name] => {
            let p = profile::load(name).ok_or_else(|| format!("no profile named {name}"))?;
            let d = open()?;
            p.apply(&d).map_err(fmt)?;
            println!("applied profile: {}", p.name);
            report_fans(&d);
            Ok(())
        }

        other => Err(format!(
            "unknown command: {}\nrun `alien help`",
            other.join(" ")
        )),
    }
}

/// Die quietly when our output pipe closes, the way every other CLI does.
///
/// Rust sets `SIGPIPE` to `SIG_IGN` at startup, so `write` returns EPIPE and
/// the standard library **panics**. `alien watch | head -5` therefore ends in a
/// Rust backtrace instead of just stopping — which it did, the first time it
/// was piped anywhere. Restoring the default disposition makes the process
/// terminate on the signal like `cat` or `journalctl` would.
fn restore_default_sigpipe() {
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    // SAFETY: setting a signal to its default disposition before any threads
    // exist. No handler state to race with.
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

/// Report what the machine supports, rather than assuming the reference model.
fn capabilities(d: &Device) -> Result<(), String> {
    let c = d.capabilities();
    println!("{:<26} {}", "control", "supported");
    println!("{:-<26} {:-<11}", "", "");
    for (name, s) in c.rows() {
        let mark = match s {
            alien_core::Support::Yes => "yes",
            alien_core::Support::No => "no",
            alien_core::Support::Unverified => "accepted, unverified",
        };
        println!("{name:<26} {mark}");
    }
    if !c.misc_subindices.is_empty() {
        let list: Vec<String> = c.misc_subindices.iter().map(|s| format!("{s:#04x}")).collect();
        println!("\nmisc-setting sub-indices accepted: {}", list.join(" "));
    }
    for n in &c.notes {
        println!("\nnote: {n}");
    }
    Ok(())
}

fn open() -> Result<Device, String> {
    // No root check here. Device::open prefers the daemon, and an unprivileged
    // member of the `alien` group is a fully supported caller — gating on uid
    // 0 turned those users away from a socket that was working fine.
    Device::open().map_err(fmt)
}

fn fmt(e: alien_core::Error) -> String {
    e.to_string()
}

fn parse_fan(s: &str) -> Result<Fan, String> {
    match s {
        "cpu" => Ok(Fan::Cpu),
        "gpu" => Ok(Fan::Gpu),
        _ => Err(format!("fan must be cpu or gpu, got {s}")),
    }
}

fn flag(v: u8) -> &'static str {
    match v {
        0 => "off",
        2 => "turbo",
        _ => "unknown",
    }
}

/// Print the fan speeds once they have stopped moving.
///
/// Two traps here, and the second one bit after the first was fixed.
///
/// A fixed short delay is worse than useless: at 600 ms after `fan auto` the
/// fans read 4477 RPM while on their way to 2608 — a number that looks like
/// confirmation and is true of no steady state.
///
/// But polling for "two consecutive samples agree" has the same failure in
/// reverse: for the first second or so after the call the fan has not *started*
/// moving, so consecutive samples agree perfectly and the loop reports the old
/// speed as settled. That is exactly how `fan cpu 40` came to print 5882 RPM.
/// So there is a mandatory settling delay before stability counts at all.
fn report_fans(d: &Device) {
    const TOLERANCE: u16 = 120;
    const STEP: std::time::Duration = std::time::Duration::from_millis(700);
    // The EC takes ~1s to react. Anything stable before that is the fan not
    // having moved yet, not the fan having arrived.
    const MIN_SETTLE: std::time::Duration = std::time::Duration::from_millis(2500);
    const MAX_WAIT: usize = 16; // ~11s, comfortably past a full ramp

    let started = std::time::Instant::now();
    let mut prev = d.sensors();
    for _ in 0..MAX_WAIT {
        std::thread::sleep(STEP);
        let now = d.sensors();
        if started.elapsed() < MIN_SETTLE {
            prev = now;
            continue;
        }
        let settled = |a: Option<u16>, b: Option<u16>| match (a, b) {
            (Some(x), Some(y)) => x.abs_diff(y) <= TOLERANCE,
            (None, None) => true,
            _ => false,
        };
        if settled(prev.cpu_fan_rpm, now.cpu_fan_rpm) && settled(prev.gpu_fan_rpm, now.gpu_fan_rpm) {
            println!("cpu fan: {}   gpu fan: {}", rpm(now.cpu_fan_rpm), rpm(now.gpu_fan_rpm));
            return;
        }
        prev = now;
    }
    println!(
        "cpu fan: {}   gpu fan: {}   (still changing — run `alien status` again)",
        rpm(prev.cpu_fan_rpm),
        rpm(prev.gpu_fan_rpm)
    );
}

fn rpm(v: Option<u16>) -> String {
    v.map(|r| format!("{r} RPM")).unwrap_or_else(|| "—".into())
}

fn temp(v: Option<u16>) -> String {
    v.map(|t| format!("{t} °C")).unwrap_or_else(|| "—".into())
}

fn status(d: &Device) -> Result<(), String> {
    let s = d.sensors();
    let cpu_oc = d.overclock(OverclockTarget::Cpu).unwrap_or(0);
    let gpu_oc = d.overclock(OverclockTarget::Gpu).unwrap_or(0);
    println!("interface   {}", d.method_path());
    println!("cpu temp    {}", temp(s.cpu_temp_c));
    println!("gpu temp    {}", temp(s.gpu_temp_c));
    println!("board temp  {}", temp(s.system_temp_c));
    println!("cpu fan     {}", rpm(s.cpu_fan_rpm));
    println!("gpu fan     {}", rpm(s.gpu_fan_rpm));
    println!("cpu turbo   {}", flag(cpu_oc));
    println!("gpu turbo   {}", flag(gpu_oc));
    if let Ok(b) = d.backlight() {
        println!(
            "backlight   {} · brightness {} · speed {} · {}{}",
            b.effect.name(),
            b.brightness,
            b.speed,
            b.colour.to_hex(),
            if b.reverse { " · reversed" } else { "" }
        );
    }
    Ok(())
}

fn json(d: &Device) -> Result<(), String> {
    let s = d.sensors();
    println!(
        r#"{{"cpu_temp_c":{},"gpu_temp_c":{},"system_temp_c":{},"cpu_fan_rpm":{},"gpu_fan_rpm":{},"cpu_turbo":{},"gpu_turbo":{}}}"#,
        opt(s.cpu_temp_c),
        opt(s.gpu_temp_c),
        opt(s.system_temp_c),
        opt(s.cpu_fan_rpm),
        opt(s.gpu_fan_rpm),
        d.overclock(OverclockTarget::Cpu).unwrap_or(0),
        d.overclock(OverclockTarget::Gpu).unwrap_or(0),
    );
    Ok(())
}

fn opt(v: Option<u16>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "null".into())
}

fn watch(d: &Device, secs: f64) -> Result<(), String> {
    let period = std::time::Duration::from_secs_f64(secs.max(0.1));
    println!("  cpu °C   gpu °C   sys °C   cpu RPM   gpu RPM");
    loop {
        let s = d.sensors();
        println!(
            "{:>8} {:>8} {:>8} {:>9} {:>9}",
            s.cpu_temp_c.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            s.gpu_temp_c.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            s.system_temp_c.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            s.cpu_fan_rpm.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            s.gpu_fan_rpm.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
        );
        std::thread::sleep(period);
    }
}

/// Report whether this machine can be driven, and why not if it cannot.
///
/// Written to be the first thing a user on an unknown model runs, and to be
/// useful when pasted into a bug report — so it prints the DMI strings that
/// identify the model, not just pass/fail.
fn doctor() -> Result<(), String> {
    let read = |p: &str| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_else(|_| "unknown".into());
    println!("vendor      {}", read("/sys/class/dmi/id/sys_vendor"));
    println!("model       {}", read("/sys/class/dmi/id/product_name"));
    println!("bios        {}", read("/sys/class/dmi/id/bios_version"));

    let guid_present = std::fs::read_dir("/sys/bus/wmi/devices")
        .map(|d| {
            d.flatten().any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_uppercase()
                    .starts_with(alien_core::wmi::GAMING_GUID)
            })
        })
        .unwrap_or(false);
    println!(
        "gaming wmi  {}",
        if guid_present { "present" } else { "NOT FOUND — this model may not be supported" }
    );

    let acpi_call = std::path::Path::new("/proc/acpi/call").exists();
    println!(
        "acpi_call   {}",
        if acpi_call { "loaded" } else { "MISSING — run: modprobe acpi_call" }
    );

    match alien_core::preflight_direct() {
        Ok(()) => match Device::open() {
            Ok(d) => {
                println!("dispatch    {}", d.method_path());
                let s = d.sensors();
                println!(
                    "telemetry   cpu {} · gpu {} / fans {} {}",
                    temp(s.cpu_temp_c),
                    temp(s.gpu_temp_c),
                    rpm(s.cpu_fan_rpm),
                    rpm(s.gpu_fan_rpm)
                );
                println!("\nsupported.");
            }
            Err(e) => println!("\nfirmware reachable but no dispatch method found: {e}"),
        },
        Err(e) => println!("\ncannot talk to the firmware: {e}"),
    }
    Ok(())
}
