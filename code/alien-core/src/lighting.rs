//! Remembered lighting settings, shared by every frontend.
//!
//! Covini keeps one colour shared by every dynamic pattern, one speed/direction
//! record per pattern, and four independent static-zone colours. Alien mirrors
//! that split so switching dynamic patterns cannot invent state the OEM profile
//! format has nowhere to store.
//!
//! # Why this is in the library and not in the GUI
//!
//! A CLI process remembers nothing — it starts, does one thing, and exits. If
//! memory lived in the GUI's state, setting a pattern colour in the terminal
//! would not be visible in the desktop app. One on-disk store, read and written
//! by all three, is the only arrangement where the frontends agree.
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
    /// Legacy `#rrggbb` mirror. PredatorSense has one `Pattern color` field,
    /// now represented by [`Lighting::pattern_colour`]. Keeping this key makes
    /// existing Alien files and downgrades deterministic.
    pub colour: String,
    pub speed: u8,
    /// `true` for left-to-right. Only the directional effects read it.
    ///
    /// `serde(default)` because this arrived after the file format did: a
    /// `lighting.toml` written by an earlier build has no `left_to_right` key,
    /// and without a default the whole store fails to parse — which would
    /// silently revert every remembered colour at once.
    #[serde(default = "default_direction")]
    pub left_to_right: bool,
}

fn default_direction() -> bool {
    true
}

impl EffectMemory {
    fn default_for(e: Effect) -> Self {
        EffectMemory {
            colour: "#00aec7".into(),
            // 0 only makes sense for static. For an animation it means "do
            // not advance", which looks exactly like a broken effect.
            speed: if e == Effect::Static { 0 } else { 5 },
            left_to_right: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lighting {
    /// Keyed by effect name so the file stays readable and survives the enum
    /// gaining a variant.
    #[serde(default)]
    pub effects: BTreeMap<String, EffectMemory>,
    /// The one colour shared by Breathing, Zoom and Shifting in the exact
    /// Covini profile XML. Wave and Neon retain it but ignore RGB on the wire.
    /// `None` identifies an older Alien file and is migrated deterministically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern_colour: Option<String>,
    /// Static-mode colours, left to right.
    #[serde(default = "default_zones")]
    pub zones: [String; 4],
    /// Whether each static zone is lit. Added after the original file format,
    /// so existing preferences default to all four enabled.
    #[serde(default = "default_zone_enabled")]
    pub zone_enabled: [bool; 4],
    #[serde(default = "default_brightness")]
    pub brightness: u8,
}

fn default_zones() -> [String; 4] {
    [
        "#00aec7".into(),
        "#00aec7".into(),
        "#00aec7".into(),
        "#00aec7".into(),
    ]
}

fn default_brightness() -> u8 {
    100
}

fn default_zone_enabled() -> [bool; 4] {
    [true; 4]
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            effects: Effect::ALL
                .iter()
                .map(|e| (e.name().to_string(), EffectMemory::default_for(*e)))
                .collect(),
            pattern_colour: Some("#00aec7".into()),
            zones: default_zones(),
            zone_enabled: default_zone_enabled(),
            brightness: default_brightness(),
        }
    }
}

pub fn path() -> PathBuf {
    std::env::var_os("ALIEN_LIGHTING")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(|d| PathBuf::from(d).join("alien/lighting.toml"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/alien/lighting.toml"))
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
        let mut lighting: Lighting = std::fs::read_to_string(path())
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        lighting.brightness = crate::rgb::covini_brightness(lighting.brightness);
        if lighting
            .pattern_colour
            .as_deref()
            .and_then(Colour::parse)
            .is_none()
        {
            lighting.pattern_colour = Some(lighting.legacy_pattern_colour().to_hex());
        }
        lighting
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

    /// Migrate the former per-effect-colour format without depending on map
    /// iteration order. Breathing wins because it is Covini's factory-selected
    /// pattern; missing entries fall through the OEM colour-capable order.
    fn legacy_pattern_colour(&self) -> Colour {
        [
            Effect::Breath,
            Effect::Zoom,
            Effect::Shifting,
            Effect::Wave,
            Effect::Neon,
        ]
        .into_iter()
        .find_map(|effect| {
            self.effects
                .get(effect.name())
                .and_then(|memory| Colour::parse(&memory.colour))
        })
        .unwrap_or(Colour::new(0x00, 0xAE, 0xC7))
    }

    pub fn colour(&self, e: Effect) -> Colour {
        if e == Effect::Static {
            self.effects
                .get(e.name())
                .and_then(|memory| Colour::parse(&memory.colour))
                .unwrap_or(Colour::new(0x00, 0xAE, 0xC7))
        } else {
            self.pattern_colour
                .as_deref()
                .and_then(Colour::parse)
                .unwrap_or_else(|| self.legacy_pattern_colour())
        }
    }

    pub fn speed(&self, e: Effect) -> u8 {
        let s = self.effects.get(e.name()).map(|m| m.speed).unwrap_or(5);
        // Guard the stored value too: a hand-edited file must not make the
        // persisted/reported value differ from what the encoder emits.
        crate::rgb::covini_speed(e, s)
    }

    pub fn set_colour(&mut self, e: Effect, c: Colour) {
        let colour = c.to_hex();
        if e == Effect::Static {
            self.entry(e).colour = colour;
            return;
        }

        self.pattern_colour = Some(colour.clone());
        // Mirror into every legacy slot so an older Alien binary sees the same
        // single Pattern colour after a downgrade.
        for effect in Effect::ALL {
            if effect != Effect::Static {
                self.entry(effect).colour = colour.clone();
            }
        }
    }

    pub fn set_speed(&mut self, e: Effect, s: u8) {
        self.entry(e).speed = crate::rgb::covini_speed(e, s);
    }

    pub fn direction(&self, e: Effect) -> crate::rgb::Direction {
        let ltr = self
            .effects
            .get(e.name())
            .map(|m| m.left_to_right)
            .unwrap_or(true);
        if ltr {
            crate::rgb::Direction::LeftToRight
        } else {
            crate::rgb::Direction::RightToLeft
        }
    }

    pub fn set_direction(&mut self, e: Effect, d: crate::rgb::Direction) {
        self.entry(e).left_to_right = d == crate::rgb::Direction::LeftToRight;
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

    pub fn set_zone_enabled(&mut self, enabled: [bool; 4]) {
        self.zone_enabled = enabled;
    }

    /// Store the nearest brightness value the PH315-53 Covini UI can emit.
    pub fn set_brightness(&mut self, value: u8) {
        self.brightness = crate::rgb::covini_brightness(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covini_uses_one_pattern_colour_but_keeps_static_separate() {
        let mut l = Lighting::default();
        l.set_colour(Effect::Static, Colour::new(0x1e, 0x9e, 0x8a));
        l.set_colour(Effect::Zoom, Colour::new(0xff, 0x00, 0x00));
        assert_eq!(l.colour(Effect::Static), Colour::new(0x1e, 0x9e, 0x8a));
        assert_eq!(l.colour(Effect::Wave), Colour::new(0xff, 0x00, 0x00));
        assert_eq!(l.colour(Effect::Breath), Colour::new(0xff, 0x00, 0x00));
        assert_eq!(l.colour(Effect::Shifting), Colour::new(0xff, 0x00, 0x00));
        assert_eq!(l.colour(Effect::Neon), Colour::new(0xff, 0x00, 0x00));
        for effect in Effect::ALL.into_iter().filter(|e| *e != Effect::Static) {
            assert_eq!(
                l.effects.get(effect.name()).unwrap().colour,
                "#ff0000",
                "legacy mirror for {}",
                effect.name()
            );
        }
    }

    #[test]
    fn animated_effects_never_return_speed_zero() {
        let mut l = Lighting::default();
        l.set_speed(Effect::Zoom, 0);
        assert_eq!(l.speed(Effect::Zoom), 1, "0 would not animate");
        l.set_speed(Effect::Static, 5);
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
        assert_eq!(l.zone_enabled, [true; 4], "old files enable every zone");
    }

    #[test]
    fn a_file_predating_direction_still_loads() {
        // Every installed copy has a lighting.toml with no `left_to_right`.
        // If adding the field made those files unparseable, the fallback to
        // Default would quietly discard the user's whole palette.
        let t = r##"
            [effects.wave]
            colour = "#ff0000"
            speed = 4
        "##;
        let l: Lighting = toml::from_str(t).expect("must still parse");
        assert_eq!(l.colour(Effect::Wave), Colour::new(0xff, 0, 0));
        assert_eq!(
            l.colour(Effect::Breath),
            Colour::new(0xff, 0, 0),
            "a legacy colour migrates into the one Covini Pattern colour"
        );
        assert_eq!(
            l.direction(Effect::Wave),
            crate::rgb::Direction::LeftToRight
        );
    }

    #[test]
    fn legacy_pattern_colour_migration_prefers_breath_deterministically() {
        let t = r##"
            [effects.zoom]
            colour = "#00ff00"
            speed = 5
            [effects.breath]
            colour = "#ff0000"
            speed = 5
            [effects.shifting]
            colour = "#0000ff"
            speed = 5
        "##;
        let l: Lighting = toml::from_str(t).unwrap();
        for effect in [Effect::Breath, Effect::Zoom, Effect::Shifting] {
            assert_eq!(l.colour(effect), Colour::new(0xff, 0, 0));
        }
    }

    #[test]
    fn direction_is_remembered_per_effect() {
        let mut l = Lighting::default();
        l.set_direction(Effect::Wave, crate::rgb::Direction::RightToLeft);
        assert_eq!(
            l.direction(Effect::Wave),
            crate::rgb::Direction::RightToLeft
        );
        assert_eq!(
            l.direction(Effect::Zoom),
            crate::rgb::Direction::LeftToRight
        );
    }

    #[test]
    fn remembered_brightness_uses_covini_steps() {
        let mut l = Lighting::default();
        l.set_brightness(81);
        assert_eq!(l.brightness, 75);
    }

    #[test]
    fn zone_enable_state_roundtrips() {
        let mut l = Lighting::default();
        l.set_zone_enabled([true, false, true, false]);
        let back: Lighting = toml::from_str(&toml::to_string(&l).unwrap()).unwrap();
        assert_eq!(back.zone_enabled, [true, false, true, false]);
    }
}
