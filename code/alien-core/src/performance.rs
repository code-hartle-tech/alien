//! Live Linux clock telemetry for the PredatorSense-style performance dials.
//!
//! These are measurements, not promises inferred from a selected mode. CPU
//! frequency comes from cpufreq; NVIDIA clock and utilisation come from
//! `nvidia-smi` when present. Missing providers stay missing instead of being
//! rendered as zero.

use std::path::{Path, PathBuf};

/// The one CPU power policy recovered from all three PH315-53 Acer profiles.
///
/// Normal, Fast and Turbo differ in profile identity only; this is not a set
/// of three CPU modes. Values are kept in the units used by Linux powercap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuPowerTarget {
    pub pl1_uw: u64,
    pub pl2_uw: u64,
    pub pl1_time_window_us: u64,
}

pub const PH315_53_CPU_POWER: CpuPowerTarget = CpuPowerTarget {
    pl1_uw: 70_000_000,
    pl2_uw: 107_000_000,
    pl1_time_window_us: 28_000_000,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Performance {
    pub cpu_mhz: Option<u32>,
    pub cpu_max_mhz: Option<u32>,
    pub gpu_mhz: Option<u32>,
    pub gpu_max_mhz: Option<u32>,
    pub gpu_usage_pct: Option<u8>,
}

impl Performance {
    pub fn sample() -> Self {
        let (cpu_mhz, cpu_max_mhz) = cpu_frequencies(Path::new("/sys/devices/system/cpu"));
        let (gpu_mhz, gpu_max_mhz, gpu_usage_pct) = nvidia_frequencies();
        Performance {
            cpu_mhz,
            cpu_max_mhz,
            gpu_mhz,
            gpu_max_mhz,
            gpu_usage_pct,
        }
    }
}

/// A named Linux powercap constraint.
///
/// The kernel's constraint index is deliberately retained only as evidence;
/// PL1/PL2 classification comes from constraint names, never from assuming
/// constraint 0 or 1 has a fixed meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerConstraint {
    pub index: u8,
    pub kernel_name: String,
    pub power_limit_uw: Option<u64>,
    pub max_power_uw: Option<u64>,
    pub time_window_us: Option<u64>,
    pub power_limit_has_write_bit: bool,
    pub time_window_has_write_bit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowercapPackage {
    pub sysfs_path: PathBuf,
    pub kernel_name: String,
    pub enabled: Option<bool>,
    pub pl1: Option<PowerConstraint>,
    pub pl2: Option<PowerConstraint>,
}

/// Read-only capability and live state for Intel RAPL.
///
/// Alien intentionally has no write method here. The target machine was not
/// reachable for a live sysfs/write/readback/rollback check, and powercap
/// exposes no separate PH315-53 "short power enable" bit. Reporting the exact
/// OEM target and the missing prerequisite is safer than shipping an
/// unverified privileged write path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowercapStatus {
    pub dmi_vendor: Option<String>,
    pub dmi_product_name: Option<String>,
    pub package: Option<PowercapPackage>,
}

impl PowercapStatus {
    pub fn sample() -> Self {
        Self::sample_at(
            Path::new("/sys/class/powercap"),
            Path::new("/sys/class/dmi/id"),
        )
    }

    fn sample_at(powercap_root: &Path, dmi_root: &Path) -> Self {
        PowercapStatus {
            dmi_vendor: read_text(&dmi_root.join("sys_vendor")),
            dmi_product_name: read_text(&dmi_root.join("product_name")),
            package: find_intel_package(powercap_root),
        }
    }

    /// The DMI guard a future PH315-53 writer would have to pass.
    pub fn is_ph315_53(&self) -> bool {
        self.dmi_vendor
            .as_deref()
            .is_some_and(|vendor| vendor.trim().to_ascii_lowercase().starts_with("acer"))
            && self
                .dmi_product_name
                .as_deref()
                .is_some_and(|product| product.trim().eq_ignore_ascii_case("Predator PH315-53"))
    }

    pub fn can_read_limits(&self) -> bool {
        self.package
            .as_ref()
            .is_some_and(|package| package.pl1.is_some() && package.pl2.is_some())
    }

    /// Explain exactly why Alien remains read-only on this machine.
    pub fn write_gap(&self) -> String {
        if !self.is_ph315_53() {
            let vendor = self.dmi_vendor.as_deref().unwrap_or("unknown vendor");
            let product = self.dmi_product_name.as_deref().unwrap_or("unknown model");
            return format!("disabled: DMI reports {vendor} {product}, not Acer Predator PH315-53");
        }

        let Some(package) = &self.package else {
            return "disabled: no Intel package zone was found under /sys/class/powercap".into();
        };
        if package.enabled == Some(false) {
            return "disabled: the Intel package powercap zone reports enabled=0".into();
        }
        let Some(pl1) = &package.pl1 else {
            return "disabled: no constraint named long_term/long-term/PL1 was found".into();
        };
        let Some(pl2) = &package.pl2 else {
            return "disabled: no constraint named short_term/short-term/PL2 was found".into();
        };
        if pl1.power_limit_uw.is_none() || pl2.power_limit_uw.is_none() {
            return "disabled: a named constraint has no readable power_limit_uw".into();
        }
        if pl1.time_window_us.is_none() {
            return "disabled: the named PL1 constraint has no readable time_window_us".into();
        }
        if pl1
            .max_power_uw
            .is_some_and(|max| max < PH315_53_CPU_POWER.pl1_uw)
            || pl2
                .max_power_uw
                .is_some_and(|max| max < PH315_53_CPU_POWER.pl2_uw)
        {
            return "disabled: the OEM target exceeds a kernel-advertised constraint maximum"
                .into();
        }
        if !pl1.power_limit_has_write_bit
            || !pl2.power_limit_has_write_bit
            || !pl1.time_window_has_write_bit
        {
            return "disabled: the required named powercap files are not writable".into();
        }

        "disabled: named powercap files look writable, but PH315-53 write/readback/rollback \
         and the XTU short-power-enable mapping have not been verified live"
            .into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstraintKind {
    Pl1,
    Pl2,
}

fn find_intel_package(root: &Path) -> Option<PowercapPackage> {
    let mut zones: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("intel-rapl:"))
        })
        .collect();
    zones.sort();

    zones.into_iter().find_map(|zone| {
        let kernel_name = read_text(&zone.join("name"))?;
        if !kernel_name.to_ascii_lowercase().starts_with("package-") {
            return None;
        }

        let mut pl1 = None;
        let mut pl2 = None;
        // The interface normally has two constraints, but scanning a bounded
        // range avoids encoding that layout as an ABI assumption.
        for index in 0..=15 {
            let Some(constraint) = read_constraint(&zone, index) else {
                continue;
            };
            match classify_constraint(&constraint.kernel_name) {
                Some(ConstraintKind::Pl1) if pl1.is_none() => pl1 = Some(constraint),
                Some(ConstraintKind::Pl2) if pl2.is_none() => pl2 = Some(constraint),
                _ => {}
            }
        }

        Some(PowercapPackage {
            sysfs_path: zone.clone(),
            kernel_name,
            enabled: read_text(&zone.join("enabled")).and_then(|value| match value.as_str() {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            }),
            pl1,
            pl2,
        })
    })
}

fn read_constraint(zone: &Path, index: u8) -> Option<PowerConstraint> {
    let prefix = format!("constraint_{index}");
    let kernel_name = read_text(&zone.join(format!("{prefix}_name")))?;
    let power_limit = zone.join(format!("{prefix}_power_limit_uw"));
    let time_window = zone.join(format!("{prefix}_time_window_us"));
    Some(PowerConstraint {
        index,
        kernel_name,
        power_limit_uw: read_u64(&power_limit),
        max_power_uw: read_u64(&zone.join(format!("{prefix}_max_power_uw"))),
        time_window_us: read_u64(&time_window),
        power_limit_has_write_bit: has_write_bit(&power_limit),
        time_window_has_write_bit: has_write_bit(&time_window),
    })
}

fn classify_constraint(name: &str) -> Option<ConstraintKind> {
    let normalized = name.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "long_term" | "longterm" | "pl1" => Some(ConstraintKind::Pl1),
        "short_term" | "shortterm" | "pl2" => Some(ConstraintKind::Pl2),
        _ => None,
    }
}

fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_text(path)?.parse().ok()
}

#[cfg(unix)]
fn has_write_bit(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o222 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn has_write_bit(_path: &Path) -> bool {
    false
}

fn cpu_frequencies(root: &Path) -> (Option<u32>, Option<u32>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return (None, None);
    };
    let mut current = Vec::new();
    let mut maximum = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !name
            .strip_prefix("cpu")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let cpufreq = entry.path().join("cpufreq");
        if let Some(khz) = read_number(&cpufreq.join("scaling_cur_freq")) {
            current.push(khz);
        }
        if let Some(khz) = read_number(&cpufreq.join("scaling_max_freq")) {
            maximum.push(khz);
        }
    }
    // PredatorSense presents one CPU dial. The arithmetic mean is the most
    // useful single number for a multicore package: it rises with sustained
    // package performance instead of reporting one momentarily boosted core.
    (
        mean_khz(&current),
        maximum.into_iter().max().map(|v| v / 1000),
    )
}

fn read_number(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn mean_khz(values: &[u32]) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let total: u64 = values.iter().map(|v| u64::from(*v)).sum();
    Some((total / values.len() as u64 / 1000) as u32)
}

fn nvidia_frequencies() -> (Option<u32>, Option<u32>, Option<u8>) {
    let Ok(out) = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=clocks.current.graphics,clocks.max.graphics,utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
    else {
        return (None, None, None);
    };
    if !out.status.success() {
        return (None, None, None);
    }
    parse_nvidia(&String::from_utf8_lossy(&out.stdout))
}

fn parse_nvidia(text: &str) -> (Option<u32>, Option<u32>, Option<u8>) {
    let mut fields = text
        .lines()
        .next()
        .unwrap_or_default()
        .split(',')
        .map(str::trim);
    (
        fields.next().and_then(|v| v.parse().ok()),
        fields.next().and_then(|v| v.parse().ok()),
        fields.next().and_then(|v| v.parse().ok()),
    )
}

/// Exact PH315-53 GPU modes recovered from PredatorSense command 45.
///
/// These are signed P0 clock-domain offsets, not absolute clock locks,
/// application clocks, power limits or PowerMizer preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GpuMode {
    Normal = 0,
    Faster = 1,
    Turbo = 2,
}

// PredatorSense's shared helper actually selects
// max(requested_cpu_level, requested_gpu_level) + 1. The certified PH315-53
// configuration disables CPU overclocking and Alien exposes no CPU-mode
// setter, so its requested CPU contribution is fixed at Normal (zero). This
// constant makes that exact-target assumption visible instead of presenting
// GPU level + 1 as a generic Acer rule.
const PH315_53_REQUESTED_CPU_LEVEL: u8 = 0;

fn acer_shared_fan_table(requested_cpu_level: u8, requested_gpu_level: u8) -> u8 {
    debug_assert!(requested_cpu_level <= 3 && requested_gpu_level <= 3);
    std::cmp::max(requested_cpu_level, requested_gpu_level) + 1
}

impl GpuMode {
    pub const fn offsets(self) -> GpuClockOffsets {
        match self {
            GpuMode::Normal => GpuClockOffsets {
                graphics_mhz: 0,
                memory_mhz: 0,
            },
            GpuMode::Faster => GpuClockOffsets {
                graphics_mhz: 50,
                memory_mhz: 30,
            },
            GpuMode::Turbo => GpuClockOffsets {
                graphics_mhz: 100,
                memory_mhz: 60,
            },
        }
    }

    pub fn fan_table(self) -> u8 {
        acer_shared_fan_table(PH315_53_REQUESTED_CPU_LEVEL, self as u8)
    }

    pub const fn label(self) -> &'static str {
        match self {
            GpuMode::Normal => "normal",
            GpuMode::Faster => "faster",
            GpuMode::Turbo => "turbo",
        }
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(GpuMode::Normal),
            1 => Some(GpuMode::Faster),
            2 => Some(GpuMode::Turbo),
            _ => None,
        }
    }
}

/// Deliberately awkward acknowledgement required by every GPU-mode setter.
///
/// NVIDIA documents clock manipulation as an unsupported, privileged feature.
/// Keeping the acknowledgement in the typed API prevents a normal profile or
/// a stray boolean toggle from silently becoming an overclock request.
pub const GPU_MODE_ACKNOWLEDGEMENT: &str = "I_ACCEPT_UNSUPPORTED_GPU_OVERCLOCK_RISK";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuModeOptIn(());

impl GpuModeOptIn {
    pub fn acknowledge(value: &str) -> Result<Self, String> {
        if value == GPU_MODE_ACKNOWLEDGEMENT {
            Ok(GpuModeOptIn(()))
        } else {
            Err(format!(
                "GPU mode requires the exact acknowledgement {GPU_MODE_ACKNOWLEDGEMENT}"
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuClockOffsets {
    pub graphics_mhz: i32,
    pub memory_mhz: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuOffsetRange {
    pub current_mhz: i32,
    pub min_mhz: i32,
    pub max_mhz: i32,
}

impl GpuOffsetRange {
    fn contains(self, value: i32) -> bool {
        self.min_mhz <= value && value <= self.max_mhz
    }
}

/// Getter-confirmed state of all three Linux legs of Acer command 45.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuModeState {
    pub graphics: GpuOffsetRange,
    pub memory: GpuOffsetRange,
    pub fan_table: u8,
    pub gpoc: u8,
}

impl GpuModeState {
    /// Whether this snapshot's live driver ranges admit both mode offsets.
    /// Frontends use this to disable impossible targets before confirmation;
    /// the setter always re-queries and enforces the ranges again.
    pub fn target_fits(self, mode: GpuMode) -> bool {
        let target = mode.offsets();
        self.graphics.contains(target.graphics_mhz) && self.memory.contains(target.memory_mhz)
    }

    /// A mode is reported only when both NVML P0 offsets and both Acer
    /// firmware fields agree. Requested/presentation state is never enough.
    pub fn confirmed_mode(self) -> Option<GpuMode> {
        [GpuMode::Normal, GpuMode::Faster, GpuMode::Turbo]
            .into_iter()
            .find(|mode| {
                let target = mode.offsets();
                self.graphics.current_mhz == target.graphics_mhz
                    && self.memory.current_mhz == target.memory_mhz
                    && self.fan_table == mode.fan_table()
                    && self.gpoc == *mode as u8
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuClockDomain {
    Graphics,
    Memory,
}

pub(crate) trait GpuClockIo {
    fn read_offset(&mut self, domain: GpuClockDomain) -> Result<GpuOffsetRange, String>;
    fn set_offset(&mut self, domain: GpuClockDomain, offset_mhz: i32) -> Result<(), String>;
}

/// The two PH315-53 firmware legs that surround the Nvidia offset operation.
/// Implemented by the already-serialized ACPI transport.
pub(crate) trait GpuFirmwareIo {
    fn get_fan_table(&mut self) -> Result<u8, String>;
    fn set_fan_table(&mut self, table: u8) -> Result<(), String>;
    fn get_gpoc(&mut self) -> Result<u8, String>;
    fn set_gpoc(&mut self, level: u8) -> Result<(), String>;
}

/// Read all three stored legs without issuing a setter.
///
/// This is not side-effect-free: the PH315-53 fn23 GPOC getter itself sends
/// the OEM discrete-GPU notification. Frontends must make this an explicit
/// manual refresh, never a background telemetry poll.
pub(crate) fn read_exact_gpu_mode(
    firmware: &mut impl GpuFirmwareIo,
) -> Result<GpuModeState, String> {
    let mut clocks = NvmlClockIo::open_exact_ph315_53()?;
    read_gpu_state(&mut clocks, firmware)
}

/// Apply Acer's recovered order with stronger Linux-side rollback semantics.
///
/// Write order is P0 graphics offset, P0 memory offset, conditional Acer fan
/// table, then Acer GPOC. NVAPI accepted both offsets in one sparse structure;
/// public Linux NVML has one setter per clock domain, so the two domains cannot
/// be made driver-atomic. Any failure or readback mismatch restores every leg
/// that may have changed, in reverse order, and reports rollback verification.
pub(crate) fn apply_exact_gpu_mode(
    firmware: &mut impl GpuFirmwareIo,
    mode: GpuMode,
    _opt_in: GpuModeOptIn,
) -> Result<GpuModeState, String> {
    let mut clocks = NvmlClockIo::open_exact_ph315_53()?;
    apply_gpu_mode_with(&mut clocks, firmware, mode)
}

fn read_gpu_state(
    clocks: &mut impl GpuClockIo,
    firmware: &mut impl GpuFirmwareIo,
) -> Result<GpuModeState, String> {
    let (graphics, memory, fan_table) = read_gpu_state_prefix(clocks, firmware)?;
    let gpoc = firmware
        .get_gpoc()
        .map_err(|error| format!("Acer GPOC getter failed: {error}"))?;
    validate_gpu_state(GpuModeState {
        graphics,
        memory,
        fan_table,
        gpoc,
    })
}

fn read_gpu_state_prefix(
    clocks: &mut impl GpuClockIo,
    firmware: &mut impl GpuFirmwareIo,
) -> Result<(GpuOffsetRange, GpuOffsetRange, u8), String> {
    let graphics = clocks
        .read_offset(GpuClockDomain::Graphics)
        .map_err(|error| format!("NVML P0 graphics offset query failed: {error}"))?;
    let memory = clocks
        .read_offset(GpuClockDomain::Memory)
        .map_err(|error| format!("NVML P0 memory offset query failed: {error}"))?;
    let fan_table = firmware
        .get_fan_table()
        .map_err(|error| format!("Acer fan-table getter failed: {error}"))?;
    Ok((graphics, memory, fan_table))
}

fn validate_gpu_state(state: GpuModeState) -> Result<GpuModeState, String> {
    for (label, range) in [("graphics", state.graphics), ("memory", state.memory)] {
        if range.min_mhz > range.max_mhz || !range.contains(range.current_mhz) {
            return Err(format!(
                "NVML P0 {label} offset state is inconsistent: current {} MHz, range {}..{} MHz",
                range.current_mhz, range.min_mhz, range.max_mhz
            ));
        }
    }
    Ok(state)
}

fn apply_gpu_mode_with(
    clocks: &mut impl GpuClockIo,
    firmware: &mut impl GpuFirmwareIo,
    mode: GpuMode,
) -> Result<GpuModeState, String> {
    let before = read_gpu_state(clocks, firmware)?;
    if !(0..=4).contains(&before.fan_table) {
        return Err(format!(
            "refusing GPU mode: saved Acer fan table {} is outside the characterized 0..4 domain",
            before.fan_table
        ));
    }
    if before.gpoc > 2 {
        return Err(format!(
            "refusing GPU mode: saved Acer GPOC {} is outside the characterized 0..2 domain",
            before.gpoc
        ));
    }

    let target = mode.offsets();
    if !before.graphics.contains(target.graphics_mhz) {
        return Err(format!(
            "refusing GPU mode: requested P0 graphics offset {} MHz is outside driver range {}..{} MHz",
            target.graphics_mhz, before.graphics.min_mhz, before.graphics.max_mhz
        ));
    }
    if !before.memory.contains(target.memory_mhz) {
        return Err(format!(
            "refusing GPU mode: requested P0 memory offset {} MHz is outside driver range {}..{} MHz",
            target.memory_mhz, before.memory.min_mhz, before.memory.max_mhz
        ));
    }

    let offset_attempted = true;
    let mut fan_attempted = false;
    let mut gpoc_attempted = false;

    if let Err(error) = clocks.set_offset(GpuClockDomain::Graphics, target.graphics_mhz) {
        return Err(failed_with_rollback(
            format!("NVML P0 graphics setter failed: {error}"),
            clocks,
            firmware,
            before,
            offset_attempted,
            fan_attempted,
            gpoc_attempted,
        ));
    }
    if let Err(error) = clocks.set_offset(GpuClockDomain::Memory, target.memory_mhz) {
        return Err(failed_with_rollback(
            format!("NVML P0 memory setter failed: {error}"),
            clocks,
            firmware,
            before,
            offset_attempted,
            fan_attempted,
            gpoc_attempted,
        ));
    }
    if let Err(error) = verify_offsets(clocks, target) {
        return Err(failed_with_rollback(
            error,
            clocks,
            firmware,
            before,
            offset_attempted,
            fan_attempted,
            gpoc_attempted,
        ));
    }

    // PSSvc queries the table and writes only when it differs.
    if before.fan_table != mode.fan_table() {
        fan_attempted = true;
        if let Err(error) = firmware.set_fan_table(mode.fan_table()) {
            return Err(failed_with_rollback(
                format!("Acer fan-table setter failed: {error}"),
                clocks,
                firmware,
                before,
                offset_attempted,
                fan_attempted,
                gpoc_attempted,
            ));
        }
        match firmware.get_fan_table() {
            Ok(value) if value == mode.fan_table() => {}
            result => {
                return Err(failed_with_rollback(
                    format!(
                        "Acer fan-table readback mismatch: requested {}, got {result:?}",
                        mode.fan_table()
                    ),
                    clocks,
                    firmware,
                    before,
                    offset_attempted,
                    fan_attempted,
                    gpoc_attempted,
                ));
            }
        }
    }

    // PSSvc always sends this final WMI request, even when the stored value is
    // already equal, because it also issues the discrete-GPU notification.
    gpoc_attempted = true;
    if let Err(error) = firmware.set_gpoc(mode as u8) {
        return Err(failed_with_rollback(
            format!("Acer GPOC setter failed: {error}"),
            clocks,
            firmware,
            before,
            offset_attempted,
            fan_attempted,
            gpoc_attempted,
        ));
    }
    match firmware.get_gpoc() {
        Ok(value) if value == mode as u8 => {}
        result => {
            return Err(failed_with_rollback(
                format!(
                    "Acer GPOC readback mismatch: requested {}, got {result:?}",
                    mode as u8
                ),
                clocks,
                firmware,
                before,
                offset_attempted,
                fan_attempted,
                gpoc_attempted,
            ));
        }
    }

    // GPOC was getter-confirmed immediately above. Re-read the other three
    // legs, but do not call fn23 a redundant second time: that nominal getter
    // sends another OEM GPU notification on every invocation.
    let after = read_gpu_state_prefix(clocks, firmware)
        .and_then(|(graphics, memory, fan_table)| {
            validate_gpu_state(GpuModeState {
                graphics,
                memory,
                fan_table,
                gpoc: mode as u8,
            })
        })
        .map_err(|error| {
            failed_with_rollback(
                format!("final compound GPU-mode readback failed: {error}"),
                clocks,
                firmware,
                before,
                offset_attempted,
                fan_attempted,
                gpoc_attempted,
            )
        })?;
    if after.confirmed_mode() != Some(mode) {
        return Err(failed_with_rollback(
            format!(
                "final compound GPU-mode state does not confirm {}: {after:?}",
                mode.label()
            ),
            clocks,
            firmware,
            before,
            offset_attempted,
            fan_attempted,
            gpoc_attempted,
        ));
    }
    Ok(after)
}

fn verify_offsets(clocks: &mut impl GpuClockIo, target: GpuClockOffsets) -> Result<(), String> {
    let graphics = clocks.read_offset(GpuClockDomain::Graphics)?;
    let memory = clocks.read_offset(GpuClockDomain::Memory)?;
    if graphics.current_mhz != target.graphics_mhz || memory.current_mhz != target.memory_mhz {
        return Err(format!(
            "NVML P0 offset readback mismatch: requested {}/{} MHz, got {}/{} MHz",
            target.graphics_mhz, target.memory_mhz, graphics.current_mhz, memory.current_mhz
        ));
    }
    Ok(())
}

fn failed_with_rollback(
    failure: String,
    clocks: &mut impl GpuClockIo,
    firmware: &mut impl GpuFirmwareIo,
    before: GpuModeState,
    offset_attempted: bool,
    fan_attempted: bool,
    gpoc_attempted: bool,
) -> String {
    let mut rollback = Vec::new();
    if gpoc_attempted {
        rollback.push(restore_firmware_value(
            firmware,
            GpuFirmwareField::Gpoc,
            before.gpoc,
        ));
    }
    if fan_attempted {
        rollback.push(restore_firmware_value(
            firmware,
            GpuFirmwareField::FanTable,
            before.fan_table,
        ));
    }
    if offset_attempted {
        rollback.push(restore_offset(
            clocks,
            GpuClockDomain::Memory,
            before.memory.current_mhz,
        ));
        rollback.push(restore_offset(
            clocks,
            GpuClockDomain::Graphics,
            before.graphics.current_mhz,
        ));
    }
    if rollback.is_empty() {
        format!("{failure}; no mutation was attempted")
    } else {
        format!("{failure}; rollback: {}", rollback.join("; "))
    }
}

#[derive(Clone, Copy)]
enum GpuFirmwareField {
    FanTable,
    Gpoc,
}

fn restore_firmware_value(
    firmware: &mut impl GpuFirmwareIo,
    field: GpuFirmwareField,
    previous: u8,
) -> String {
    let label = match field {
        GpuFirmwareField::FanTable => "fan table",
        GpuFirmwareField::Gpoc => "GPOC",
    };
    if matches!(get_firmware_value(firmware, field), Ok(current) if current == previous) {
        return format!("{label} already at saved {previous}");
    }
    let setter = set_firmware_value(firmware, field, previous);
    let readback = get_firmware_value(firmware, field);
    match (setter, readback) {
        (Ok(()), Ok(current)) if current == previous => {
            format!("{label} restored and getter-confirmed at {previous}")
        }
        (Ok(()), Ok(current)) => {
            format!("{label} restore readback mismatch: wanted {previous}, got {current}")
        }
        (Ok(()), Err(error)) => format!("{label} restore getter failed: {error}"),
        (Err(setter), Ok(current)) if current == previous => format!(
            "{label} restore setter failed: {setter}; getter nevertheless confirms saved {previous}"
        ),
        (Err(setter), Ok(current)) => format!(
            "{label} restore setter failed: {setter}; readback mismatch: wanted {previous}, got {current}"
        ),
        (Err(setter), Err(getter)) => format!(
            "{label} restore setter failed: {setter}; restore getter also failed: {getter}"
        ),
    }
}

fn get_firmware_value(
    firmware: &mut impl GpuFirmwareIo,
    field: GpuFirmwareField,
) -> Result<u8, String> {
    match field {
        GpuFirmwareField::FanTable => firmware.get_fan_table(),
        GpuFirmwareField::Gpoc => firmware.get_gpoc(),
    }
}

fn set_firmware_value(
    firmware: &mut impl GpuFirmwareIo,
    field: GpuFirmwareField,
    value: u8,
) -> Result<(), String> {
    match field {
        GpuFirmwareField::FanTable => firmware.set_fan_table(value),
        GpuFirmwareField::Gpoc => firmware.set_gpoc(value),
    }
}

fn restore_offset(clocks: &mut impl GpuClockIo, domain: GpuClockDomain, previous: i32) -> String {
    let label = match domain {
        GpuClockDomain::Graphics => "P0 graphics offset",
        GpuClockDomain::Memory => "P0 memory offset",
    };
    if matches!(clocks.read_offset(domain), Ok(current) if current.current_mhz == previous) {
        return format!("{label} already at saved {previous} MHz");
    }
    let setter = clocks.set_offset(domain, previous);
    let readback = clocks.read_offset(domain);
    match (setter, readback) {
        (Ok(()), Ok(current)) if current.current_mhz == previous => {
            format!("{label} restored and getter-confirmed at {previous} MHz")
        }
        (Ok(()), Ok(current)) => format!(
            "{label} restore readback mismatch: wanted {previous} MHz, got {} MHz",
            current.current_mhz
        ),
        (Ok(()), Err(error)) => format!("{label} restore getter failed: {error}"),
        (Err(setter), Ok(current)) if current.current_mhz == previous => format!(
            "{label} restore setter failed: {setter}; getter nevertheless confirms saved {previous} MHz"
        ),
        (Err(setter), Ok(current)) => format!(
            "{label} restore setter failed: {setter}; readback mismatch: wanted {previous} MHz, got {} MHz",
            current.current_mhz
        ),
        (Err(setter), Err(getter)) => format!(
            "{label} restore setter failed: {setter}; restore getter also failed: {getter}"
        ),
    }
}

#[cfg(target_os = "linux")]
struct NvmlClockIo {
    library: NvmlLibrary,
    device: *mut std::ffi::c_void,
}

#[cfg(target_os = "linux")]
impl NvmlClockIo {
    fn open_exact_ph315_53() -> Result<Self, String> {
        let bus_id = exact_ph315_53_gpu_bus(Path::new("/sys/bus/pci/devices"))?;
        let library = NvmlLibrary::open()?;
        let bus = std::ffi::CString::new(bus_id.clone()).map_err(|error| error.to_string())?;
        let mut device = std::ptr::null_mut();
        let result = unsafe { (library.get_by_pci_bus_id)(bus.as_ptr(), &mut device) };
        library.check(result, "nvmlDeviceGetHandleByPciBusId_v2")?;
        if device.is_null() {
            return Err(format!(
                "NVML returned a null handle for exact GPU {bus_id}"
            ));
        }
        let mut uuid = [0i8; 96];
        let result = unsafe {
            (library.get_uuid)(device, uuid.as_mut_ptr(), uuid.len() as std::ffi::c_uint)
        };
        library.check(result, "nvmlDeviceGetUUID")?;
        let uuid = unsafe { std::ffi::CStr::from_ptr(uuid.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        if !uuid.starts_with("GPU-") {
            return Err(format!(
                "NVML returned malformed UUID {uuid:?} for exact GPU {bus_id}"
            ));
        }
        Ok(NvmlClockIo { library, device })
    }
}

#[cfg(not(target_os = "linux"))]
struct NvmlClockIo;

#[cfg(not(target_os = "linux"))]
impl NvmlClockIo {
    fn open_exact_ph315_53() -> Result<Self, String> {
        Err("OEM GPU modes require Linux NVML".into())
    }
}

#[cfg(not(target_os = "linux"))]
impl GpuClockIo for NvmlClockIo {
    fn read_offset(&mut self, _domain: GpuClockDomain) -> Result<GpuOffsetRange, String> {
        Err("OEM GPU modes require Linux NVML".into())
    }

    fn set_offset(&mut self, _domain: GpuClockDomain, _offset_mhz: i32) -> Result<(), String> {
        Err("OEM GPU modes require Linux NVML".into())
    }
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct NvmlClockOffset {
    version: std::ffi::c_uint,
    clock_type: std::ffi::c_int,
    pstate: std::ffi::c_int,
    clock_offset_mhz: std::ffi::c_int,
    min_clock_offset_mhz: std::ffi::c_int,
    max_clock_offset_mhz: std::ffi::c_int,
}

#[cfg(target_os = "linux")]
impl NvmlClockOffset {
    fn new(domain: GpuClockDomain, value: i32) -> Self {
        NvmlClockOffset {
            version: (1 << 24) | std::mem::size_of::<NvmlClockOffset>() as u32,
            clock_type: match domain {
                GpuClockDomain::Graphics => 0,
                GpuClockDomain::Memory => 2,
            },
            pstate: 0,
            clock_offset_mhz: value,
            min_clock_offset_mhz: 0,
            max_clock_offset_mhz: 0,
        }
    }
}

#[cfg(target_os = "linux")]
impl GpuClockIo for NvmlClockIo {
    fn read_offset(&mut self, domain: GpuClockDomain) -> Result<GpuOffsetRange, String> {
        let mut info = NvmlClockOffset::new(domain, 0);
        let result = unsafe { (self.library.get_clock_offsets)(self.device, &mut info) };
        self.library.check(result, "nvmlDeviceGetClockOffsets")?;
        Ok(GpuOffsetRange {
            current_mhz: info.clock_offset_mhz,
            min_mhz: info.min_clock_offset_mhz,
            max_mhz: info.max_clock_offset_mhz,
        })
    }

    fn set_offset(&mut self, domain: GpuClockDomain, offset_mhz: i32) -> Result<(), String> {
        let mut info = NvmlClockOffset::new(domain, offset_mhz);
        let result = unsafe { (self.library.set_clock_offsets)(self.device, &mut info) };
        self.library.check(result, "nvmlDeviceSetClockOffsets")
    }
}

#[cfg(target_os = "linux")]
type NvmlReturn = std::ffi::c_int;
#[cfg(target_os = "linux")]
type NvmlDevice = *mut std::ffi::c_void;
#[cfg(target_os = "linux")]
struct NvmlLibrary {
    handle: *mut std::ffi::c_void,
    shutdown: unsafe extern "C" fn() -> NvmlReturn,
    error_string: unsafe extern "C" fn(NvmlReturn) -> *const std::ffi::c_char,
    get_by_pci_bus_id: unsafe extern "C" fn(*const std::ffi::c_char, *mut NvmlDevice) -> NvmlReturn,
    get_uuid:
        unsafe extern "C" fn(NvmlDevice, *mut std::ffi::c_char, std::ffi::c_uint) -> NvmlReturn,
    get_clock_offsets: unsafe extern "C" fn(NvmlDevice, *mut NvmlClockOffset) -> NvmlReturn,
    set_clock_offsets: unsafe extern "C" fn(NvmlDevice, *mut NvmlClockOffset) -> NvmlReturn,
}

/// Owns a successful `dlopen` until every required symbol has been resolved.
///
/// `NvmlLibrary` cannot own the handle before its function table is complete,
/// so this guard closes it on any early `?` from `load_symbol`.
#[cfg(target_os = "linux")]
struct PendingDlHandle(*mut std::ffi::c_void);

#[cfg(target_os = "linux")]
impl PendingDlHandle {
    fn get(&self) -> *mut std::ffi::c_void {
        self.0
    }

    fn into_raw(mut self) -> *mut std::ffi::c_void {
        let handle = self.0;
        self.0 = std::ptr::null_mut();
        handle
    }
}

#[cfg(target_os = "linux")]
impl Drop for PendingDlHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                linux_dlclose(self.0);
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl NvmlLibrary {
    fn open() -> Result<Self, String> {
        let candidates = [
            "/run/opengl-driver/lib/libnvidia-ml.so.1",
            "libnvidia-ml.so.1",
            "libnvidia-ml.so",
        ];
        let mut errors = Vec::new();
        let mut handle = std::ptr::null_mut();
        for candidate in candidates {
            let name = std::ffi::CString::new(candidate).expect("static library name");
            handle = unsafe { linux_dlopen(name.as_ptr(), 2) };
            if !handle.is_null() {
                break;
            }
            errors.push(format!("{candidate}: {}", dl_error()));
        }
        if handle.is_null() {
            return Err(format!("could not load NVML: {}", errors.join("; ")));
        }

        let pending_handle = PendingDlHandle(handle);
        let handle = pending_handle.get();

        let init: unsafe extern "C" fn() -> NvmlReturn =
            unsafe { load_symbol(handle, b"nvmlInit_v2\0") }?;
        let shutdown: unsafe extern "C" fn() -> NvmlReturn =
            unsafe { load_symbol(handle, b"nvmlShutdown\0") }?;
        let error_string: unsafe extern "C" fn(NvmlReturn) -> *const std::ffi::c_char =
            unsafe { load_symbol(handle, b"nvmlErrorString\0") }?;
        let get_by_pci_bus_id: unsafe extern "C" fn(
            *const std::ffi::c_char,
            *mut NvmlDevice,
        ) -> NvmlReturn = unsafe { load_symbol(handle, b"nvmlDeviceGetHandleByPciBusId_v2\0") }?;
        let get_uuid: unsafe extern "C" fn(
            NvmlDevice,
            *mut std::ffi::c_char,
            std::ffi::c_uint,
        ) -> NvmlReturn = unsafe { load_symbol(handle, b"nvmlDeviceGetUUID\0") }?;
        let get_clock_offsets: unsafe extern "C" fn(
            NvmlDevice,
            *mut NvmlClockOffset,
        ) -> NvmlReturn = unsafe { load_symbol(handle, b"nvmlDeviceGetClockOffsets\0") }?;
        let set_clock_offsets: unsafe extern "C" fn(
            NvmlDevice,
            *mut NvmlClockOffset,
        ) -> NvmlReturn = unsafe { load_symbol(handle, b"nvmlDeviceSetClockOffsets\0") }?;
        let result = unsafe { init() };
        if result != 0 {
            let message = unsafe {
                let ptr = error_string(result);
                if ptr.is_null() {
                    format!("NVML error {result}")
                } else {
                    std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };
            return Err(format!("nvmlInit_v2 failed: {message}"));
        }
        Ok(NvmlLibrary {
            handle: pending_handle.into_raw(),
            shutdown,
            error_string,
            get_by_pci_bus_id,
            get_uuid,
            get_clock_offsets,
            set_clock_offsets,
        })
    }

    fn check(&self, result: NvmlReturn, operation: &str) -> Result<(), String> {
        if result == 0 {
            return Ok(());
        }
        let message = unsafe {
            let ptr = (self.error_string)(result);
            if ptr.is_null() {
                format!("NVML error {result}")
            } else {
                std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        };
        Err(format!("{operation}: {message} ({result})"))
    }
}

#[cfg(target_os = "linux")]
impl Drop for NvmlLibrary {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
            linux_dlclose(self.handle);
        }
    }
}

#[cfg(target_os = "linux")]
unsafe fn load_symbol<T: Copy>(
    handle: *mut std::ffi::c_void,
    name: &'static [u8],
) -> Result<T, String> {
    let name = std::ffi::CStr::from_bytes_with_nul(name).map_err(|error| error.to_string())?;
    linux_dlerror();
    let pointer = linux_dlsym(handle, name.as_ptr());
    if pointer.is_null() {
        return Err(format!(
            "missing NVML symbol {}: {}",
            name.to_string_lossy(),
            dl_error()
        ));
    }
    if std::mem::size_of::<T>() != std::mem::size_of_val(&pointer) {
        return Err(format!(
            "NVML symbol {} has an unsupported function-pointer size",
            name.to_string_lossy()
        ));
    }
    Ok(std::mem::transmute_copy(&pointer))
}

#[cfg(target_os = "linux")]
fn dl_error() -> String {
    unsafe {
        let pointer = linux_dlerror();
        if pointer.is_null() {
            "unknown dynamic-loader error".into()
        } else {
            std::ffi::CStr::from_ptr(pointer)
                .to_string_lossy()
                .into_owned()
        }
    }
}

#[cfg(target_os = "linux")]
#[link(name = "dl")]
extern "C" {
    #[link_name = "dlopen"]
    fn linux_dlopen(
        filename: *const std::ffi::c_char,
        flags: std::ffi::c_int,
    ) -> *mut std::ffi::c_void;
    #[link_name = "dlsym"]
    fn linux_dlsym(
        handle: *mut std::ffi::c_void,
        symbol: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
    #[link_name = "dlclose"]
    fn linux_dlclose(handle: *mut std::ffi::c_void) -> std::ffi::c_int;
    #[link_name = "dlerror"]
    fn linux_dlerror() -> *const std::ffi::c_char;
}

#[cfg(any(target_os = "linux", test))]
fn exact_ph315_53_gpu_bus(root: &Path) -> Result<String, String> {
    let entries = std::fs::read_dir(root)
        .map_err(|error| format!("cannot enumerate {}: {error}", root.display()))?;
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ids = [
            read_text(&path.join("vendor")),
            read_text(&path.join("device")),
            read_text(&path.join("subsystem_vendor")),
            read_text(&path.join("subsystem_device")),
        ];
        let exact = ids[0].as_deref() == Some("0x10de")
            && ids[1].as_deref() == Some("0x1f15")
            && ids[2].as_deref() == Some("0x1025")
            && ids[3].as_deref() == Some("0x1442");
        let display_class = read_text(&path.join("class"))
            .and_then(|value| u32::from_str_radix(value.trim_start_matches("0x"), 16).ok())
            .is_some_and(|class| class >> 16 == 0x03);
        if exact && display_class {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                matches.push(name.to_owned());
            }
        }
    }
    match matches.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err("exact PH315-53 GPU 10de:1f15 subsystem 1025:1442 was not found in sysfs".into()),
        many => Err(format!(
            "refusing GPU mode: found {} exact PH315-53 display GPUs ({})",
            many.len(),
            many.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(label: &str) -> Self {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "alien-performance-{}-{label}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempTree(path)
        }

        fn write(&self, relative: &str, value: &str) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, value).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct MockClocks {
        graphics: GpuOffsetRange,
        memory: GpuOffsetRange,
        log: Rc<RefCell<Vec<String>>>,
        fail_once: Option<GpuClockDomain>,
        fail_always: Option<GpuClockDomain>,
    }

    impl MockClocks {
        fn normal(log: Rc<RefCell<Vec<String>>>) -> Self {
            MockClocks {
                graphics: GpuOffsetRange {
                    current_mhz: 0,
                    min_mhz: -1000,
                    max_mhz: 1000,
                },
                memory: GpuOffsetRange {
                    current_mhz: 0,
                    min_mhz: -2000,
                    max_mhz: 6000,
                },
                log,
                fail_once: None,
                fail_always: None,
            }
        }
    }

    impl GpuClockIo for MockClocks {
        fn read_offset(&mut self, domain: GpuClockDomain) -> Result<GpuOffsetRange, String> {
            self.log.borrow_mut().push(format!("get-clock-{domain:?}"));
            Ok(match domain {
                GpuClockDomain::Graphics => self.graphics,
                GpuClockDomain::Memory => self.memory,
            })
        }

        fn set_offset(&mut self, domain: GpuClockDomain, value: i32) -> Result<(), String> {
            self.log
                .borrow_mut()
                .push(format!("set-clock-{domain:?}-{value}"));
            match domain {
                GpuClockDomain::Graphics => self.graphics.current_mhz = value,
                GpuClockDomain::Memory => self.memory.current_mhz = value,
            }
            if self.fail_always == Some(domain) {
                return Err("injected persistent setter failure after apply".into());
            }
            if self.fail_once == Some(domain) {
                self.fail_once = None;
                return Err("injected one-shot setter failure after apply".into());
            }
            Ok(())
        }
    }

    struct MockFirmware {
        fan_table: u8,
        gpoc: u8,
        log: Rc<RefCell<Vec<String>>>,
        fail_fan_once: bool,
        fail_fan_always: bool,
        fail_gpoc_once: bool,
    }

    impl MockFirmware {
        fn normal(log: Rc<RefCell<Vec<String>>>) -> Self {
            MockFirmware {
                fan_table: 1,
                gpoc: 0,
                log,
                fail_fan_once: false,
                fail_fan_always: false,
                fail_gpoc_once: false,
            }
        }
    }

    impl GpuFirmwareIo for MockFirmware {
        fn get_fan_table(&mut self) -> Result<u8, String> {
            self.log.borrow_mut().push("get-fan".into());
            Ok(self.fan_table)
        }

        fn set_fan_table(&mut self, table: u8) -> Result<(), String> {
            self.log.borrow_mut().push(format!("set-fan-{table}"));
            self.fan_table = table;
            if self.fail_fan_always {
                return Err("injected persistent fan setter failure after apply".into());
            }
            if self.fail_fan_once {
                self.fail_fan_once = false;
                return Err("injected fan setter failure after apply".into());
            }
            Ok(())
        }

        fn get_gpoc(&mut self) -> Result<u8, String> {
            self.log.borrow_mut().push("get-gpoc".into());
            Ok(self.gpoc)
        }

        fn set_gpoc(&mut self, level: u8) -> Result<(), String> {
            self.log.borrow_mut().push(format!("set-gpoc-{level}"));
            self.gpoc = level;
            if self.fail_gpoc_once {
                self.fail_gpoc_once = false;
                return Err("injected GPOC setter failure after apply".into());
            }
            Ok(())
        }
    }

    fn mutation_log(log: &Rc<RefCell<Vec<String>>>) -> Vec<String> {
        log.borrow()
            .iter()
            .filter(|entry| entry.starts_with("set-"))
            .cloned()
            .collect()
    }

    #[test]
    fn parses_nvidia_csv() {
        assert_eq!(
            parse_nvidia("975, 2100, 42\n"),
            (Some(975), Some(2100), Some(42))
        );
        assert_eq!(parse_nvidia("N/A, 2100, N/A\n"), (None, Some(2100), None));
    }

    #[test]
    fn gpu_modes_have_exact_predatorsense_offsets() {
        // The native helper is max(cpu,gpu)+1. CPU OC is disabled on this
        // exact package, so the modeled CPU contribution must stay zero.
        assert_eq!(PH315_53_REQUESTED_CPU_LEVEL, 0);
        assert_eq!(GpuMode::Normal.offsets(), GpuClockOffsets::default());
        assert_eq!(
            GpuMode::Faster.offsets(),
            GpuClockOffsets {
                graphics_mhz: 50,
                memory_mhz: 30
            }
        );
        assert_eq!(
            GpuMode::Turbo.offsets(),
            GpuClockOffsets {
                graphics_mhz: 100,
                memory_mhz: 60
            }
        );
        assert_eq!(GpuMode::Normal.fan_table(), 1);
        assert_eq!(GpuMode::Faster.fan_table(), 2);
        assert_eq!(GpuMode::Turbo.fan_table(), 3);
        assert_eq!(acer_shared_fan_table(2, GpuMode::Normal as u8), 3);
        assert_eq!(acer_shared_fan_table(3, GpuMode::Faster as u8), 4);
    }

    #[test]
    fn gpu_mode_requires_the_exact_opt_in() {
        assert!(GpuModeOptIn::acknowledge(GPU_MODE_ACKNOWLEDGEMENT).is_ok());
        assert!(GpuModeOptIn::acknowledge("yes").is_err());
        assert!(GpuModeOptIn::acknowledge("").is_err());
    }

    #[test]
    fn gpu_mode_snapshot_exposes_live_range_gating_for_frontends() {
        let state = GpuModeState {
            graphics: GpuOffsetRange {
                current_mhz: 0,
                min_mhz: -50,
                max_mhz: 99,
            },
            memory: GpuOffsetRange {
                current_mhz: 0,
                min_mhz: -100,
                max_mhz: 100,
            },
            fan_table: 1,
            gpoc: 0,
        };
        assert!(state.target_fits(GpuMode::Normal));
        assert!(state.target_fits(GpuMode::Faster));
        assert!(!state.target_fits(GpuMode::Turbo));
    }

    #[test]
    fn compound_gpu_mode_writes_in_acer_order_and_confirms_every_leg() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut clocks = MockClocks::normal(Rc::clone(&log));
        let mut firmware = MockFirmware::normal(Rc::clone(&log));
        let state = apply_gpu_mode_with(&mut clocks, &mut firmware, GpuMode::Turbo).unwrap();
        assert_eq!(state.confirmed_mode(), Some(GpuMode::Turbo));
        assert_eq!(
            mutation_log(&log),
            [
                "set-clock-Graphics-100",
                "set-clock-Memory-60",
                "set-fan-3",
                "set-gpoc-2",
            ]
        );
        assert_eq!(
            log.borrow()
                .iter()
                .filter(|entry| entry.as_str() == "get-gpoc")
                .count(),
            2,
            "one pre-state getter and one setter readback; a redundant final fn23 would notify the GPU again"
        );
    }

    #[test]
    fn normal_means_zero_offsets_not_restore_the_previous_mode() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut clocks = MockClocks::normal(Rc::clone(&log));
        clocks.graphics.current_mhz = 100;
        clocks.memory.current_mhz = 60;
        let mut firmware = MockFirmware::normal(Rc::clone(&log));
        firmware.fan_table = 3;
        firmware.gpoc = 2;
        let state = apply_gpu_mode_with(&mut clocks, &mut firmware, GpuMode::Normal).unwrap();
        assert_eq!(state.confirmed_mode(), Some(GpuMode::Normal));
        assert_eq!(state.graphics.current_mhz, 0);
        assert_eq!(state.memory.current_mhz, 0);
    }

    #[test]
    fn out_of_range_target_aborts_before_any_mutation() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut clocks = MockClocks::normal(Rc::clone(&log));
        clocks.graphics.max_mhz = 99;
        let mut firmware = MockFirmware::normal(Rc::clone(&log));
        let error = apply_gpu_mode_with(&mut clocks, &mut firmware, GpuMode::Turbo).unwrap_err();
        assert!(error.contains("outside driver range"));
        assert!(mutation_log(&log).is_empty());
    }

    #[test]
    fn partial_memory_failure_rolls_back_both_offsets_without_touching_firmware() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut clocks = MockClocks::normal(Rc::clone(&log));
        clocks.fail_once = Some(GpuClockDomain::Memory);
        let mut firmware = MockFirmware::normal(Rc::clone(&log));
        let error = apply_gpu_mode_with(&mut clocks, &mut firmware, GpuMode::Turbo).unwrap_err();
        assert!(error.contains("memory setter failed"));
        assert!(error.contains("P0 memory offset restored and getter-confirmed at 0 MHz"));
        assert!(error.contains("P0 graphics offset restored and getter-confirmed at 0 MHz"));
        assert_eq!(clocks.graphics.current_mhz, 0);
        assert_eq!(clocks.memory.current_mhz, 0);
        assert_eq!(firmware.fan_table, 1);
        assert_eq!(firmware.gpoc, 0);
        assert!(!mutation_log(&log)
            .iter()
            .any(|entry| entry.starts_with("set-fan") || entry.starts_with("set-gpoc")));
    }

    #[test]
    fn partial_fan_failure_rolls_back_fan_then_offsets() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut clocks = MockClocks::normal(Rc::clone(&log));
        let mut firmware = MockFirmware::normal(Rc::clone(&log));
        firmware.fail_fan_once = true;
        let error = apply_gpu_mode_with(&mut clocks, &mut firmware, GpuMode::Turbo).unwrap_err();
        assert!(error.contains("fan-table setter failed"));
        assert_eq!(clocks.graphics.current_mhz, 0);
        assert_eq!(clocks.memory.current_mhz, 0);
        assert_eq!(firmware.fan_table, 1);
        assert_eq!(firmware.gpoc, 0);
        let mutations = mutation_log(&log);
        assert_eq!(
            &mutations[mutations.len() - 3..],
            ["set-fan-1", "set-clock-Memory-0", "set-clock-Graphics-0"]
        );
    }

    #[test]
    fn partial_gpoc_failure_rolls_back_every_leg_in_reverse_order() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut clocks = MockClocks::normal(Rc::clone(&log));
        let mut firmware = MockFirmware::normal(Rc::clone(&log));
        firmware.fail_gpoc_once = true;
        let error = apply_gpu_mode_with(&mut clocks, &mut firmware, GpuMode::Turbo).unwrap_err();
        assert!(error.contains("GPOC setter failed"));
        assert_eq!(clocks.graphics.current_mhz, 0);
        assert_eq!(clocks.memory.current_mhz, 0);
        assert_eq!(firmware.fan_table, 1);
        assert_eq!(firmware.gpoc, 0);
        let mutations = mutation_log(&log);
        assert_eq!(
            &mutations[mutations.len() - 4..],
            [
                "set-gpoc-0",
                "set-fan-1",
                "set-clock-Memory-0",
                "set-clock-Graphics-0"
            ]
        );
    }

    #[test]
    fn rollback_failure_is_never_hidden() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut clocks = MockClocks::normal(Rc::clone(&log));
        clocks.fail_always = Some(GpuClockDomain::Graphics);
        let mut firmware = MockFirmware::normal(Rc::clone(&log));
        let error = apply_gpu_mode_with(&mut clocks, &mut firmware, GpuMode::Turbo).unwrap_err();
        assert!(error.contains("graphics setter failed"));
        assert!(error.contains("P0 graphics offset restore setter failed"));
        assert!(error.contains("getter nevertheless confirms saved 0 MHz"));
        assert!(log
            .borrow()
            .windows(2)
            .any(|pair| pair == ["set-clock-Graphics-0", "get-clock-Graphics"]));
    }

    #[test]
    fn firmware_rollback_setter_error_is_followed_by_readback() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut clocks = MockClocks::normal(Rc::clone(&log));
        let mut firmware = MockFirmware::normal(Rc::clone(&log));
        firmware.fail_fan_always = true;
        let error = apply_gpu_mode_with(&mut clocks, &mut firmware, GpuMode::Turbo).unwrap_err();
        assert!(error.contains("fan table restore setter failed"));
        assert!(error.contains("getter nevertheless confirms saved 1"));
        assert!(log
            .borrow()
            .windows(2)
            .any(|pair| pair == ["set-fan-1", "get-fan"]));
    }

    #[test]
    fn exact_gpu_guard_requires_all_four_ids_and_one_display_function() {
        let tree = TempTree::new("gpu-guard");
        tree.write("0000:01:00.0/vendor", "0x10de\n");
        tree.write("0000:01:00.0/device", "0x1f15\n");
        tree.write("0000:01:00.0/subsystem_vendor", "0x1025\n");
        tree.write("0000:01:00.0/subsystem_device", "0x1442\n");
        tree.write("0000:01:00.0/class", "0x030000\n");
        assert_eq!(exact_ph315_53_gpu_bus(&tree.0).unwrap(), "0000:01:00.0");

        tree.write("0000:02:00.0/vendor", "0x10de\n");
        tree.write("0000:02:00.0/device", "0x1f15\n");
        tree.write("0000:02:00.0/subsystem_vendor", "0x1025\n");
        tree.write("0000:02:00.0/subsystem_device", "0x1442\n");
        tree.write("0000:02:00.0/class", "0x030200\n");
        assert!(exact_ph315_53_gpu_bus(&tree.0)
            .unwrap_err()
            .contains("found 2 exact"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "read-only target-hardware NVML probe"]
    fn live_exact_target_nvml_p0_offsets_are_readable_and_fit_oem_modes() {
        let mut nvml = NvmlClockIo::open_exact_ph315_53().unwrap();
        let graphics = nvml.read_offset(GpuClockDomain::Graphics).unwrap();
        let memory = nvml.read_offset(GpuClockDomain::Memory).unwrap();
        for mode in [GpuMode::Normal, GpuMode::Faster, GpuMode::Turbo] {
            let target = mode.offsets();
            assert!(graphics.contains(target.graphics_mhz));
            assert!(memory.contains(target.memory_mhz));
        }
    }

    #[test]
    fn averages_cpu_khz_as_mhz() {
        assert_eq!(mean_khz(&[800_000, 4_200_000]), Some(2500));
        assert_eq!(mean_khz(&[]), None);
    }

    #[test]
    fn exact_ph315_53_target_is_one_policy() {
        assert_eq!(PH315_53_CPU_POWER.pl1_uw, 70_000_000);
        assert_eq!(PH315_53_CPU_POWER.pl2_uw, 107_000_000);
        assert_eq!(PH315_53_CPU_POWER.pl1_time_window_us, 28_000_000);
    }

    #[test]
    fn discovers_constraints_by_name_not_index() {
        let tree = TempTree::new("named");
        tree.write("dmi/sys_vendor", "Acer\n");
        tree.write("dmi/product_name", "Predator PH315-53\n");
        tree.write("power/intel-rapl:0/name", "package-0\n");
        tree.write("power/intel-rapl:0/enabled", "1\n");
        tree.write("power/intel-rapl:0/constraint_7_name", "long_term\n");
        tree.write(
            "power/intel-rapl:0/constraint_7_power_limit_uw",
            "45000000\n",
        );
        tree.write(
            "power/intel-rapl:0/constraint_7_max_power_uw",
            "120000000\n",
        );
        tree.write(
            "power/intel-rapl:0/constraint_7_time_window_us",
            "28000000\n",
        );
        tree.write("power/intel-rapl:0/constraint_2_name", "short-term\n");
        tree.write(
            "power/intel-rapl:0/constraint_2_power_limit_uw",
            "90000000\n",
        );
        tree.write(
            "power/intel-rapl:0/constraint_2_max_power_uw",
            "120000000\n",
        );

        let status = PowercapStatus::sample_at(&tree.0.join("power"), &tree.0.join("dmi"));
        assert!(status.is_ph315_53());
        assert!(status.can_read_limits());
        assert!(status.write_gap().contains("have not been verified live"));
        let package = status.package.unwrap();
        assert_eq!(package.pl1.unwrap().index, 7);
        assert_eq!(package.pl2.unwrap().index, 2);
    }

    #[test]
    fn refuses_to_treat_an_unnamed_constraint_as_pl1() {
        assert_eq!(classify_constraint("constraint_0"), None);
        assert_eq!(classify_constraint("long_term"), Some(ConstraintKind::Pl1));
        assert_eq!(classify_constraint("short-term"), Some(ConstraintKind::Pl2));
    }

    #[test]
    fn write_gap_is_model_guarded() {
        let tree = TempTree::new("wrong-model");
        tree.write("dmi/sys_vendor", "Acer\n");
        tree.write("dmi/product_name", "Predator PH315-54\n");
        let status = PowercapStatus::sample_at(&tree.0.join("power"), &tree.0.join("dmi"));
        assert!(status.write_gap().contains("not Acer Predator PH315-53"));
    }

    #[test]
    fn exact_model_without_powercap_stays_read_only() {
        let tree = TempTree::new("no-powercap");
        tree.write("dmi/sys_vendor", "Acer\n");
        tree.write("dmi/product_name", "Predator PH315-53\n");
        let status = PowercapStatus::sample_at(&tree.0.join("power"), &tree.0.join("dmi"));
        assert_eq!(
            status.write_gap(),
            "disabled: no Intel package zone was found under /sys/class/powercap"
        );
    }
}
