//! Named profiles — the thing PredatorSense calls a "mode".
//!
//! A profile is a snapshot of every control the interface exposes, so applying
//! one is deterministic: nothing is left at whatever the previous profile set.
//! `Option` fields mean "leave alone" and exist for partial profiles written by
//! hand; the built-ins all specify everything.

use serde::{Deserialize, Serialize};

use crate::device::{Device, Result};
use crate::rgb::{Colour, Direction, Effect};
use crate::wmi::{FanBehaviour, OverclockTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FanPolicy {
    Auto,
    Max,
    /// Per-fan duty, 0-100. Verified working; see [`Device::set_fan_percent`]
    /// for what the percentage does and does not mean in RPM.
    Manual { cpu: u8, gpu: u8 },
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub fans: Option<FanPolicy>,
    pub turbo: Option<bool>,
    pub backlight: Option<Backlight>,
}

impl Profile {
    /// Fans on the EC curve, turbo off, calm backlight. What the machine looks
    /// like with no vendor software running.
    pub fn silent() -> Profile {
        Profile {
            name: "silent".into(),
            description: "EC fan curve, turbo off — quiet, and thermally throttled under load"
                .into(),
            fans: Some(FanPolicy::Auto),
            turbo: Some(false),
            backlight: Some(Backlight {
                effect: "static".into(),
                speed: 0,
                brightness: 40,
                colour: "#1e9e8a".into(),
                reverse: false,
            }),
        }
    }

    /// Everything on. Loud, and on the reference machine ~48% faster sustained.
    pub fn turbo() -> Profile {
        Profile {
            name: "turbo".into(),
            description: "fans at maximum, turbo flags set — the fastest this chassis runs".into(),
            fans: Some(FanPolicy::Max),
            turbo: Some(true),
            backlight: Some(Backlight {
                effect: "static".into(),
                speed: 0,
                brightness: 100,
                colour: "#ff4d00".into(),
                reverse: false,
            }),
        }
    }

    /// The compromise: fans at maximum but turbo off, which on the reference
    /// machine captures the entire measured gain, since the gain was thermal.
    pub fn performance() -> Profile {
        Profile {
            name: "performance".into(),
            description: "fans at maximum, turbo off — full throughput without the OC flags".into(),
            fans: Some(FanPolicy::Max),
            turbo: Some(false),
            backlight: Some(Backlight {
                effect: "static".into(),
                speed: 0,
                brightness: 70,
                colour: "#ffb000".into(),
                reverse: false,
            }),
        }
    }

    pub fn builtins() -> Vec<Profile> {
        vec![Profile::silent(), Profile::performance(), Profile::turbo()]
    }

    /// Apply every field that is set. Stops at the first firmware rejection
    /// rather than continuing — a half-applied profile is worse than a clear
    /// error, because the user cannot tell which half took.
    pub fn apply(&self, dev: &Device) -> Result<()> {
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
        if let Some(t) = self.turbo {
            dev.set_overclock(OverclockTarget::Cpu, t)?;
            dev.set_overclock(OverclockTarget::Gpu, t)?;
        }
        if let Some(b) = &self.backlight {
            let effect = Effect::parse(&b.effect).unwrap_or(Effect::Static);
            let colour = Colour::parse(&b.colour).unwrap_or(Colour::OFF);
            let dir = if b.reverse { Direction::RightToLeft } else { Direction::LeftToRight };
            dev.set_effect(effect, b.speed, b.brightness, dir, colour)?;
        }
        Ok(())
    }
}

/// Where user profiles live. Respects `XDG_CONFIG_HOME`, but note that Alien
/// runs as root, so this is root's config dir unless the caller overrides it —
/// the CLI passes the invoking user's directory through explicitly.
pub fn config_dir() -> std::path::PathBuf {
    std::env::var_os("ALIEN_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(|d| std::path::PathBuf::from(d).join("alien")))
        .unwrap_or_else(|| std::path::PathBuf::from("/etc/alien"))
}

pub fn load(name: &str) -> Option<Profile> {
    if let Some(b) = Profile::builtins().into_iter().find(|p| p.name == name) {
        return Some(b);
    }
    let path = config_dir().join(format!("{name}.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let b = p.backlight.expect("builtins specify everything");
            assert!(Effect::parse(&b.effect).is_some(), "{} has a bogus effect", p.name);
            assert!(Colour::parse(&b.colour).is_some(), "{} has a bogus colour", p.name);
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
            turbo: None,
            backlight: None,
        };
        let back: Profile = toml::from_str(&toml::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.fans, Some(FanPolicy::Manual { cpu: 70, gpu: 80 }));
    }
}
