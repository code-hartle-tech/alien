//! Which limiter is actually binding.
//!
//! # Why this exists
//!
//! Every negative result this project has recorded came from tuning a limiter
//! that was not the one holding the machine back. GPU clock offsets of
//! +100/+60 MHz measured `855/853/855/830` — nothing — because the GPU was
//! sitting at 77–79 W against an ~80 W board power cap, so moving the
//! voltage/frequency curve up could not raise a clock the power budget was
//! already deciding. CoolBoost measured nothing for the same class of reason.
//! Meanwhile pinning the fans, which addressed the limiter that *was* binding,
//! measured +61.8 %.
//!
//! Neither PredatorSense nor any Linux vendor tool reports this. The result is
//! that everyone — including this project — tunes blind and then argues about
//! benchmark noise. Both sources below are cheap: the CPU side is plain sysfs
//! with no privileges and no MSR, and the GPU side is one NVML call.
//!
//! # What "binding" means
//!
//! A limiter is binding when relaxing *it* would raise performance. Reading a
//! high temperature does not prove thermal limiting — the reference machine
//! sits at 92 °C both when it is throttled to 1446 MHz and when it is running
//! 2406 MHz. What proves it is the throttle counter advancing, or NVML naming
//! the reason outright.

use std::fs;
use std::path::{Path, PathBuf};

/// CPU throttle event counters, read from `thermal_throttle` in sysfs.
///
/// Unprivileged, no MSR, and unaffected by kernel lockdown — which is why this
/// is the CPU-side source rather than `MSR_CORE_PERF_LIMIT_REASONS` (0x690),
/// whose availability on Comet Lake is unreliable and which needs root anyway.
///
/// Counters are monotonic since boot, so a single reading says nothing useful.
/// Take two around a workload and compare — see [`ThrottleCounters::delta`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThrottleCounters {
    /// Package hit its thermal limit and clocks were cut.
    pub package_throttle: Option<u64>,
    /// Package throttling severe enough to be flagged critical.
    pub package_power_limit: Option<u64>,
    /// Per-core thermal throttle events, summed across cores.
    pub core_throttle: Option<u64>,
}

impl ThrottleCounters {
    /// Read from a sysfs root. Pass `/sys/devices/system/cpu` in production.
    ///
    /// Package counters come from `cpu0` only. They are package-scoped, so
    /// every core reports the same value and summing them would multiply the
    /// count by the core number.
    pub fn read(cpu_root: &Path) -> ThrottleCounters {
        let pkg = cpu_root.join("cpu0/thermal_throttle");
        ThrottleCounters {
            package_throttle: read_u64(&pkg.join("package_throttle_count")),
            package_power_limit: read_u64(&pkg.join("package_power_limit_count")),
            core_throttle: sum_core_throttle(cpu_root),
        }
    }

    /// Counts accumulated between two readings.
    ///
    /// Saturating: a counter that resets (suspend, hotplug) yields zero rather
    /// than an absurd number.
    pub fn delta(&self, earlier: &ThrottleCounters) -> ThrottleCounters {
        fn d(now: Option<u64>, before: Option<u64>) -> Option<u64> {
            Some(now?.saturating_sub(before?))
        }
        ThrottleCounters {
            package_throttle: d(self.package_throttle, earlier.package_throttle),
            package_power_limit: d(self.package_power_limit, earlier.package_power_limit),
            core_throttle: d(self.core_throttle, earlier.core_throttle),
        }
    }

    /// Whether any counter advanced. Only meaningful on a [`Self::delta`].
    pub fn any_throttling(&self) -> bool {
        [
            self.package_throttle,
            self.package_power_limit,
            self.core_throttle,
        ]
        .iter()
        .any(|c| c.is_some_and(|v| v > 0))
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Sum `core_throttle_count` across every CPU that exposes one.
///
/// Core-scoped, unlike the package counters, so summing is correct here.
fn sum_core_throttle(cpu_root: &Path) -> Option<u64> {
    let entries = fs::read_dir(cpu_root).ok()?;
    let mut total = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `cpu0`..`cpuN`, not `cpufreq` or `cpuidle`.
        if !name.starts_with("cpu") || !name[3..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if name.len() == 3 {
            continue;
        }
        if let Some(v) = read_u64(&entry.path().join("thermal_throttle/core_throttle_count")) {
            total = Some(total.unwrap_or(0) + v);
        }
    }
    total
}

/// NVML's reasons for the GPU not running at its maximum clock.
///
/// Bit values are NVML's `nvmlClocksEventReason*` constants. Several are not
/// throttling at all — [`GpuLimiters::gpu_idle`] just means nothing is asking
/// for clocks — so the type keeps them distinct rather than collapsing
/// everything into a "throttled" boolean.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuLimiters {
    /// Nothing is running. Not a limit.
    pub gpu_idle: bool,
    /// Clocks pinned by an applications-clocks setting. Not a limit.
    pub applications_clocks: bool,
    /// **Software power cap.** The usual answer on this machine: the board
    /// power budget, not temperature, is choosing the clock. This is why the
    /// P0 offsets measured nothing.
    pub sw_power_cap: bool,
    /// Hardware slowdown — thermal, power brake or overcurrent. Severe.
    pub hw_slowdown: bool,
    /// Driver-side thermal slowdown.
    pub sw_thermal_slowdown: bool,
    /// Hardware thermal slowdown.
    pub hw_thermal_slowdown: bool,
    /// External power brake asserted.
    pub hw_power_brake: bool,
    /// Clocks held up for a display mode. Not a limit.
    pub display_clock_setting: bool,
    /// Sync boost group. Not a limit in a laptop.
    pub sync_boost: bool,
}

impl GpuLimiters {
    pub const GPU_IDLE: u64 = 0x0000_0000_0000_0001;
    pub const APPLICATIONS_CLOCKS: u64 = 0x0000_0000_0000_0002;
    pub const SW_POWER_CAP: u64 = 0x0000_0000_0000_0004;
    pub const HW_SLOWDOWN: u64 = 0x0000_0000_0000_0008;
    pub const SYNC_BOOST: u64 = 0x0000_0000_0000_0010;
    pub const SW_THERMAL_SLOWDOWN: u64 = 0x0000_0000_0000_0020;
    pub const HW_THERMAL_SLOWDOWN: u64 = 0x0000_0000_0000_0040;
    pub const HW_POWER_BRAKE: u64 = 0x0000_0000_0000_0080;
    pub const DISPLAY_CLOCK_SETTING: u64 = 0x0000_0000_0000_0100;

    /// Decode NVML's bitmask.
    pub fn from_bits(bits: u64) -> GpuLimiters {
        GpuLimiters {
            gpu_idle: bits & Self::GPU_IDLE != 0,
            applications_clocks: bits & Self::APPLICATIONS_CLOCKS != 0,
            sw_power_cap: bits & Self::SW_POWER_CAP != 0,
            hw_slowdown: bits & Self::HW_SLOWDOWN != 0,
            sync_boost: bits & Self::SYNC_BOOST != 0,
            sw_thermal_slowdown: bits & Self::SW_THERMAL_SLOWDOWN != 0,
            hw_thermal_slowdown: bits & Self::HW_THERMAL_SLOWDOWN != 0,
            hw_power_brake: bits & Self::HW_POWER_BRAKE != 0,
            display_clock_setting: bits & Self::DISPLAY_CLOCK_SETTING != 0,
        }
    }

    /// Whether any reason represents an actual performance limit.
    ///
    /// Idle, applications-clocks and display-clock are deliberately excluded:
    /// they explain a low clock without being something to fix.
    pub fn is_limited(&self) -> bool {
        self.sw_power_cap
            || self.hw_slowdown
            || self.sw_thermal_slowdown
            || self.hw_thermal_slowdown
            || self.hw_power_brake
    }

    /// The binding limiter, most severe first, as a short human phrase.
    pub fn summary(&self) -> &'static str {
        if self.hw_power_brake {
            "hardware power brake"
        } else if self.hw_thermal_slowdown {
            "hardware thermal slowdown"
        } else if self.sw_thermal_slowdown {
            "driver thermal slowdown"
        } else if self.hw_slowdown {
            "hardware slowdown"
        } else if self.sw_power_cap {
            "board power cap"
        } else if self.gpu_idle {
            "idle"
        } else if self.applications_clocks {
            "applications-clocks setting"
        } else if self.display_clock_setting {
            "display clock setting"
        } else {
            "none"
        }
    }
}

/// Everything the attribution layer can say at one instant.
#[derive(Debug, Clone, Default)]
pub struct Attribution {
    pub cpu: ThrottleCounters,
    /// `None` when NVML is unavailable — a laptop on the iGPU, a suspended
    /// dGPU, or no driver.
    pub gpu: Option<GpuLimiters>,
}

/// Default sysfs root for CPU topology.
///
/// Linux-only by construction. On any other target this returns a path that
/// cannot exist, so every counter reads `None` rather than zero.
///
/// The distinction matters more than it looks. A Linux path literal compiles
/// fine on Windows and simply finds nothing — and if a caller treated "no file"
/// as "no throttling", a Windows build would confidently report a healthy
/// machine it had never actually measured. That is precisely the failure mode
/// this module exists to prevent, so absence is modelled as [`None`] and
/// [`ThrottleCounters::any_throttling`] returns false only for counters that
/// genuinely read zero.
pub fn cpu_sysfs_root() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/sys/devices/system/cpu")
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Deliberately unopenable: throttle counters are a Linux kernel
        // interface and there is no equivalent to fall back to.
        PathBuf::from("/nonexistent/alien-throttle-counters-are-linux-only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Temp dir keyed by pid and a counter, cleaned on drop — the same shape
    /// the powercap tests use, so fixtures cannot collide under `--test-threads`.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(label: &str) -> TempTree {
            static N: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "alien-attr-{label}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create fixture root");
            TempTree(path)
        }

        fn cpu(&self, n: usize, file: &str, value: &str) {
            let dir = self.0.join(format!("cpu{n}/thermal_throttle"));
            fs::create_dir_all(&dir).expect("create cpu dir");
            fs::write(dir.join(file), value).expect("write counter");
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_package_counters_from_cpu0_only() {
        let t = TempTree::new("pkg");
        t.cpu(0, "package_throttle_count", "1200\n");
        t.cpu(0, "package_power_limit_count", "7\n");
        // cpu1 reports the same package value; it must not be added.
        t.cpu(1, "package_throttle_count", "1200\n");

        let c = ThrottleCounters::read(t.path());
        assert_eq!(c.package_throttle, Some(1200));
        assert_eq!(c.package_power_limit, Some(7));
    }

    #[test]
    fn sums_core_counters_across_cores() {
        let t = TempTree::new("core");
        t.cpu(0, "core_throttle_count", "10");
        t.cpu(1, "core_throttle_count", "20");
        t.cpu(2, "core_throttle_count", "30");
        assert_eq!(ThrottleCounters::read(t.path()).core_throttle, Some(60));
    }

    #[test]
    fn missing_counters_read_as_none_not_zero() {
        let t = TempTree::new("absent");
        let c = ThrottleCounters::read(t.path());
        assert_eq!(c.package_throttle, None, "absent must not look like zero");
        assert_eq!(c.core_throttle, None);
        assert!(!c.any_throttling());
    }

    #[test]
    fn delta_reports_accumulated_events() {
        let before = ThrottleCounters {
            package_throttle: Some(100),
            ..Default::default()
        };
        let after = ThrottleCounters {
            package_throttle: Some(1_393_039),
            ..Default::default()
        };
        let d = after.delta(&before);
        assert_eq!(d.package_throttle, Some(1_392_939));
        assert!(d.any_throttling());
    }

    #[test]
    fn delta_saturates_when_a_counter_resets() {
        let before = ThrottleCounters {
            package_throttle: Some(500),
            ..Default::default()
        };
        let after = ThrottleCounters {
            package_throttle: Some(3),
            ..Default::default()
        };
        assert_eq!(after.delta(&before).package_throttle, Some(0));
    }

    #[test]
    fn zero_delta_is_not_throttling() {
        let a = ThrottleCounters {
            package_throttle: Some(42),
            ..Default::default()
        };
        assert!(!a.delta(&a).any_throttling());
    }

    #[test]
    fn decodes_the_power_cap_reason_this_machine_actually_hits() {
        let g = GpuLimiters::from_bits(GpuLimiters::SW_POWER_CAP);
        assert!(g.sw_power_cap);
        assert!(g.is_limited());
        assert_eq!(g.summary(), "board power cap");
    }

    #[test]
    fn idle_is_reported_but_is_not_a_limit() {
        let g = GpuLimiters::from_bits(GpuLimiters::GPU_IDLE);
        assert!(g.gpu_idle);
        assert!(
            !g.is_limited(),
            "idle explains a low clock; it is not a cap"
        );
        assert_eq!(g.summary(), "idle");
    }

    #[test]
    fn summary_reports_the_most_severe_reason_first() {
        let g =
            GpuLimiters::from_bits(GpuLimiters::SW_POWER_CAP | GpuLimiters::HW_THERMAL_SLOWDOWN);
        assert_eq!(g.summary(), "hardware thermal slowdown");
    }

    #[test]
    fn no_bits_set_is_unlimited() {
        let g = GpuLimiters::from_bits(0);
        assert!(!g.is_limited());
        assert_eq!(g.summary(), "none");
    }

    #[test]
    fn every_documented_bit_decodes_to_its_own_field() {
        let cases = [
            (GpuLimiters::GPU_IDLE, "idle"),
            (
                GpuLimiters::APPLICATIONS_CLOCKS,
                "applications-clocks setting",
            ),
            (GpuLimiters::SW_POWER_CAP, "board power cap"),
            (GpuLimiters::HW_SLOWDOWN, "hardware slowdown"),
            (GpuLimiters::SW_THERMAL_SLOWDOWN, "driver thermal slowdown"),
            (
                GpuLimiters::HW_THERMAL_SLOWDOWN,
                "hardware thermal slowdown",
            ),
            (GpuLimiters::HW_POWER_BRAKE, "hardware power brake"),
            (GpuLimiters::DISPLAY_CLOCK_SETTING, "display clock setting"),
        ];
        for (bits, expected) in cases {
            assert_eq!(
                GpuLimiters::from_bits(bits).summary(),
                expected,
                "bits {bits:#x}"
            );
        }
    }
}
