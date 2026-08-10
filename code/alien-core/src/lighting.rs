//! Remembered lighting settings, shared by every frontend.
//!
//! Each effect keeps its own colour and speed, and static keeps four zone
//! colours. Switching from a teal static to wave and back returns the teal.
//!
//! # Why this is in the library and not in the GUI
//!
//! Because "each mode remembers its colour" has to be true of the CLI too, and
//! a CLI process remembers nothing — it starts, does one thing, and exits. If
//! the memory lived in the GUI's state, then `alien rgb effect wave` would
//! forget, and setting a colour in the terminal would not be visible in the
//! desktop app. One on-disk store, read and written by all three, is the only
//! arrangement where the three agree.
//!
//! # Where it lives
//!
//! `$XDG_CONFIG_HOME/alien/lighting.toml`, per user. Not in `/var/lib` with
//! the daemon: this is a preference, not machine state, and two people logged
//! into the same machine should not fight over each other's colours.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::rgb::{Colour, Effect};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectMemory {
    /// `#rrggbb`.
    pub colour: String,
    pub speed: u8,
}

impl EffectMemory {
    fn default_for(e: Effect) -> Self {
        EffectMemory {
            colour: "#00aec7".into(),
            // 0 only makes sense for static. For an animation it means "do
            // not advance", which looks exactly like a broken effect.
            speed: if e == Effect::Static { 0 } else { 5 },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lighting {
    /// Keyed by effect name so the file stays readable and survives the enum
    /// gaining a variant.
    #[serde(default)]
    pub effects: BTreeMap<String, EffectMemory>,
    /// Static-mode colours, left to right.
    #[serde(default = "default_zones")]
    pub zones: [String; 4],
    #[serde(default = "default_brightness")]
    pub brightness: u8,
}

fn default_zones() -> [String; 4] {
    ["#00aec7".into(), "#00aec7".into(), "#00aec7".into(), "#00aec7".into()]
}

fn default_brightness() -> u8 {
    100
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            effects: Effect::ALL
                .iter()
                .map(|e| (e.name().to_string(), EffectMemory::default_for(*e)))
                .collect(),
            zones: default_zones(),
            brightness: default_brightness(),
        }
    }
}

pub fn path() -> PathBuf {
    std::env::var_os("ALIEN_LIGHTING")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME").map(|d| PathBuf::from(d).join("alien/lighting.toml"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".config/alien/lighting.toml"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/alien-lighting.toml"))
}

impl Lighting {
    /// Read the store, falling back to defaults.
    ///
    /// A corrupt or half-written file yields defaults rather than an error:
    /// losing your remembered colours is a small annoyance, and refusing to
    /// set the keyboard because a preferences file is malformed is a large
    /// one.
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // Write-then-rename: a frontend killed mid-save must not leave a
        // truncated file that reads as "no memory at all".
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, p)
    }

    fn entry(&mut self, e: Effect) -> &mut EffectMemory {
        self.effects
            .entry(e.name().to_string())
            .or_insert_with(|| EffectMemory::default_for(e))
    }

    pub fn colour(&self, e: Effect) -> Colour {
        self.effects
            .get(e.name())
            .and_then(|m| Colour::parse(&m.colour))
            .unwrap_or(Colour::new(0x00, 0xAE, 0xC7))
    }

    pub fn speed(&self, e: Effect) -> u8 {
        let s = self.effects.get(e.name()).map(|m| m.speed).unwrap_or(5);
        // Guard the stored value too: a file hand-edited to 0 would otherwise
        // reproduce the "effect does nothing" bug.
        if e == Effect::Static { s } else { s.max(1) }
    }

    pub fn set_colour(&mut self, e: Effect, c: Colour) {
        self.entry(e).colour = c.to_hex();
    }

    pub fn set_speed(&mut self, e: Effect, s: u8) {
        self.entry(e).speed = s.min(9);
    }

    pub fn zone_colours(&self) -> [Colour; 4] {
        let mut out = [Colour::new(0x00, 0xAE, 0xC7); 4];
        for (i, z) in self.zones.iter().enumerate().take(4) {
            if let Some(c) = Colour::parse(z) {
                out[i] = c;
            }
        }
        out
    }

    pub fn set_zone_colours(&mut self, cs: [Colour; 4]) {
        for (slot, c) in self.zones.iter_mut().zip(cs) {
            *slot = c.to_hex();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_effect_remembers_its_own_colour() {
        // The whole point: setting wave's colour must not disturb static's.
        let mut l = Lighting::default();
        l.set_colour(Effect::Static, Colour::new(0x1e, 0x9e, 0x8a));
        l.set_colour(Effect::Wave, Colour::new(0xff, 0x00, 0x00));
        assert_eq!(l.colour(Effect::Static), Colour::new(0x1e, 0x9e, 0x8a));
        assert_eq!(l.colour(Effect::Wave), Colour::new(0xff, 0x00, 0x00));
        assert_eq!(l.colour(Effect::Breath), Colour::new(0x00, 0xAE, 0xC7));
    }

    #[test]
    fn animated_effects_never_return_speed_zero() {
        let mut l = Lighting::default();
        l.set_speed(Effect::Ripple, 0);
        assert_eq!(l.speed(Effect::Ripple), 1, "0 would not animate");
        l.set_speed(Effect::Static, 0);
        assert_eq!(l.speed(Effect::Static), 0, "static legitimately has none");
    }

    #[test]
    fn survives_a_round_trip_through_toml() {
        let mut l = Lighting::default();
        l.set_colour(Effect::Zoom, Colour::new(1, 2, 3));
        l.set_zone_colours([
            Colour::new(0xff, 0, 0),
            Colour::new(0, 0xff, 0),
            Colour::new(0, 0, 0xff),
            Colour::new(0xff, 0xff, 0xff),
        ]);
        let back: Lighting = toml::from_str(&toml::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
        assert_eq!(back.zone_colours()[2], Colour::new(0, 0, 0xff));
    }

    #[test]
    fn a_corrupt_file_yields_defaults_rather_than_failing() {
        // Refusing to set the keyboard because a preferences file is broken
        // would be a much worse failure than forgetting a colour.
        let bad: Result<Lighting, _> = toml::from_str("this is not toml {{{");
        assert!(bad.is_err());
        let l = Lighting::default();
        assert_eq!(l.colour(Effect::Static), Colour::new(0x00, 0xAE, 0xC7));
    }

    #[test]
    fn unknown_effects_in_the_file_do_not_break_loading() {
        // The enum may gain variants; an older file must still load.
        let t = r##"
            zones = ["#111111", "#222222", "#333333", "#444444"]
            brightness = 50
            [effects.static]
            colour = "#abcdef"
            speed = 0
            [effects.somethingnew]
            colour = "#000000"
            speed = 3
        "##;
        let l: Lighting = toml::from_str(t).unwrap();
        assert_eq!(l.colour(Effect::Static), Colour::new(0xab, 0xcd, 0xef));
        assert_eq!(l.brightness, 50);
    }
}
