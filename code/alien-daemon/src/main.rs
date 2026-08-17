//! `alien-daemon` — the one process allowed to touch the firmware.
//!
//! # Why a daemon at all
//!
//! Two reasons, and only the first is the obvious one.
//!
//! **Access.** `/proc/acpi/call` is root-only, and a flatpak or snap build
//! cannot reach it from inside its sandbox at any privilege level. Running a
//! Wayland GUI as root to work around that is worse than the problem. So the
//! privileged part is this: a few hundred lines with no UI, no network, and no
//! dependencies.
//!
//! **Correctness.** `/proc/acpi/call` is a *single global kernel buffer*, and
//! the write-then-read sequence is not atomic. Two processes calling
//! concurrently interleave — A writes its request, B writes its request, A
//! reads B's answer. Nothing returns an error; A just gets a well-formed
//! reading for a question it did not ask. A single owner holding a mutex is
//! the only way to make concurrent use safe, and every Alien frontend
//! therefore goes through here.
//!
//! # Trust model
//!
//! The socket lives at `/run/alien/alien.sock`, owned `root:alien`, mode 0660.
//! Membership of group `alien` is the privilege boundary, so it must be granted
//! as deliberately as `sudo` — a member can spin the fans, set the keyboard
//! colour and, on the exact PH315-53 target, request unsupported privileged
//! Nvidia clock offsets. The GPU acknowledgement prevents accidental profile/UI
//! writes; it is not authentication and does not weaken the group trust boundary.
//! The service retains `CAP_SYS_ADMIN` because the matching Nvidia 595.71.05
//! open kernel driver gates privileged callers on that capability; all other
//! service hardening remains in force.
//!
//! What group membership deliberately does **not** grant is arbitrary firmware
//! access: every request is checked against [`alien_core::policy`], which
//! allowlists the functions Alien uses and refuses the persistent-CMOS
//! sub-index. Without that this would be a "call any SMM method as root"
//! service wearing a fan-control costume.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alien_core::policy::{self, Verdict};
use alien_core::socket::{encode_gpu_mode_state, encode_hex, parse_request, socket_path};
use alien_core::transport::{AcpiCall, Transport, TransportError};
use alien_core::{GpuMode, GpuModeOptIn, GPU_MODE_ACKNOWLEDGEMENT};

const TYPED_MUTATION_INTERVAL: Duration = Duration::from_millis(100);
const NOTIFYING_GPU_GET_INTERVAL: Duration = Duration::from_secs(1);

fn admit_after_interval(previous: &mut Option<Instant>, now: Instant, interval: Duration) -> bool {
    if previous.is_some_and(|instant| now.saturating_duration_since(instant) < interval) {
        return false;
    }
    *previous = Some(now);
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonCommand {
    Run,
    Version,
}

fn parse_daemon_args(args: &[&str]) -> Result<DaemonCommand, &'static str> {
    match args {
        [] => Ok(DaemonCommand::Run),
        ["-V" | "--version" | "version"] => Ok(DaemonCommand::Version),
        _ => Err("usage: alien-daemon [--version]"),
    }
}

fn main() -> std::process::ExitCode {
    // Parse every argument before opening acpi_call or touching the socket.
    // This matters for a privileged service: even a harmless-looking query
    // such as `--version` must be guaranteed side-effect free.
    let raw_args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let Some(args) = raw_args
        .iter()
        .map(|arg| arg.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        eprintln!("alien-daemon: usage: alien-daemon [--version]");
        return std::process::ExitCode::FAILURE;
    };

    match parse_daemon_args(&args) {
        Ok(DaemonCommand::Version) => {
            println!("alien-daemon {}", env!("CARGO_PKG_VERSION"));
            std::process::ExitCode::SUCCESS
        }
        Err(usage) => {
            eprintln!("alien-daemon: {usage}");
            std::process::ExitCode::FAILURE
        }
        Ok(DaemonCommand::Run) => match run() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("alien-daemon: {e}");
                std::process::ExitCode::FAILURE
            }
        },
    }
}

fn run() -> Result<(), String> {
    let transport = AcpiCall::detect().map_err(|e| {
        format!("cannot reach the firmware: {e}\nis the acpi_call module loaded, and is this an Acer gaming machine?")
    })?;
    eprintln!("alien-daemon: firmware via {}", transport.describe());

    let path = socket_path();
    let dir = path
        .parent()
        .ok_or_else(|| format!("socket path {} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    // AcpiCall::detect holds the cross-process interface lock for the lifetime
    // of `transport`. With that lock held, this can only be a stale pathname;
    // no live Alien daemon or supported direct caller can be behind it.
    let _ = std::fs::remove_file(&path);
    let listener =
        UnixListener::bind(&path).map_err(|e| format!("cannot bind {}: {e}", path.display()))?;

    // 0660 before anyone can connect. Set after bind because bind() applies the
    // process umask, which would otherwise decide our permissions for us.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))
        .map_err(|e| format!("cannot chmod {}: {e}", path.display()))?;
    match chown_to_alien_group(&path) {
        Ok(gid) => eprintln!(
            "alien-daemon: listening on {} (group alien, gid {gid})",
            path.display()
        ),
        Err(e) => eprintln!(
            "alien-daemon: listening on {} — WARNING: {e}\n\
             the socket is root-only, so unprivileged clients cannot connect.",
            path.display()
        ),
    }

    // The mutex is the whole point: it serialises the non-atomic
    // write-then-read against the global kernel buffer.
    let firmware = Arc::new(Mutex::new(transport));
    let last_typed_mutation = Arc::new(Mutex::new(None));
    let last_notifying_gpu_get = Arc::new(Mutex::new(None));

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let fw = Arc::clone(&firmware);
                let mutation_gate = Arc::clone(&last_typed_mutation);
                let gpu_get_gate = Arc::clone(&last_notifying_gpu_get);
                // One thread per client. A GUI polling telemetry must not be
                // blocked behind another client's slow call, and there are
                // never more than a handful of these.
                std::thread::spawn(move || {
                    if let Err(e) = serve(stream, fw, mutation_gate, gpu_get_gate) {
                        eprintln!("alien-daemon: client dropped: {e}");
                    }
                });
            }
            Err(e) => eprintln!("alien-daemon: accept failed: {e}"),
        }
    }
    Ok(())
}

fn serve(
    stream: UnixStream,
    firmware: Arc<Mutex<AcpiCall>>,
    last_typed_mutation: Arc<Mutex<Option<Instant>>>,
    last_notifying_gpu_get: Arc<Mutex<Option<Instant>>>,
) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    const MAX_REQUEST_LINE: usize = 96;
    loop {
        // Bound allocation before parsing. Every valid request is at most an
        // eight-byte payload plus a tiny ASCII envelope; an unbounded
        // `BufRead::lines()` lets any group member force the root daemon to
        // allocate until newline or OOM.
        let mut raw = Vec::with_capacity(MAX_REQUEST_LINE + 1);
        let n = (&mut reader)
            .take((MAX_REQUEST_LINE + 1) as u64)
            .read_until(b'\n', &mut raw)?;
        if n == 0 {
            break;
        }
        if raw.len() > MAX_REQUEST_LINE {
            if raw.last() != Some(&b'\n') {
                drain_to_newline(&mut reader)?;
            }
            writeln!(out, "ERR request line exceeds {MAX_REQUEST_LINE} bytes")?;
            out.flush()?;
            continue;
        }
        while matches!(raw.last(), Some(b'\n' | b'\r')) {
            raw.pop();
        }
        let Ok(line) = std::str::from_utf8(&raw) else {
            writeln!(out, "ERR request must be ASCII/UTF-8")?;
            out.flush()?;
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }

        let reply = if line.starts_with("FEATURE ") {
            match parse_feature_request(line) {
                None => "ERR unintelligible typed feature request".into(),
                Some(request) => {
                    if request.is_notifying_gpu_get() {
                        let mut previous = last_notifying_gpu_get
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if !admit_after_interval(
                            &mut previous,
                            Instant::now(),
                            NOTIFYING_GPU_GET_INTERVAL,
                        ) {
                            eprintln!(
                                "alien-daemon: rate-limited notifying typed getter {}",
                                request.audit_name()
                            );
                            writeln!(
                                out,
                                "ERR notifying GPU-mode reads must be at least {} ms apart",
                                NOTIFYING_GPU_GET_INTERVAL.as_millis()
                            )?;
                            out.flush()?;
                            continue;
                        }
                        eprintln!(
                            "alien-daemon: notifying typed getter requested: {}",
                            request.audit_name()
                        );
                    }
                    if request.is_mutation() {
                        let mut previous = last_typed_mutation
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if previous
                            .is_some_and(|instant| instant.elapsed() < TYPED_MUTATION_INTERVAL)
                        {
                            eprintln!(
                                "alien-daemon: rate-limited typed mutation {}",
                                request.audit_name()
                            );
                            writeln!(
                                out,
                                "ERR typed feature mutations must be at least {} ms apart",
                                TYPED_MUTATION_INTERVAL.as_millis()
                            )?;
                            out.flush()?;
                            continue;
                        }
                        *previous = Some(Instant::now());
                        eprintln!(
                            "alien-daemon: typed mutation requested: {}",
                            request.audit_name()
                        );
                    }

                    match typed_feature(request, &firmware) {
                        Ok(response) => {
                            if request.is_notifying_gpu_get() {
                                eprintln!(
                                    "alien-daemon: notifying typed getter completed: {}",
                                    request.audit_name()
                                );
                            }
                            if request.is_mutation() {
                                if response.is_empty() {
                                    // The only empty successful mutation reply
                                    // is LCD `Ok(None)`: its getter reported
                                    // byte 6 = ff, so no write was attempted.
                                    eprintln!(
                                        "alien-daemon: typed mutation not applied (getter reports unsupported): {}",
                                        request.audit_name()
                                    );
                                } else {
                                    eprintln!(
                                        "alien-daemon: typed mutation confirmed: {}",
                                        request.audit_name()
                                    );
                                }
                            }
                            format!("OK {}", encode_hex(&response))
                        }
                        Err(error) => {
                            if request.is_notifying_gpu_get() {
                                eprintln!(
                                    "alien-daemon: notifying typed getter failed: {}: {error}",
                                    request.audit_name()
                                );
                            }
                            if request.is_mutation() {
                                eprintln!(
                                    "alien-daemon: typed mutation failed: {}: {error}",
                                    request.audit_name()
                                );
                            }
                            encode_typed_error(error)
                        }
                    }
                }
            }
        } else {
            match parse_request(line) {
                None => "ERR unintelligible request; expected CALL or FEATURE".to_string(),
                Some((function, payload)) => match policy::check(function, &payload) {
                    Verdict::Deny(reason) => {
                        // Logged, because a denial is either a bug in a client or
                        // somebody probing, and both are worth seeing.
                        eprintln!("alien-daemon: denied fn {function:#04x}: {reason}");
                        format!("ERR {reason}")
                    }
                    Verdict::Allow => {
                        let guard = firmware
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        match guard.call_bytes(function, &payload) {
                            Ok(resp) => format!("OK {}", encode_hex(&resp)),
                            Err(e) => format!("ERR {e}"),
                        }
                    }
                },
            }
        };

        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}

fn typed_feature(
    request: FeatureRequest,
    firmware: &Arc<Mutex<AcpiCall>>,
) -> Result<Vec<u8>, TransportError> {
    let guard = firmware
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match request {
        FeatureRequest::GetCoolBoost => guard.coolboost().map(|enabled| vec![enabled as u8]),
        FeatureRequest::SetCoolBoost(enabled) => guard
            .set_coolboost(enabled)
            .map(|enabled| vec![enabled as u8]),
        FeatureRequest::GetKeyboardTimeout => guard
            .keyboard_timeout()
            .map(|state| vec![state.brightness, state.seconds]),
        FeatureRequest::SetKeyboardTimeout(seconds) => guard
            .set_keyboard_timeout(seconds)
            .map(|state| vec![state.brightness, state.seconds]),
        FeatureRequest::GetLcdOverdrive => guard
            .lcd_overdrive()
            .map(|state| state.map(|enabled| vec![enabled as u8]).unwrap_or_default()),
        FeatureRequest::SetLcdOverdrive(enabled) => guard
            .set_lcd_overdrive(enabled)
            .map(|state| state.map(|enabled| vec![enabled as u8]).unwrap_or_default()),
        FeatureRequest::GetGpuMode => guard.gpu_mode().map(encode_gpu_mode_state),
        FeatureRequest::SetGpuMode(mode) => {
            let opt_in = GpuModeOptIn::acknowledge(GPU_MODE_ACKNOWLEDGEMENT)
                .expect("daemon constant is the exact acknowledgement");
            guard.set_gpu_mode(mode, opt_in).map(encode_gpu_mode_state)
        }
    }
}

fn encode_typed_error(error: TransportError) -> String {
    match error {
        TransportError::FirmwareStatus { operation, status } => {
            format!("ERR FWSTATUS {status:02x} {operation}")
        }
        other => format!("ERR {other}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeatureRequest {
    GetCoolBoost,
    SetCoolBoost(bool),
    GetKeyboardTimeout,
    SetKeyboardTimeout(u8),
    GetLcdOverdrive,
    SetLcdOverdrive(bool),
    GetGpuMode,
    SetGpuMode(GpuMode),
}

impl FeatureRequest {
    fn is_notifying_gpu_get(self) -> bool {
        self == FeatureRequest::GetGpuMode
    }

    fn is_mutation(self) -> bool {
        matches!(
            self,
            FeatureRequest::SetCoolBoost(_)
                | FeatureRequest::SetKeyboardTimeout(_)
                | FeatureRequest::SetLcdOverdrive(_)
                | FeatureRequest::SetGpuMode(_)
        )
    }

    fn audit_name(self) -> &'static str {
        match self {
            FeatureRequest::GetCoolBoost => "coolboost get",
            FeatureRequest::SetCoolBoost(false) => "coolboost off",
            FeatureRequest::SetCoolBoost(true) => "coolboost on",
            FeatureRequest::GetKeyboardTimeout => "keyboard-timeout get",
            FeatureRequest::SetKeyboardTimeout(0) => "keyboard-timeout off",
            FeatureRequest::SetKeyboardTimeout(30) => "keyboard-timeout 30 seconds",
            FeatureRequest::SetKeyboardTimeout(_) => "keyboard-timeout invalid",
            FeatureRequest::GetLcdOverdrive => "lcd-overdrive get",
            FeatureRequest::SetLcdOverdrive(false) => "lcd-overdrive off",
            FeatureRequest::SetLcdOverdrive(true) => "lcd-overdrive on",
            FeatureRequest::GetGpuMode => "gpu-mode get",
            FeatureRequest::SetGpuMode(GpuMode::Normal) => "gpu-mode normal",
            FeatureRequest::SetGpuMode(GpuMode::Faster) => "gpu-mode faster",
            FeatureRequest::SetGpuMode(GpuMode::Turbo) => "gpu-mode turbo",
        }
    }
}

fn parse_feature_request(line: &str) -> Option<FeatureRequest> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    Some(match parts.as_slice() {
        ["FEATURE", "coolboost", "get"] => FeatureRequest::GetCoolBoost,
        ["FEATURE", "coolboost", "set", "0"] => FeatureRequest::SetCoolBoost(false),
        ["FEATURE", "coolboost", "set", "1"] => FeatureRequest::SetCoolBoost(true),
        ["FEATURE", "keyboard-timeout", "get"] => FeatureRequest::GetKeyboardTimeout,
        ["FEATURE", "keyboard-timeout", "set", "0"] => FeatureRequest::SetKeyboardTimeout(0),
        ["FEATURE", "keyboard-timeout", "set", "30"] => FeatureRequest::SetKeyboardTimeout(30),
        ["FEATURE", "lcd-overdrive", "get"] => FeatureRequest::GetLcdOverdrive,
        ["FEATURE", "lcd-overdrive", "set", "0"] => FeatureRequest::SetLcdOverdrive(false),
        ["FEATURE", "lcd-overdrive", "set", "1"] => FeatureRequest::SetLcdOverdrive(true),
        ["FEATURE", "gpu-mode", "get"] => FeatureRequest::GetGpuMode,
        ["FEATURE", "gpu-mode", "set", mode, acknowledgement]
            if *acknowledgement == GPU_MODE_ACKNOWLEDGEMENT =>
        {
            FeatureRequest::SetGpuMode(match *mode {
                "normal" => GpuMode::Normal,
                "faster" => GpuMode::Faster,
                "turbo" => GpuMode::Turbo,
                _ => return None,
            })
        }
        _ => return None,
    })
}

fn drain_to_newline(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(());
        }
        if let Some(index) = available.iter().position(|byte| *byte == b'\n') {
            reader.consume(index + 1);
            return Ok(());
        }
        let len = available.len();
        reader.consume(len);
    }
}

/// Give the socket to group `alien`.
///
/// Done by hand rather than with a crate: this is two libc calls, and the
/// daemon's dependency list is part of its trust story.
fn chown_to_alien_group(path: &std::path::Path) -> Result<u32, String> {
    let gid = group_id("alien").ok_or("group `alien` does not exist — create it and re-run")?;
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| "socket path contains a NUL byte".to_string())?;

    // SAFETY: c_path is a valid NUL-terminated string that outlives the call;
    // -1 for the uid means "leave the owner alone".
    let rc = unsafe { libc_chown(c_path.as_ptr(), u32::MAX, gid) };
    if rc != 0 {
        return Err(format!(
            "chown to group alien failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(gid)
}

extern "C" {
    #[link_name = "chown"]
    fn libc_chown(path: *const std::ffi::c_char, uid: u32, gid: u32) -> i32;
}

/// Look up a group by name in `/etc/group`.
///
/// Parsing the file directly rather than calling `getgrnam` keeps this
/// dependency-free and avoids the not-thread-safe static buffer. The trade-off
/// is real and worth stating: this does **not** see groups that come from LDAP,
/// SSSD or any other NSS backend. For a local hardware-control group that is
/// the right call; if it ever needs to work with a directory service, this is
/// the function to replace.
fn group_id(name: &str) -> Option<u32> {
    find_gid(&std::fs::read_to_string("/etc/group").ok()?, name)
}

/// Pull a gid out of `/etc/group` content. Split from the file read so it can
/// be tested against the awkward lines real systems contain.
fn find_gid(text: &str, name: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        // Format: name:password:gid:members
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        fields.nth(1)?.parse().ok() // skip the password field, take the gid
    })
}

#[cfg(test)]
mod tests {
    use super::{
        admit_after_interval, encode_typed_error, find_gid, parse_daemon_args,
        parse_feature_request, DaemonCommand, FeatureRequest, GpuMode, GPU_MODE_ACKNOWLEDGEMENT,
        NOTIFYING_GPU_GET_INTERVAL,
    };
    use alien_core::transport::TransportError;

    const GROUP_FILE: &str = "\
root:x:0:
wheel:x:998:vz
alien:x:1001:vz,someoneelse
video:x:26:
";

    #[test]
    fn notifying_gpu_get_has_a_distinct_exact_cooldown() {
        let base = std::time::Instant::now();
        let mut previous = None;
        assert!(admit_after_interval(
            &mut previous,
            base,
            NOTIFYING_GPU_GET_INTERVAL
        ));
        assert!(!admit_after_interval(
            &mut previous,
            base + NOTIFYING_GPU_GET_INTERVAL - std::time::Duration::from_millis(1),
            NOTIFYING_GPU_GET_INTERVAL
        ));
        assert!(admit_after_interval(
            &mut previous,
            base + NOTIFYING_GPU_GET_INTERVAL,
            NOTIFYING_GPU_GET_INTERVAL
        ));
    }

    #[test]
    fn finds_the_gid() {
        assert_eq!(find_gid(GROUP_FILE, "alien"), Some(1001));
        assert_eq!(find_gid(GROUP_FILE, "root"), Some(0));
    }

    #[test]
    fn absent_group_is_none_rather_than_a_default() {
        // Falling back to a guessed gid would silently widen access.
        assert_eq!(find_gid(GROUP_FILE, "nosuchgroup"), None);
    }

    #[test]
    fn does_not_match_a_group_by_prefix() {
        // "alien" must not match "alien-users" or vice versa.
        assert_eq!(find_gid("alien-users:x:1002:\n", "alien"), None);
        assert_eq!(find_gid(GROUP_FILE, "ali"), None);
    }

    #[test]
    fn survives_malformed_lines() {
        assert_eq!(find_gid("broken\n\nalien:x:7:\n", "alien"), Some(7));
        assert_eq!(find_gid("alien:x:notanumber:\n", "alien"), None);
    }

    #[test]
    fn daemon_arguments_are_side_effect_free_and_exact() {
        assert_eq!(parse_daemon_args(&[]), Ok(DaemonCommand::Run));
        for spelling in ["-V", "--version", "version"] {
            assert_eq!(parse_daemon_args(&[spelling]), Ok(DaemonCommand::Version));
        }

        for rejected in [
            vec!["--help"],
            vec!["--version", "ignored"],
            vec!["run"],
            vec![""],
        ] {
            assert!(
                parse_daemon_args(&rejected).is_err(),
                "accepted {rejected:?}"
            );
        }
    }

    #[test]
    fn typed_feature_parser_accepts_only_the_exact_commands() {
        assert_eq!(
            parse_feature_request("FEATURE coolboost get"),
            Some(FeatureRequest::GetCoolBoost)
        );
        assert_eq!(
            parse_feature_request("FEATURE keyboard-timeout set 30"),
            Some(FeatureRequest::SetKeyboardTimeout(30))
        );
        assert_eq!(
            parse_feature_request("FEATURE lcd-overdrive set 0"),
            Some(FeatureRequest::SetLcdOverdrive(false))
        );
        assert_eq!(
            parse_feature_request(&format!(
                "FEATURE gpu-mode set faster {GPU_MODE_ACKNOWLEDGEMENT}"
            )),
            Some(FeatureRequest::SetGpuMode(GpuMode::Faster))
        );

        for rejected in [
            "FEATURE coolboost set true",
            "FEATURE coolboost set 1 ignored",
            "FEATURE keyboard-timeout set 15",
            "FEATURE keyboard-timeout get 0",
            "FEATURE lcd-overdrive set 2",
            "FEATURE gpu-mode set turbo",
            "FEATURE gpu-mode set turbo I_ACCEPT_RISK",
            "FEATURE gpu-mode set ludicrous I_ACCEPT_UNSUPPORTED_GPU_OVERCLOCK_RISK",
            "FEATURE win-menu set 1",
        ] {
            assert_eq!(parse_feature_request(rejected), None, "accepted {rejected}");
        }
    }

    #[test]
    fn mutation_classification_and_firmware_status_encoding_are_exact() {
        assert!(!FeatureRequest::GetCoolBoost.is_mutation());
        assert!(FeatureRequest::SetCoolBoost(true).is_mutation());
        assert!(FeatureRequest::SetKeyboardTimeout(30).is_mutation());
        assert!(FeatureRequest::SetLcdOverdrive(false).is_mutation());
        assert!(!FeatureRequest::GetGpuMode.is_mutation());
        assert!(FeatureRequest::SetGpuMode(GpuMode::Turbo).is_mutation());

        assert_eq!(
            encode_typed_error(TransportError::FirmwareStatus {
                operation: "keyboard-timeout getter".into(),
                status: 0xe2,
            }),
            "ERR FWSTATUS e2 keyboard-timeout getter"
        );
    }
}
