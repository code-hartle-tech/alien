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
//! as deliberately as `sudo` — a member can spin the fans and set the keyboard
//! colour, which is the point.
//!
//! What group membership deliberately does **not** grant is arbitrary firmware
//! access: every request is checked against [`alien_core::policy`], which
//! allowlists the functions Alien uses and refuses the persistent-CMOS
//! sub-index. Without that this would be a "call any SMM method as root"
//! service wearing a fan-control costume.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use alien_core::policy::{self, Verdict};
use alien_core::socket::{encode_hex, parse_request, socket_path};
use alien_core::transport::{AcpiCall, Transport};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("alien-daemon: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let transport = AcpiCall::detect().map_err(|e| {
        format!("cannot reach the firmware: {e}\nis the acpi_call module loaded, and is this an Acer gaming machine?")
    })?;
    eprintln!("alien-daemon: firmware via {}", transport.describe());

    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }

    // A socket left behind by a crash blocks bind(). Removing it is safe here
    // only because we then bind immediately — if another daemon is genuinely
    // running, its bind still holds and ours fails, which is what we want.
    let _ = std::fs::remove_file(&path);
    let listener =
        UnixListener::bind(&path).map_err(|e| format!("cannot bind {}: {e}", path.display()))?;

    // 0660 before anyone can connect. Set after bind because bind() applies the
    // process umask, which would otherwise decide our permissions for us.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))
        .map_err(|e| format!("cannot chmod {}: {e}", path.display()))?;
    match chown_to_alien_group(&path) {
        Ok(gid) => eprintln!("alien-daemon: listening on {} (group alien, gid {gid})", path.display()),
        Err(e) => eprintln!(
            "alien-daemon: listening on {} — WARNING: {e}\n\
             the socket is root-only, so unprivileged clients cannot connect.",
            path.display()
        ),
    }

    // The mutex is the whole point: it serialises the non-atomic
    // write-then-read against the global kernel buffer.
    let firmware = Arc::new(Mutex::new(transport));

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let fw = Arc::clone(&firmware);
                // One thread per client. A GUI polling telemetry must not be
                // blocked behind another client's slow call, and there are
                // never more than a handful of these.
                std::thread::spawn(move || {
                    if let Err(e) = serve(stream, fw) {
                        eprintln!("alien-daemon: client dropped: {e}");
                    }
                });
            }
            Err(e) => eprintln!("alien-daemon: accept failed: {e}"),
        }
    }
    Ok(())
}

fn serve(stream: UnixStream, firmware: Arc<Mutex<AcpiCall>>) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let reply = match parse_request(&line) {
            None => "ERR unintelligible request; expected: CALL <fn-hex> <payload-hex>".to_string(),
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
        };

        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
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
    use super::find_gid;

    const GROUP_FILE: &str = "\
root:x:0:
wheel:x:998:vz
alien:x:1001:vz,someoneelse
video:x:26:
";

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
}
