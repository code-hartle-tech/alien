//! The arc gauges on the dashboard.
//!
//! Drawn rather than imported. An arc, a needle-free filled sweep, and a
//! numeral is the whole thing — a sprite sheet would be bigger, would not
//! scale, and would mean shipping someone else's artwork.

use eframe::egui::{self, Color32, Pos2, Stroke};

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
    pub unit: &'a str,
    pub max: f32,
    pub colour: Color32,
}

impl Gauge<'_> {
    pub fn show(self, ui: &mut egui::Ui, size: f32) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }

        let p = ui.painter();
        let centre = rect.center();
        let radius = size * 0.40;
        let width = size * 0.075;

        // Track.
        arc(p, centre, radius, START, SWEEP, Stroke::new(width, theme::LINE));

        // Filled portion. An absent reading draws no sweep at all rather than
        // a zero — a gauge pinned at the bottom looks like a real measurement,
        // and "this sensor did not answer" is a different statement.
        if let Some(v) = self.value {
            let frac = (v as f32 / self.max).clamp(0.0, 1.0);
            if frac > 0.0 {
                arc(p, centre, radius, START, SWEEP * frac, Stroke::new(width, self.colour));
                // Cap: a small bright dot at the end of the sweep reads as a
                // live indicator rather than a static bar.
                let a = START + SWEEP * frac;
                let tip = Pos2::new(centre.x + radius * a.cos(), centre.y + radius * a.sin());
                p.circle_filled(tip, width * 0.42, theme::HILITE);
            }
        }

        // Value.
        let (text, colour) = match self.value {
            Some(v) => (format!("{v}"), theme::TEXT),
            None => ("—".to_string(), theme::MUTED),
        };
        p.text(
            centre - egui::vec2(0.0, size * 0.04),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(size * 0.26),
            colour,
        );
        p.text(
            centre + egui::vec2(0.0, size * 0.13),
            egui::Align2::CENTER_CENTER,
            self.unit,
            egui::FontId::proportional(size * 0.10),
            theme::MUTED,
        );
        p.text(
            centre + egui::vec2(0.0, size * 0.36),
            egui::Align2::CENTER_CENTER,
            self.label,
            egui::FontId::proportional(size * 0.10),
            theme::MUTED,
        );

        response
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

/// A compact history plot, used under the gauges.
pub fn sparkplot(ui: &mut egui::Ui, values: &[u16], size: egui::Vec2, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if !ui.is_rect_visible(rect) || values.len() < 2 {
        return;
    }
    let p = ui.painter();
    let lo = *values.iter().min().unwrap_or(&0) as f32;
    let hi = *values.iter().max().unwrap_or(&1) as f32;
    // A flat series is drawn as a flat line rather than being normalised into
    // noise that implies variation which is not there.
    let span = if (hi - lo).abs() < f32::EPSILON { 1.0 } else { hi - lo };

    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = rect.left() + rect.width() * (i as f32 / (values.len() - 1) as f32);
            let t = (*v as f32 - lo) / span;
            let y = rect.bottom() - rect.height() * t;
            Pos2::new(x, y)
        })
        .collect();

    // Fill under the curve, then the curve, so the line stays crisp on top.
    let mut fill = pts.clone();
    fill.push(Pos2::new(rect.right(), rect.bottom()));
    fill.push(Pos2::new(rect.left(), rect.bottom()));
    p.add(egui::Shape::convex_polygon(
        fill,
        colour.gamma_multiply(0.18),
        Stroke::NONE,
    ));
    p.add(egui::Shape::line(pts, Stroke::new(1.5, colour)));
}
