//! The arc gauges and history plots.
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
        arc(p, centre, radius, START, SWEEP, Stroke::new(width, theme::LINE));

        // Filled portion. An absent reading draws no sweep at all rather than
        // a zero — a gauge pinned at the bottom looks like a real measurement,
        // and "this sensor did not answer" is a different statement.
        if let Some(v) = self.value {
            let frac = (v as f32 / self.max).clamp(0.0, 1.0);
            if frac > 0.0 {
                arc(p, centre, radius, START, SWEEP * frac, Stroke::new(width, self.colour));
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
        let (mark_w, gap) = if self.mark.is_some() { (8.0, 5.0) } else { (0.0, 0.0) };
        let mut x = centre.x - (tw + mark_w + gap) / 2.0;
        if let Some(m) = self.mark {
            glyph::draw(p, Pos2::new(x + mark_w / 2.0, uy), m, 7.0, unit_colour);
            x += mark_w + gap;
        }
        p.text(Pos2::new(x, uy), egui::Align2::LEFT_CENTER, self.unit, unit_font, unit_colour);

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
        p.circle_filled(Pos2::new(c.x + r * a.cos(), c.y + r * a.sin()), 0.6, theme::LINE);
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
        let cap_h = 14.0;
        let plot = egui::Rect::from_min_max(
            Pos2::new(rect.left() + pad, rect.top() + 8.0),
            Pos2::new(rect.right() - pad, rect.bottom() - cap_h - 8.0),
        );
        plot_into(ui.painter(), self.values, plot, self.colour, self.min_span);

        let font = theme::mono(10.0);
        let y = rect.bottom() - cap_h / 2.0 - 5.0;
        ui.painter().text(
            Pos2::new(rect.left() + pad, y),
            egui::Align2::LEFT_CENTER,
            self.title,
            font.clone(),
            theme::MUTED,
        );

        // Reading on the right, with its mark trailing it as the design has it.
        let mut x = rect.right() - pad;
        if let Some(m) = self.mark {
            glyph::draw(ui.painter(), Pos2::new(x - 4.0, y), m, 7.0, self.reading_colour);
            x -= 13.0;
        }
        ui.painter().text(
            Pos2::new(x, y),
            egui::Align2::RIGHT_CENTER,
            &self.reading,
            font,
            self.reading_colour,
        );
    }
}

/// Fill-plus-line plot inside an arbitrary rect.
///
/// # Gaps are gaps, not zeroes
///
/// A sample the firmware did not answer is `None`, and the line breaks there.
/// The first version stored missing readings as `0` and plotted them, which
/// produced a CPU temperature history full of cliffs to the floor — a plot
/// that looked like the processor repeatedly hit absolute zero. It is the same
/// rule the gauges follow: a missing reading is not a measurement of nothing.
pub fn plot_into(
    p: &egui::Painter,
    values: &[Option<u16>],
    rect: egui::Rect,
    colour: Color32,
    min_span: f32,
) {
    if values.len() < 2 || rect.width() <= 1.0 || rect.height() <= 1.0 {
        return;
    }
    let present: Vec<u16> = values.iter().flatten().copied().collect();
    if present.len() < 2 {
        return;
    }
    let lo = *present.iter().min().unwrap() as f32;
    let hi = *present.iter().max().unwrap() as f32;

    // Scale to at least `min_span`, centred on the data.
    //
    // Fitting the axis to the observed range exactly is what a chart library
    // does, and it is wrong here: a CPU idling between 61 and 64 °C rendered
    // as a full-height sawtooth that looked like thermal chaos. The floor
    // makes a small wiggle look like a small wiggle, which is the honest
    // picture and the one the design draws.
    let mid = (lo + hi) / 2.0;
    let span = (hi - lo).max(min_span);
    let base = mid - span / 2.0;

    let y_of = |v: u16| {
        let t = ((v as f32 - base) / span).clamp(0.0, 1.0);
        // Inset the extremes so the peak is not clipped by the frame.
        rect.bottom() - (0.08 + 0.84 * t) * rect.height()
    };
    let x_of = |i: usize| rect.left() + rect.width() * (i as f32 / (values.len() - 1) as f32);

    // Split into unbroken runs and draw each on its own.
    let mut run: Vec<Pos2> = Vec::new();
    let flush = |run: &mut Vec<Pos2>| {
        if run.len() >= 2 {
            let mut fill = run.clone();
            fill.push(Pos2::new(run[run.len() - 1].x, rect.bottom()));
            fill.push(Pos2::new(run[0].x, rect.bottom()));
            // Not convex_polygon: a real series is not convex, and egui's
            // convex path renders a concave outline as a fan that bleeds
            // outside the curve.
            p.add(egui::Shape::Path(egui::epaint::PathShape {
                points: fill,
                closed: true,
                fill: colour.gamma_multiply(0.13),
                stroke: egui::epaint::PathStroke::NONE,
            }));
            p.add(egui::Shape::line(std::mem::take(run), Stroke::new(1.5_f32, colour)));
        }
        run.clear();
    };

    for (i, v) in values.iter().enumerate() {
        match v {
            Some(v) => run.push(Pos2::new(x_of(i), y_of(*v))),
            None => flush(&mut run),
        }
    }
    flush(&mut run);
}
