//! Dense terminal telemetry graphs built from Unicode Braille cells.
//!
//! The visual grammar is deliberately familiar to terminal resource monitors:
//! each cell carries two time samples, vertically quantised into four dots;
//! panels expose the current, minimum and maximum readings beside the trace;
//! and a block-cell fallback remains available for fonts without Braille.
//! The implementation here is original Rust tailored to Alien's fixed ANSI
//! renderer. It does not include or translate code from another project.

use crate::term::{fg, Rgb};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphMode {
    Braille,
    Block,
}

#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub floor: u16,
    pub ceiling: u16,
}

impl Scale {
    pub fn fitted(samples: &[Option<u16>], minimum_span: u16) -> Self {
        let observed_min = samples.iter().flatten().copied().min().unwrap_or(0);
        let observed_max = samples
            .iter()
            .flatten()
            .copied()
            .max()
            .unwrap_or(minimum_span.max(1));
        let span = observed_max
            .saturating_sub(observed_min)
            .max(minimum_span.max(1));
        let centre = (u32::from(observed_min) + u32::from(observed_max)) / 2;
        let half = u32::from(span).div_ceil(2);
        // Keep one representable unit above the floor. In normal telemetry the
        // half-span already guarantees this; the explicit clamp also makes the
        // invariant hold for adversarial u16::MAX-only input without a cast
        // wrapping 65_536 back to zero.
        let floor = centre.saturating_sub(half).min(u32::from(u16::MAX - 1));
        let ceiling = (floor + u32::from(span)).min(u32::from(u16::MAX));
        Self {
            floor: floor as u16,
            ceiling: ceiling.clamp(floor + 1, u32::from(u16::MAX)) as u16,
        }
    }

    fn level(self, sample: u16, rows: usize) -> usize {
        let steps = rows.saturating_mul(4).max(1);
        let span = usize::from(self.ceiling.saturating_sub(self.floor).max(1));
        let value = usize::from(sample.saturating_sub(self.floor)).min(span);
        ((value * steps + span / 2) / span).min(steps)
    }
}

pub struct GraphSpec<'a> {
    pub label: &'a str,
    pub unit: &'a str,
    pub samples: &'a [Option<u16>],
    pub width: usize,
    pub rows: usize,
    pub minimum_span: u16,
    pub colour: Rgb,
    pub dim: Rgb,
    pub mode: GlyphMode,
}

/// Render a compact bordered metric panel, excluding its outer indentation.
///
/// `width` is the exact visible-cell width. A Braille cell encodes two
/// consecutive samples, so a 50-column plot preserves up to 100 points.
pub fn panel(spec: GraphSpec<'_>) -> Vec<String> {
    let width = spec.width.max(18);
    let inner = width - 2;
    let rows = spec.rows.max(1);
    let latest = spec.samples.last().copied().flatten();
    let observed_min = spec.samples.iter().flatten().copied().min();
    let observed_max = spec.samples.iter().flatten().copied().max();
    let scale = Scale::fitted(spec.samples, spec.minimum_span);

    let reading = latest
        .map(|value| format!("{value} {}", spec.unit))
        .unwrap_or_else(|| format!("— {}", spec.unit));
    let title_budget = inner.saturating_sub(reading.chars().count() + 1);
    let title = truncate(spec.label, title_budget);
    let gap = inner.saturating_sub(title.chars().count() + reading.chars().count());

    let mut lines = Vec::with_capacity(rows + 3);
    lines.push(format!(
        "{}{}{}{}{}",
        fg(spec.colour, "┌"),
        fg(spec.colour, &title),
        fg(spec.dim, &"─".repeat(gap)),
        fg(latest.map_or(spec.dim, |_| spec.colour), &reading),
        fg(spec.colour, "┐"),
    ));

    let graph = graph_rows(spec.samples, inner, rows, scale, spec.mode);
    for row in graph {
        lines.push(format!(
            "{}{}{}",
            fg(spec.colour, "│"),
            fg(spec.colour, &row),
            fg(spec.colour, "│"),
        ));
    }

    let range = match (observed_min, observed_max) {
        (Some(min), Some(max)) => format!("min {min}  max {max}"),
        _ => "min —  max —".into(),
    };
    let scale_label = format!("scale {}..{}", scale.floor, scale.ceiling);
    let gap = inner.saturating_sub(range.chars().count() + scale_label.chars().count());
    let footer = if gap > 0 {
        format!("{range}{}{scale_label}", " ".repeat(gap))
    } else {
        truncate(&format!("{range} {scale_label}"), inner)
    };
    lines.push(format!(
        "{}{}{}",
        fg(spec.colour, "└"),
        fg(spec.dim, &pad(&footer, inner)),
        fg(spec.colour, "┘"),
    ));
    lines
}

pub fn graph_rows(
    samples: &[Option<u16>],
    width: usize,
    rows: usize,
    scale: Scale,
    mode: GlyphMode,
) -> Vec<String> {
    let rows = rows.max(1);
    let sample_capacity = match mode {
        GlyphMode::Braille => width.saturating_mul(2),
        GlyphMode::Block => width,
    };
    let tail = &samples[samples.len().saturating_sub(sample_capacity)..];
    let blank_cells = width.saturating_sub(match mode {
        GlyphMode::Braille => tail.len().div_ceil(2),
        GlyphMode::Block => tail.len(),
    });
    (0..rows)
        .map(|row| {
            let band_top = (rows - row) * 4;
            let band_bottom = band_top - 4;
            let mut line = String::with_capacity(width);
            line.push_str(&" ".repeat(blank_cells));
            match mode {
                GlyphMode::Braille => {
                    for pair in tail.chunks(2) {
                        let left =
                            pair[0].map(|value| sample_fill(scale, value, rows, band_bottom));
                        let right = pair
                            .get(1)
                            .copied()
                            .flatten()
                            .map(|value| sample_fill(scale, value, rows, band_bottom));
                        line.push(braille_cell(left, right));
                    }
                }
                GlyphMode::Block => {
                    for sample in tail {
                        line.push(match sample {
                            Some(value) => {
                                block_cell(sample_fill(scale, *value, rows, band_bottom))
                            }
                            None => ' ',
                        });
                    }
                }
            }
            pad(&line, width)
        })
        .collect()
}

fn band_fill(level: usize, band_bottom: usize) -> u8 {
    level.saturating_sub(band_bottom).min(4) as u8
}

/// Quantise a real measurement into one vertical band.
///
/// A value exactly at the scale floor would otherwise have zero set dots and
/// render identically to a missing sample. Give real measurements one lowest
/// dot in the bottom band only; upper bands remain empty. This makes stopped
/// fans and genuine zero readings visible without fabricating height.
fn sample_fill(scale: Scale, value: u16, rows: usize, band_bottom: usize) -> u8 {
    let fill = band_fill(scale.level(value, rows), band_bottom);
    if band_bottom == 0 {
        fill.max(1)
    } else {
        fill
    }
}

fn braille_cell(left: Option<u8>, right: Option<u8>) -> char {
    // Use a normal space whenever this vertical band has no lit dots. A
    // measured floor value receives one bottom-band dot in `sample_fill`, so
    // this still keeps real zero distinct from missing while preventing U+2800
    // placeholders from leaking into every empty upper band.
    if left.unwrap_or(0) == 0 && right.unwrap_or(0) == 0 {
        return ' ';
    }
    // Braille dot order is 1,2,3,7 on the left and 4,5,6,8 on the right.
    // Setting bottom-up makes each pair a filled area sample instead of a
    // disconnected line, which stays readable at one terminal row.
    const LEFT: [u8; 4] = [0x40, 0x04, 0x02, 0x01];
    const RIGHT: [u8; 4] = [0x80, 0x20, 0x10, 0x08];
    let mut bits = 0u8;
    for mask in LEFT.iter().take(usize::from(left.unwrap_or(0).min(4))) {
        bits |= mask;
    }
    for mask in RIGHT.iter().take(usize::from(right.unwrap_or(0).min(4))) {
        bits |= mask;
    }
    char::from_u32(0x2800 + u32::from(bits)).expect("Braille mask is a Unicode scalar")
}

fn block_cell(fill: u8) -> char {
    match fill.min(4) {
        0 => ' ',
        1 => '▂',
        2 => '▄',
        3 => '▆',
        _ => '█',
    }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.into();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    value.chars().take(width - 1).collect::<String>() + "…"
}

fn pad(value: &str, width: usize) -> String {
    let mut value = truncate(value, width);
    value.push_str(&" ".repeat(width.saturating_sub(value.chars().count())));
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: Rgb = Rgb(0x3f, 0xe8, 0x6c);
    const DIM: Rgb = Rgb(0x46, 0x56, 0x4b);

    fn visible(s: &str) -> String {
        let mut visible = String::new();
        let mut chars = s.chars();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                visible.push(ch);
                continue;
            }
            if chars.next() == Some('[') {
                for csi in chars.by_ref() {
                    if csi.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        }
        visible
    }

    #[test]
    fn braille_uses_two_samples_per_cell() {
        let rows = graph_rows(
            &[Some(0), Some(100), Some(50), Some(75)],
            2,
            1,
            Scale {
                floor: 0,
                ceiling: 100,
            },
            GlyphMode::Braille,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].chars().count(), 2);
        assert!(rows[0]
            .chars()
            .all(|glyph| ('\u{2800}'..='\u{28ff}').contains(&glyph)));
    }

    #[test]
    fn minimum_span_prevents_small_noise_from_filling_panel() {
        let scale = Scale::fitted(&[Some(61), Some(62), Some(63), Some(64)], 20);
        assert!(scale.ceiling - scale.floor >= 20);
        assert!(scale.floor <= 61);
        assert!(scale.ceiling >= 64);
    }

    #[test]
    fn panel_has_exact_cell_width_and_dense_metadata() {
        let samples: Vec<Option<u16>> = (0..90).map(|i| Some(55 + (i % 14))).collect();
        let lines = panel(GraphSpec {
            label: "CPU THERMAL · 120 s",
            unit: "°C",
            samples: &samples,
            width: 52,
            rows: 3,
            minimum_span: 20,
            colour: GREEN,
            dim: DIM,
            mode: GlyphMode::Braille,
        });
        assert_eq!(lines.len(), 5);
        for line in &lines {
            assert_eq!(visible(line).chars().count(), 52, "{line:?}");
        }
        let text = visible(&lines.join("\n"));
        assert!(text.contains("CPU THERMAL"));
        assert!(text.contains("60 °C"));
        assert!(text.contains("min 55"));
        assert!(text.contains("max 68"));
        assert!(text.contains("scale"));
    }

    #[test]
    fn block_fallback_avoids_braille_codepoints() {
        let rows = graph_rows(
            &[Some(0), Some(25), Some(50), Some(75), Some(100)],
            5,
            2,
            Scale {
                floor: 0,
                ceiling: 100,
            },
            GlyphMode::Block,
        );
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .flat_map(|row| row.chars())
            .all(|glyph| !('\u{2800}'..='\u{28ff}').contains(&glyph)));
    }

    #[test]
    fn consecutive_missing_samples_are_blank_time_columns_not_zeroes() {
        let samples = [Some(75), Some(80), None, None, Some(70), Some(65)];
        let scale = Scale::fitted(&samples, 20);
        let rows = graph_rows(&samples, 3, 2, scale, GlyphMode::Braille);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.chars().count(), 3);
            assert_eq!(row.chars().nth(1), Some(' '), "missing pair must be a gap");
        }
        assert!(rows
            .iter()
            .any(|row| row.chars().next().is_some_and(|glyph| glyph != ' ')));
        assert!(rows
            .iter()
            .any(|row| row.chars().nth(2).is_some_and(|glyph| glyph != ' ')));
    }

    #[test]
    fn missing_pair_and_measured_zero_pair_are_distinct() {
        assert_eq!(braille_cell(None, None), ' ');
        let rows = graph_rows(
            &[Some(0), Some(0), None, None],
            2,
            2,
            Scale {
                floor: 0,
                ceiling: 100,
            },
            GlyphMode::Braille,
        );
        assert_eq!(rows[0], "  ", "zero must not leak into the upper band");
        assert_ne!(
            rows[1].chars().next(),
            Some(' '),
            "measured zero needs a visible bottom dot"
        );
        assert_eq!(
            rows[1].chars().nth(1),
            Some(' '),
            "missing pair must remain visibly blank"
        );
    }

    #[test]
    fn block_fallback_shows_measured_zero_but_not_missing() {
        let rows = graph_rows(
            &[Some(0), None],
            2,
            1,
            Scale {
                floor: 0,
                ceiling: 100,
            },
            GlyphMode::Block,
        );
        assert_eq!(rows, vec!["▂ "]);
    }

    #[test]
    fn maximum_u16_scale_never_wraps_its_ceiling() {
        let scale = Scale::fitted(&[Some(u16::MAX), None, Some(u16::MAX)], 1);
        assert_eq!(scale.ceiling, u16::MAX);
        assert!(scale.floor < scale.ceiling);
    }

    #[test]
    fn missing_samples_do_not_affect_range_or_latest_reading() {
        let samples = [Some(61), None, Some(64), None];
        let scale = Scale::fitted(&samples, 20);
        assert!(scale.floor <= 61);
        assert!(scale.ceiling >= 64);

        let lines = panel(GraphSpec {
            label: "CPU",
            unit: "°C",
            samples: &samples,
            width: 32,
            rows: 2,
            minimum_span: 20,
            colour: GREEN,
            dim: DIM,
            mode: GlyphMode::Braille,
        });
        let text = visible(&lines.join("\n"));
        assert!(
            text.contains("— °C"),
            "latest unavailable must remain unknown"
        );
        assert!(text.contains("min 61  max 64"));
        assert!(!text.contains("min 0"));
    }
}
