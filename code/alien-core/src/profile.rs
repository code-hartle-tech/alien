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
use crate::rgb::{covini_brightness, Colour, Direction, Effect, BRIGHTNESS_STEPS};
use crate::wmi::FanBehaviour;
use crate::Lighting;

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

    pub fn validate(&self) -> std::result::Result<(), String> {
        validate_name(&self.name)?;
        if let Some(FanPolicy::Manual { cpu, gpu }) = self.fans {
            if cpu > 100 || gpu > 100 {
                return Err("manual fan percentages must be 0..100".into());
            }
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
