//! Named profiles — the thing PredatorSense calls a "mode".
//!
//! A profile is a snapshot of Alien’s mutable fan and lighting controls. CPU
//! power is deliberately absent: Acer’s three PH315-53 profiles
//! carry one identical 70 W / 107 W / 28 s policy, and Alien currently reports
//! Linux powercap state read-only rather than pretending each profile changes
//! it. The old raw GPU-flag field remains parseable for compatibility but is
//! deliberately ignored: applying it separately could split the guarded OEM
//! GPU-mode transaction. Optional fields mean "leave alone" for hand-written
//! partial profiles.

use serde::{Deserialize, Serialize};

use crate::device::{Device, Error, Result};
use crate::rgb::{covini_brightness, Colour, Direction, Effect, Zone, BRIGHTNESS_STEPS};
use crate::wmi::{Fan, FanBehaviour, FanMode};
use crate::Lighting;

/// Apply one fan's independent mode, leaving the other fan untouched.
///
/// Manual duty is two calls in order — the percentage is ignored unless the fan
/// is already in manual — and [`Device::set_fan_percent`] does both.
fn apply_side(dev: &Device, fan: Fan, side: FanSide) -> Result<()> {
    match side {
        FanSide::Auto => dev.set_fan_behaviour(FanBehaviour::Single {
            fan,
            mode: FanMode::Auto,
        }),
        FanSide::Manual { percent } => dev.set_fan_percent(fan, percent),
    }
}

/// What one fan should do, independently of the other.
///
/// The firmware has always supported this — [`FanBehaviour::Single`] encodes a
/// per-fan mode word and is unit-tested against the vendor constants — but no
/// frontend could express it, so a profile could only put *both* fans on the EC
/// curve or *both* on manual duty. PredatorSense's Custom mode does allow the
/// mixed case, which made this the last real 1:1 fan-control parity gap.
///
/// The mixed case is genuinely useful here: the GPU fan shares heat pipes with
/// the CPU, so pinning it while leaving the CPU fan on the EC curve cools both
/// dies for much less noise than running both at maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FanSide {
    /// Hand this fan to the EC's own curve, leaving the other untouched.
    Auto,
    /// Manual duty for this fan, 0-100.
    Manual { percent: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FanPolicy {
    Auto,
    Max,
    /// Per-fan duty, 0-100. Verified working; see [`Device::set_fan_percent`]
    /// for what the percentage does and does not mean in RPM.
    Manual {
        cpu: u8,
        gpu: u8,
    },
    /// Independent per-fan mode — the PredatorSense "Custom" case where one fan
    /// runs on the EC curve while the other runs a fixed duty.
    Split {
        cpu: FanSide,
        gpu: FanSide,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backlight {
    pub effect: String,
    pub speed: u8,
    pub brightness: u8,
    /// `#rrggbb`. Ignored by effects that use the firmware's own palette.
    pub colour: String,
    #[serde(default)]
    pub reverse: bool,
    /// Four static-zone colours. Present for profiles captured from a
    /// four-zone keyboard; absent in older profiles and animated modes.
    #[serde(default)]
    pub zones: Option<[String; 4]>,
    /// Static-zone enable mask. Absent in older profiles and animated modes.
    #[serde(default)]
    pub zone_enabled: Option<[bool; 4]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub fans: Option<FanPolicy>,
    /// Deprecated raw GPU firmware flag, retained only so old TOML still
    /// parses. `apply` never writes it: an independent GPOC mutation could
    /// split the guarded Normal/Faster/Turbo compound state.
    #[serde(default, alias = "turbo")]
    pub gpu_turbo: Option<bool>,
    pub backlight: Option<Backlight>,
}

impl Profile {
    /// Fans on the EC curve with a calm backlight.
    pub fn silent() -> Profile {
        Profile {
            name: "silent".into(),
            description: "EC fan curve + calm lighting — CPU/GPU policy unchanged".into(),
            fans: Some(FanPolicy::Auto),
            gpu_turbo: None,
            backlight: Some(Backlight {
                effect: "static".into(),
                speed: 0,
                brightness: 50,
                colour: "#1e9e8a".into(),
                reverse: false,
                zones: None,
                zone_enabled: None,
            }),
        }
    }

    /// Legacy profile name: maximum fans plus high-contrast lighting.
    pub fn turbo() -> Profile {
        Profile {
            name: "turbo".into(),
            description: "legacy name: maximum fans + red lighting; GPU mode unchanged".into(),
            fans: Some(FanPolicy::Max),
            gpu_turbo: None,
            backlight: Some(Backlight {
                effect: "static".into(),
                speed: 0,
                brightness: 100,
                colour: "#ff4d00".into(),
                reverse: false,
                zones: None,
                zone_enabled: None,
            }),
        }
    }

    /// Fans at maximum with amber lighting. On the reference machine the
    /// measured gain came from cooling, not from an invented CPU mode
    /// difference.
    pub fn performance() -> Profile {
        Profile {
            name: "performance".into(),
            description: "maximum fans + amber lighting — CPU/GPU policy unchanged".into(),
            fans: Some(FanPolicy::Max),
            gpu_turbo: None,
            backlight: Some(Backlight {
                effect: "static".into(),
                speed: 0,
                brightness: 75,
                colour: "#ffb000".into(),
                reverse: false,
                zones: None,
                zone_enabled: None,
            }),
        }
    }

    pub fn builtins() -> Vec<Profile> {
        vec![Profile::silent(), Profile::performance(), Profile::turbo()]
    }

    /// Whether this profile carried the deprecated raw-GPOC field.
    ///
    /// Frontends use this only to emit an explicit migration warning. The
    /// value is never applied to hardware.
    pub fn deprecated_gpu_flag_ignored(&self) -> bool {
        self.gpu_turbo.is_some()
    }

    /// Capture the machine's current state as a profile.
    ///
    /// # What this can and cannot see
    ///
    /// Lighting and per-fan **duty** are real firmware reads. Fan **mode** is
    /// not: this firmware exposes no fan-mode getter at all — only
    /// [`Device::fan_percent`], which returns the last requested duty. So there
    /// is no way to ask the hardware "are you on the EC curve right now?".
    ///
    /// `fans` is therefore taken from `known_fans`, which the caller supplies
    /// when it has its own record — the GUI keeps one and captions it *"MODE
    /// UNKNOWN — firmware has no getter"*, and the cooling controller knows
    /// what it last set. With `None`, `fans` is left **unset**, meaning "leave
    /// the fans alone on apply".
    ///
    /// It is tempting to read [`Device::fan_percent`] and record the result as
    /// [`FanPolicy::Manual`] instead. That is wrong, and measurably so: the
    /// duty register only reflects the last *manual* write, so on a machine
    /// sitting at maximum it still reports whatever duty was set before that.
    /// Capturing a machine at 5882/6122 RPM produced `manual 60/60` — a profile
    /// that silently applies far less airflow than the state it claims to
    /// reproduce. Recording nothing is honest; recording a stale register as
    /// fact is not.
    ///
    /// This is the restore-baseline primitive: Alien could apply profiles but
    /// had no way to capture one, which made crash-safe restore impossible.
    pub fn snapshot(
        dev: &Device,
        name: String,
        known_fans: Option<FanPolicy>,
    ) -> Result<Profile> {
        let fans = known_fans;
        let state = dev.backlight()?;
        // Zone colours are only meaningful for the static effect; an animated
        // effect drives them from the firmware's own palette, so capturing them
        // would record a transient frame as if it were configuration.
        let zones = if state.effect == Effect::Static {
            let mut out = [const { String::new() }; 4];
            for (i, zone) in [Zone::One, Zone::Two, Zone::Three, Zone::Four]
                .into_iter()
                .enumerate()
            {
                out[i] = dev.zone_colour(zone)?.to_hex();
            }
            Some(out)
        } else {
            None
        };
        Ok(Profile {
            name,
            description: "captured from live hardware state".into(),
            fans,
            // Never captured: replaying raw GPOC separately can split the
            // guarded OEM GPU-mode transaction.
            gpu_turbo: None,
            backlight: Some(Backlight {
                effect: state.effect.name().into(),
                speed: state.speed,
                brightness: state.brightness,
                colour: zones
                    .as_ref()
                    .map_or_else(|| state.colour.to_hex(), |z| z[0].clone()),
                reverse: state.reverse,
                zones,
                // The enable mask has no getter either; a captured static zone
                // is recorded as enabled because it reported a colour.
                zone_enabled: None,
            }),
        })
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        validate_name(&self.name)?;
        match self.fans {
            Some(FanPolicy::Manual { cpu, gpu }) if cpu > 100 || gpu > 100 => {
                return Err("manual fan percentages must be 0..100".into());
            }
            Some(FanPolicy::Split { cpu, gpu }) => {
                for side in [cpu, gpu] {
                    if let FanSide::Manual { percent } = side {
                        if percent > 100 {
                            return Err("manual fan percentages must be 0..100".into());
                        }
                    }
                }
            }
            _ => {}
        }
        let Some(backlight) = &self.backlight else {
            return Ok(());
        };
        let effect = Effect::parse(&backlight.effect)
            .ok_or_else(|| format!("unknown backlight effect: {}", backlight.effect))?;
        if !BRIGHTNESS_STEPS.contains(&backlight.brightness) {
            return Err("backlight brightness must be one of 0, 25, 50, 75, 100".into());
        }
        if (effect == Effect::Static && backlight.speed != 0)
            || (effect != Effect::Static && !(1..=9).contains(&backlight.speed))
        {
            return Err("static speed must be 0; animated speed must be 1..9".into());
        }
        Colour::parse(&backlight.colour)
            .ok_or_else(|| format!("invalid backlight colour: {}", backlight.colour))?;
        if backlight.reverse && !effect.honours_direction() {
            return Err(format!(
                "{} does not support reverse direction",
                effect.name()
            ));
        }
        if effect != Effect::Static
            && (backlight.zones.is_some() || backlight.zone_enabled.is_some())
        {
            return Err("zone colours and enable masks apply only to static lighting".into());
        }
        if let Some(zones) = &backlight.zones {
            for colour in zones {
                Colour::parse(colour)
                    .ok_or_else(|| format!("invalid static-zone colour: {colour}"))?;
            }
        }
        Ok(())
    }

    /// Apply every field that is set, after validating the whole profile.
    /// Hardware calls are ordered but cannot be transactional because this
    /// firmware has no fan-mode getter/rollback primitive; an error explicitly
    /// means an earlier field may already have landed.
    pub fn apply(&self, dev: &Device) -> Result<()> {
        self.validate().map_err(Error::Invalid)?;
        if let Some(fans) = self.fans {
            match fans {
                FanPolicy::Auto => dev.set_fan_behaviour(FanBehaviour::Auto)?,
                FanPolicy::Max => dev.set_fan_behaviour(FanBehaviour::Max)?,
                FanPolicy::Manual { cpu, gpu } => {
                    dev.set_fan_percent(crate::wmi::Fan::Cpu, cpu)?;
                    dev.set_fan_percent(crate::wmi::Fan::Gpu, gpu)?;
                }
                FanPolicy::Split { cpu, gpu } => {
                    apply_side(dev, crate::wmi::Fan::Cpu, cpu)?;
                    apply_side(dev, crate::wmi::Fan::Gpu, gpu)?;
                }
            }
        }
        if let Some(b) = &self.backlight {
            let effect = Effect::parse(&b.effect).expect("profile validated before hardware calls");
            let colour = Colour::parse(&b.colour).expect("profile validated before hardware calls");
            let brightness = covini_brightness(b.brightness);
            let dir = if b.reverse {
                Direction::RightToLeft
            } else {
                Direction::LeftToRight
            };
            let zones = b.zones.as_ref().map(|values| {
                std::array::from_fn(|index| {
                    Colour::parse(&values[index]).expect("profile zone colour validated")
                })
            });
            let zone_enabled = b.zone_enabled.unwrap_or([true; 4]);
            let mut memory = Lighting::load();
            if effect == Effect::Static {
                dev.set_zone_colours_enabled(
                    zones.unwrap_or([colour; 4]),
                    zone_enabled,
                    brightness,
                )?;
            } else {
                dev.prepare_lighting(memory.zone_enabled)?;
                dev.set_effect(effect, b.speed, brightness, dir, colour)?;
            }

            // Keep the shared per-user memory aligned with what the profile
            // just put on the hardware. Without this, applying a profile from
            // any frontend worked physically but the next GUI/TUI render used
            // the previous colour and captured that stale value again.
            memory.set_brightness(brightness);
            memory.set_speed(effect, b.speed);
            memory.set_direction(effect, dir);
            if effect == Effect::Static {
                let applied_zones = zones.unwrap_or([colour; 4]);
                memory.set_zone_colours(applied_zones);
                memory.set_zone_enabled(zone_enabled);
                memory.set_colour(Effect::Static, applied_zones[0]);
            } else {
                memory.set_colour(effect, colour);
            }
            memory.save().map_err(|error| {
                Error::State(format!(
                    "hardware applied but lighting memory was not saved: {error}"
                ))
            })?;
        }
        Ok(())
    }
}

/// Where user profiles live. Frontends are unprivileged daemon clients, so
/// profiles belong to the invoking user's config directory, beside the shared
/// lighting preferences.
pub fn config_dir() -> std::path::PathBuf {
    std::env::var_os("ALIEN_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME").map(|d| std::path::PathBuf::from(d).join("alien"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".config/alien"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/etc/alien"))
}

pub fn load(name: &str) -> Option<Profile> {
    if let Some(b) = Profile::builtins().into_iter().find(|p| p.name == name) {
        return Some(b);
    }
    let path = config_dir().join(format!("{name}.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    let profile: Profile = toml::from_str(&text).ok()?;
    profile.validate().ok()?;
    Some(profile)
}

/// Built-in and user profiles, with malformed files omitted and names sorted.
pub fn list() -> Vec<Profile> {
    let mut profiles = Profile::builtins();
    if let Ok(entries) = std::fs::read_dir(config_dir()) {
        let mut user: Vec<Profile> = entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .filter_map(|text| toml::from_str(&text).ok())
            .filter(|profile: &Profile| profile.validate().is_ok())
            .filter(|profile: &Profile| {
                !profiles.iter().any(|builtin| builtin.name == profile.name)
            })
            .collect();
        user.sort_by(|a, b| a.name.cmp(&b.name));
        user.dedup_by(|a, b| a.name == b.name);
        profiles.extend(user);
    }
    profiles
}

/// Persist a user profile atomically.
pub fn save(profile: &Profile) -> std::result::Result<std::path::PathBuf, String> {
    profile.validate()?;
    if Profile::builtins()
        .iter()
        .any(|builtin| builtin.name == profile.name)
    {
        return Err(format!("{} is a built-in profile name", profile.name));
    }
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.toml", profile.name));
    let tmp = dir.join(format!(".{}.tmp", profile.name));
    let text =
        toml::to_string_pretty(profile).map_err(|e| format!("cannot encode profile: {e}"))?;
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot install {}: {e}", path.display()))?;
    Ok(path)
}

pub fn delete(name: &str) -> std::result::Result<(), String> {
    validate_name(name)?;
    if Profile::builtins()
        .iter()
        .any(|builtin| builtin.name == name)
    {
        return Err(format!("{name} is built in and cannot be deleted"));
    }
    let path = config_dir().join(format!("{name}.toml"));
    std::fs::remove_file(&path).map_err(|e| format!("cannot delete {}: {e}", path.display()))
}

fn validate_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() || name.len() > 32 {
        return Err("profile name must be 1-32 characters".into());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("profile name may contain only letters, numbers, - and _".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Transport, TransportError};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(u32, Vec<u8>)>>);

    impl Transport for Arc<Recorder> {
        fn call_bytes(
            &self,
            function: u32,
            payload: &[u8],
        ) -> std::result::Result<Vec<u8>, TransportError> {
            self.0.lock().unwrap().push((function, payload.to_vec()));
            Ok(vec![0; 9])
        }

        fn describe(&self) -> String {
            "profile recorder".into()
        }
    }

    /// The exact per-fan mode words, from the vendor constants in `wmi.rs`.
    const CPU_AUTO_WORD: u64 = 0x0001_0001;
    const GPU_AUTO_WORD: u64 = 0x0040_0008;

    /// Regression guard for a bug caught on real hardware: capturing a machine
    /// whose fans were at maximum (5882/6122 RPM) produced `manual 60/60`,
    /// because the duty register still held a much older manual write. A
    /// profile that quietly applies less airflow than the state it claims to
    /// reproduce is worse than one that declines to guess.
    #[test]
    fn snapshot_without_a_known_policy_records_no_fan_state() {
        let rec = Arc::new(Recorder(Mutex::new(Vec::new())));
        let dev = Device::with_transport(Box::new(rec.clone()));
        let p = Profile::snapshot(&dev, "cap".into(), None).expect("snapshot");
        assert_eq!(p.fans, None, "must not invent a fan policy from a stale register");

        let calls = rec.0.lock().unwrap().clone();
        assert!(
            !calls
                .iter()
                .any(|(f, _)| *f == crate::wmi::Function::GetFanSpeed as u32),
            "must not even read the duty register - its value is not evidence"
        );
    }

    #[test]
    fn snapshot_keeps_a_policy_the_caller_actually_knows() {
        let rec = Arc::new(Recorder(Mutex::new(Vec::new())));
        let dev = Device::with_transport(Box::new(rec.clone()));
        let known = FanPolicy::Split {
            cpu: FanSide::Auto,
            gpu: FanSide::Manual { percent: 70 },
        };
        let p = Profile::snapshot(&dev, "cap".into(), Some(known)).expect("snapshot");
        assert_eq!(p.fans, Some(known));
    }

    #[test]
    fn split_policy_sets_each_fan_independently() {
        let rec = Arc::new(Recorder(Mutex::new(Vec::new())));
        let dev = Device::with_transport(Box::new(rec.clone()));
        Profile {
            name: "mixed".into(),
            description: String::new(),
            fans: Some(FanPolicy::Split {
                cpu: FanSide::Auto,
                gpu: FanSide::Manual { percent: 70 },
            }),
            gpu_turbo: None,
            backlight: None,
        }
        .apply(&dev)
        .expect("split policy applies");

        let calls = rec.0.lock().unwrap().clone();
        // CPU: one single-fan Auto word, and no duty write.
        assert_eq!(calls[0].0, crate::wmi::Function::SetFanBehaviour as u32);
        assert_eq!(calls[0].1, CPU_AUTO_WORD.to_le_bytes().to_vec());
        // GPU: manual mode first, then the percentage - the order matters
        // because the duty is ignored unless the fan is already manual.
        assert_eq!(calls[1].0, crate::wmi::Function::SetFanBehaviour as u32);
        assert_eq!(calls[2].0, crate::wmi::Function::SetFanSpeed as u32);
        assert_eq!(calls[2].1[0], crate::wmi::Fan::Gpu as u8);
        assert_eq!(calls[2].1[1], 70);
        assert_eq!(calls.len(), 3, "must not touch the CPU fan's duty");
    }

    #[test]
    fn split_auto_uses_single_fan_words_not_the_both_fans_word() {
        let rec = Arc::new(Recorder(Mutex::new(Vec::new())));
        let dev = Device::with_transport(Box::new(rec.clone()));
        Profile {
            name: "both-auto-split".into(),
            description: String::new(),
            fans: Some(FanPolicy::Split {
                cpu: FanSide::Auto,
                gpu: FanSide::Auto,
            }),
            gpu_turbo: None,
            backlight: None,
        }
        .apply(&dev)
        .expect("applies");
        let calls = rec.0.lock().unwrap().clone();
        assert_eq!(calls[0].1, CPU_AUTO_WORD.to_le_bytes().to_vec());
        assert_eq!(calls[1].1, GPU_AUTO_WORD.to_le_bytes().to_vec());
    }

    #[test]
    fn split_policy_survives_toml() {
        let p = Profile {
            name: "mixed".into(),
            description: "d".into(),
            fans: Some(FanPolicy::Split {
                cpu: FanSide::Auto,
                gpu: FanSide::Manual { percent: 65 },
            }),
            gpu_turbo: None,
            backlight: None,
        };
        let text = toml::to_string(&p).expect("serialise");
        let back: Profile = toml::from_str(&text).expect("deserialise");
        assert_eq!(back.fans, p.fans);
    }

    #[test]
    fn split_rejects_out_of_range_duty() {
        let p = Profile {
            name: "bad".into(),
            description: String::new(),
            fans: Some(FanPolicy::Split {
                cpu: FanSide::Manual { percent: 101 },
                gpu: FanSide::Auto,
            }),
            gpu_turbo: None,
            backlight: None,
        };
        assert!(p.validate().unwrap_err().contains("0..100"));
    }

    #[test]
    fn builtins_have_unique_names() {
        let names: Vec<_> = Profile::builtins().into_iter().map(|p| p.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names.len(), sorted.len());
    }

    #[test]
    fn builtins_reference_real_effects() {
        for p in Profile::builtins() {
            assert!(
                !p.deprecated_gpu_flag_ignored(),
                "built-in {} must not carry the deprecated raw GPOC field",
                p.name
            );
            let b = p.backlight.expect("builtins specify everything");
            assert!(
                Effect::parse(&b.effect).is_some(),
                "{} has a bogus effect",
                p.name
            );
            assert!(
                Colour::parse(&b.colour).is_some(),
                "{} has a bogus colour",
                p.name
            );
        }
    }

    #[test]
    fn profiles_roundtrip_through_toml() {
        let p = Profile::turbo();
        let text = toml::to_string(&p).unwrap();
        let back: Profile = toml::from_str(&text).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn manual_fan_policy_survives_toml() {
        let p = Profile {
            name: "custom".into(),
            description: String::new(),
            fans: Some(FanPolicy::Manual { cpu: 70, gpu: 80 }),
            gpu_turbo: None,
            backlight: None,
        };
        let back: Profile = toml::from_str(&toml::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.fans, Some(FanPolicy::Manual { cpu: 70, gpu: 80 }));
    }

    #[test]
    fn legacy_turbo_key_parses_as_gpu_only() {
        let text = r#"
name = "old"
fans = "auto"
turbo = true
"#;
        let profile: Profile = toml::from_str(text).unwrap();
        assert_eq!(profile.gpu_turbo, Some(true));
    }

    #[test]
    fn deprecated_profile_gpu_flag_never_writes_either_wmi_flag() {
        let recorder = Arc::new(Recorder::default());
        let device = Device::with_transport(Box::new(recorder.clone()));
        let profile = Profile {
            name: "gpu-only".into(),
            description: String::new(),
            fans: None,
            gpu_turbo: Some(true),
            backlight: None,
        };

        profile.apply(&device).unwrap();
        assert!(profile.deprecated_gpu_flag_ignored());
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[test]
    fn profile_names_are_safe_file_stems() {
        for good in ["desk", "quiet-night", "gpu_2"] {
            assert!(validate_name(good).is_ok());
        }
        for bad in ["", "../escape", "has space", "slash/name"] {
            assert!(validate_name(bad).is_err());
        }
    }
}
