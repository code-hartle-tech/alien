//! Getting a WMI call to the firmware from Linux userspace.
//!
//! There are two routes and they are not equivalent.
//!
//! * **acpi_call** — write an ACPI method invocation to `/proc/acpi/call` and
//!   read the result back. Needs the out-of-tree `acpi_call` module, and needs
//!   the machine's ACPI method path, which varies by model.
//! * **The kernel WMI bus** — `/sys/bus/wmi/devices/<GUID>-<n>`. Present on
//!   every machine we have seen, but the kernel only exposes an *invoke*
//!   interface to drivers, not to userspace, so a small kernel module is
//!   required to use it.
//!
//! We default to acpi_call because it needs no bespoke module, and fall back to
//! reporting clearly when it is absent rather than silently doing nothing —
//! this whole interface is full of calls that return success while changing
//! nothing, so the transport layer must never add to that.
//!
//! # Why this is a trait
//!
//! Two reasons, and the second is a correctness bug rather than a preference.
//!
//! 1. **Sandboxes cannot reach `/proc/acpi/call` at all.** A flatpak or snap
//!    build has no path to the firmware, and running a Wayland GUI as root to
//!    work around that trades one broken thing for a worse one. So the same
//!    `Device` has to be drivable over a socket to a privileged daemon.
//!
//! 2. **`/proc/acpi/call` is a single global kernel buffer, and write-then-read
//!    is not atomic.** Two processes calling at once interleave: A writes, B
//!    writes, A reads *B's* answer. Nothing errors — A just gets a plausible
//!    reading for the wrong request. Direct access is therefore only safe when
//!    exactly one process uses it, which is precisely what the daemon
//!    guarantees by owning the file behind a mutex.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::wmi::Status;

const ACPI_CALL: &str = "/proc/acpi/call";

#[derive(Debug)]
pub enum TransportError {
    /// `/proc/acpi/call` is missing — the acpi_call module is not loaded.
    AcpiCallUnavailable,
    /// We could not find the WMI dispatch method in the ACPI namespace.
    MethodNotFound,
    /// `/proc/acpi/call` exists but is not writable by this process.
    PermissionDenied,
    /// The firmware refused the call at the ACPI level.
    AcpiFailure(String),
    Io(std::io::Error),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::AcpiCallUnavailable => write!(
                f,
                "/proc/acpi/call not present — load the acpi_call kernel module"
            ),
            TransportError::MethodNotFound => write!(
                f,
                "could not locate the gaming WMI method in the ACPI namespace"
            ),
            TransportError::PermissionDenied => write!(
                f,
                "direct firmware access needs root. Either run with sudo, or install and start \
                 alien-daemon and join the `alien` group — that is the supported way for an \
                 unprivileged desktop app to reach the hardware"
            ),
            TransportError::AcpiFailure(s) => write!(f, "ACPI call failed: {s}"),
            TransportError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        TransportError::Io(e)
    }
}

/// Candidate ACPI paths for the gaming WMI dispatch method.
///
/// The object id in `_WDG` determines the method name (`BH` → `WMBH`), and the
/// device path differs between models. Rather than hardcode one machine, probe
/// the plausible set — a wrong path fails cleanly with "not found" instead of
/// poking an unrelated method.
const METHOD_CANDIDATES: &[&str] = &[
    "\\_SB.PCI0.WMID.WMBH",
    "\\_SB.PCI0.WMID.WMBE",
    "\\_SB.WMID.WMBH",
    "\\_SB.PCI0.LPCB.WMID.WMBH",
];

/// Anything that can carry a WMI call to the firmware.
///
/// Implementors: [`AcpiCall`] (direct, needs root) and
/// [`crate::socket::SocketClient`] (via the daemon, needs only socket access).
pub trait Transport: Send + Sync {
    /// Invoke a function whose payload is a byte buffer.
    fn call_bytes(&self, function: u32, buf: &[u8]) -> Result<Vec<u8>, TransportError>;

    /// Invoke a function whose payload is a 64-bit word.
    ///
    /// The firmware still receives a buffer — the word goes on the wire
    /// little-endian. Function 14 (fan behaviour) is the one that needs this,
    /// and passing it as a two-byte pair is a silent no-op.
    fn call_u64(&self, function: u32, word: u64) -> Result<Vec<u8>, TransportError> {
        self.call_bytes(function, &word.to_le_bytes())
    }

    /// How this transport reaches the firmware, for display and bug reports.
    fn describe(&self) -> String;
}

pub struct AcpiCall {
    method: String,
}

impl AcpiCall {
    /// Probe for a usable dispatch method.
    ///
    /// Probing uses `GetSysInfo` (function 5) with a CPU-temperature request,
    /// deliberately: it is a pure read, so a wrong guess cannot change machine
    /// state.
    ///
    /// # The probe has to be strict
    ///
    /// A loose "did it answer?" test picks the wrong method. On the reference
    /// machine `WMBE` exists, accepts the call, and returns a bare `0x0` — a
    /// perfectly well-formed answer that means nothing. Accepting it selected
    /// an inert interface while the working one sat next in the list, and
    /// every subsequent call then "succeeded" against a method that does
    /// nothing.
    ///
    /// So the probe demands a **buffer-shaped** reply carrying a plausible
    /// reading: status byte 0 and a non-zero temperature under 150 °C. A
    /// machine that is genuinely unsupported fails this and says so, which is
    /// the outcome we want over a confident wrong answer.
    pub fn detect() -> Result<Self, TransportError> {
        if !Path::new(ACPI_CALL).exists() {
            return Err(TransportError::AcpiCallUnavailable);
        }
        // Distinguish "not permitted" from "not found" before probing.
        // Otherwise every candidate fails on EACCES and the caller is told the
        // machine has no gaming interface, which is both wrong and unhelpful.
        if let Err(e) = fs::OpenOptions::new().write(true).open(ACPI_CALL) {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(TransportError::PermissionDenied);
            }
        }
        for m in METHOD_CANDIDATES {
            let probe = format!("{m} 0x1 0x05 {{0x01,0x01}}");
            let Ok(resp) = raw_call(&probe) else { continue };
            // A scalar reply is not a sensor reply, whatever its value.
            if !resp.trim_start().starts_with('{') {
                continue;
            }
            let Ok(bytes) = parse_buffer(&resp) else { continue };
            if bytes.first() != Some(&0) {
                continue;
            }
            match sensor_u16(&bytes) {
                Some(t) if t > 0 && t < 150 => {
                    return Ok(AcpiCall { method: (*m).to_string() })
                }
                _ => continue,
            }
        }
        Err(TransportError::MethodNotFound)
    }

    /// Status byte of the last response.
    pub fn status(resp: &[u8]) -> Status {
        Status(resp.first().copied().unwrap_or(0xFF))
    }

    pub fn method_path(&self) -> &str {
        &self.method
    }
}

impl Transport for AcpiCall {
    fn call_bytes(&self, function: u32, buf: &[u8]) -> Result<Vec<u8>, TransportError> {
        let args = buf
            .iter()
            .map(|b| format!("0x{b:02x}"))
            .collect::<Vec<_>>()
            .join(",");
        let cmd = format!("{} 0x1 {:#04x} {{{}}}", self.method, function, args);
        parse_buffer(&raw_call(&cmd)?)
    }

    fn describe(&self) -> String {
        format!("acpi_call {}", self.method)
    }
}

fn raw_call(cmd: &str) -> Result<String, TransportError> {
    {
        let mut f = fs::OpenOptions::new().write(true).open(ACPI_CALL)?;
        f.write_all(cmd.as_bytes())?;
    }

    // ── Read with ONE read() syscall. This is not a style choice. ───────────
    //
    // `fs::read` and `read_to_string` both return an EMPTY string here, and
    // the failure is silent: the call really executed, the firmware really
    // answered, and userspace sees nothing. Cost us a wrong conclusion — the
    // probe below fell through to the next candidate method and "detected"
    // the wrong interface on a machine where the right one was working.
    //
    // Why: `/proc/acpi/call` reports st_size 0, like most procfs files. So
    // `fs::read` takes 0 as its size hint, allocates a zero-capacity buffer,
    // issues a zero-length read, gets Ok(0) back, and — correctly, by the Read
    // contract — treats that as EOF. Verified on the reference machine: a
    // zero-length read returns 0 while a 512-byte read on a fresh fd returns
    // the full 49-byte response.
    //
    // The buffer is not consumed by reading (re-reading returns the same
    // answer), so the single read is safe as well as sufficient. 512 bytes is
    // ample: the longest response this interface produces is an eight-byte
    // buffer rendered as text, ~49 bytes.
    let mut f = fs::File::open(PathBuf::from(ACPI_CALL))?;
    let mut buf = [0u8; 512];
    let n = f.read(&mut buf)?;

    // The result is NUL-terminated; trailing NULs confuse every parser that
    // does not expect them.
    let s = String::from_utf8_lossy(&buf[..n])
        .trim_end_matches('\0')
        .trim()
        .to_string();
    if s.starts_with("Error") {
        return Err(TransportError::AcpiFailure(s));
    }
    Ok(s)
}

/// Parse `{0x00, 0x64, 0x00, ...}` or a bare `0x5` integer return.
fn parse_buffer(s: &str) -> Result<Vec<u8>, TransportError> {
    let t = s.trim();
    if let Some(inner) = t.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
        Ok(inner
            .split(',')
            .filter_map(|p| {
                let p = p.trim().trim_start_matches("0x");
                u8::from_str_radix(p, 16).ok()
            })
            .collect())
    } else if let Some(hex) = t.strip_prefix("0x") {
        // Integer return: expose it little-endian so callers can treat every
        // response uniformly.
        u64::from_str_radix(hex, 16)
            .map(|v| v.to_le_bytes().to_vec())
            .map_err(|_| TransportError::AcpiFailure(format!("unparsable: {t}")))
    } else {
        Err(TransportError::AcpiFailure(format!("unparsable: {t}")))
    }
}

/// Decode a 16-bit little-endian sensor value from a response buffer.
///
/// Sensor replies put the value in bytes 1..=2, after the status byte.
pub fn sensor_u16(resp: &[u8]) -> Option<u16> {
    match (resp.get(1), resp.get(2)) {
        (Some(&lo), Some(&hi)) => Some(u16::from_le_bytes([lo, hi])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_buffer_return() {
        let v = parse_buffer("{0x00, 0x6b, 0x0f, 0x00}").unwrap();
        assert_eq!(v, vec![0x00, 0x6b, 0x0f, 0x00]);
    }

    #[test]
    fn parses_integer_return() {
        // TUVR-style scalar return.
        let v = parse_buffer("0x5").unwrap();
        assert_eq!(v[0], 5);
    }

    #[test]
    fn decodes_a_real_fan_reading() {
        // Captured verbatim from the reference machine: 0x0f6b = 3947 RPM.
        let resp = parse_buffer("{0x00, 0x6b, 0x0f, 0x00}").unwrap();
        assert_eq!(sensor_u16(&resp), Some(3947));
    }

    #[test]
    fn rejects_garbage_rather_than_guessing() {
        assert!(parse_buffer("not a buffer").is_err());
    }
}
