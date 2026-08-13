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

use alien_core::performance::{Performance, PowerConstraint, PowercapStatus, PH315_53_CPU_POWER};
use alien_core::profile;
use alien_core::wmi::OverclockTarget;
use alien_core::{
    covini_brightness, covini_speed, Colour, Device, Direction, Effect, Fan, GpuMode, GpuModeOptIn,
    GpuModeState, Zone, GPU_MODE_ACKNOWLEDGEMENT,
};

const USAGE: &str = "\
alien — Acer Predator control for Linux

USAGE
    alien <command> [args]

COMMANDS
    status                    temperatures, RPM, clocks and live power limits
    clocks                    live CPU/GPU clocks in MHz
    power status              read Linux RAPL limits and the exact OEM target
    watch [interval]          live telemetry (default 1s); Ctrl-C to stop

    fan max                   both fans to maximum
    fan auto                  both fans back to the EC curve
    fan <cpu|gpu> <percent>   manual duty for one fan, 0-100

    coolboost status|on|off   APGe state with setter readback/rollback
    keyboard-timeout status  read APGe brightness + timeout field
    keyboard-timeout 0|30    preserve brightness and set exact timeout seconds
    lcd-overdrive status|on|off   conditional getter-gated panel control

    gpu-mode status            manual read; Acer GPOC getter notifies the GPU
    gpu-mode <normal|faster|turbo> --i-accept-unsupported-gpu-overclock-risk
                               transactional OEM mode with full readback
    gpu-flag status           manual raw GPU/CPU read (not an OEM OC mode)
    gpu-flag on|off           disabled: would split the guarded OEM mode
    turbo status              deprecated read-only alias for gpu-flag status

    rgb <colour>              all four zones to one colour (#rrggbb)
    rgb zone <1-4> <colour>   colour one zone
    rgb zone <1-4> on|off     enable or disable one static zone
    rgb zones <c1> <c2> <c3> <c4>   all four zones at once
    rgb effect <name> [speed] [brightness] [colour] [ltr|rtl]
    rgb off                   backlight off
    rgb status                read the backlight state back from firmware
    rgb key <name> <colour>   EXPERIMENTAL unverified ITE transport
    rgb keys                  list the key names understood

    capabilities              what this specific machine can actually do

    profile list              show available profiles
    profile apply <name>      apply one

    json                      machine-readable status

    doctor                    check whether this machine is supported

EFFECTS
    static breath wave zoom shifting neon

    Colour applies to: static breath zoom shifting
    Neon and wave run the firmware's own palette and take no colour.
    Reverse direction applies to wave and shifting only.
    Everything except static animates, so speed 1-9 (0 would not move).
    Brightness is snapped to the PH315-53 steps: 0, 25, 50, 75, 100.

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
        ["clocks"] => clocks(),
        ["power", "status"] => power_status(),
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
            let pct: u8 = pct
                .parse()
                .map_err(|_| format!("not a percentage: {pct}"))?;
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

        ["coolboost", "status"] => {
            let enabled = open()?.coolboost().map_err(fmt)?;
            println!(
                "CoolBoost {} (getter-confirmed; PH315-53 setter reinit transient confirmed, no sustained cooling lift in controlled A/B/A)",
                if enabled { "on" } else { "off" }
            );
            Ok(())
        }
        ["coolboost", value @ ("on" | "off")] => {
            let enabled = *value == "on";
            let confirmed = open()?.set_coolboost(enabled).map_err(fmt)?;
            println!(
                "CoolBoost {} (getter-confirmed; PH315-53 setter reinit transient confirmed, no sustained cooling lift in controlled A/B/A)",
                if confirmed { "on" } else { "off" }
            );
            Ok(())
        }
        ["keyboard-timeout", "status"] => {
            let state = open()?.keyboard_timeout().map_err(fmt)?;
            println!("keyboard timeout {} s", state.seconds);
            println!("preserved APGe brightness byte {}", state.brightness);
            println!("timed optical light-off/wake behavior remains unverified");
            Ok(())
        }
        ["keyboard-timeout", seconds @ ("0" | "30")] => {
            let seconds: u8 = seconds.parse().expect("matched numeric literal");
            let state = open()?.set_keyboard_timeout(seconds).map_err(fmt)?;
            println!(
                "keyboard timeout {} s (getter-confirmed; brightness {} preserved)",
                state.seconds, state.brightness
            );
            Ok(())
        }
        ["lcd-overdrive", "status"] => {
            match open()?.lcd_overdrive().map_err(fmt)? {
                Some(enabled) => println!(
                    "LCD overdrive {} (firmware getter; panel effect unverified)",
                    if enabled { "on" } else { "off" }
                ),
                None => println!("LCD overdrive unsupported by the live firmware getter"),
            }
            Ok(())
        }
        ["lcd-overdrive", value @ ("on" | "off")] => {
            let enabled = *value == "on";
            match open()?.set_lcd_overdrive(enabled).map_err(fmt)? {
                Some(confirmed) => println!(
                    "LCD overdrive {} (getter-confirmed; panel effect unverified)",
                    if confirmed { "on" } else { "off" }
                ),
                None => return Err("LCD overdrive is not supported by this live getter".into()),
            }
            Ok(())
        }

        ["gpu-mode", "status"] => {
            println!("note: Acer's GPOC getter sends one OEM GPU notification");
            print_gpu_mode_state(open()?.gpu_mode().map_err(fmt)?);
            Ok(())
        }
        ["gpu-mode", mode @ ("normal" | "faster" | "turbo"), "--i-accept-unsupported-gpu-overclock-risk"] =>
        {
            let mode = match *mode {
                "normal" => GpuMode::Normal,
                "faster" => GpuMode::Faster,
                "turbo" => GpuMode::Turbo,
                _ => unreachable!("pattern limits GPU modes"),
            };
            let opt_in = GpuModeOptIn::acknowledge(GPU_MODE_ACKNOWLEDGEMENT)
                .expect("CLI flag maps to the exact acknowledgement");
            let state = open()?.set_gpu_mode(mode, opt_in).map_err(fmt)?;
            println!(
                "OEM GPU {} applied and getter-confirmed across NVML P0 offsets, fan table and GPOC",
                mode.label()
            );
            println!(
                "NVIDIA offsets last only for this driver lifetime; Normal explicitly writes 0/0 MHz"
            );
            println!("Acer GPOC readback sends an OEM GPU notification");
            print_gpu_mode_state(state);
            Ok(())
        }

        ["gpu-flag", "status"] | ["turbo", "status"] => {
            println!("note: Acer's raw GPU-flag getter sends one OEM GPU notification");
            let d = open()?;
            let cpu = d.overclock(OverclockTarget::Cpu).map_err(fmt)?;
            let gpu = d.overclock(OverclockTarget::Gpu).map_err(fmt)?;
            println!(
                "cpu firmware flag: {} (performance gain unproven)",
                set_flag(cpu)
            );
            println!(
                "gpu firmware flag: {} (not PredatorSense Normal/Faster/Turbo)",
                set_flag(gpu)
            );
            Ok(())
        }
        ["gpu-flag", v @ ("on" | "off")] | ["turbo", v @ ("on" | "off")] => {
            Err(format!(
                "raw GPU-flag {v} is disabled: it would change only GPOC and split NVML offsets/fan-table state; use guarded `gpu-mode normal|faster|turbo`"
            ))
        }

        ["rgb", "off"] => {
            let mut mem = alien_core::Lighting::load();
            let dev = open()?;
            dev.backlight_off(mem.zone_colours(), mem.zone_enabled)
                .map_err(fmt)?;
            mem.set_brightness(0);
            mem.save().map_err(|error| {
                format!(
                    "brightness-0 request was accepted, but lighting settings were not saved: \
                     {error}"
                )
            })?;
            println!(
                "backlight brightness 0 requested · active mode retained · optical effect unverified"
            );
            Ok(())
        }
        ["rgb", "zone", z, colour] => {
            let idx: usize = z.parse().map_err(|_| format!("not a zone: {z}"))?;
            let zone = Zone::from_index(idx.wrapping_sub(1))
                .ok_or_else(|| format!("zone must be 1-4, got {z}"))?;
            let mut mem = alien_core::Lighting::load();
            let dev = open()?;
            let active = dev.backlight().map_err(fmt)?.effect;
            if *colour == "on" || *colour == "off" {
                let previous = mem.zone_enabled;
                let mut enabled = previous;
                enabled[idx - 1] = *colour == "on";
                if active == Effect::Static {
                    dev.update_zone_enabled(mem.zone_colours(), previous, enabled)
                        .map_err(fmt)?;
                } else {
                    dev.set_zone_colours_enabled(mem.zone_colours(), enabled, mem.brightness)
                        .map_err(fmt)?;
                }
                mem.set_zone_enabled(enabled);
                save_lighting(&mem)?;
                println!(
                    "zone {idx} {} requested and saved · no firmware mask getter · optical effect unverified",
                    if enabled[idx - 1] { "on" } else { "off" }
                );
                return Ok(());
            }
            let c = Colour::parse(colour).ok_or_else(|| format!("not a colour: {colour}"))?;
            let mut colours = mem.zone_colours();
            colours[idx - 1] = c;
            let previous = mem.zone_enabled;
            let mut enabled = previous;
            enabled[idx - 1] = true;
            if active != Effect::Static {
                dev.set_zone_colours_enabled(colours, enabled, mem.brightness)
                    .map_err(fmt)?;
            } else if previous[idx - 1] {
                dev.prepare_lighting(enabled).map_err(fmt)?;
                dev.set_zone_colour(zone, c).map_err(fmt)?;
            } else {
                dev.update_zone_enabled(colours, previous, enabled)
                    .map_err(fmt)?;
            }
            mem.set_zone_colours(colours);
            mem.set_zone_enabled(enabled);
            mem.set_colour(Effect::Static, colours[0]);
            save_lighting(&mem)?;
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
                    Effect::ALL
                        .iter()
                        .map(|e| e.name())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })?;
            // Speed and direction are per-pattern in Covini's XML; colour is
            // the one shared Pattern colour used by every colour-capable mode.
            let mut mem = alien_core::Lighting::load();
            let (values, requested_direction) = split_effect_args(rest)?;
            let speed = covini_speed(
                effect,
                parse_effect_u8(&values, 0, "speed", mem.speed(effect))?,
            );
            let brightness =
                covini_brightness(parse_effect_u8(&values, 1, "brightness", mem.brightness)?);
            let colour_arg = values.get(2).copied();
            let colour = match colour_arg {
                Some(_) if !effect.honours_colour() => {
                    return Err(format!(
                        "{} uses the firmware palette and has no colour field",
                        effect.name()
                    ));
                }
                Some(value) => {
                    Colour::parse(value).ok_or_else(|| format!("not a colour: {value}"))?
                }
                None if effect == Effect::Static => mem.zone_colours()[0],
                None => mem.colour(effect),
            };
            if requested_direction.is_some() && !effect.honours_direction() {
                return Err(format!(
                    "{} has no direction setting; ltr/rtl apply to wave and shifting",
                    effect.name()
                ));
            }
            let dir = requested_direction.unwrap_or_else(|| mem.direction(effect));

            // Entering Static with no explicit colour is the OEM profile-apply
            // operation: restore all four saved colours and the saved mask.
            // Supplying a colour is Alien's explicit ALL-ALIKE convenience.
            let static_state = (effect == Effect::Static)
                .then(|| static_effect_state(&mem, colour_arg.map(|_| colour)));

            let d = open()?;
            if let Some((zones, enabled)) = static_state {
                // Static colour lives in the per-zone registers, not in the
                // effect payload.
                d.set_zone_colours_enabled(zones, enabled, brightness)
                    .map_err(fmt)?;
            } else {
                d.prepare_lighting(mem.zone_enabled).map_err(fmt)?;
                d.set_effect(effect, speed, brightness, dir, colour)
                    .map_err(fmt)?;
            }

            mem.set_speed(effect, speed);
            mem.set_direction(effect, dir);
            mem.set_brightness(brightness);
            if let Some((zones, enabled)) = static_state {
                mem.set_zone_colours(zones);
                mem.set_zone_enabled(enabled);
                mem.set_colour(Effect::Static, zones[0]);
            } else {
                mem.set_colour(effect, colour);
            }
            save_lighting(&mem)?;

            let detail = if let Some((zones, enabled)) = static_state {
                format!(
                    " · zones {}",
                    zones
                        .into_iter()
                        .zip(enabled)
                        .map(|(zone, on)| if on { zone.to_hex() } else { "off".into() })
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            } else if effect.honours_colour() {
                format!(" · {}", colour.to_hex())
            } else {
                String::new()
            };
            let verification = if effect == Effect::Static {
                ""
            } else {
                " · request accepted · optical effect unverified"
            };
            println!(
                "effect {} · speed {speed} · brightness {brightness}{detail}{verification}",
                effect.name(),
            );
            Ok(())
        }
        ["rgb", "zones", a, b, c, d] => {
            let cs = [a, b, c, d]
                .iter()
                .map(|x| Colour::parse(x).ok_or_else(|| format!("not a colour: {x}")))
                .collect::<Result<Vec<_>, _>>()?;
            let mut mem = alien_core::Lighting::load();
            let dev = open()?;
            let bright = dev
                .backlight()
                .map(|b| b.brightness)
                .unwrap_or(mem.brightness);
            dev.set_zone_colours([cs[0], cs[1], cs[2], cs[3]], bright)
                .map_err(fmt)?;
            mem.set_zone_colours([cs[0], cs[1], cs[2], cs[3]]);
            mem.set_zone_enabled([true; 4]);
            mem.set_colour(Effect::Static, cs[0]);
            mem.set_brightness(bright);
            save_lighting(&mem)?;
            for (i, z) in Zone::ALL.iter().enumerate() {
                let got = dev
                    .zone_colour(*z)
                    .map(|c| c.to_hex())
                    .unwrap_or_else(|_| "?".into());
                println!("zone {} -> {}", i + 1, got);
            }
            Ok(())
        }
        ["rgb", "keys"] => {
            println!("EXPERIMENTAL: source-mapped ITE names; transport is hardware-unverified.");
            println!("Packaged frontends intentionally have no hidraw permission rule.");
            let keys = alien_core::perkey::known_keys();
            println!("{} key names understood:", keys.len());
            for chunk in keys.chunks(12) {
                println!("  {}", chunk.join(" "));
            }
            Ok(())
        }
        ["rgb", "key", name, colour] => {
            let _ = (name, colour);
            Err("per-key writes are disabled: the source-mapped ITE path has no live hardware \
                 validation or packaged hidraw permission rule"
                .into())
        }
        ["rgb", "status"] => {
            let d = open()?;
            let b = d.backlight().map_err(fmt)?;
            println!("effect     {}", b.effect.name());
            println!("speed      {}", b.speed);
            println!("brightness {}", b.brightness);
            if b.effect == Effect::Static {
                let zones = Zone::ALL
                    .into_iter()
                    .map(|z| {
                        d.zone_colour(z)
                            .map(|c| c.to_hex())
                            .unwrap_or_else(|_| "?".into())
                    })
                    .collect::<Vec<_>>();
                println!("zones      {}", zones.join(" "));
            } else if b.effect.honours_colour() {
                println!("colour     {}", b.colour.to_hex());
            }
            let enabled = alien_core::Lighting::load().zone_enabled;
            println!(
                "saved mask {} (no firmware getter)",
                enabled
                    .into_iter()
                    .map(|on| if on { "on" } else { "off" })
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            if b.effect.honours_direction() {
                println!(
                    "direction  {}",
                    if b.reverse {
                        "right-to-left"
                    } else {
                        "left-to-right"
                    }
                );
            }
            Ok(())
        }
        ["rgb", colour] => {
            let c = Colour::parse(colour).ok_or_else(|| format!("not a colour: {colour}"))?;
            let mut mem = alien_core::Lighting::load();
            open()?
                .set_zone_colours([c; 4], mem.brightness)
                .map_err(fmt)?;
            mem.set_zone_colours([c; 4]);
            mem.set_zone_enabled([true; 4]);
            mem.set_colour(Effect::Static, c);
            save_lighting(&mem)?;
            println!("all zones -> {}", c.to_hex());
            Ok(())
        }

        ["profile", "list"] => {
            for p in profile::list() {
                println!("{:<12} {}", p.name, p.description);
            }
            Ok(())
        }
        ["profile", "apply", name] => {
            let p = profile::load(name).ok_or_else(|| format!("no profile named {name}"))?;
            let d = open()?;
            p.apply(&d).map_err(fmt)?;
            println!("applied profile: {} (GPU mode unchanged)", p.name);
            if p.deprecated_gpu_flag_ignored() {
                println!(
                    "warning: deprecated gpu_turbo/turbo field ignored; use guarded gpu-mode (raw flag is status-only)"
                );
            }
            report_fans(&d);
            Ok(())
        }

        other => Err(format!(
            "unknown command: {}\nrun `alien help`",
            other.join(" ")
        )),
    }
}

fn print_gpu_mode_state(state: GpuModeState) {
    println!(
        "P0 graphics offset  {:+} MHz (driver range {:+}..{:+})",
        state.graphics.current_mhz, state.graphics.min_mhz, state.graphics.max_mhz
    );
    println!(
        "P0 memory offset    {:+} MHz (driver range {:+}..{:+})",
        state.memory.current_mhz, state.memory.min_mhz, state.memory.max_mhz
    );
    println!("Acer fan table      {}", state.fan_table);
    println!("Acer GPOC           {}", state.gpoc);
    match state.confirmed_mode() {
        Some(mode) => println!("confirmed OEM mode  {}", mode.label()),
        None => println!("confirmed OEM mode  none (compound legs disagree)"),
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
    let power = PowercapStatus::sample();
    println!("{:<26} supported", "control");
    println!("{:-<26} {:-<11}", "", "");
    for (name, s) in c.rows() {
        let mark = match s {
            alien_core::Support::Yes => "yes",
            alien_core::Support::No => "no",
            alien_core::Support::Unverified if name == "CoolBoost protocol" => {
                "getter; PH315-53 A/B/A: no sustained lift"
            }
            alien_core::Support::Unverified
                if matches!(
                    name,
                    "30-second keyboard timeout" | "LCD overdrive protocol"
                ) =>
            {
                "getter confirmed; physical effect unverified"
            }
            alien_core::Support::Unverified => "accepted, unverified",
            alien_core::Support::Unknown if name == "raw GPU firmware flag" => {
                "manual read only; automatic notifying getter skipped"
            }
            alien_core::Support::Unknown => "unknown; getter failed",
        };
        println!("{name:<26} {mark}");
    }
    println!(
        "{:<26} {}",
        "CPU power limits (read)",
        if power.can_read_limits() { "yes" } else { "no" }
    );
    println!("{:<26} no — read-only", "CPU power limits (write)");
    if !c.misc_subindices.is_empty() {
        let list: Vec<String> = c
            .misc_subindices
            .iter()
            .map(|s| format!("{s:#04x}"))
            .collect();
        println!("\nmisc-setting sub-indices accepted: {}", list.join(" "));
    }
    for n in &c.notes {
        println!("\nnote: {n}");
    }
    println!("\npower write: {}", power.write_gap());
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

fn save_lighting(lighting: &alien_core::Lighting) -> Result<(), String> {
    lighting
        .save()
        .map_err(|error| format!("hardware changed, but lighting settings were not saved: {error}"))
}

/// Remove direction tokens from the positional effect arguments.
///
/// Keeping this parser separate makes malformed speed/brightness fail before
/// `Device::open`: `rgb effect wave garbage` must not silently resend a
/// remembered value and report success.
fn split_effect_args<'a>(rest: &'a [&'a str]) -> Result<(Vec<&'a str>, Option<Direction>), String> {
    let mut values = Vec::new();
    let mut direction = None;
    for value in rest {
        let parsed_direction = match *value {
            "ltr" | "forward" => Some(Direction::LeftToRight),
            "rtl" | "reverse" => Some(Direction::RightToLeft),
            _ => None,
        };
        if let Some(parsed) = parsed_direction {
            if direction.is_some_and(|existing| existing != parsed) {
                return Err("choose only one direction: ltr or rtl".into());
            }
            direction = Some(parsed);
        } else {
            values.push(*value);
        }
    }
    if values.len() > 3 {
        return Err(format!(
            "too many effect values; expected speed, brightness and optional colour, got {}",
            values.len()
        ));
    }
    Ok((values, direction))
}

fn parse_effect_u8(values: &[&str], index: usize, label: &str, default: u8) -> Result<u8, String> {
    values.get(index).map_or(Ok(default), |value| {
        value
            .parse::<u8>()
            .map_err(|_| format!("{label} must be an integer from 0 to 255, got {value}"))
    })
}

fn static_effect_state(
    lighting: &alien_core::Lighting,
    explicit: Option<Colour>,
) -> ([Colour; 4], [bool; 4]) {
    explicit
        .map(|colour| ([colour; 4], [true; 4]))
        .unwrap_or_else(|| (lighting.zone_colours(), lighting.zone_enabled))
}

fn parse_fan(s: &str) -> Result<Fan, String> {
    match s {
        "cpu" => Ok(Fan::Cpu),
        "gpu" => Ok(Fan::Gpu),
        _ => Err(format!("fan must be cpu or gpu, got {s}")),
    }
}

fn set_flag(v: u8) -> &'static str {
    match v {
        0 => "off",
        2 => "set",
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
        if settled(prev.cpu_fan_rpm, now.cpu_fan_rpm) && settled(prev.gpu_fan_rpm, now.gpu_fan_rpm)
        {
            println!(
                "cpu fan: {}   gpu fan: {}",
                rpm(now.cpu_fan_rpm),
                rpm(now.gpu_fan_rpm)
            );
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

fn explicitly_unsupported(error: &alien_core::Error) -> bool {
    matches!(
        error,
        alien_core::Error::Transport(
            alien_core::transport::TransportError::FirmwareStatus { .. }
                | alien_core::transport::TransportError::UnsupportedEndpoint(_)
        )
    )
}

fn typed_feature_value<T>(result: alien_core::Result<T>) -> (Option<bool>, Option<T>) {
    match result {
        Ok(value) => (Some(true), Some(value)),
        Err(error) if explicitly_unsupported(&error) => (Some(false), None),
        Err(_) => (None, None),
    }
}

fn status(d: &Device) -> Result<(), String> {
    let s = d.sensors();
    let p = Performance::sample();
    let power = PowercapStatus::sample();
    let cpu_oc = d.overclock(OverclockTarget::Cpu).ok();
    println!("interface   {}", d.method_path());
    println!("cpu temp    {}", temp(s.cpu_temp_c));
    println!("gpu temp    {}", temp(s.gpu_temp_c));
    println!("board temp  {}", temp(s.system_temp_c));
    println!("cpu fan     {}", rpm(s.cpu_fan_rpm));
    println!("gpu fan     {}", rpm(s.gpu_fan_rpm));
    println!(
        "coolboost   {}",
        match d.coolboost() {
            Ok(true) => "on (getter; PH315-53 A/B/A found no sustained cooling lift)",
            Ok(false) => "off (getter; PH315-53 A/B/A found no sustained cooling lift)",
            Err(error) if explicitly_unsupported(&error) => "unsupported",
            Err(_) => "unknown (getter failed)",
        }
    );
    println!(
        "kb timeout  {}",
        match d.keyboard_timeout() {
            Ok(state) => format!("{} s (brightness byte {})", state.seconds, state.brightness),
            Err(error) if explicitly_unsupported(&error) => "unsupported".into(),
            Err(_) => "unknown (getter failed)".into(),
        }
    );
    println!(
        "lcd od      {}",
        match d.lcd_overdrive() {
            Ok(Some(true)) => "on (getter-confirmed field; panel effect unverified)",
            Ok(Some(false)) => "off (getter-confirmed field; panel effect unverified)",
            Ok(None) => "unsupported",
            Err(error) if explicitly_unsupported(&error) => "unsupported",
            Err(_) => "unknown (getter failed)",
        }
    );
    println!(
        "cpu flag    {} (gain unproven)",
        cpu_oc.map(set_flag).unwrap_or("unavailable")
    );
    println!("gpu mode    not sampled (explicit `alien gpu-mode status` sends a GPU notification)");
    println!("cpu clock   {}", mhz(p.cpu_mhz, p.cpu_max_mhz));
    println!("gpu clock   {}", mhz(p.gpu_mhz, p.gpu_max_mhz));
    println!("gpu load    {}", percent(p.gpu_usage_pct));
    println!("cpu power   {}", power_brief(&power));
    if let Ok(b) = d.backlight() {
        let mut detail = format!(
            "{} · brightness {} · speed {}",
            b.effect.name(),
            b.brightness,
            b.speed
        );
        if b.effect == Effect::Static {
            let zones = Zone::ALL
                .into_iter()
                .map(|z| {
                    d.zone_colour(z)
                        .map(|c| c.to_hex())
                        .unwrap_or_else(|_| "?".into())
                })
                .collect::<Vec<_>>();
            detail.push_str(&format!(" · zones {}", zones.join(" ")));
        } else if b.effect.honours_colour() {
            detail.push_str(&format!(" · {}", b.colour.to_hex()));
        }
        if b.effect.honours_direction() && b.reverse {
            detail.push_str(" · reversed");
        }
        println!("backlight   {detail}");
    }
    Ok(())
}

fn clocks() -> Result<(), String> {
    let p = Performance::sample();
    println!("cpu clock   {}", mhz(p.cpu_mhz, p.cpu_max_mhz));
    println!("gpu clock   {}", mhz(p.gpu_mhz, p.gpu_max_mhz));
    println!("gpu load    {}", percent(p.gpu_usage_pct));
    Ok(())
}

fn power_status() -> Result<(), String> {
    let status = PowercapStatus::sample();
    let vendor = status.dmi_vendor.as_deref().unwrap_or("unknown vendor");
    let product = status
        .dmi_product_name
        .as_deref()
        .unwrap_or("unknown model");
    println!("model       {vendor} {product}");
    println!(
        "OEM target  PL1 {} · PL2 {} · PL1 window {}",
        microwatts(PH315_53_CPU_POWER.pl1_uw),
        microwatts(PH315_53_CPU_POWER.pl2_uw),
        microseconds(PH315_53_CPU_POWER.pl1_time_window_us),
    );
    println!("OEM modes   Normal = Fast = Turbo for CPU settings");
    println!("PL2 enable  not separately exposed by Linux powercap");

    match &status.package {
        Some(package) => {
            println!(
                "backend     {} ({})",
                package.kernel_name,
                package.sysfs_path.display()
            );
            println!(
                "zone        {}",
                match package.enabled {
                    Some(true) => "enabled",
                    Some(false) => "disabled",
                    None => "enabled state unavailable",
                }
            );
            print_constraint("live PL1", package.pl1.as_ref());
            print_constraint("live PL2", package.pl2.as_ref());
        }
        None => println!("backend     unavailable — no Intel package powercap zone"),
    }
    println!("write       read-only — {}", status.write_gap());
    Ok(())
}

fn print_constraint(label: &str, constraint: Option<&PowerConstraint>) {
    match constraint {
        Some(c) => {
            let time = c
                .time_window_us
                .map(microseconds)
                .map(|value| format!(" · window {value}"))
                .unwrap_or_default();
            println!(
                "{label:<11} {} · name {} · index {}{}",
                c.power_limit_uw
                    .map(microwatts)
                    .unwrap_or_else(|| "unreadable".into()),
                c.kernel_name,
                c.index,
                time,
            );
        }
        None => println!("{label:<11} unavailable — no recognized named constraint"),
    }
}

fn power_brief(status: &PowercapStatus) -> String {
    let Some(package) = &status.package else {
        return "unavailable (read-only; run alien power status)".into();
    };
    let pl1 = package
        .pl1
        .as_ref()
        .and_then(|constraint| constraint.power_limit_uw)
        .map(microwatts)
        .unwrap_or_else(|| "PL1 unavailable".into());
    let pl2 = package
        .pl2
        .as_ref()
        .and_then(|constraint| constraint.power_limit_uw)
        .map(microwatts)
        .unwrap_or_else(|| "PL2 unavailable".into());
    format!("PL1 {pl1} · PL2 {pl2} (read-only)")
}

fn microwatts(value: u64) -> String {
    if value.checked_rem(1_000_000) == Some(0) {
        format!("{} W", value / 1_000_000)
    } else {
        format!("{:.3} W", value as f64 / 1_000_000.0)
    }
}

fn microseconds(value: u64) -> String {
    if value.checked_rem(1_000_000) == Some(0) {
        format!("{} s", value / 1_000_000)
    } else {
        format!("{:.3} s", value as f64 / 1_000_000.0)
    }
}

fn mhz(current: Option<u32>, maximum: Option<u32>) -> String {
    match (current, maximum) {
        (Some(now), Some(max)) => format!("{now} / {max} MHz"),
        (Some(now), None) => format!("{now} MHz"),
        (None, Some(max)) => format!("— / {max} MHz"),
        (None, None) => "—".into(),
    }
}

fn percent(value: Option<u8>) -> String {
    value.map(|v| format!("{v}%")).unwrap_or_else(|| "—".into())
}

fn json(d: &Device) -> Result<(), String> {
    let s = d.sensors();
    let p = Performance::sample();
    let power = PowercapStatus::sample();
    let pl1 = power
        .package
        .as_ref()
        .and_then(|package| package.pl1.as_ref())
        .and_then(|constraint| constraint.power_limit_uw);
    let pl2 = power
        .package
        .as_ref()
        .and_then(|package| package.pl2.as_ref())
        .and_then(|constraint| constraint.power_limit_uw);
    let pl1_window = power
        .package
        .as_ref()
        .and_then(|package| package.pl1.as_ref())
        .and_then(|constraint| constraint.time_window_us);
    let cpu_flag = d.overclock(OverclockTarget::Cpu).ok();
    // Function 23 selector 5 sends an OEM GPU notification. JSON status is
    // commonly polled, so leave this field null instead of turning telemetry
    // into a hidden notification loop; `gpu-mode status` is the explicit read.
    let gpu_flag: Option<u8> = None;
    let (coolboost_supported, coolboost) = typed_feature_value(d.coolboost());
    let (keyboard_timeout_supported, keyboard_timeout) = typed_feature_value(d.keyboard_timeout());
    let (lcd_overdrive_supported, lcd_overdrive) = match d.lcd_overdrive() {
        Ok(Some(state)) => (Some(true), Some(state)),
        Ok(None) => (Some(false), None),
        Err(error) if explicitly_unsupported(&error) => (Some(false), None),
        Err(_) => (None, None),
    };
    println!(
        r#"{{"cpu_temp_c":{},"gpu_temp_c":{},"system_temp_c":{},"cpu_fan_rpm":{},"gpu_fan_rpm":{},"coolboost_supported":{},"coolboost_protocol_state":{},"keyboard_timeout_supported":{},"keyboard_timeout_seconds":{},"keyboard_brightness_byte":{},"lcd_overdrive_supported":{},"lcd_overdrive_protocol_state":{},"cpu_firmware_flag":{},"gpu_firmware_flag":{},"gpu_firmware_flag_sampled":false,"gpu_firmware_flag_is_oem_oc_mode":false,"cpu_mhz":{},"cpu_max_mhz":{},"gpu_mhz":{},"gpu_max_mhz":{},"gpu_usage_pct":{},"cpu_pl1_uw":{},"cpu_pl2_uw":{},"cpu_pl1_time_window_us":{},"cpu_power_write_supported":false}}"#,
        opt(s.cpu_temp_c),
        opt(s.gpu_temp_c),
        opt(s.system_temp_c),
        opt(s.cpu_fan_rpm),
        opt(s.gpu_fan_rpm),
        opt(coolboost_supported),
        opt(coolboost),
        opt(keyboard_timeout_supported),
        opt(keyboard_timeout.map(|state| state.seconds)),
        opt(keyboard_timeout.map(|state| state.brightness)),
        opt(lcd_overdrive_supported),
        opt(lcd_overdrive),
        opt(cpu_flag),
        opt(gpu_flag),
        opt(p.cpu_mhz),
        opt(p.cpu_max_mhz),
        opt(p.gpu_mhz),
        opt(p.gpu_max_mhz),
        opt(p.gpu_usage_pct),
        opt(pl1),
        opt(pl2),
        opt(pl1_window),
    );
    Ok(())
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "null".into())
}

fn watch(d: &Device, secs: f64) -> Result<(), String> {
    let period = std::time::Duration::from_secs_f64(secs.max(0.1));
    println!("  cpu °C   gpu °C   sys °C   cpu RPM   gpu RPM");
    loop {
        let s = d.sensors();
        println!(
            "{:>8} {:>8} {:>8} {:>9} {:>9}",
            s.cpu_temp_c
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
            s.gpu_temp_c
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
            s.system_temp_c
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
            s.cpu_fan_rpm
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
            s.gpu_fan_rpm
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into()),
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
    let read = |p: &str| {
        std::fs::read_to_string(p)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".into())
    };
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
        if guid_present {
            "present"
        } else {
            "NOT FOUND — this model may not be supported"
        }
    );

    let acpi_call = std::path::Path::new("/proc/acpi/call").exists();
    println!(
        "acpi_call   {}",
        if acpi_call {
            "loaded"
        } else {
            "MISSING — run: modprobe acpi_call"
        }
    );
    let power = PowercapStatus::sample();
    println!(
        "powercap    {}",
        if power.can_read_limits() {
            "named PL1/PL2 readable (write disabled)"
        } else {
            "named PL1/PL2 unavailable"
        }
    );
    println!("power gap  {}", power.write_gap());

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_arguments_keep_positions_but_extract_direction() {
        let (values, direction) = split_effect_args(&["5", "75", "ltr", "#123456"]).unwrap();
        assert_eq!(values, ["5", "75", "#123456"]);
        assert_eq!(direction, Some(Direction::LeftToRight));
        assert!(split_effect_args(&["ltr", "rtl"]).is_err());
        assert!(split_effect_args(&["1", "2", "3", "4"]).is_err());
    }

    #[test]
    fn supplied_effect_numbers_are_never_silently_ignored() {
        assert_eq!(parse_effect_u8(&[], 0, "speed", 5).unwrap(), 5);
        assert_eq!(parse_effect_u8(&["9"], 0, "speed", 5).unwrap(), 9);
        assert!(parse_effect_u8(&["garbage"], 0, "speed", 5).is_err());
        assert!(parse_effect_u8(&["999"], 0, "speed", 5).is_err());
    }

    #[test]
    fn static_without_explicit_colour_restores_saved_zones_and_mask() {
        let mut lighting = alien_core::Lighting::default();
        let zones = [
            Colour::new(1, 2, 3),
            Colour::new(4, 5, 6),
            Colour::new(7, 8, 9),
            Colour::new(10, 11, 12),
        ];
        let enabled = [true, false, true, false];
        lighting.set_zone_colours(zones);
        lighting.set_zone_enabled(enabled);
        assert_eq!(static_effect_state(&lighting, None), (zones, enabled));

        let all_alike = Colour::new(0xaa, 0xbb, 0xcc);
        assert_eq!(
            static_effect_state(&lighting, Some(all_alike)),
            ([all_alike; 4], [true; 4])
        );
    }
}
