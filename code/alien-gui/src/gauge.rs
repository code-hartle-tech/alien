//! The arc gauges and dense history monitors.
//!
//! Drawn rather than imported. An arc, a filled sweep and a numeral is the
//! whole thing — a sprite sheet would be bigger, would not scale, and would
//! mean shipping someone else's artwork.
//!
//! Geometry follows the design's own SVG: in a 130-unit box, a track of radius
//! 52 and width 8 sweeping 240° from 150°, a dotted index ring at radius 58,
//! and a 4-unit node riding the end of the sweep. Everything below is that,
//! divided by 130 and multiplied by whatever size the caller asks for.

use eframe::egui::{self, Color32, Pos2, Stroke};

use crate::glyph::{self, Mark};
use crate::theme;

/// Sweep of the arc, in radians, and where it starts.
///
/// 240° opening downward: the standard instrument-cluster look, and it leaves
/// room under the arc for a label without overlapping the sweep.
const SWEEP: f32 = std::f32::consts::PI * 4.0 / 3.0;
const START: f32 = std::f32::consts::PI * 5.0 / 6.0;

pub struct Gauge<'a> {
    pub label: &'a str,
    pub value: Option<u16>,
    /// Drawn under the numeral: `°C`, `RPM`, or `°C WARM`.
    pub unit: &'a str,
    /// Optional status mark ahead of the unit.
    pub mark: Option<Mark>,
    pub max: f32,
    pub colour: Color32,
    /// Colour of the node at the end of the sweep.
    pub tip: Color32,
}

impl<'a> Gauge<'a> {
    /// A temperature gauge, with the band colour and status mark chosen from
    /// the reading itself.
    pub fn temp(label: &'a str, value: Option<u16>) -> Self {
        let warm = value.is_some_and(|v| v >= 70);
        let colour = theme::temp_colour(value.unwrap_or(0));
        Gauge {
            label,
            value,
            unit: if warm { "°C WARM" } else { "°C" },
            mark: value.map(|_| if warm { Mark::TriUp } else { Mark::Dot }),
            max: 100.0,
            colour,
            tip: if warm { theme::AMBER_LT } else { theme::BRIGHT },
        }
    }

    /// A fan gauge. No status mark: an RPM is not in a band, it is just a
    /// number, and a green dot beside it would imply a judgement we are not
    /// making.
    pub fn fan(label: &'a str, value: Option<u16>, max: f32) -> Self {
        Gauge {
            label,
            value,
            unit: "RPM",
            mark: None,
            max,
            colour: theme::GREEN,
            tip: theme::BRIGHT,
        }
    }

    pub fn show(self, ui: &mut egui::Ui, size: f32) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }

        let k = size / 130.0;
        let p = ui.painter();
        let centre = rect.center();
        let radius = 52.0 * k;
        let width = 8.0 * k;

        // Dotted index ring, outside the track.
        index_ring(p, centre, 58.0 * k);

        // Track.
        arc(
            p,
            centre,
            radius,
            START,
            SWEEP,
            Stroke::new(width, theme::LINE),
        );

        // Filled portion. An absent reading draws no sweep at all rather than
        // a zero — a gauge pinned at the bottom looks like a real measurement,
        // and "this sensor did not answer" is a different statement.
        if let Some(v) = self.value {
            let frac = (v as f32 / self.max).clamp(0.0, 1.0);
            if frac > 0.0 {
                arc(
                    p,
                    centre,
                    radius,
                    START,
                    SWEEP * frac,
                    Stroke::new(width, self.colour),
                );
                let a = START + SWEEP * frac;
                let node = Pos2::new(centre.x + radius * a.cos(), centre.y + radius * a.sin());
                p.circle_filled(node, 4.0 * k, self.tip);
            }
        }

        // Reading.
        let (text, colour) = match self.value {
            Some(v) => (v.to_string(), theme::TEXT),
            None => ("—".to_owned(), theme::DIM),
        };
        p.text(
            centre - egui::vec2(0.0, 9.5 * k),
            egui::Align2::CENTER_CENTER,
            text,
            theme::mono_b((size * 0.23).max(11.0)),
            colour,
        );

        // Unit line, with its status mark centred as a unit with the text.
        let unit_font = theme::mono(11.0 * k.max(0.75));
        let uy = centre.y + 15.0 * k;
        let unit_colour = match self.value {
            None => theme::DIM,
            Some(_) => self.colour,
        };
        let tw = theme::tracked_width(ui.ctx(), self.unit, &unit_font, 0.0);
        let (mark_w, gap) = if self.mark.is_some() {
            (8.0, 5.0)
        } else {
            (0.0, 0.0)
        };
        let mut x = centre.x - (tw + mark_w + gap) / 2.0;
        if let Some(m) = self.mark {
            glyph::draw(p, Pos2::new(x + mark_w / 2.0, uy), m, 7.0, unit_colour);
            x += mark_w + gap;
        }
        p.text(
            Pos2::new(x, uy),
            egui::Align2::LEFT_CENTER,
            self.unit,
            unit_font,
            unit_colour,
        );

        // Name, tracked, well clear of the sweep.
        let lf = theme::sans_b(10.0 * k.max(0.8));
        let lw = theme::tracked_width(ui.ctx(), self.label, &lf, 2.0);
        theme::tracked(
            ui.ctx(),
            ui.painter(),
            Pos2::new(centre.x - lw / 2.0, centre.y + 51.5 * k),
            self.label,
            lf,
            theme::MUTED,
            2.0,
        );

        response
    }
}

/// The ring of index dots around the outside of the dial.
fn index_ring(p: &egui::Painter, c: Pos2, r: f32) {
    // The design's `stroke-dasharray="1 8"` — one unit on, eight off — works
    // out at a dot roughly every 9 units of circumference.
    let n = ((2.0 * std::f32::consts::PI * r / 9.0) as usize).clamp(12, 64);
    for i in 0..n {
        let a = 2.0 * std::f32::consts::PI * (i as f32 / n as f32);
        p.circle_filled(
            Pos2::new(c.x + r * a.cos(), c.y + r * a.sin()),
            0.6,
            theme::LINE,
        );
    }
}

/// Approximate an arc with a polyline.
///
/// egui has no arc primitive, and the segment count is tied to the angle so a
/// short sweep is not over-tessellated while a long one stays smooth.
fn arc(p: &egui::Painter, c: Pos2, r: f32, start: f32, sweep: f32, stroke: Stroke) {
    let segments = ((sweep.abs() * r * 0.25) as usize).clamp(8, 96);
    let pts: Vec<Pos2> = (0..=segments)
        .map(|i| {
            let a = start + sweep * (i as f32 / segments as f32);
            Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(pts, stroke));
}

/// A framed history plot with its own caption row.
pub struct SparkCard<'a> {
    pub title: &'a str,
    pub values: &'a [Option<u16>],
    pub colour: Color32,
    /// Right-hand reading, already formatted.
    pub reading: String,
    pub reading_colour: Color32,
    pub mark: Option<Mark>,
    /// Smallest range the y-axis will show, in the series' own units.
    pub min_span: f32,
}

impl SparkCard<'_> {
    pub fn show(self, ui: &mut egui::Ui, size: egui::Vec2) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        theme::card(ui.painter(), rect);

        let pad = 10.0;
        let layout = SparkLayout::for_height(rect.height());
        let plot = egui::Rect::from_min_max(
            Pos2::new(rect.left() + pad, rect.top() + layout.plot_top),
            Pos2::new(rect.right() - pad, rect.top() + layout.plot_bottom),
        );
        let scale = PlotScale::fitted(self.values, self.min_span);
        dense_plot_into(ui.painter(), self.values, plot, self.colour, scale);

        let font = theme::mono(10.0);
        let top_y = rect.top() + layout.header_y;
        ui.painter().text(
            Pos2::new(rect.left() + pad, top_y),
            egui::Align2::LEFT_CENTER,
            self.title,
            font.clone(),
            theme::MUTED,
        );

        // Current reading occupies the strongest, top-right instrument slot.
        let mut x = rect.right() - pad;
        if let Some(m) = self.mark {
            glyph::draw(
                ui.painter(),
                Pos2::new(x - 4.0, top_y),
                m,
                7.0,
                self.reading_colour,
            );
            x -= 13.0;
        }
        ui.painter().text(
            Pos2::new(x, top_y),
            egui::Align2::RIGHT_CENTER,
            &self.reading,
            theme::mono_b(10.0),
            self.reading_colour,
        );

        if let Some(footer_y) = layout.footer_y {
            let footer_y = rect.top() + footer_y;
            let text = SparkFooterText::new(observed_range(self.values), scale);
            let widths = text.widths(|value| theme::tracked_width(ui.ctx(), value, &font, 0.0));
            let mode = spark_footer_mode((rect.width() - 2.0 * pad).max(0.0), widths);
            let pair = match mode {
                SparkFooterMode::Detailed => Some((&text.detailed_range, &text.detailed_scale)),
                SparkFooterMode::Compact => Some((&text.compact_range, &text.compact_scale)),
                SparkFooterMode::RangeOnly | SparkFooterMode::Hidden => None,
            };

            if let Some((range, scale)) = pair {
                ui.painter().text(
                    Pos2::new(rect.left() + pad, footer_y),
                    egui::Align2::LEFT_CENTER,
                    range,
                    font.clone(),
                    theme::MUTED,
                );
                ui.painter().text(
                    Pos2::new(rect.right() - pad, footer_y),
                    egui::Align2::RIGHT_CENTER,
                    scale,
                    font,
                    theme::DIM,
                );
            } else if mode == SparkFooterMode::RangeOnly {
                ui.painter().text(
                    Pos2::new(rect.center().x, footer_y),
                    egui::Align2::CENTER_CENTER,
                    &text.compact_range,
                    font,
                    theme::MUTED,
                );
            }
        }
    }
}

/// Vertical slots for a history card, expressed relative to its top edge.
///
/// Fan cards can be as short as 36 px in the responsive stack. At that size a
/// metadata footer would consume the graph, so compact cards keep the title
/// and current reading but devote the rest to the trace. Dashboard cards at
/// 104 px retain the complete min/max/scale footer.
#[derive(Clone, Copy, Debug)]
struct SparkLayout {
    header_y: f32,
    plot_top: f32,
    plot_bottom: f32,
    footer_y: Option<f32>,
}

impl SparkLayout {
    const DETAILED_MIN_HEIGHT: f32 = 76.0;

    fn for_height(height: f32) -> Self {
        if height >= Self::DETAILED_MIN_HEIGHT {
            Self {
                header_y: 11.0,
                plot_top: 21.0,
                plot_bottom: (height - 19.0).max(22.0),
                footer_y: Some(height - 8.0),
            }
        } else {
            Self {
                header_y: 8.0,
                plot_top: 15.0,
                plot_bottom: (height - 5.0).max(16.0),
                footer_y: None,
            }
        }
    }
}

const SPARK_FOOTER_MIN_GAP: f32 = 8.0;

/// Complete and compact forms of the same footer metadata.
///
/// The compact form removes the labels but not the values: `47–50` is the
/// observed range and `44…54` is the plotted scale. If even those two values
/// cannot maintain an eight-pixel separation, scale is elided before either
/// string is allowed to collide with the other.
#[derive(Debug, PartialEq, Eq)]
struct SparkFooterText {
    detailed_range: String,
    detailed_scale: String,
    compact_range: String,
    compact_scale: String,
}

impl SparkFooterText {
    fn new(range: (Option<u16>, Option<u16>), scale: PlotScale) -> Self {
        let (detailed_range, compact_range) = match range {
            (Some(minimum), Some(maximum)) => (
                format!("MIN {minimum}  MAX {maximum}"),
                format!("{minimum}–{maximum}"),
            ),
            _ => ("MIN —  MAX —".into(), "—".into()),
        };
        Self {
            detailed_range,
            detailed_scale: format!("SCALE {:.0}..{:.0}", scale.floor, scale.ceiling),
            compact_range,
            compact_scale: format!("{:.0}…{:.0}", scale.floor, scale.ceiling),
        }
    }

    fn widths(&self, mut width_of: impl FnMut(&str) -> f32) -> SparkFooterWidths {
        SparkFooterWidths {
            detailed_range: width_of(&self.detailed_range),
            detailed_scale: width_of(&self.detailed_scale),
            compact_range: width_of(&self.compact_range),
            compact_scale: width_of(&self.compact_scale),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SparkFooterWidths {
    detailed_range: f32,
    detailed_scale: f32,
    compact_range: f32,
    compact_scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SparkFooterMode {
    Detailed,
    Compact,
    RangeOnly,
    Hidden,
}

fn spark_footer_mode(available_width: f32, widths: SparkFooterWidths) -> SparkFooterMode {
    let pair_fits = |left: f32, right: f32| left + SPARK_FOOTER_MIN_GAP + right <= available_width;
    if pair_fits(widths.detailed_range, widths.detailed_scale) {
        SparkFooterMode::Detailed
    } else if pair_fits(widths.compact_range, widths.compact_scale) {
        SparkFooterMode::Compact
    } else if widths.compact_range <= available_width {
        SparkFooterMode::RangeOnly
    } else {
        SparkFooterMode::Hidden
    }
}

#[derive(Clone, Copy, Debug)]
struct PlotScale {
    floor: f32,
    ceiling: f32,
}

impl PlotScale {
    fn fitted(values: &[Option<u16>], min_span: f32) -> Self {
        let (minimum, maximum) = observed_range(values);
        let minimum = minimum.unwrap_or(0) as f32;
        let maximum = maximum.unwrap_or_else(|| min_span.max(1.0) as u16) as f32;
        let span = (maximum - minimum).max(min_span.max(1.0));
        let midpoint = (minimum + maximum) / 2.0;
        Self {
            floor: (midpoint - span / 2.0).max(0.0),
            ceiling: (midpoint - span / 2.0).max(0.0) + span,
        }
    }

    fn fraction(self, value: u16) -> f32 {
        ((value as f32 - self.floor) / (self.ceiling - self.floor).max(1.0)).clamp(0.0, 1.0)
    }
}

fn observed_range(values: &[Option<u16>]) -> (Option<u16>, Option<u16>) {
    let mut present = values.iter().flatten().copied();
    let Some(first) = present.next() else {
        return (None, None);
    };
    present.fold((Some(first), Some(first)), |(minimum, maximum), value| {
        (
            Some(minimum.unwrap_or(value).min(value)),
            Some(maximum.unwrap_or(value).max(value)),
        )
    })
}

/// Dot-matrix history inside an arbitrary rect.
///
/// # Gaps are gaps, not zeroes
///
/// A sample the firmware did not answer is `None`, and the line breaks there.
/// The first version stored missing readings as `0` and plotted them, which
/// produced a CPU temperature history full of cliffs to the floor — a plot
/// that looked like the processor repeatedly hit absolute zero. It is the same
/// rule the gauges follow: a missing reading is not a measurement of nothing.
fn dense_plot_into(
    p: &egui::Painter,
    values: &[Option<u16>],
    rect: egui::Rect,
    colour: Color32,
    scale: PlotScale,
) {
    if values.len() < 2 || rect.width() <= 1.0 || rect.height() <= 1.0 {
        return;
    }
    if values.iter().flatten().count() < 2 {
        return;
    }

    // A faint grid anchors time and scale without turning the plot into a
    // conventional chart. This is still Alien's flat instrument panel.
    for i in 0..=4 {
        let y = egui::lerp(rect.bottom()..=rect.top(), i as f32 / 4.0);
        p.hline(rect.x_range(), y, Stroke::new(1.0_f32, theme::LINE));
    }
    for i in 0..=6 {
        let x = egui::lerp(rect.left()..=rect.right(), i as f32 / 6.0);
        p.vline(
            x,
            rect.y_range(),
            Stroke::new(1.0_f32, theme::LINE.gamma_multiply(0.6)),
        );
    }

    let x_step = (rect.width() / values.len().saturating_sub(1).max(1) as f32).max(1.0);
    let dot_step = (rect.height() / 12.0).clamp(2.0, 5.0);
    let radius = (x_step.min(dot_step) * 0.32).clamp(0.65, 1.45);
    for (index, value) in values.iter().enumerate() {
        let Some(value) = value else { continue };
        let x = rect.left()
            + rect.width() * (index as f32 / values.len().saturating_sub(1).max(1) as f32);
        let y = rect.bottom() - scale.fraction(*value) * rect.height();
        let mut dot_y = rect.bottom() - radius;
        while dot_y >= y {
            let distance = ((dot_y - y) / rect.height().max(1.0)).clamp(0.0, 1.0);
            p.circle_filled(
                Pos2::new(x, dot_y),
                radius,
                colour.gamma_multiply(0.34 + 0.66 * (1.0 - distance)),
            );
            dot_y -= dot_step;
        }
    }

    if let Some((index, value)) = values
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, value)| value.map(|value| (index, value)))
    {
        let x = rect.left()
            + rect.width() * (index as f32 / values.len().saturating_sub(1).max(1) as f32);
        let y = rect.bottom() - scale.fraction(value) * rect.height();
        p.circle_stroke(Pos2::new(x, y), 3.0, Stroke::new(1.0_f32, theme::BRIGHT));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_spark_layout(height: f32, minimum_plot_height: f32, detailed: bool) {
        let layout = SparkLayout::for_height(height);
        assert!(layout.header_y < layout.plot_top, "header must clear plot");
        assert!(
            layout.plot_bottom > layout.plot_top,
            "{height}px card inverted its plot"
        );
        assert!(
            layout.plot_bottom - layout.plot_top >= minimum_plot_height,
            "{height}px card left only {}px of plot",
            layout.plot_bottom - layout.plot_top
        );
        assert!(layout.plot_bottom <= height);
        assert_eq!(layout.footer_y.is_some(), detailed);
        if let Some(footer_y) = layout.footer_y {
            assert!(layout.plot_bottom < footer_y, "plot must clear footer");
            assert!(footer_y < height, "footer must remain inside card");
        }
    }

    #[test]
    fn responsive_spark_geometry_preserves_real_plot_space() {
        assert_spark_layout(36.0, 16.0, false);
        assert_spark_layout(60.0, 36.0, false);
        assert_spark_layout(104.0, 60.0, true);
    }

    #[test]
    fn plot_scale_keeps_an_honest_minimum_span() {
        let scale = PlotScale::fitted(&[Some(61), Some(62), None, Some(64)], 20.0);
        assert!(scale.ceiling - scale.floor >= 20.0);
        assert!(scale.floor <= 61.0);
        assert!(scale.ceiling >= 64.0);
    }

    #[test]
    fn observed_range_ignores_gaps() {
        assert_eq!(
            observed_range(&[None, Some(44), None, Some(81), Some(52)]),
            (Some(44), Some(81))
        );
        assert_eq!(observed_range(&[None, None]), (None, None));
    }

    #[test]
    fn gaps_do_not_become_zeroes_in_scale_or_range() {
        let values = [Some(61), None, None, Some(64), None];
        let scale = PlotScale::fitted(&values, 20.0);
        assert_eq!(observed_range(&values), (Some(61), Some(64)));
        assert!(scale.floor <= 61.0);
        assert!(scale.ceiling >= 64.0);
        assert!(scale.floor > 0.0, "missing values must not inject zero");
    }

    #[test]
    fn plot_scale_clamps_outliers_to_the_panel() {
        let scale = PlotScale {
            floor: 40.0,
            ceiling: 80.0,
        };
        assert_eq!(scale.fraction(10), 0.0);
        assert_eq!(scale.fraction(100), 1.0);
    }

    fn mono_10_width(text: &str) -> f32 {
        // The bundled IBM Plex Mono advances every glyph by 0.6 em.
        text.chars().count() as f32 * 6.0
    }

    fn footer_widths(text: &SparkFooterText) -> SparkFooterWidths {
        text.widths(mono_10_width)
    }

    fn occupied_width(mode: SparkFooterMode, widths: SparkFooterWidths) -> f32 {
        match mode {
            SparkFooterMode::Detailed => {
                widths.detailed_range + SPARK_FOOTER_MIN_GAP + widths.detailed_scale
            }
            SparkFooterMode::Compact => {
                widths.compact_range + SPARK_FOOTER_MIN_GAP + widths.compact_scale
            }
            SparkFooterMode::RangeOnly => widths.compact_range,
            SparkFooterMode::Hidden => 0.0,
        }
    }

    #[test]
    fn default_three_up_fan_footer_compacts_without_collision() {
        // 980 window - 168 nav - 40 central margins = 772. The history column
        // is (772 - 12) * .63, then divided into three cards with two 10 px gaps.
        let card_width = (((980.0_f32 - 168.0 - 40.0 - 12.0) * 0.63) - 20.0) / 3.0;
        assert!((card_width - 152.933_33).abs() < 0.001);
        let available_width = card_width - 20.0;
        let text = SparkFooterText::new(
            (Some(5769), Some(6122)),
            PlotScale {
                floor: 5646.0,
                ceiling: 6246.0,
            },
        );
        let widths = footer_widths(&text);
        let mode = spark_footer_mode(available_width, widths);

        assert_eq!(mode, SparkFooterMode::Compact);
        assert!(occupied_width(mode, widths) <= available_width);
    }

    #[test]
    fn wide_footer_keeps_detailed_min_max_and_scale_labels() {
        let available_width = 271.0 - 20.0;
        let text = SparkFooterText::new(
            (Some(5769), Some(6122)),
            PlotScale {
                floor: 5646.0,
                ceiling: 6246.0,
            },
        );
        let widths = footer_widths(&text);
        let mode = spark_footer_mode(available_width, widths);

        assert_eq!(mode, SparkFooterMode::Detailed);
        assert!(occupied_width(mode, widths) <= available_width);
    }

    #[test]
    fn narrower_footer_elides_scale_before_text_can_overlap() {
        let available_width = 120.0 - 20.0;
        let text = SparkFooterText::new(
            (Some(5769), Some(6122)),
            PlotScale {
                floor: 5646.0,
                ceiling: 6246.0,
            },
        );
        let widths = footer_widths(&text);
        let mode = spark_footer_mode(available_width, widths);

        assert_eq!(mode, SparkFooterMode::RangeOnly);
        assert!(occupied_width(mode, widths) <= available_width);
    }

    #[test]
    fn five_digit_extremes_never_force_a_pair_into_default_card() {
        let available_width = 152.933_33 - 20.0;
        let text = SparkFooterText::new(
            (Some(65_000), Some(u16::MAX)),
            PlotScale {
                floor: 64_700.0,
                ceiling: 65_835.0,
            },
        );
        let widths = footer_widths(&text);
        let mode = spark_footer_mode(available_width, widths);

        assert_eq!(mode, SparkFooterMode::RangeOnly);
        assert!(occupied_width(mode, widths) <= available_width);
    }

    #[test]
    fn footer_hides_when_even_the_range_cannot_fit() {
        let available_width = 50.0;
        let text = SparkFooterText::new(
            (Some(65_000), Some(u16::MAX)),
            PlotScale {
                floor: 64_700.0,
                ceiling: 65_835.0,
            },
        );
        let widths = footer_widths(&text);
        let mode = spark_footer_mode(available_width, widths);

        assert_eq!(mode, SparkFooterMode::Hidden);
        assert_eq!(occupied_width(mode, widths), 0.0);
    }
}
