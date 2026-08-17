//! Crash-safe profile leases — apply something temporarily and always get back.
//!
//! # The problem this solves
//!
//! Applying a profile for the duration of a game is easy. *Un*-applying it when
//! the game dies badly is the part everyone gets wrong.
//!
//! Feral's gamemode — the de-facto standard on Linux, and the right thing to
//! hook — keeps its restore baseline **in memory only**, on a singleton
//! context. `SIGTERM` is handled; `SIGKILL`, OOM and power loss are not. Kill
//! `gamemoded` mid-session and the machine keeps its performance governor
//! forever, because the value it would have restored died with the process. Its
//! crash detection is also a five-second `kill(pid, 0)` sweep with a real
//! PID-reuse window. `asusctl`, `system76-power` and `supergfxctl` do not
//! implement app-triggered profiles at all, so they have nothing to restore.
//!
//! Windows does this **better**. PredatorSense persists its GameSync baseline
//! to an INI plus a registry sentinel — `ActiveIndex = 999` meaning "applied,
//! restore pending" — and checks it on next launch, so a crash mid-game is
//! recoverable on the next start.
//!
//! This module is that idea, done properly: the baseline goes to disk *before*
//! the hardware is touched, and a sentinel marks the window in which a restore
//! is owed.
//!
//! # Why the state directory choice matters
//!
//! Not the per-user profile directory. That resolves under `$HOME`, and the one
//! process that most needs to read a pending restore at startup — a root
//! service, or a `systemd-sleep` hook running with `user.slice` frozen — cannot
//! reach it. State lives somewhere both can see.
//!
//! Whether it should survive a reboot is a real question with a real answer:
//! **it should not.** Firmware fan state does not survive a power cycle either,
//! so a lease found after a reboot describes hardware that no longer exists in
//! that state. Honouring it would restore a baseline captured against a machine
//! that has since reset itself. `/run` is therefore correct and `/var/lib`
//! is not — the tmpfs clearing on boot *is* the expiry mechanism.

use std::fs;
use std::path::{Path, PathBuf};

use crate::profile::Profile;

/// Directory holding lease state. Created by the daemon's `RuntimeDirectory`.
///
/// `ALIEN_STATE_DIR` overrides it, which is what the tests use.
pub fn state_dir() -> PathBuf {
    std::env::var_os("ALIEN_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/alien"))
}

fn lease_path(dir: &Path) -> PathBuf {
    dir.join("lease.toml")
}

/// A profile applied on someone's behalf, with the state to undo it.
///
/// Serialised whole, so the sentinel and the baseline cannot disagree — there
/// is no window in which a marker says "restore pending" but the thing to
/// restore is missing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lease {
    /// Who asked for it — an executable name, `gamemode`, a profile name.
    /// Recorded for diagnostics only; never used to decide anything.
    pub holder: String,
    /// What was applied.
    pub applied: String,
    /// The state to return to. `None` means the holder could not establish a
    /// baseline, in which case [`Lease::fallback_is_max`] governs recovery.
    pub baseline: Option<Profile>,
}

impl Lease {
    /// Whether recovery must force fans to maximum instead of trusting the
    /// baseline's fan policy.
    ///
    /// True when there is no baseline **or** the baseline carries no fan
    /// policy. That second case is the one that bites, and it is the normal
    /// case here: [`Profile::snapshot`] deliberately leaves `fans` unset
    /// because this firmware has no fan-mode getter, so almost every captured
    /// baseline has `fans: None`.
    ///
    /// The two `None`s mean different things and must not be conflated. In a
    /// hand-written profile, `fans: None` means *leave the fans alone* — a
    /// perfectly good instruction. In a restore baseline it means *we never
    /// knew what they were*, and "leave alone" is then exactly wrong, because
    /// the lease itself changed them. Applying such a baseline would restore
    /// the lighting and quietly abandon the fans wherever the game left them.
    ///
    /// Erring toward maximum is the house rule for this hardware, not a general
    /// one: the EC curve permits 95 °C and 1446 MHz, and `acer-wmi` maps
    /// `pwm_enable = 0` to `ACER_WMID_FAN_MODE_TURBO`. A recovery path that has
    /// to guess should guess loud.
    pub fn fallback_is_max(&self) -> bool {
        self.baseline.as_ref().is_none_or(|p| p.fans.is_none())
    }
}

/// Errors that can come out of lease handling.
#[derive(Debug)]
pub enum LeaseError {
    Io(String),
    Encode(String),
    Decode(String),
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeaseError::Io(e) => write!(f, "lease I/O failed: {e}"),
            LeaseError::Encode(e) => write!(f, "cannot encode lease: {e}"),
            LeaseError::Decode(e) => write!(f, "cannot decode lease: {e}"),
        }
    }
}

impl std::error::Error for LeaseError {}

type Result<T> = std::result::Result<T, LeaseError>;

/// Record a lease **before** touching hardware.
///
/// Write-then-rename, so a crash mid-write leaves either the old lease or the
/// new one and never a half-parsed file. The ordering is the whole point: if
/// the process dies between this call and the hardware write, recovery restores
/// a baseline that was already true, which is harmless. Dying the other way
/// round — hardware changed, nothing recorded — is the case that strands the
/// machine, and it is the case gamemode has.
pub fn acquire(lease: &Lease) -> Result<()> {
    let dir = state_dir();
    fs::create_dir_all(&dir).map_err(|e| LeaseError::Io(e.to_string()))?;
    let text = toml::to_string(lease).map_err(|e| LeaseError::Encode(e.to_string()))?;
    let tmp = dir.join(".lease.tmp");
    fs::write(&tmp, text.as_bytes()).map_err(|e| LeaseError::Io(e.to_string()))?;
    fs::rename(&tmp, lease_path(&dir)).map_err(|e| LeaseError::Io(e.to_string()))
}

/// The outstanding lease, if a restore is owed.
pub fn current() -> Result<Option<Lease>> {
    let path = lease_path(&state_dir());
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(LeaseError::Io(e.to_string())),
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| LeaseError::Decode(e.to_string()))
}

/// Clear the lease. Call only after the hardware is actually back.
///
/// Idempotent: releasing a lease that is not held is not an error, because a
/// recovery path and a normal exit can both legitimately reach here.
pub fn release() -> Result<()> {
    match fs::remove_file(lease_path(&state_dir())) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(LeaseError::Io(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// `state_dir` reads a process-global env var, so lease tests cannot run
    /// concurrently with each other.
    static ENV: Mutex<()> = Mutex::new(());

    struct Scoped(PathBuf);

    impl Scoped {
        fn new(label: &str) -> Scoped {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "alien-lease-{label}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).expect("fixture dir");
            std::env::set_var("ALIEN_STATE_DIR", &p);
            Scoped(p)
        }
    }

    impl Drop for Scoped {
        fn drop(&mut self) {
            std::env::remove_var("ALIEN_STATE_DIR");
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn lease() -> Lease {
        Lease {
            holder: "gamemode".into(),
            applied: "performance".into(),
            baseline: Some(Profile::silent()),
        }
    }

    #[test]
    fn nothing_outstanding_on_a_clean_machine() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _s = Scoped::new("clean");
        assert_eq!(current().expect("read"), None);
    }

    #[test]
    fn a_lease_survives_the_process_that_took_it() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _s = Scoped::new("survive");
        acquire(&lease()).expect("acquire");
        // Nothing in this process holds the lease in memory - reading it back
        // is exactly what a restarted daemon would do.
        let found = current().expect("read").expect("a lease is outstanding");
        assert_eq!(found.applied, "performance");
        assert_eq!(
            found.baseline.as_ref().map(|p| p.name.as_str()),
            Some("silent")
        );
    }

    #[test]
    fn release_clears_it() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _s = Scoped::new("release");
        acquire(&lease()).expect("acquire");
        release().expect("release");
        assert_eq!(current().expect("read"), None);
    }

    #[test]
    fn releasing_twice_is_not_an_error() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _s = Scoped::new("twice");
        acquire(&lease()).expect("acquire");
        release().expect("first");
        release().expect("a recovery path and a normal exit can both land here");
    }

    #[test]
    fn acquiring_again_replaces_rather_than_stacking() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _s = Scoped::new("replace");
        acquire(&lease()).expect("first");
        let mut second = lease();
        second.holder = "steam".into();
        second.applied = "turbo".into();
        acquire(&second).expect("second");
        let found = current().expect("read").expect("outstanding");
        assert_eq!(found.holder, "steam");
        assert_eq!(found.applied, "turbo");
    }

    /// Regression guard for a gap found on hardware: `gamesync begin` captured
    /// a baseline whose lighting was real but whose `fans` was unset, because
    /// snapshot will not invent fan state. Applying that baseline restored the
    /// lighting and silently abandoned the fans at the lease's setting.
    #[test]
    fn a_baseline_with_no_fan_policy_still_recovers_to_maximum() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _s = Scoped::new("nofans");
        let mut captured = Profile::silent();
        captured.fans = None; // exactly what Profile::snapshot produces
        let l = Lease {
            holder: "gamemode".into(),
            applied: "silent".into(),
            baseline: Some(captured),
        };
        acquire(&l).expect("acquire");
        let found = current().expect("read").expect("outstanding");
        assert!(
            found.fallback_is_max(),
            "a baseline that never knew the fan state must not be treated as \
             'leave the fans alone' - the lease itself changed them"
        );
    }

    #[test]
    fn a_baseline_that_does_carry_a_fan_policy_is_trusted() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _s = Scoped::new("withfans");
        let l = Lease {
            holder: "controller".into(),
            applied: "silent".into(),
            baseline: Some(Profile::performance()), // fans: Some(Max)
        };
        acquire(&l).expect("acquire");
        let found = current().expect("read").expect("outstanding");
        assert!(!found.fallback_is_max(), "a known policy must be honoured");
    }

    #[test]
    fn a_lease_without_a_baseline_recovers_to_maximum() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _s = Scoped::new("nobaseline");
        let l = Lease {
            holder: "unknown".into(),
            applied: "performance".into(),
            baseline: None,
        };
        acquire(&l).expect("acquire");
        let found = current().expect("read").expect("outstanding");
        assert!(
            found.fallback_is_max(),
            "with no baseline, recovery must escalate to Max - EC auto permits 95 C"
        );
    }

    #[test]
    fn a_corrupt_lease_reports_rather_than_silently_vanishing() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let s = Scoped::new("corrupt");
        fs::write(lease_path(&s.0), b"this is not toml {{{").expect("write junk");
        let err = current().expect_err("must not read as 'no lease outstanding'");
        assert!(matches!(err, LeaseError::Decode(_)));
    }

    #[test]
    fn no_partial_file_is_ever_visible() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let s = Scoped::new("atomic");
        acquire(&lease()).expect("acquire");
        // The temp file must not survive; a reader that globbed the directory
        // would otherwise find two candidate leases.
        assert!(!s.0.join(".lease.tmp").exists());
        assert!(lease_path(&s.0).exists());
    }
}
