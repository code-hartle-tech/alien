//! Four-zone keyboard backlight.
//!
//! Acer's keyboards split into four vertical zones, numbered left to right.
//! Two independent mechanisms drive them and they overwrite each other:
//!
//! * **Static** ([`Zone`] + [`Colour`]) — one colour per zone, set individually.
//! * **Effects** ([`Effect`]) — a firmware animation across all four zones.
//!
//! Setting an effect discards static colours, and setting a static colour stops
//! the effect. That is firmware behaviour, not a limitation of this crate, so
//! the API keeps them as separate calls rather than pretending they compose.

/// A keyboard zone. The wire format is a bitmask, so zones can be addressed
/// together, but the firmware applies one colour per call regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    One = 1,
    Two = 2,
    Three = 4,
    Four = 8,
}

impl Zone {
    pub const ALL: [Zone; 4] = [Zone::One, Zone::Two, Zone::Three, Zone::Four];

    pub fn from_index(i: usize) -> Option<Zone> {
        Zone::ALL.get(i).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Colour {
    pub const OFF: Colour = Colour { r: 0, g: 0, b: 0 };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Colour { r, g, b }
    }

    /// Parse `#rrggbb`, `rrggbb`, or `#rgb`.
    pub fn parse(s: &str) -> Option<Colour> {
        let h = s.trim().trim_start_matches('#');
        match h.len() {
            6 => Some(Colour {
                r: u8::from_str_radix(&h[0..2], 16).ok()?,
                g: u8::from_str_radix(&h[2..4], 16).ok()?,
                b: u8::from_str_radix(&h[4..6], 16).ok()?,
            }),
            3 => {
                let d = |i: usize| -> Option<u8> {
                    let v = u8::from_str_radix(&h[i..i + 1], 16).ok()?;
                    Some(v * 17 // 0xF -> 0xFF, the usual short-hex expansion
                    )
                };
                Some(Colour { r: d(0)?, g: d(1)?, b: d(2)? })
            }
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Payload for `SetGamingRgbKb` (function 6) — a **u64**, not a byte record.
///
/// Wire layout, little-endian: `[zone, R, G, B, 0, 0, 0, 0]`.
///
/// # Two things here were wrong for a long time
///
/// The first version sent `{mode, zone, r, g, b}`, a byte record modelled on
/// the neighbouring functions. The firmware accepted it, returned status 0,
/// and did nothing — which is how this ended up documented as "per-zone
/// static is unverified and probably inert on this model". It was not inert;
/// it was being sent a payload it could not parse.
///
/// Then the vendor-derived notes give the packing as
/// `(R<<24) | (G<<16) | (B<<8) | zone`. That is **transposed**: sending 0xff
/// in the byte those notes call red lights the keyboard BLUE. Measured on
/// hardware, byte 1 is red and byte 3 is blue, i.e. `zone | R<<8 | G<<16 |
/// B<<24`. Confirmed by setting four zones to four distinct colours and
/// reading each one back.
pub fn zone_word(zone: Zone, c: Colour) -> u64 {
    (zone as u64) | ((c.r as u64) << 8) | ((c.g as u64) << 16) | ((c.b as u64) << 24)
}

/// Firmware backlight animations.
///
/// `Neon`, `Wave` and `Ripple` ignore the colour field — the firmware picks
/// its own palette — so the API takes the colour anyway and documents which
/// effects honour it rather than exposing two incompatible call shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Solid colour across all zones. Honours colour.
    Static = 0,
    /// Cycles the whole keyboard through the spectrum.
    Breath = 1,
    /// Firmware palette sweep.
    Neon = 2,
    /// Colour travels across the zones.
    Wave = 3,
    /// Lights the zone under each keypress.
    Shifting = 4,
    /// Expanding rings from each keypress.
    Zoom = 5,
    /// Wave triggered by keypress.
    Ripple = 6,
}

impl Effect {
    /// Whether this effect uses the colour argument at all. Worth surfacing in
    /// a UI — a colour picker that silently does nothing is a bug report.
    pub fn honours_colour(self) -> bool {
        matches!(self, Effect::Static | Effect::Breath | Effect::Wave | Effect::Zoom)
    }

    pub fn name(self) -> &'static str {
        match self {
            Effect::Static => "static",
            Effect::Breath => "breath",
            Effect::Neon => "neon",
            Effect::Wave => "wave",
            Effect::Shifting => "shifting",
            Effect::Zoom => "zoom",
            Effect::Ripple => "ripple",
        }
    }

    pub fn parse(s: &str) -> Option<Effect> {
        Some(match s.to_ascii_lowercase().as_str() {
            "static" => Effect::Static,
            "breath" => Effect::Breath,
            "neon" => Effect::Neon,
            "wave" => Effect::Wave,
            "shifting" => Effect::Shifting,
            "zoom" => Effect::Zoom,
            "ripple" => Effect::Ripple,
            _ => return None,
        })
    }

    pub const ALL: [Effect; 7] = [
        Effect::Static,
        Effect::Breath,
        Effect::Neon,
        Effect::Wave,
        Effect::Shifting,
        Effect::Zoom,
        Effect::Ripple,
    ];
}

/// Travel direction for directional effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    RightToLeft = 1,
    LeftToRight = 2,
}

/// Payload for `SetGamingKBBacklight` (function 20) — **sixteen** bytes.
///
/// ```text
/// [ mode, speed, brightness, 0, direction, R, G, B, 0x03, 0x01, 0,0,0,0,0,0 ]
/// ```
///
/// # The last two bytes are the whole game
///
/// This was an eight-byte buffer for a long time, stopping right before the
/// `0x03, 0x01` at offsets 8 and 9. The firmware accepted the short buffer,
/// stored the fields it did get, and reported them back perfectly through
/// function 21 — so it looked verified. Nothing ever lit up, because those
/// two bytes are what makes the write take effect.
///
/// That is the most expensive lesson in this file: a readback proves the
/// firmware stored a value. It says nothing about whether the hardware acted
/// on it, and here the two came apart completely.
pub fn effect_payload(
    effect: Effect,
    speed: u8,
    brightness: u8,
    dir: Direction,
    c: Colour,
) -> [u8; 16] {
    [
        effect as u8,
        speed.min(9),
        brightness.min(100),
        0,
        dir as u8,
        c.r,
        c.g,
        c.b,
        0x03,
        0x01,
        0,
        0,
        0,
        0,
        0,
        0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_hex_forms() {
        assert_eq!(Colour::parse("#1e9e8a"), Some(Colour::new(0x1e, 0x9e, 0x8a)));
        assert_eq!(Colour::parse("1e9e8a"), Some(Colour::new(0x1e, 0x9e, 0x8a)));
        assert_eq!(Colour::parse("#f0a"), Some(Colour::new(0xff, 0x00, 0xaa)));
        assert_eq!(Colour::parse("nope"), None);
    }

    #[test]
    fn hex_roundtrips() {
        let c = Colour::new(0x1e, 0x9e, 0x8a);
        assert_eq!(Colour::parse(&c.to_hex()), Some(c));
    }

    #[test]
    fn zone_masks_are_powers_of_two() {
        let masks: Vec<u8> = Zone::ALL.iter().map(|z| *z as u8).collect();
        assert_eq!(masks, vec![1, 2, 4, 8]);
    }

    #[test]
    fn effect_payload_clamps_out_of_range() {
        let p = effect_payload(Effect::Wave, 99, 200, Direction::LeftToRight, Colour::OFF);
        assert_eq!(p[1], 9, "speed clamped to 9");
        assert_eq!(p[2], 100, "brightness clamped to 100");
    }

    #[test]
    fn zone_word_puts_red_in_byte_one_and_blue_in_byte_three() {
        // Measured on hardware. The vendor notes say (R<<24)|(G<<16)|(B<<8),
        // which is transposed: following them lights the keyboard blue when
        // you ask for red.
        let w = zone_word(Zone::One, Colour::new(0xAA, 0xBB, 0xCC));
        let b = w.to_le_bytes();
        assert_eq!(b[0], 0x01, "zone id");
        assert_eq!(b[1], 0xAA, "red");
        assert_eq!(b[2], 0xBB, "green");
        assert_eq!(b[3], 0xCC, "blue");
    }

    #[test]
    fn every_zone_id_is_a_distinct_low_byte() {
        let words: Vec<u8> = Zone::ALL
            .iter()
            .map(|z| zone_word(*z, Colour::OFF).to_le_bytes()[0])
            .collect();
        assert_eq!(words, vec![1, 2, 4, 8]);
    }

    #[test]
    fn effect_payload_is_sixteen_bytes_with_the_commit_marker() {
        // The short 8-byte buffer was accepted, stored and read back — and
        // never lit anything, because it stopped before these two bytes.
        let p = effect_payload(Effect::Wave, 5, 100, Direction::LeftToRight, Colour::new(1, 2, 3));
        assert_eq!(p.len(), 16);
        assert_eq!(p[8], 0x03, "commit marker byte 8");
        assert_eq!(p[9], 0x01, "commit marker byte 9");
        assert_eq!(&p[..8], &[Effect::Wave as u8, 5, 100, 0, 2, 1, 2, 3]);
    }
}
