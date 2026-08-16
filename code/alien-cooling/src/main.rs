//! `alien-cooling` — the temperature-driven fan controller.
//!
//! # What this is
//!
//! An unprivileged client of `alien-daemon`. It samples telemetry, runs the
//! control law in [`alien_core::cooling`], and writes per-fan duty when — and
//! only when — the decision changes. It touches no firmware directly and needs
//! no capabilities: membership of group `alien` is the whole privilege story.
//!
//! # Why it is a separate process
//!
//! `alien-daemon` is deliberately a broker. It owns `/proc/acpi/call` behind a
//! mutex because that file is a single global kernel buffer, and it holds
//! `CAP_SYS_ADMIN` for the NVML clock path. Putting a policy loop inside it
//! would mean a bug in a fan curve running inside the one process allowed to
//! call arbitrary SMM methods. The controller therefore lives out here, behind
//! the same socket and the same policy allowlist as every other client.
//!
//! # The boot-order trap
//!
//! [`Device::open`] falls back to direct `/proc/acpi/call` when the socket is
//! absent. For a long-running service started at boot that is a footgun: if
//! this process wins the race it takes the `flock` at `/run/alien/daemon.lock`
//! and `alien-daemon` then fails to start with `InterfaceBusy`. So the socket
//! is required explicitly and the fallback never runs — see [`connect`].
//!
//! # Restoring on the way out
//!
//! There is no EC watchdog on this hardware. thinkfan can lean on the ThinkPad
//! EC's own 120-second timer; `acer-wmi` has no equivalent. If this process
//! dies while holding the fans in manual, nothing in firmware winds them back —
//! they stay where they were set, indefinitely.
//!
//! Restore therefore has two layers, and it is worth being exact about why not
//! three. The obvious third — a [`Drop`] guard — **does not work here**: the
//! workspace release profile sets `panic = "abort"`, so an unwinding restore
//! would silently never run in exactly the builds that ship. Claiming it would
//! be worse than not having it. What actually runs:
//!
//! 1. a signal handler for `SIGTERM`/`SIGINT`, which is the normal stop path,
//! 2. `ExecStopPost` in the unit, which covers abort, `SIGKILL` and panic.
//!
//! Layer 2 is the one that matters, because it is the only one that survives
//! the process not getting to run any more code.
//!
//! And the process asks to be un-killable by the OOM killer, refusing to start
//! if that cannot be arranged — being killed is precisely the failure mode with
//! no recovery path.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use alien_core::cooling::{Action, Controller, Curve, Reading, Reason};
#[cfg(unix)]
use alien_core::socket::SocketClient;
use alien_core::{Device, Fan, FanBehaviour};

/// How often telemetry is sampled.
///
/// Deliberately faster than the fans can move. Sampling and actuation are
/// different rates: reading costs five socket round trips and improves the
/// estimate, while writing has to respect an 8-10 second mechanical settle.
/// The dwell timers in [`alien_core::cooling`] gate the writes; this only gates
/// the reads.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// Reconnect backoff after the daemon goes away, matching the GUI's poller.
const RETRY: Duration = Duration::from_secs(3);

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {}
        ["-V" | "--version" | "version"] => {
            println!("alien-cooling {}", env!("CARGO_PKG_VERSION"));
            return std::process::ExitCode::SUCCESS;
        }
        ["--dry-run"] => return run(true, false),
        // Running by hand as an ordinary user cannot lower oom_score_adj, so
        // the refusal below would make manual use impossible. You may opt out
        // — but you have to say so, rather than discovering later that the
        // guarantee was never in force.
        ["--allow-oom-kill"] => return run(false, true),
        _ => {
            eprintln!("usage: alien-cooling [--dry-run|--allow-oom-kill|--version]");
            return std::process::ExitCode::FAILURE;
        }
    }
    run(false, false)
}

fn run(dry: bool, allow_oom: bool) -> std::process::ExitCode {
    let curve = Curve::default();
    if let Err(e) = curve.validate() {
        eprintln!("alien-cooling: built-in curve is invalid: {e}");
        return std::process::ExitCode::FAILURE;
    }

    // Refuse to run rather than run OOM-killable. A controller that can be
    // reaped while holding the fans in manual leaves them there forever.
    if !dry && !allow_oom {
        if let Err(e) = protect_from_oom() {
            eprintln!("alien-cooling: refusing to start: {e}");
            eprintln!(
                "  This process must not be OOM-killable: there is no EC watchdog on this\n\
                   hardware, so being killed while holding the fans in manual would leave\n\
                   them there indefinitely. The packaged unit sets OOMScoreAdjust=-1000.\n\
                   To run by hand anyway, pass --allow-oom-kill and accept that risk."
            );
            return std::process::ExitCode::FAILURE;
        }
    }
    if allow_oom {
        log("WARNING: --allow-oom-kill; a kill here leaves the fans wherever they were set");
    }

    install_signal_handlers();

    log(&format!(
        "starting; sampling every {}s, curve floor {}%, board override {} C, critical {} C{}",
        SAMPLE_INTERVAL.as_secs(),
        curve.floor_duty,
        curve.board_override_c,
        curve.critical_c,
        if dry { " (DRY RUN — no writes)" } else { "" }
    ));

    let mut controller = Controller::new(curve);
    let mut device: Option<Device> = None;

    while !stopping() {
        let dev = match device {
            Some(ref d) => d,
            None => match connect() {
                Ok(d) => {
                    log("connected to alien-daemon");
                    device = Some(d);
                    device.as_ref().expect("just set")
                }
                Err(e) => {
                    log(&format!("waiting for alien-daemon: {e}"));
                    sleep_interruptible(RETRY);
                    continue;
                }
            },
        };

        match dev.try_sensors() {
            Ok(sensors) => {
                let reading = Reading {
                    cpu_c: sensors.cpu_temp_c,
                    gpu_c: sensors.gpu_temp_c,
                    board_c: sensors.system_temp_c,
                    cpu_rpm: sensors.cpu_fan_rpm,
                    gpu_rpm: sensors.gpu_fan_rpm,
                };
                let decision = controller.step(reading, Instant::now());
                if decision.changed {
                    log(&format!(
                        "{} -> {} (cpu {} C, gpu {} C, board {} C)",
                        describe(decision.reason),
                        describe_action(decision.action),
                        opt(reading.cpu_c),
                        opt(reading.gpu_c),
                        opt(reading.board_c),
                    ));
                    if !dry {
                        if let Err(e) = apply(dev, decision.action) {
                            log(&format!("apply failed: {e}"));
                            // A firmware rejection is survivable and the socket
                            // stays usable; a transport failure is not.
                            if e.is_link_lost() {
                                device = None;
                                continue;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                if e.is_link_lost() {
                    log("alien-daemon went away; reconnecting");
                    device = None;
                    continue;
                }
                log(&format!("sensor read failed: {e}"));
            }
        }

        sleep_interruptible(SAMPLE_INTERVAL);
    }

    log("stopping; handing fans back to maximum");
    if !dry {
        restore_max();
    }
    std::process::ExitCode::SUCCESS
}

/// Connect over the socket, never falling back to direct firmware access.
///
/// `Device::open` would silently take `/proc/acpi/call` if the socket were
/// missing, which for a boot-time service means racing `alien-daemon` for the
/// lock and winning. Constructing the transport directly makes that impossible.
#[cfg(unix)]
fn connect() -> Result<Device, String> {
    let client = SocketClient::connect().map_err(|e| e.to_string())?;
    Ok(Device::with_transport(Box::new(client)))
}

/// The controller has no Windows story yet, and should say so plainly.
///
/// Not a stub for its own sake: the whole binary type-checks for Windows, so
/// this is the single point where a WMI transport would be constructed instead.
/// The control law in `alien_core::cooling` is already portable and its tests
/// already run on any target.
#[cfg(not(unix))]
fn connect() -> Result<Device, String> {
    Err("alien-cooling needs the alien-daemon socket, which is POSIX-only. A \
         Windows build would construct a WMI transport here instead."
        .into())
}

fn apply(dev: &Device, action: Action) -> alien_core::device::Result<()> {
    match action {
        Action::Max => dev.set_fan_behaviour(FanBehaviour::Max),
        Action::Duty(pct) => {
            dev.set_fan_percent(Fan::Cpu, pct)?;
            dev.set_fan_percent(Fan::Gpu, pct)
        }
    }
}

/// Last-resort restore. Best effort by definition — if this fails there is
/// nothing further to try, and saying so is more useful than a silent exit.
fn restore_max() {
    match connect().and_then(|d| d.fans_max().map_err(|e| e.to_string())) {
        Ok(()) => log("fans restored to maximum"),
        Err(e) => log(&format!(
            "COULD NOT RESTORE FANS: {e} — run `alien fan max` manually"
        )),
    }
}

/// Ask the kernel not to OOM-kill this process.
///
/// `oom_score_adj` is writable down to -1000 only with `CAP_SYS_RESOURCE` or as
/// root; under systemd the unit sets `OOMScoreAdjust=-1000` and this call then
/// merely confirms it. Reading it back is the point — the write succeeding is
/// not proof, since a restricted process can be silently clamped.
fn protect_from_oom() -> Result<(), String> {
    const PATH: &str = "/proc/self/oom_score_adj";
    let current = std::fs::read_to_string(PATH)
        .map_err(|e| format!("cannot read {PATH}: {e}"))?
        .trim()
        .parse::<i32>()
        .map_err(|e| format!("cannot parse {PATH}: {e}"))?;
    if current <= -900 {
        return Ok(());
    }
    std::fs::write(PATH, b"-1000")
        .map_err(|e| format!("cannot lower oom_score_adj (currently {current}): {e}"))?;
    let after = std::fs::read_to_string(PATH)
        .map_err(|e| format!("cannot re-read {PATH}: {e}"))?
        .trim()
        .parse::<i32>()
        .map_err(|e| format!("cannot parse {PATH}: {e}"))?;
    if after > -900 {
        return Err(format!(
            "oom_score_adj is {after} after writing -1000; the write was clamped"
        ));
    }
    Ok(())
}

/// Set on `SIGTERM`/`SIGINT`. A `static` rather than a captured `Arc` because a
/// signal handler is a bare `extern "C"` function with no environment.
static STOP: AtomicBool = AtomicBool::new(false);

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

/// Ask for the fans back on the way out.
///
/// A raw `signal(2)` handler rather than a crate: this binary is vendored into
/// six packaging formats and every dependency there is a licence review. The
/// handler does nothing but one relaxed atomic store, which is
/// async-signal-safe — the restore itself happens on the main thread, because
/// writing to a socket from a signal handler is not.
fn install_signal_handlers() {
    extern "C" fn handler(_sig: i32) {
        STOP.store(true, Ordering::SeqCst);
    }
    let f = handler as extern "C" fn(i32) as *const () as usize;
    unsafe {
        signal_raw(SIGTERM, f);
        signal_raw(SIGINT, f);
    }
}

unsafe extern "C" {
    #[link_name = "signal"]
    fn signal_raw(sig: i32, handler: usize) -> usize;
}

fn stopping() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Sleep, but wake early when asked to stop, so shutdown is not gated on the
/// sample interval.
fn sleep_interruptible(total: Duration) {
    const TICK: Duration = Duration::from_millis(200);
    let end = Instant::now() + total;
    while Instant::now() < end {
        if stopping() {
            return;
        }
        std::thread::sleep(TICK.min(end.saturating_duration_since(Instant::now())));
    }
}

fn describe(r: Reason) -> &'static str {
    match r {
        Reason::Priming => "priming",
        Reason::Curve => "curve",
        Reason::BoardSaturation => "board saturating",
        Reason::Critical => "CRITICAL",
        Reason::TachLost => "TACHOMETER LOST",
        Reason::SensorsLost => "sensors lost",
    }
}

fn describe_action(a: Action) -> String {
    match a {
        Action::Max => "maximum".into(),
        Action::Duty(p) => format!("{p}%"),
    }
}

fn opt(v: Option<u16>) -> String {
    v.map_or_else(|| "--".into(), |v| v.to_string())
}

/// Timestamped line to stderr, which is where the journal picks it up.
fn log(msg: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "alien-cooling: {msg}");
}
