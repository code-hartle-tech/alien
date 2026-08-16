//! The Acer gaming WMI interface.
//!
//! Everything in this module was verified against real hardware (Predator
//! Helios 300 PH315-53) rather than copied from documentation — Acer publishes
//! none. Where a call is known-good, the measured effect is recorded in the
//! doc comment, because on this interface "the firmware returned success" and
//! "the hardware did the thing" are genuinely different outcomes.
//!
//! # The interface
//!
//! One WMI GUID exposes everything: fans, RGB, turbo and telemetry. It maps to
//! an ACPI method — on our reference machine `\_SB.PCI0.WMID.WMBH`, declared in
//! **SSDT12**, not the DSDT. That detail matters: anyone grepping only the DSDT
//! concludes the interface does not exist.
//!
//! # The trap that cost us most
//!
//! The DSDT contains `FANG`/`FANW`/`CLCK` under `EC0` that look exactly like
//! fan control. They are not. `CLCK`/`DUTY`/`THEN` are the **Intel CPU
//! clock-throttle register** at I/O `0x1810` (ACPI `ABASE+0x10`, PROC_CNT).
//! Writes never latch because `_PTC` selects FFixedHW (MSR
//! `IA32_CLOCK_MODULATION`) on modern CPUs, leaving the legacy register inert.

use std::fmt;

/// Acer gaming WMI GUID. Present on Predator and Nitro machines.
pub const GAMING_GUID: &str = "7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56";

/// Function IDs on the gaming interface.
///
/// **These are decimal in the vendor and community sources.** Mixing bases is
/// the single easiest way to address the wrong function — `SET_FAN_BEHAVIOUR`
/// is 14, i.e. `0x0E`, and reading it as hex sends you to `0x14`, the dynamic
/// RGB effect setter, which will happily return success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Function {
    SetGamingLed = 2,
    GetGamingLed = 4,
    /// Temperatures and fan RPM.
    GetSysInfo = 5,
    /// Static per-zone keyboard colour.
    SetStaticLed = 6,
    GetStaticLed = 7,
    /// **The fan lever.** Takes a u64, not the `{subunit, value}` byte pair its
    /// neighbours use — see [`FanBehaviour`].
    SetFanBehaviour = 14,
    /// Manual per-fan percentage. Accepted by firmware; on our reference
    /// machine the EC then arbitrates the actual duty (see module docs).
    SetFanSpeed = 16,
    GetFanSpeed = 17,
    /// Select the opaque Acer fan-curve table used by OEM performance modes.
    SetFanTable = 18,
    /// Read the stored Acer fan-curve table selector.
    GetFanTable = 19,
    /// Keyboard backlight effect (mode, speed, brightness, direction, colour).
    SetKbBacklight = 20,
    GetKbBacklight = 21,
    /// Miscellaneous settings, including the overclock/turbo flags.
    ///
    /// ⚠️ **Sub-index 6 writes a persistent CMOS byte.** Never sweep this
    /// function blindly.
    SetMiscSetting = 22,
    GetMiscSetting = 23,
}

/// Which fan. The IDs are not 1/2 — the GPU fan is 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fan {
    Cpu = 1,
    Gpu = 4,
}

/// Fan behaviour word for [`Function::SetFanBehaviour`].
///
/// # Encoding
///
/// The low 16 bits are a fan bitmask, `1 << (id - 1)` — so CPU (id 1) is bit 0
/// and GPU (id 4) is bit 3, giving `0b1001` = `0x9` for both. Above that sits a
/// two-bit mode field per fan, at bit `16 + 2 * (id - 1)`:
///
/// | mode | meaning |
/// |------|---------|
/// | 1    | automatic (EC's own curve) |
/// | 2    | maximum |
/// | 3    | manual (then set the percentage with [`Function::SetFanSpeed`]) |
///
/// So `0x820009` is "both fans, both mode 2" = both at maximum.
///
/// # Measured effect
///
/// On the reference machine, switching from automatic to maximum moved the CPU
/// fan 4477 → 5882 RPM and the GPU fan 5454 → 6122 RPM, dropped the GPU from
/// 86 °C to 81 °C at idle, and lifted 7-zip from ~26.1k to ~38.7k MIPS
/// (**+48%**) — the stock EC curve was holding the CPU in thermal throttle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanBehaviour {
    /// Both fans on the EC's own curve.
    Auto,
    /// Both fans at maximum.
    Max,
    /// Both fans manual; follow with per-fan percentages.
    Manual,
    /// One fan only, leaving the other untouched.
    Single { fan: Fan, mode: FanMode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanMode {
    Auto = 1,
    Max = 2,
    Manual = 3,
}

impl FanBehaviour {
    /// Build the u64 the firmware expects.
    pub fn to_word(self) -> u64 {
        match self {
            // Both fans: mask 0x9 (bits 0 and 3), mode in the high half.
            FanBehaviour::Auto => 0x0041_0009,
            FanBehaviour::Max => 0x0082_0009,
            FanBehaviour::Manual => 0x00C3_0009,
            FanBehaviour::Single { fan, mode } => {
                let id = fan as u64;
                let mask: u64 = 1 << (id - 1);
                let m: u64 = mode as u64;
                (m << (16 + 2 * (id - 1))) | mask
            }
        }
    }
}

/// Percentage payload for [`Function::SetFanSpeed`]: `(pct << 8) | fan_id`.
///
/// The vendor implementation writes this as `((pct * 25600) / 100) & 0xFF00`
/// plus the id, which is the same arithmetic taking the scenic route.
pub fn fan_speed_word(fan: Fan, percent: u8) -> u64 {
    let pct = percent.min(100) as u64;
    (pct << 8) | (fan as u64)
}

/// Overclock / turbo flags for [`Function::SetMiscSetting`].
///
/// # The value is 2, not 1
///
/// This cost a wrong conclusion: we tested with `1` and measured nothing.
/// Sampling the firmware while the chassis Turbo button was physically pressed
/// showed both sub-indices sitting at **2** — `ACER_WMID_OC_TURBO`.
///
/// # Why CPU turbo does nothing here, and where the real knob is
///
/// Retested correctly, with fans pinned at maximum so thermals could not mask
/// the result, there is still no measurable CPU clock or benchmark change on
/// the reference SKU. Decompiling PredatorSense 3.00.3152 explains it rather
/// than contradicting it: the app gates CPU overclock on
/// `Feature.ini → OverclockSupport CPU`, which is **0** for the PH315-53, and
/// its own service logs carry a "Not support CPU overclock" path. The WMI
/// write is issued and is genuinely inert on this model.
///
/// What PredatorSense actually calls "CPU turbo" on Intel machines is **Intel
/// XTU**: `.xtu` profiles driven through `XtuService.exe` and the
/// `iocbios2.sys` driver, adjusting PL1/PL2 power limits and turbo ratios —
/// not base clock, and not this WMI interface. On a 10th-gen H CPU the
/// multiplier is locked, so power limits are the only lever. Alien does not
/// implement that path; `intel-undervolt` and the RAPL sysfs knobs are where
/// it lives on Linux.
///
/// **GPU** overclock does not consist of this flag. PredatorSense applies the
/// `PredatorSense.ini [OC_GPU]` MHz offsets, the shared fan table and then this
/// GPOC selector as one compound command. Alien rejects independent raw GPU
/// setters and exposes the guarded compound path separately.
///
/// Kept in the API because the flags are real, they persist, and models with
/// `OverclockSupport CPU=1` should behave differently — but do not promise
/// users a frequency gain without measuring it on their machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverclockTarget {
    Gpu = 5,
    Cpu = 7,
}

pub const OC_OFF: u8 = 0;
pub const OC_TURBO: u8 = 2;

/// `SetMiscSetting` payload: low byte index, next byte value.
pub fn misc_word(index: u8, value: u8) -> u64 {
    ((value as u64) << 8) | index as u64
}

/// Sensor identifiers for [`Function::GetSysInfo`].
///
/// # How these were identified
///
/// A read-only sweep of ids 1–32 returned five live sensors, which were then
/// matched against ground truth (`nvidia-smi`, `coretemp`, and the kernel
/// thermal zones) sampled at the same moment:
///
/// | id | reading | matches | verdict |
/// |----|---------|---------|---------|
/// | 1 | 86 | coretemp package 88, `B0D4` 86 | CPU |
/// | 2 | 6000 | — | CPU fan RPM |
/// | 3 | 72 | `SEN2` 71, `pch_cometlake` 71 | board / system |
/// | 6 | 6122 | — | GPU fan RPM |
/// | 0x0A | 74 | `nvidia-smi` 74, and no other zone reads 74 | GPU |
///
/// **`0x0A` is the GPU, not a generic "system" temperature**, which is what an
/// earlier version of this enum called it. It matched `nvidia-smi` exactly in
/// every one of eleven samples, and no kernel thermal zone shares that value.
/// Mislabelling it would have put the GPU's temperature under a CPU heading in
/// every frontend.
#[derive(Debug, Clone, Copy)]
pub enum Sensor {
    CpuTemp = 1,
    CpuFanRpm = 2,
    /// Board / chassis sensor. Tracks ACPI `SEN2`, not either processor.
    SystemTemp = 3,
    GpuFanRpm = 6,
    GpuTemp = 0x0A,
}

/// The firmware's status convention: the low byte of the returned word is a
/// status code where zero means success. A non-zero value means the firmware
/// **rejected** the call — it is safe to iterate, not a sign of damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub u8);

impl Status {
    pub fn is_ok(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => write!(f, "ok"),
            1 => write!(f, "rejected: bad subunit"),
            2 => write!(f, "rejected: value out of range"),
            n => write!(f, "rejected: firmware status {n:#04x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_fan_words_match_the_vendor_constants() {
        assert_eq!(FanBehaviour::Auto.to_word(), 0x410009);
        assert_eq!(FanBehaviour::Max.to_word(), 0x820009);
        assert_eq!(FanBehaviour::Manual.to_word(), 0xC30009);
    }

    #[test]
    fn single_fan_words_match_the_vendor_constants() {
        // These four are quoted from the vendor-derived implementations, so
        // they are the check that the bitmask/mode derivation is right rather
        // than merely self-consistent.
        assert_eq!(
            FanBehaviour::Single {
                fan: Fan::Cpu,
                mode: FanMode::Auto
            }
            .to_word(),
            0x1_0001
        );
        assert_eq!(
            FanBehaviour::Single {
                fan: Fan::Cpu,
                mode: FanMode::Manual
            }
            .to_word(),
            0x3_0001
        );
        assert_eq!(
            FanBehaviour::Single {
                fan: Fan::Gpu,
                mode: FanMode::Auto
            }
            .to_word(),
            0x40_0008
        );
        assert_eq!(
            FanBehaviour::Single {
                fan: Fan::Gpu,
                mode: FanMode::Manual
            }
            .to_word(),
            0xC0_0008
        );
    }

    #[test]
    fn fan_speed_encoding() {
        assert_eq!(fan_speed_word(Fan::Cpu, 100), 0x6401);
        assert_eq!(fan_speed_word(Fan::Gpu, 50), 0x3204);
        // Clamped rather than wrapping — the firmware rejects >100 with
        // status 2, and silently sending garbage is worse than clamping.
        assert_eq!(fan_speed_word(Fan::Cpu, 255), 0x6401);
    }

    #[test]
    fn misc_encoding_matches_turbo() {
        assert_eq!(misc_word(OverclockTarget::Cpu as u8, OC_TURBO), 0x0207);
        assert_eq!(misc_word(OverclockTarget::Gpu as u8, OC_TURBO), 0x0205);
    }
}
