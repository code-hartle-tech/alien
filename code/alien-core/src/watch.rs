//! Process lifetime tracking, and knowing when a game is actually running.
//!
//! # pidfd, not polling
//!
//! gamemode notices a dead client with a five-second sweep of `kill(pid, 0)`
//! across every registered PID. That has a real PID-reuse window: if the game's
//! PID is recycled inside those five seconds, gamemode stays engaged until an
//! unrelated process exits. Its D-Bus API even accepts pidfds — and then
//! discards them, keeping only the PID.
//!
//! `pidfd_open(2)` is strictly better and costs nothing. It has **no capability
//! check at all**, and polling the descriptor returns readable the moment the
//! task exits. Exact, immediate, and immune to reuse: the descriptor refers to
//! *that* process, not to a number that may later mean something else.
//!
//! # Detecting a game without being told
//!
//! The reliable trigger is gamemode, which every major launcher already drives.
//! But when nothing announces itself, NVML can answer the question directly:
//! `nvmlDeviceGetGraphicsRunningProcesses` lists the PIDs holding a graphics
//! context on the GPU. That is a far stronger signal than GPU utilisation,
//! which video playback and compositing trip constantly.
//!
//! None of the Linux vendor tools surveyed use it. It needs no privileges.
//!
//! ⚠ One caveat that matters on a laptop: initialising NVML **wakes a
//! runtime-suspended dGPU**. Polling it unconditionally would keep the discrete
//! GPU awake and cost battery for nothing, so callers must gate on the device
//! already being active — see [`dgpu_is_awake`].

use std::path::Path;

/// Owned pidfd. Closes on drop.
///
/// Deliberately thin: this exists to answer "is that exact process still
/// alive", and nothing else.
#[derive(Debug)]
pub struct PidFd {
    fd: i32,
    pid: i32,
}

// SAFETY: the fd is owned exclusively and only ever passed to poll/close.
unsafe impl Send for PidFd {}

unsafe extern "C" {
    fn syscall(num: std::ffi::c_long, ...) -> std::ffi::c_long;
    fn close(fd: i32) -> i32;
    fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

/// `__NR_pidfd_open` on x86-64 and aarch64 alike.
const SYS_PIDFD_OPEN: std::ffi::c_long = 434;
const POLLIN: i16 = 0x001;

impl PidFd {
    /// Open a descriptor for a live process.
    ///
    /// Fails if the process is already gone, which is itself useful: a caller
    /// racing a short-lived process learns immediately rather than watching a
    /// PID that now means nothing.
    pub fn open(pid: i32) -> Result<PidFd, String> {
        if pid <= 0 {
            return Err(format!("not a pid: {pid}"));
        }
        // SAFETY: pidfd_open takes (pid, flags) and returns an fd or -1.
        let fd = unsafe { syscall(SYS_PIDFD_OPEN, pid, 0i32) } as i32;
        if fd < 0 {
            return Err(format!("pidfd_open({pid}) failed; process may have exited"));
        }
        Ok(PidFd { fd, pid })
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Whether the process has exited. Never blocks.
    pub fn has_exited(&self) -> bool {
        self.wait_exit(0)
    }

    /// Block until the process exits or `timeout_ms` elapses.
    ///
    /// Returns true if it exited. A negative timeout waits indefinitely.
    pub fn wait_exit(&self, timeout_ms: i32) -> bool {
        let mut pfd = PollFd {
            fd: self.fd,
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: one valid PollFd, count 1.
        let n = unsafe { poll(&mut pfd, 1, timeout_ms) };
        n > 0 && (pfd.revents & POLLIN) != 0
    }
}

impl Drop for PidFd {
    fn drop(&mut self) {
        // SAFETY: fd is owned and not closed elsewhere.
        unsafe {
            close(self.fd);
        }
    }
}

/// Whether a dGPU is out of runtime suspend.
///
/// Gate NVML polling on this. Initialising NVML wakes a suspended GPU, so a
/// detector that ignored it would defeat runtime power management — burning
/// battery to ask whether a game is running on a GPU that is plainly asleep.
///
/// `bus` is a PCI address like `0000:01:00.0`. Returns `None` when the path
/// does not exist, which is not the same as "suspended".
pub fn dgpu_is_awake(sysfs_devices: &Path, bus: &str) -> Option<bool> {
    let status = std::fs::read_to_string(
        sysfs_devices.join(bus).join("power/runtime_status"),
    )
    .ok()?;
    Some(status.trim() == "active")
}

/// Default PCI device root.
pub fn pci_devices_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/sys/bus/pci/devices")
}

/// A process holding a graphics context on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsProcess {
    pub pid: i32,
    /// GPU memory in bytes, or `None` when the driver declines to say.
    pub used_gpu_memory: Option<u64>,
}

/// Why a graphics-context query returned nothing.
///
/// Distinguishing these matters: "the GPU is asleep" and "nothing is rendering"
/// look identical from a bare empty list, and only one of them means no game is
/// running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphicsQuery {
    /// Processes holding a graphics context. Empty means nothing is rendering.
    Processes(Vec<GraphicsProcess>),
    /// The dGPU is runtime-suspended, so nothing can be rendering on it and
    /// waking it to confirm would cost battery for no information.
    GpuAsleep,
    /// NVML is absent, or the query failed.
    Unavailable(String),
}

#[cfg(target_os = "linux")]
mod nvml {
    use super::{GraphicsProcess, GraphicsQuery};

    type Device = *mut std::ffi::c_void;

    /// `nvmlProcessInfo_v2_t`. The layout is fixed by NVML's ABI.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcessInfo {
        pid: std::ffi::c_uint,
        used_gpu_memory: std::ffi::c_ulonglong,
        gpu_instance_id: std::ffi::c_uint,
        compute_instance_id: std::ffi::c_uint,
    }

    /// NVML reports "no memory figure available" as this sentinel rather than 0.
    const MEMORY_UNKNOWN: std::ffi::c_ulonglong = u64::MAX;

    unsafe extern "C" {
        fn dlopen(path: *const std::ffi::c_char, flags: std::ffi::c_int)
            -> *mut std::ffi::c_void;
        fn dlsym(
            handle: *mut std::ffi::c_void,
            symbol: *const std::ffi::c_char,
        ) -> *mut std::ffi::c_void;
        fn dlclose(handle: *mut std::ffi::c_void) -> std::ffi::c_int;
    }

    /// Enumerate graphics contexts on the first NVML device.
    ///
    /// Deliberately its own tiny binding rather than an extension of the
    /// GPU-mode NVML wrapper: that one is welded to a guarded clock-offset
    /// transaction with readback and rollback, and a read-only process query has
    /// no business sharing a code path with it.
    pub fn graphics_processes() -> GraphicsQuery {
        let handle = unsafe {
            let mut h = std::ptr::null_mut();
            for name in [
                c"/run/opengl-driver/lib/libnvidia-ml.so.1",
                c"libnvidia-ml.so.1",
                c"libnvidia-ml.so",
            ] {
                h = dlopen(name.as_ptr(), 2);
                if !h.is_null() {
                    break;
                }
            }
            h
        };
        if handle.is_null() {
            return GraphicsQuery::Unavailable("libnvidia-ml not present".into());
        }

        let result = unsafe { query(handle) };
        unsafe {
            dlclose(handle);
        }
        result
    }

    unsafe fn sym(handle: *mut std::ffi::c_void, name: &std::ffi::CStr) -> Option<*mut std::ffi::c_void> {
        let p = unsafe { dlsym(handle, name.as_ptr()) };
        (!p.is_null()).then_some(p)
    }

    unsafe fn query(handle: *mut std::ffi::c_void) -> GraphicsQuery {
        unsafe {
            let Some(init) = sym(handle, c"nvmlInit_v2") else {
                return GraphicsQuery::Unavailable("nvmlInit_v2 missing".into());
            };
            let Some(shutdown) = sym(handle, c"nvmlShutdown") else {
                return GraphicsQuery::Unavailable("nvmlShutdown missing".into());
            };
            let Some(by_index) = sym(handle, c"nvmlDeviceGetHandleByIndex_v2") else {
                return GraphicsQuery::Unavailable("nvmlDeviceGetHandleByIndex_v2 missing".into());
            };
            // v3 first; older drivers only carry v2.
            let procs = sym(handle, c"nvmlDeviceGetGraphicsRunningProcesses_v3")
                .or_else(|| sym(handle, c"nvmlDeviceGetGraphicsRunningProcesses_v2"));
            let Some(procs) = procs else {
                return GraphicsQuery::Unavailable(
                    "nvmlDeviceGetGraphicsRunningProcesses missing".into(),
                );
            };

            let init: extern "C" fn() -> i32 = std::mem::transmute(init);
            let shutdown: extern "C" fn() -> i32 = std::mem::transmute(shutdown);
            let by_index: extern "C" fn(std::ffi::c_uint, *mut Device) -> i32 =
                std::mem::transmute(by_index);
            let procs: extern "C" fn(Device, *mut std::ffi::c_uint, *mut ProcessInfo) -> i32 =
                std::mem::transmute(procs);

            if init() != 0 {
                return GraphicsQuery::Unavailable("nvmlInit_v2 failed".into());
            }

            let mut device: Device = std::ptr::null_mut();
            if by_index(0, &mut device) != 0 || device.is_null() {
                shutdown();
                return GraphicsQuery::Unavailable("no NVML device 0".into());
            }

            // Ask for the count first: NVML returns INSUFFICIENT_SIZE (7) and
            // fills in how many entries it wants.
            let mut count: std::ffi::c_uint = 0;
            let rc = procs(device, &mut count, std::ptr::null_mut());
            // 0 = success with nothing running; 7 = needs a buffer this big.
            if rc != 0 && rc != 7 {
                shutdown();
                return GraphicsQuery::Unavailable(format!(
                    "nvmlDeviceGetGraphicsRunningProcesses returned {rc}"
                ));
            }
            if count == 0 {
                shutdown();
                return GraphicsQuery::Processes(Vec::new());
            }

            let mut buf = vec![
                ProcessInfo {
                    pid: 0,
                    used_gpu_memory: 0,
                    gpu_instance_id: 0,
                    compute_instance_id: 0,
                };
                count as usize
            ];
            let rc = procs(device, &mut count, buf.as_mut_ptr());
            shutdown();
            if rc != 0 {
                return GraphicsQuery::Unavailable(format!("second query returned {rc}"));
            }
            buf.truncate(count as usize);
            GraphicsQuery::Processes(
                buf.into_iter()
                    .map(|p| GraphicsProcess {
                        pid: p.pid as i32,
                        used_gpu_memory: (p.used_gpu_memory != MEMORY_UNKNOWN)
                            .then_some(p.used_gpu_memory),
                    })
                    .collect(),
            )
        }
    }
}

/// Which processes are holding a graphics context, if it is cheap to ask.
///
/// **Gated on the dGPU already being awake.** Initialising NVML wakes a
/// runtime-suspended GPU, so an ungated poller would hold the discrete GPU on
/// permanently just to keep asking whether a game had started — defeating
/// runtime power management to answer a question whose answer is obviously
/// "no". [`GraphicsQuery::GpuAsleep`] is returned instead.
///
/// This is a stronger signal than GPU utilisation, which video playback and
/// desktop compositing trip constantly. None of the Linux vendor control tools
/// surveyed use it.
pub fn graphics_processes(sysfs_devices: &Path, bus: &str) -> GraphicsQuery {
    // Only a definite "suspended" short-circuits. Unknown falls through: a
    // desktop GPU has no runtime-PM node, and treating its absence as asleep
    // would silently disable detection on every such machine.
    if dgpu_is_awake(sysfs_devices, bus) == Some(false) {
        return GraphicsQuery::GpuAsleep;
    }
    #[cfg(target_os = "linux")]
    {
        nvml::graphics_processes()
    }
    #[cfg(not(target_os = "linux"))]
    {
        GraphicsQuery::Unavailable("NVML process enumeration is implemented for Linux".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Tree(std::path::PathBuf);

    impl Tree {
        fn new(label: &str) -> Tree {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "alien-watch-{label}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).expect("fixture");
            Tree(p)
        }

        fn gpu(&self, bus: &str, status: &str) {
            let d = self.0.join(bus).join("power");
            fs::create_dir_all(&d).expect("gpu dir");
            fs::write(d.join("runtime_status"), status).expect("write");
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_active_dgpu_reads_awake() {
        let t = Tree::new("active");
        t.gpu("0000:01:00.0", "active\n");
        assert_eq!(dgpu_is_awake(&t.0, "0000:01:00.0"), Some(true));
    }

    #[test]
    fn a_suspended_dgpu_reads_asleep() {
        let t = Tree::new("suspended");
        t.gpu("0000:01:00.0", "suspended\n");
        assert_eq!(
            dgpu_is_awake(&t.0, "0000:01:00.0"),
            Some(false),
            "polling NVML here would wake the GPU and defeat runtime PM"
        );
    }

    #[test]
    fn a_missing_device_is_unknown_not_asleep() {
        let t = Tree::new("absent");
        assert_eq!(
            dgpu_is_awake(&t.0, "0000:99:00.0"),
            None,
            "absent must not be reported as a definite state"
        );
    }

    #[test]
    fn a_sleeping_dgpu_is_reported_without_waking_it() {
        let t = Tree::new("nowake");
        t.gpu("0000:01:00.0", "suspended\n");
        assert_eq!(
            graphics_processes(&t.0, "0000:01:00.0"),
            GraphicsQuery::GpuAsleep,
            "must short-circuit: initialising NVML would wake the GPU"
        );
    }

    #[test]
    fn an_absent_runtime_pm_node_does_not_block_the_query() {
        // A desktop GPU has no runtime_status. Treating that as "asleep" would
        // silently disable detection on every such machine.
        let t = Tree::new("nopm");
        assert!(
            !matches!(
                graphics_processes(&t.0, "0000:99:00.0"),
                GraphicsQuery::GpuAsleep
            ),
            "unknown power state must fall through, not short-circuit"
        );
    }

    #[test]
    fn an_empty_process_list_is_distinct_from_an_unavailable_query() {
        // The distinction is the whole point: "nothing is rendering" and "we
        // could not ask" look identical as a bare empty vector.
        let empty = GraphicsQuery::Processes(Vec::new());
        let missing = GraphicsQuery::Unavailable("libnvidia-ml not present".into());
        assert_ne!(empty, missing);
        assert_ne!(empty, GraphicsQuery::GpuAsleep);
    }

    #[test]
    fn pidfd_rejects_nonsense_pids() {
        assert!(PidFd::open(0).is_err());
        assert!(PidFd::open(-1).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pidfd_tracks_this_process_as_alive() {
        let me = std::process::id() as i32;
        let fd = PidFd::open(me).expect("own pid is always openable");
        assert_eq!(fd.pid(), me);
        assert!(!fd.has_exited(), "this process is demonstrably running");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pidfd_notices_a_child_exiting() {
        use std::process::Command;
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let pid = child.id() as i32;
        // A race here is expected and fine: if the child already exited,
        // pidfd_open fails, which is itself the correct answer.
        if let Ok(fd) = PidFd::open(pid) {
            assert!(
                fd.wait_exit(5000),
                "poll must report the exit rather than timing out"
            );
        }
        let _ = child.wait();
    }
}
