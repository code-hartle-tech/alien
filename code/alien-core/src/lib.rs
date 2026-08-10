//! **Alien** — Acer Predator / Nitro hardware control for Linux.
//!
//! A clean-room implementation of the vendor's gaming WMI protocol: fans, RGB,
//! turbo and telemetry, with no Windows and no vendor software. Every constant
//! here was confirmed against real firmware; where a call is accepted but has
//! no observable effect on the reference machine, the doc comment says so
//! rather than implying it works.
//!
//! ```no_run
//! use alien_core::{Device, FanBehaviour};
//!
//! let dev = Device::open()?;
//! dev.set_fan_behaviour(FanBehaviour::Max)?;
//! println!("{:?}", dev.sensors());
//! # Ok::<(), alien_core::Error>(())
//! ```
//!
//! Requires root (writes `/proc/acpi/call`) and the `acpi_call` kernel module.

pub mod capability;
pub mod device;
pub mod lighting;
pub mod perkey;
pub mod policy;
pub mod profile;
pub mod rgb;
pub mod socket;
pub mod transport;
pub mod wmi;

pub use capability::{Capabilities, Support};
pub use lighting::Lighting;
pub use device::{BacklightState, Device, Error, Result, Sensors};
pub use transport::Transport;
pub use rgb::{Colour, Direction, Effect, Zone};
pub use wmi::{Fan, FanBehaviour, FanMode, OverclockTarget};

/// Whether **direct** firmware access is possible in this process.
///
/// Only meaningful for the no-daemon path. It used to gate every call, which
/// was wrong the moment the daemon existed: a member of the `alien` group with
/// a running daemon needs neither root nor the kernel module, and was being
/// turned away with "root privileges are required" while the socket sat there
/// working. [`Device::open`] is the right entry point; this is for diagnostics
/// like `alien doctor`.
pub fn preflight_direct() -> std::result::Result<(), &'static str> {
    if !std::path::Path::new("/proc/acpi/call").exists() {
        return Err("the acpi_call kernel module is not loaded (try: modprobe acpi_call)");
    }
    if !nix_is_root() {
        return Err("root privileges are required to write /proc/acpi/call");
    }
    Ok(())
}

fn nix_is_root() -> bool {
    // Avoiding a dependency for one syscall: /proc/self/status is stable and
    // present wherever /proc/acpi/call is.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(|u| u == "0"))
        })
        .unwrap_or(false)
}
