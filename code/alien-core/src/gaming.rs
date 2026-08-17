//! Game-side environment levers — the ones that measurably move frames.
//!
//! # Why these live here and not in the daemon
//!
//! Nothing in this module touches firmware. These are environment variables the
//! NVIDIA driver and DXVK-NVAPI read at process start, so they belong to the
//! session, not to a privileged broker. Alien's job on the hardware side is
//! fans and telemetry; this is the small set of *software* settings that were
//! worth encoding because getting them wrong is common and costly.
//!
//! # Why only two of them
//!
//! Most of the advice in circulation is obsolete, and shipping it would make
//! Alien another blog post. Deliberately excluded, each for a specific reason:
//!
//! - `DXVK_ASYNC` — never accepted upstream, and DXVK 2.0's graphics pipeline
//!   libraries superseded the problem it addressed. It is an unread variable on
//!   stock Proton.
//! - `DXVK_STATE_CACHE` — the state cache was removed in DXVK 2.7.
//! - `PROTON_NO_ESYNC` — esync was removed in Proton 11.
//! - `PROTON_ENABLE_NVAPI` — default-on since Proton 9.
//! - `__GL_THREADED_OPTIMIZATIONS` — OpenGL only, and games here run Vulkan
//!   through DXVK. It is also off by default, not on.
//!
//! What survives is two levers with real, measured effects.

/// A single environment setting, with the reasoning attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvLever {
    pub key: &'static str,
    pub value: &'static str,
    /// One line for `alien gaming explain`.
    pub why: &'static str,
    /// Whether this must be set for the whole session rather than per game.
    pub global_only: bool,
}

/// Pin the DLSS Super Resolution preset.
///
/// # Why this is not a micro-optimisation
///
/// On Turing the *default* is a net loss. Community measurement on an RTX 2060
/// at 1080p puts the current default transformer preset at roughly **7 % slower
/// than native TAA**, while preset K runs about **+20 %**. So a user who turns
/// DLSS on expecting free frames can end up slower than not using it, and the
/// fix is one variable.
///
/// Turing supports DLSS Super Resolution under Proton via DXVK-NVAPI, which is
/// default-on since Proton 9. Frame Generation is Ada and later — not available
/// on this GPU, and claiming otherwise would be the kind of thing this project
/// exists not to do.
pub const DLSS_PRESET: EnvLever = EnvLever {
    key: "DXVK_NVAPI_DRS_NGX_DLSS_SR_OVERRIDE_RENDER_PRESET_SELECTION",
    value: "K",
    why: "preset K is ~+20% on Turing; the current default runs ~7% slower than native TAA",
    global_only: false,
};

/// Stop the driver pruning the shader cache Steam just spent minutes priming.
///
/// Raises 1 % and 0.1 % lows — it removes stutter, it does not raise average
/// frame rate, and saying otherwise would oversell it.
///
/// **Must be global.** Setting it as a per-game launch option is actively
/// worse: the driver then prunes the cache for every process that lacks the
/// variable, so a per-game setting means the cache is repeatedly rebuilt.
pub const SHADER_CACHE_SIZE: EnvLever = EnvLever {
    key: "__GL_SHADER_DISK_CACHE_SIZE",
    // 12 GiB. Large enough that Fossilize-primed caches survive; the default is
    // small enough that they routinely do not.
    value: "12884901888",
    why: "keeps Steam's precompiled shader cache from being pruned; fixes stutter, not averages",
    global_only: true,
};

/// Companion to [`SHADER_CACHE_SIZE`] — the size alone is not enough.
pub const SHADER_CACHE_SKIP_CLEANUP: EnvLever = EnvLever {
    key: "__GL_SHADER_DISK_CACHE_SKIP_CLEANUP",
    value: "1",
    why: "without this the driver still prunes on its own schedule regardless of size",
    global_only: true,
};

/// Everything Alien recommends, in the order it should be presented.
pub fn levers() -> [EnvLever; 3] {
    [DLSS_PRESET, SHADER_CACHE_SIZE, SHADER_CACHE_SKIP_CLEANUP]
}

/// Levers that must be set session-wide rather than per game.
pub fn global_levers() -> Vec<EnvLever> {
    levers().into_iter().filter(|l| l.global_only).collect()
}

/// A `systemd` `environment.d` drop-in applying the global levers.
///
/// `environment.d` rather than a shell profile because it reaches graphical
/// sessions launched by the display manager, which a `.bashrc` does not — and a
/// game started from Steam has never seen a login shell.
pub fn environment_d_dropin() -> String {
    let mut out = String::from(
        "# Written by `alien gaming apply`.\n\
         #\n\
         # Session-wide NVIDIA shader cache settings. These must be global: as a\n\
         # per-game launch option the driver prunes the cache for every process\n\
         # that lacks them, so the cache gets rebuilt over and over.\n\
         #\n\
         # Takes effect for sessions started after the next login.\n\n",
    );
    for lever in global_levers() {
        out.push_str(&format!(
            "# {}\n{}={}\n\n",
            lever.why, lever.key, lever.value
        ));
    }
    out
}

/// The Steam launch option for per-game levers.
///
/// `gamemoderun` is included because it is the standard trigger the rest of the
/// desktop already understands, and it is what lets Alien's gamesync hooks fire.
pub fn steam_launch_options() -> String {
    let per_game: Vec<String> = levers()
        .into_iter()
        .filter(|l| !l.global_only)
        .map(|l| format!("{}={}", l.key, l.value))
        .collect();
    format!("{} gamemoderun %command%", per_game.join(" "))
}

/// Whether a lever is already active in this process's environment.
pub fn is_set(lever: &EnvLever) -> Option<String> {
    std::env::var(lever.key).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levers_are_scoped_the_way_the_driver_needs() {
        // Asserted through the collection rather than on the constants, so the
        // check is about what `levers()` actually publishes.
        let global: Vec<&str> = levers()
            .into_iter()
            .filter(|l| l.global_only)
            .map(|l| l.key)
            .collect();
        assert!(global.contains(&SHADER_CACHE_SIZE.key));
        assert!(global.contains(&SHADER_CACHE_SKIP_CLEANUP.key));
        // DLSS preset belongs on the launch line: a per-title
        // quality/performance choice, not a machine setting.
        assert!(!global.contains(&DLSS_PRESET.key));
    }

    #[test]
    fn the_dropin_carries_only_global_levers() {
        let text = environment_d_dropin();
        assert!(text.contains(SHADER_CACHE_SIZE.key));
        assert!(text.contains(SHADER_CACHE_SKIP_CLEANUP.key));
        assert!(
            !text.contains(DLSS_PRESET.key),
            "a per-game lever in environment.d would apply it to everything"
        );
    }

    #[test]
    fn the_dropin_is_valid_environment_d_syntax() {
        for line in environment_d_dropin().lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=').expect("KEY=VALUE");
            assert!(!k.is_empty() && !v.is_empty(), "bad line: {line}");
            assert!(
                !k.contains(' ') && !line.contains('"'),
                "environment.d does not do shell quoting: {line}"
            );
        }
    }

    #[test]
    fn launch_options_include_the_gamemode_trigger() {
        let opts = steam_launch_options();
        assert!(opts.contains("gamemoderun"));
        assert!(opts.ends_with("%command%"), "Steam substitutes at the end");
        assert!(opts.contains(DLSS_PRESET.key));
    }

    #[test]
    fn launch_options_do_not_repeat_the_global_levers() {
        let opts = steam_launch_options();
        assert!(
            !opts.contains(SHADER_CACHE_SIZE.key),
            "per-game shader cache settings make stutter worse, not better"
        );
    }

    #[test]
    fn no_obsolete_variable_is_recommended() {
        // Regression guard. Each of these was standard advice once and is now
        // either removed upstream or default-on; recommending them would make
        // Alien another stale blog post.
        let dead = [
            "DXVK_ASYNC",
            "DXVK_STATE_CACHE",
            "PROTON_NO_ESYNC",
            "PROTON_ENABLE_NVAPI",
            "__GL_THREADED_OPTIMIZATIONS",
            "PROTON_DLSS_UPGRADE",
        ];
        let all = format!("{}{}", environment_d_dropin(), steam_launch_options());
        for key in dead {
            assert!(
                !all.contains(key),
                "{key} is obsolete and must not be recommended"
            );
        }
    }

    #[test]
    fn every_lever_explains_itself() {
        for l in levers() {
            assert!(!l.why.is_empty(), "{} has no rationale", l.key);
            assert!(!l.value.is_empty());
        }
    }
}
