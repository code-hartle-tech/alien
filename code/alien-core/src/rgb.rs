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
                    Some(
                        v * 17, // 0xF -> 0xFF, the usual short-hex expansion
                    )
                };
                Some(Colour {
                    r: d(0)?,
                    g: d(1)?,
                    b: d(2)?,
                })
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

/// `SetGamingLEDBehavior` word used by Covini to enable static zones.
///
/// Low byte 8 selects the zone-status operation. Zones 1 through 4 occupy
/// bits 40 through 43, respectively. The path is visible both in
/// `LightingPage_Covini` and in the PH315-53's function-2 WSMI dispatch.
pub fn zone_enable_word(enabled: [bool; 4]) -> u64 {
    enabled
        .into_iter()
        .enumerate()
        .fold(8u64, |word, (index, on)| {
            word | ((on as u64) << (40 + index))
        })
}

/// True only for the exact Covini zone-mask shape Alien is willing to send.
pub fn is_zone_enable_word(word: u64) -> bool {
    const ZONE_BITS: u64 = 0x0F << 40;
    word & !ZONE_BITS == 8
}

/// Firmware backlight animations.
///
/// `Neon` and `Wave` ignore the colour field — the firmware picks its own
/// palette — so the API takes the colour anyway and documents which effects
/// honour it rather than exposing two incompatible call shapes.
///
/// The names and values here follow the **Covini** family used by the PH315-53
/// in its model-certified PredatorSense 3.00.3152 package. Covini offers five
/// animated modes. Values 6 and 7 (`Meteor` and `Twinkling`) belong to the
/// newer Clubman family and must not be sent to this keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Solid colour across all zones. Honours colour.
    Static = 0,
    /// Breathing pattern using the selected Pattern colour.
    Breath = 1,
    /// Firmware palette sweep.
    Neon = 2,
    /// Wave pattern using the firmware palette.
    Wave = 3,
    /// Shifting pattern using the selected Pattern colour and direction.
    Shifting = 4,
    /// Zoom pattern using the selected Pattern colour.
    Zoom = 5,
}

impl Effect {
    /// Whether this effect uses the colour argument at all. Worth surfacing in
    /// a UI — a colour picker that silently does nothing is a bug report.
    pub fn honours_colour(self) -> bool {
        matches!(
            self,
            Effect::Static | Effect::Breath | Effect::Shifting | Effect::Zoom
        )
    }

    /// Covini exposes a direction selector only for Wave and Shifting. The
    /// other animations leave the direction byte at zero.
    pub fn honours_direction(self) -> bool {
        matches!(self, Effect::Wave | Effect::Shifting)
    }

    pub fn name(self) -> &'static str {
        match self {
            Effect::Static => "static",
            Effect::Breath => "breath",
            Effect::Neon => "neon",
            Effect::Wave => "wave",
            Effect::Shifting => "shifting",
            Effect::Zoom => "zoom",
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
            _ => return None,
        })
    }

    /// Display order from `LightingDynamicUI_Covini`: breathing, wave, zoom,
    /// shifting, neon. Static is the separate first tab in PredatorSense.
    pub const ALL: [Effect; 6] = [
        Effect::Static,
        Effect::Breath,
        Effect::Wave,
        Effect::Zoom,
        Effect::Shifting,
        Effect::Neon,
    ];
}

/// Travel direction for directional effects.
///
/// The compiled Covini BAML applies the leftward/rightward visual styles to
/// raw tags 2/1 respectively, and live PH315-53 PECM capture confirms Alien's
/// `L-R` selection writes 2 while `R-L` writes 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    RightToLeft = 1,
    LeftToRight = 2,
}

/// Brightness values emitted by the PH315-53 PredatorSense UI.
///
/// Its visible slider is logical levels 1 through 5; the managed Covini code
/// serialises them as `(level - 1) * 25`. Alien exposes the wire value as a
/// percentage, so inputs are snapped to the nearest certified step.
pub const BRIGHTNESS_STEPS: [u8; 5] = [0, 25, 50, 75, 100];

pub fn covini_brightness(value: u8) -> u8 {
    let clamped = value.min(100) as u16;
    (((clamped + 12) / 25) * 25) as u8
}

/// Canonical speed value emitted by the PH315-53 Covini UI.
///
/// Static has no animation and always serialises zero. Every animated pattern
/// is constrained to the BAML slider's exact 1-through-9 range. Frontends use
/// the same helper for persistence and reporting so they cannot claim a raw
/// value that differs from the byte sent to firmware.
pub fn covini_speed(effect: Effect, value: u8) -> u8 {
    if effect == Effect::Static {
        0
    } else {
        value.clamp(1, 9)
    }
}

/// Exact PH315-53 payload for `SetGamingKBBacklight` (function 20) — a u64.
///
/// ```text
/// [ mode, speed, brightness, mode_flag, direction, R, G, B ]
/// ```
/// Wave sets `mode_flag` to 8. Only Wave and Shifting set `direction`.
///
/// This shape is independently proved at every layer: Covini managed code
/// packs a `ulong`; the 3.00.3152 native service accepts at most eight IPC
/// bytes and zero-pads the WMI SAFEARRAY; and the PH315-53 `WMBH` ACPI method
/// reads only bytes 0 through 7. The `3,1` bytes used by later Clubman code are
/// not commit markers and do not belong in this target's protocol.
pub fn effect_payload(
    effect: Effect,
    speed: u8,
    brightness: u8,
    dir: Direction,
    c: Colour,
) -> [u8; 8] {
    let speed = covini_speed(effect, speed);
    let (mode_flag, direction) = match effect {
        Effect::Wave => (8, dir as u8),
        Effect::Shifting => (0, dir as u8),
        _ => (0, 0),
    };
    // Static colour travels through function 6, not through this function-20
    // colour field. Wave and Neon use palettes generated by the firmware.
    // The exact Covini code therefore leaves RGB zero for all three; carrying
    // a remembered colour here makes the payload differ from PredatorSense
    // even if the firmware happens to ignore it.
    let colour = match effect {
        Effect::Breath | Effect::Zoom | Effect::Shifting => c,
        Effect::Static | Effect::Wave | Effect::Neon => Colour::OFF,
    };
    [
        effect as u8,
        speed,
        covini_brightness(brightness),
        mode_flag,
        direction,
        colour.r,
        colour.g,
        colour.b,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_hex_forms() {
        assert_eq!(
            Colour::parse("#1e9e8a"),
            Some(Colour::new(0x1e, 0x9e, 0x8a))
        );
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
    fn speed_matches_covini_static_and_animated_ranges() {
        assert_eq!(covini_speed(Effect::Static, 5), 0);
        assert_eq!(covini_speed(Effect::Breath, 0), 1);
        assert_eq!(covini_speed(Effect::Wave, 5), 5);
        assert_eq!(covini_speed(Effect::Shifting, 99), 9);
    }

    #[test]
    fn brightness_matches_the_five_step_covini_slider() {
        assert_eq!(BRIGHTNESS_STEPS, [0, 25, 50, 75, 100]);
        assert_eq!(covini_brightness(0), 0);
        assert_eq!(covini_brightness(12), 0);
        assert_eq!(covini_brightness(13), 25);
        assert_eq!(covini_brightness(62), 50);
        assert_eq!(covini_brightness(63), 75);
        assert_eq!(covini_brightness(255), 100);
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
    fn covini_zone_enable_mask_uses_only_bits_40_through_43() {
        let word = zone_enable_word([true, false, true, false]);
        assert_eq!(word, 8 | (1 << 40) | (1 << 42));
        assert_eq!(word.to_le_bytes(), [8, 0, 0, 0, 0, 0b0101, 0, 0]);
        assert!(is_zone_enable_word(word));
        assert!(is_zone_enable_word(zone_enable_word([false; 4])));
        assert!(!is_zone_enable_word(word | (1 << 39)));
        assert!(!is_zone_enable_word(9));
    }

    #[test]
    fn covini_effect_payload_is_the_exact_eight_byte_u64() {
        let p = effect_payload(
            Effect::Wave,
            5,
            100,
            Direction::LeftToRight,
            Colour::new(1, 2, 3),
        );
        assert_eq!(p, [Effect::Wave as u8, 5, 100, 8, 2, 0, 0, 0]);
        assert_eq!(
            effect_payload(
                Effect::Neon,
                9,
                50,
                Direction::RightToLeft,
                Colour::new(1, 2, 3)
            ),
            [Effect::Neon as u8, 9, 50, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            effect_payload(
                Effect::Static,
                0,
                75,
                Direction::RightToLeft,
                Colour::new(1, 2, 3)
            ),
            [Effect::Static as u8, 0, 75, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            effect_payload(
                Effect::Shifting,
                3,
                25,
                Direction::RightToLeft,
                Colour::new(1, 2, 3)
            ),
            [Effect::Shifting as u8, 3, 25, 0, 1, 1, 2, 3]
        );
    }

    #[test]
    fn every_covini_mode_has_an_exact_golden_record() {
        let colour = Colour::new(0x11, 0x22, 0x33);
        let cases = [
            (
                Effect::Static,
                Direction::LeftToRight,
                [0, 0, 100, 0, 0, 0, 0, 0],
            ),
            (
                Effect::Breath,
                Direction::LeftToRight,
                [1, 5, 100, 0, 0, 0x11, 0x22, 0x33],
            ),
            (
                Effect::Wave,
                Direction::LeftToRight,
                [3, 5, 100, 8, 2, 0, 0, 0],
            ),
            (
                Effect::Wave,
                Direction::RightToLeft,
                [3, 5, 100, 8, 1, 0, 0, 0],
            ),
            (
                Effect::Zoom,
                Direction::RightToLeft,
                [5, 5, 100, 0, 0, 0x11, 0x22, 0x33],
            ),
            (
                Effect::Shifting,
                Direction::LeftToRight,
                [4, 5, 100, 0, 2, 0x11, 0x22, 0x33],
            ),
            (
                Effect::Shifting,
                Direction::RightToLeft,
                [4, 5, 100, 0, 1, 0x11, 0x22, 0x33],
            ),
            (
                Effect::Neon,
                Direction::LeftToRight,
                [2, 5, 100, 0, 0, 0, 0, 0],
            ),
        ];

        for (effect, direction, expected) in cases {
            assert_eq!(
                effect_payload(effect, 5, 100, direction, colour),
                expected,
                "{} {direction:?}",
                effect.name()
            );
        }
    }

    #[test]
    fn every_animated_mode_accepts_exact_speed_and_brightness_endpoints() {
        for effect in [
            Effect::Breath,
            Effect::Wave,
            Effect::Zoom,
            Effect::Shifting,
            Effect::Neon,
        ] {
            let slow_off =
                effect_payload(effect, 1, 0, Direction::RightToLeft, Colour::new(1, 2, 3));
            let fast_full =
                effect_payload(effect, 9, 100, Direction::LeftToRight, Colour::new(1, 2, 3));
            assert_eq!((slow_off[1], slow_off[2]), (1, 0), "{}", effect.name());
            assert_eq!((fast_full[1], fast_full[2]), (9, 100), "{}", effect.name());
        }
    }

    #[test]
    fn covini_effect_names_and_colour_rules_match_predatorsense_3152() {
        assert!(
            Effect::parse("ripple").is_none(),
            "ripple belongs to a different controller"
        );
        assert!(
            Effect::parse("meteor").is_none(),
            "meteor belongs to Clubman"
        );
        assert!(
            Effect::parse("twinkling").is_none(),
            "twinkling belongs to Clubman"
        );
        for effect in [
            Effect::Static,
            Effect::Breath,
            Effect::Shifting,
            Effect::Zoom,
        ] {
            assert!(
                effect.honours_colour(),
                "{} should accept colour",
                effect.name()
            );
        }
        for effect in [Effect::Neon, Effect::Wave] {
            assert!(
                !effect.honours_colour(),
                "{} uses the firmware palette",
                effect.name()
            );
        }
        assert!(Effect::Wave.honours_direction());
        assert!(Effect::Shifting.honours_direction());
        for effect in [Effect::Static, Effect::Breath, Effect::Neon, Effect::Zoom] {
            assert!(
                !effect.honours_direction(),
                "{} has no direction control",
                effect.name()
            );
        }
        assert_eq!(
            Effect::ALL,
            [
                Effect::Static,
                Effect::Breath,
                Effect::Wave,
                Effect::Zoom,
                Effect::Shifting,
                Effect::Neon,
            ]
        );
    }

    #[test]
    fn covini_direction_bytes_are_effect_specific() {
        let colour = Colour::new(1, 2, 3);
        let wave = effect_payload(Effect::Wave, 5, 100, Direction::RightToLeft, colour);
        assert_eq!(wave, [3, 5, 100, 8, 1, 0, 0, 0]);

        let shifting = effect_payload(Effect::Shifting, 5, 100, Direction::LeftToRight, colour);
        assert_eq!(shifting, [4, 5, 100, 0, 2, 1, 2, 3]);

        let zoom = effect_payload(Effect::Zoom, 5, 100, Direction::RightToLeft, colour);
        assert_eq!(zoom, [5, 5, 100, 0, 0, 1, 2, 3]);
    }
}
