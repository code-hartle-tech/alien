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

use std::path::PathBuf;

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Path;

// The direct transport needs POSIX file descriptors and flock. Gated on `unix`
// rather than `linux` because these APIs exist on macOS too and the crate has
// always type-checked there; Windows is the only target that lacks them. The
// trait, the error type and the socket client are portable, which is what lets
// a Windows WMI transport slot in beside this one rather than replace the file.
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::wmi::Status;

#[cfg(unix)]
use crate::performance::{self, GpuFirmwareIo};
use crate::performance::{GpuMode, GpuModeOptIn, GpuModeState};
#[cfg(unix)]
use crate::wmi::{misc_word, Function};

const ACPI_CALL: &str = "/proc/acpi/call";
const INTERFACE_LOCK: &str = "/run/alien/daemon.lock";
const GAMING_METHOD_EXACT: &str = "\\_SB.PCI0.WMID.WMBH";
const APGE_METHOD_EXACT: &str = "\\_SB.PCI0.WMID.WMAA";
const GAMING_WMI_GUID: &str = "7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56";
const APGE_WMI_GUID: &str = "61EF69EA-865C-4BC3-A502-A0DEBA0CB531";
/// Managed PredatorSense passes fn23's selector as a `uint32`, not a short.
const GPU_GPOC_SELECTOR: [u8; 4] = 5u32.to_le_bytes();

#[derive(Debug)]
pub enum TransportError {
    /// `/proc/acpi/call` is missing — the acpi_call module is not loaded.
    AcpiCallUnavailable,
    /// We could not find the WMI dispatch method in the ACPI namespace.
    MethodNotFound,
    /// `/proc/acpi/call` exists but is not writable by this process.
    PermissionDenied,
    /// Another Alien process owns the non-atomic global acpi_call buffer.
    InterfaceBusy(PathBuf),
    /// This transport does not implement an advanced typed endpoint.
    UnsupportedEndpoint(&'static str),
    /// A typed WMI method returned a non-zero firmware status byte.
    FirmwareStatus {
        operation: String,
        status: u8,
    },
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
            TransportError::InterfaceBusy(path) => write!(
                f,
                "another Alien process owns the firmware interface lock at {}",
                path.display()
            ),
            TransportError::UnsupportedEndpoint(name) => {
                write!(f, "transport does not support {name}")
            }
            TransportError::FirmwareStatus { operation, status } => {
                write!(f, "{operation} returned firmware status {status:#04x}")
            }
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
    GAMING_METHOD_EXACT,
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

    fn coolboost(&self) -> Result<bool, TransportError> {
        Err(TransportError::UnsupportedEndpoint("CoolBoost"))
    }

    /// Set CoolBoost and return its getter-confirmed state.
    fn set_coolboost(&self, _enabled: bool) -> Result<bool, TransportError> {
        Err(TransportError::UnsupportedEndpoint("CoolBoost"))
    }

    fn keyboard_timeout(&self) -> Result<KeyboardTimeoutState, TransportError> {
        Err(TransportError::UnsupportedEndpoint("keyboard timeout"))
    }

    /// Set timeout 0/30 and return brightness plus getter-confirmed timeout.
    fn set_keyboard_timeout(&self, _seconds: u8) -> Result<KeyboardTimeoutState, TransportError> {
        Err(TransportError::UnsupportedEndpoint("keyboard timeout"))
    }

    /// `None` means the exact getter reports the conditional LCD feature absent.
    fn lcd_overdrive(&self) -> Result<Option<bool>, TransportError> {
        Err(TransportError::UnsupportedEndpoint("LCD overdrive"))
    }

    /// Set and getter-confirm LCD overdrive; `None` remains unsupported.
    fn set_lcd_overdrive(&self, _enabled: bool) -> Result<Option<bool>, TransportError> {
        Err(TransportError::UnsupportedEndpoint("LCD overdrive"))
    }

    /// Read both NVML P0 offsets plus Acer fan-table/GPOC state. The Acer GPOC
    /// getter sends an OEM GPU notification, so callers must not poll this.
    fn gpu_mode(&self) -> Result<GpuModeState, TransportError> {
        Err(TransportError::UnsupportedEndpoint("OEM GPU mode"))
    }

    /// Apply exact PH315-53 Normal/Faster/Turbo semantics transactionally.
    fn set_gpu_mode(
        &self,
        _mode: GpuMode,
        _opt_in: GpuModeOptIn,
    ) -> Result<GpuModeState, TransportError> {
        Err(TransportError::UnsupportedEndpoint("OEM GPU mode"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardTimeoutState {
    pub brightness: u8,
    pub seconds: u8,
}

/// Direct `/proc/acpi/call` access. POSIX-only: it needs file descriptors,
/// `flock` and a procfs that Windows does not have.
#[cfg(unix)]
pub struct AcpiCall {
    method: String,
    // `flock` is released when this File drops. Holding it in the transport
    // makes direct callers and the daemon share one cross-process owner, not
    // merely one mutex per process.
    _interface_lock: fs::File,
    call_lock: std::sync::Mutex<()>,
}

#[cfg(unix)]
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
        let interface_lock = acquire_interface_lock()?;
        for m in METHOD_CANDIDATES {
            // _WDG advertises one instance, whose zero-based WMI index is 0.
            // PH315-53 AML happens to ignore Arg0, which is why index 1 once
            // appeared to work, but it is not the declared interface.
            let probe = format!("{m} 0x0 0x05 {{0x01,0x01}}");
            let Ok(resp) = raw_call(&probe) else { continue };
            // A scalar reply is not a sensor reply, whatever its value.
            if !resp.trim_start().starts_with('{') {
                continue;
            }
            let Ok(bytes) = parse_buffer(&resp) else {
                continue;
            };
            if bytes.first() != Some(&0) {
                continue;
            }
            match sensor_u16(&bytes) {
                Some(t) if t > 0 && t < 150 => {
                    return Ok(AcpiCall {
                        method: (*m).to_string(),
                        _interface_lock: interface_lock,
                        call_lock: std::sync::Mutex::new(()),
                    })
                }
                _ => continue,
            }
        }
        Err(TransportError::MethodNotFound)
    }

    pub fn method_path(&self) -> &str {
        &self.method
    }

    fn ensure_advanced_target(&self) -> Result<(), TransportError> {
        let read = |name: &str| {
            fs::read_to_string(format!("/sys/class/dmi/id/{name}"))
                .ok()
                .map(|value| value.trim().to_owned())
        };
        let vendor = read("sys_vendor");
        let product = read("product_name");
        let bios = read("bios_version");
        // The sysfs suffix is the kernel's WMI-device enumeration id, not the
        // method's zero-based instance argument. On the reference boot these
        // are `-6` and `-1`, while the AML method instance is still 0.
        let gaming_devices = wmi_guid_device_count(GAMING_WMI_GUID);
        let apge_devices = wmi_guid_device_count(APGE_WMI_GUID);
        if vendor
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("Acer"))
            && product
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("Predator PH315-53"))
            && bios.as_deref() == Some("V1.07")
            && gaming_devices == 1
            && apge_devices == 1
        {
            Ok(())
        } else {
            Err(TransportError::AcpiFailure(format!(
                "advanced WMI calls require Acer Predator PH315-53 BIOS V1.07 and exactly one device for each WMI GUID; found {} {} {} (gaming GUID count {}, APGe GUID count {})",
                vendor.as_deref().unwrap_or("unknown-vendor"),
                product.as_deref().unwrap_or("unknown-product"),
                bios.as_deref().unwrap_or("unknown-bios"),
                gaming_devices,
                apge_devices
            )))
        }
    }

    fn invoke_unlocked(
        &self,
        method_path: &str,
        method_id: u32,
        input: &[u8],
    ) -> Result<Vec<u8>, TransportError> {
        let args = input
            .iter()
            .map(|byte| format!("0x{byte:02x}"))
            .collect::<Vec<_>>()
            .join(",");
        let command = format!("{method_path} 0x0 {method_id:#04x} {{{args}}}");
        parse_buffer(&raw_call(&command)?)
    }

    fn coolboost_unlocked(&self) -> Result<bool, TransportError> {
        let response = self.invoke_unlocked(APGE_METHOD_EXACT, 2, &0x0000_0207u32.to_le_bytes())?;
        require_status_and_len(&response, 8, "CoolBoost getter")?;
        match response[1] {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(TransportError::AcpiFailure(format!(
                "CoolBoost getter returned out-of-domain state {value}"
            ))),
        }
    }

    fn write_coolboost_unlocked(&self, enabled: bool) -> Result<(), TransportError> {
        let word = 7u64 | ((enabled as u64) << 16);
        let response = self.invoke_unlocked(APGE_METHOD_EXACT, 1, &word.to_le_bytes())?;
        require_status_and_len(&response, 4, "CoolBoost setter")
    }

    fn keyboard_timeout_unlocked(&self) -> Result<KeyboardTimeoutState, TransportError> {
        // H=0 is the exact native fallback. It is the sole candidate we may
        // probe; sweeping the unproven 0..255 hotkey index is forbidden.
        let response = self.invoke_unlocked(APGE_METHOD_EXACT, 2, &0x0008_0001u32.to_le_bytes())?;
        require_status_and_len(&response, 8, "keyboard-timeout getter")?;
        if !matches!(response[5], 0 | 30) {
            return Err(TransportError::AcpiFailure(format!(
                "keyboard-timeout getter returned unsupported value {}",
                response[5]
            )));
        }
        Ok(KeyboardTimeoutState {
            brightness: response[4],
            seconds: response[5],
        })
    }

    fn write_keyboard_timeout_unlocked(
        &self,
        brightness: u8,
        seconds: u8,
    ) -> Result<(), TransportError> {
        if !matches!(seconds, 0 | 30) {
            return Err(TransportError::AcpiFailure(
                "keyboard timeout must be exactly 0 or 30 seconds".into(),
            ));
        }
        let payload = [2, 0, 8, 0, brightness, seconds, 0, 0];
        let response = self.invoke_unlocked(APGE_METHOD_EXACT, 1, &payload)?;
        require_status_and_len(&response, 4, "keyboard-timeout setter")
    }

    fn lcd_overdrive_unlocked(&self) -> Result<Option<bool>, TransportError> {
        let response = self.invoke_unlocked(GAMING_METHOD_EXACT, 3, &0u32.to_le_bytes())?;
        require_status_and_len(&response, 8, "LCD-overdrive getter")?;
        match response[6] {
            0 => Ok(Some(false)),
            1 => Ok(Some(true)),
            0xff => Ok(None),
            value => Err(TransportError::AcpiFailure(format!(
                "LCD-overdrive getter returned unknown capability byte {value:#04x}"
            ))),
        }
    }

    fn write_lcd_overdrive_unlocked(&self, enabled: bool) -> Result<(), TransportError> {
        let word = 0x10u64 | ((enabled as u64) << 48);
        let response = self.invoke_unlocked(GAMING_METHOD_EXACT, 1, &word.to_le_bytes())?;
        require_status_and_len(&response, 4, "LCD-overdrive setter")
    }

    fn gpu_fan_table_unlocked(&self) -> Result<u8, TransportError> {
        let response =
            self.invoke_unlocked(GAMING_METHOD_EXACT, Function::GetFanTable as u32, &[])?;
        require_status_and_exact_len(&response, 8, "GPU-mode fan-table getter")?;
        Ok(response[1])
    }

    fn write_gpu_fan_table_unlocked(&self, table: u8) -> Result<(), TransportError> {
        if table > 4 {
            return Err(TransportError::AcpiFailure(format!(
                "GPU-mode fan table {table} is outside the characterized 0..4 domain"
            )));
        }
        let response = self.invoke_unlocked(
            GAMING_METHOD_EXACT,
            Function::SetFanTable as u32,
            &(table as u64).to_le_bytes(),
        )?;
        require_status_and_exact_len(&response, 4, "GPU-mode fan-table setter")
    }

    fn gpu_gpoc_unlocked(&self) -> Result<u8, TransportError> {
        let response = self.invoke_unlocked(
            GAMING_METHOD_EXACT,
            Function::GetMiscSetting as u32,
            &GPU_GPOC_SELECTOR,
        )?;
        require_status_and_exact_len(&response, 8, "GPU-mode GPOC getter")?;
        Ok(response[1])
    }

    fn write_gpu_gpoc_unlocked(&self, level: u8) -> Result<(), TransportError> {
        if level > 2 {
            return Err(TransportError::AcpiFailure(format!(
                "GPU-mode GPOC level {level} is outside the characterized 0..2 domain"
            )));
        }
        let response = self.invoke_unlocked(
            GAMING_METHOD_EXACT,
            Function::SetMiscSetting as u32,
            &misc_word(5, level).to_le_bytes(),
        )?;
        require_status_and_exact_len(&response, 4, "GPU-mode GPOC setter")
    }
}

#[cfg(unix)]
struct AcpiGpuFirmware<'a>(&'a AcpiCall);

#[cfg(unix)]
impl GpuFirmwareIo for AcpiGpuFirmware<'_> {
    fn get_fan_table(&mut self) -> Result<u8, String> {
        self.0
            .gpu_fan_table_unlocked()
            .map_err(|error| error.to_string())
    }

    fn set_fan_table(&mut self, table: u8) -> Result<(), String> {
        self.0
            .write_gpu_fan_table_unlocked(table)
            .map_err(|error| error.to_string())
    }

    fn get_gpoc(&mut self) -> Result<u8, String> {
        self.0
            .gpu_gpoc_unlocked()
            .map_err(|error| error.to_string())
    }

    fn set_gpoc(&mut self, level: u8) -> Result<(), String> {
        self.0
            .write_gpu_gpoc_unlocked(level)
            .map_err(|error| error.to_string())
    }
}

/// How many kernel WMI devices carry this GUID.
///
/// Reads the Linux WMI bus, so it is POSIX-gated with the rest of the direct
/// path. A Windows build answers the same question through WMI itself.
#[cfg(unix)]
fn wmi_guid_device_count(guid: &str) -> usize {
    let prefix = format!("{guid}-");
    fs::read_dir("/sys/bus/wmi/devices")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .count()
}

fn require_status_and_len(
    response: &[u8],
    minimum: usize,
    operation: &str,
) -> Result<(), TransportError> {
    if response.len() < minimum {
        return Err(TransportError::AcpiFailure(format!(
            "{operation} returned {} byte(s), expected at least {minimum}",
            response.len()
        )));
    }
    if response[0] != 0 {
        return Err(TransportError::FirmwareStatus {
            operation: operation.into(),
            status: response[0],
        });
    }
    Ok(())
}

fn require_status_and_exact_len(
    response: &[u8],
    expected: usize,
    operation: &str,
) -> Result<(), TransportError> {
    if response.len() != expected {
        return Err(TransportError::AcpiFailure(format!(
            "{operation} returned {} byte(s), expected exactly {expected}",
            response.len()
        )));
    }
    if response[0] != 0 {
        return Err(TransportError::FirmwareStatus {
            operation: operation.into(),
            status: response[0],
        });
    }
    Ok(())
}

#[cfg(unix)]
fn acquire_interface_lock() -> Result<fs::File, TransportError> {
    let path = std::env::var_os("ALIEN_INTERFACE_LOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(INTERFACE_LOCK));
    let parent = path.parent().ok_or_else(|| {
        TransportError::AcpiFailure(format!(
            "interface lock path {} has no parent directory",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            TransportError::PermissionDenied
        } else {
            TransportError::Io(e)
        }
    })?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                TransportError::PermissionDenied
            } else {
                TransportError::Io(e)
            }
        })?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    // Linux flock constants: LOCK_EX | LOCK_NB. The lock is advisory, but all
    // supported Alien paths acquire it before ever touching /proc/acpi/call.
    if unsafe { libc_flock(lock.as_raw_fd(), 2 | 4) } != 0 {
        return Err(TransportError::InterfaceBusy(path));
    }
    Ok(lock)
}

extern "C" {
    #[link_name = "flock"]
    fn libc_flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
}

#[cfg(unix)]
impl Transport for AcpiCall {
    fn call_bytes(&self, function: u32, buf: &[u8]) -> Result<Vec<u8>, TransportError> {
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.invoke_unlocked(&self.method, function, buf)
    }

    fn describe(&self) -> String {
        format!("acpi_call {}", self.method)
    }

    fn coolboost(&self) -> Result<bool, TransportError> {
        self.ensure_advanced_target()?;
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.coolboost_unlocked()
    }

    fn set_coolboost(&self, enabled: bool) -> Result<bool, TransportError> {
        self.ensure_advanced_target()?;
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = self.coolboost_unlocked()?;
        let write = self.write_coolboost_unlocked(enabled);
        // A failed setter reply does not prove the firmware ignored the
        // write. Always re-read after the attempt before deciding whether a
        // rollback is needed.
        let readback = self.coolboost_unlocked();
        if write.is_ok() && matches!(&readback, Ok(current) if *current == enabled) {
            return Ok(enabled);
        }
        let rollback = match &readback {
            Ok(current) if *current == previous => {
                "not needed; getter remained at saved state".into()
            }
            _ => match self.write_coolboost_unlocked(previous) {
                Err(error) => format!("setter failed: {error}"),
                Ok(()) => match self.coolboost_unlocked() {
                    Ok(current) if current == previous => "getter-confirmed".into(),
                    Ok(current) => format!("getter mismatched at {current}"),
                    Err(error) => format!("getter failed: {error}"),
                },
            },
        };
        Err(TransportError::AcpiFailure(format!(
            "CoolBoost setter result {write:?}; readback {readback:?}; rollback to {previous}: {rollback}"
        )))
    }

    fn keyboard_timeout(&self) -> Result<KeyboardTimeoutState, TransportError> {
        self.ensure_advanced_target()?;
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.keyboard_timeout_unlocked()
    }

    fn set_keyboard_timeout(&self, seconds: u8) -> Result<KeyboardTimeoutState, TransportError> {
        self.ensure_advanced_target()?;
        if !matches!(seconds, 0 | 30) {
            return Err(TransportError::AcpiFailure(
                "keyboard timeout must be exactly 0 or 30 seconds".into(),
            ));
        }
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = self.keyboard_timeout_unlocked()?;
        let write = self.write_keyboard_timeout_unlocked(previous.brightness, seconds);
        let readback = self.keyboard_timeout_unlocked();
        if write.is_ok()
            && matches!(
                &readback,
                Ok(current)
                    if current.seconds == seconds && current.brightness == previous.brightness
            )
        {
            return Ok(KeyboardTimeoutState {
                brightness: previous.brightness,
                seconds,
            });
        }
        let rollback = match &readback {
            Ok(current) if *current == previous => {
                "not needed; getter remained at saved state".into()
            }
            _ => {
                match self.write_keyboard_timeout_unlocked(previous.brightness, previous.seconds) {
                    Err(error) => format!("setter failed: {error}"),
                    Ok(()) => match self.keyboard_timeout_unlocked() {
                        Ok(current) if current == previous => "getter-confirmed".into(),
                        Ok(current) => format!("getter mismatched at {current:?}"),
                        Err(error) => format!("getter failed: {error}"),
                    },
                }
            }
        };
        Err(TransportError::AcpiFailure(format!(
            "keyboard-timeout setter result {write:?}; readback {readback:?}; rollback to {} seconds: {rollback}",
            previous.seconds
        )))
    }

    fn lcd_overdrive(&self) -> Result<Option<bool>, TransportError> {
        self.ensure_advanced_target()?;
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.lcd_overdrive_unlocked()
    }

    fn set_lcd_overdrive(&self, enabled: bool) -> Result<Option<bool>, TransportError> {
        self.ensure_advanced_target()?;
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(previous) = self.lcd_overdrive_unlocked()? else {
            return Ok(None);
        };
        let write = self.write_lcd_overdrive_unlocked(enabled);
        let readback = self.lcd_overdrive_unlocked();
        if write.is_ok() && matches!(&readback, Ok(Some(current)) if *current == enabled) {
            return Ok(Some(enabled));
        }
        let rollback = match &readback {
            Ok(Some(current)) if *current == previous => {
                "not needed; getter remained at saved state".into()
            }
            _ => match self.write_lcd_overdrive_unlocked(previous) {
                Err(error) => format!("setter failed: {error}"),
                Ok(()) => match self.lcd_overdrive_unlocked() {
                    Ok(Some(current)) if current == previous => "getter-confirmed".into(),
                    Ok(current) => format!("getter mismatched at {current:?}"),
                    Err(error) => format!("getter failed: {error}"),
                },
            },
        };
        Err(TransportError::AcpiFailure(format!(
            "LCD-overdrive setter result {write:?}; readback {readback:?}; rollback to {previous}: {rollback}"
        )))
    }

    fn gpu_mode(&self) -> Result<GpuModeState, TransportError> {
        self.ensure_advanced_target()?;
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut firmware = AcpiGpuFirmware(self);
        performance::read_exact_gpu_mode(&mut firmware).map_err(TransportError::AcpiFailure)
    }

    fn set_gpu_mode(
        &self,
        mode: GpuMode,
        opt_in: GpuModeOptIn,
    ) -> Result<GpuModeState, TransportError> {
        self.ensure_advanced_target()?;
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut firmware = AcpiGpuFirmware(self);
        performance::apply_exact_gpu_mode(&mut firmware, mode, opt_in)
            .map_err(TransportError::AcpiFailure)
    }
}

#[cfg(unix)]
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

/// Parse one brace-delimited acpi_call buffer without dropping bad fields.
///
/// A bare scalar such as `0x0` is a structurally different ACPI return. WMI
/// methods in this transport promise buffers, so expanding that scalar into
/// eight zero bytes would turn an inert/wrong method into a plausible status-0
/// response. Likewise, skipping one malformed token would shift every field
/// after it. Both cases are hard failures.
fn parse_buffer(s: &str) -> Result<Vec<u8>, TransportError> {
    let t = s.trim();
    let inner = t
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or_else(|| {
            TransportError::AcpiFailure(format!("expected a brace-delimited ACPI buffer, got: {t}"))
        })?;
    if inner.trim().is_empty() {
        return Err(TransportError::AcpiFailure(
            "ACPI buffer contained no bytes".into(),
        ));
    }
    inner
        .split(',')
        .map(|field| {
            let field = field.trim();
            let digits = field
                .strip_prefix("0x")
                .or_else(|| field.strip_prefix("0X"))
                .ok_or_else(|| {
                    TransportError::AcpiFailure(format!(
                        "malformed byte `{field}` in ACPI buffer: {t}"
                    ))
                })?;
            u8::from_str_radix(digits, 16).map_err(|_| {
                TransportError::AcpiFailure(format!("malformed byte `{field}` in ACPI buffer: {t}"))
            })
        })
        .collect()
}

/// Status byte of a firmware reply.
///
/// Protocol, not transport: every reply carries its status in byte 0 regardless
/// of whether it arrived over `/proc/acpi/call`, the broker socket, or a WMI
/// method call. This was an associated function on `AcpiCall`, which quietly
/// made it unreachable from any other transport — the kind of coupling that
/// only shows up when a second one is added.
///
/// An empty reply decodes as `0xFF`, not success: a call that returned nothing
/// must never read as one that returned OK.
pub fn status(resp: &[u8]) -> Status {
    Status(resp.first().copied().unwrap_or(0xFF))
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
    fn rejects_scalar_returns_from_wmi_methods() {
        assert!(parse_buffer("0x0").is_err());
        assert!(parse_buffer("0x5").is_err());
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
        assert!(parse_buffer("{}").is_err());
        assert!(parse_buffer("{0x00, nope, 0x01}").is_err());
        assert!(parse_buffer("{0x00,,0x01}").is_err());
        assert!(parse_buffer("{0x100}").is_err());
        assert!(parse_buffer("{00,0x01}").is_err());
    }

    #[test]
    fn gpu_gpoc_getter_uses_the_exact_uint32_selector_shape() {
        assert_eq!(GPU_GPOC_SELECTOR, [5, 0, 0, 0]);
    }

    #[test]
    fn gpu_mode_getters_require_an_exact_uint64_reply() {
        for len in [0, 1, 2, 7, 9, 16] {
            assert!(
                require_status_and_exact_len(&vec![0; len], 8, "GPU-mode getter test").is_err()
            );
        }
        assert!(require_status_and_exact_len(&[0; 8], 8, "GPU-mode getter test").is_ok());
        assert!(matches!(
            require_status_and_exact_len(&[0xe2; 8], 8, "GPU-mode getter test"),
            Err(TransportError::FirmwareStatus { status: 0xe2, .. })
        ));
    }

    #[test]
    fn gpu_mode_setters_require_an_exact_uint32_reply() {
        for len in [0, 1, 2, 3, 5, 8] {
            assert!(
                require_status_and_exact_len(&vec![0; len], 4, "GPU-mode setter test").is_err()
            );
        }
        assert!(require_status_and_exact_len(&[0; 4], 4, "GPU-mode setter test").is_ok());
        assert!(matches!(
            require_status_and_exact_len(&[0xe2; 4], 4, "GPU-mode setter test"),
            Err(TransportError::FirmwareStatus { status: 0xe2, .. })
        ));
    }
}
