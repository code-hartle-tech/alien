//! Status marks, drawn as polygons.
//!
//! # Why these are not characters
//!
//! The design uses ● ▲ ◇ ✕ ▮ ▾ as status indicators throughout. Neither
//! bundled face has a single one of them: checked against the actual `cmap`
//! tables, Archivo and IBM Plex Mono are both missing every mark in this file.
//!
//! Relying on egui's fallback fonts to cover them would mean the entire status
//! vocabulary of the UI — every "is this on", "is this hot", "did this work" —
//! rests on whichever stock face happens to have the codepoint, at whatever
//! size and baseline that face chose. One missing glyph is a tofu box in the
//! middle of a reading.
//!
//! Drawn, they are exact: the dot next to a temperature is the same dot every
//! time, sized to the text it sits beside, in the colour that carries the
//! meaning. It is also the same argument the rest of this crate already makes
//! about not shipping bitmaps.

use eframe::egui::{self, Color32, Pos2, Stroke};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// ● — a live reading, or an enabled state.
    Dot,
    /// ▲ — warning; also "turbo".
    TriUp,
    /// ▾ — a menu affordance.
    TriDown,
    /// ◇ — this effect uses the firmware's own palette.
    Diamond,
    /// ✕ — unsupported, or a hard failure.
    Cross,
    /// ▮ — a blinking cursor, for "still settling".
    Bar,
    /// ? — accepted by firmware, effect unconfirmed.
    Query,
    /// › — the status-bar prompt.
    Chevron,
}

/// Draw `mark` centred on `c`, sized to roughly `size` across, in `colour`.
pub fn draw(p: &egui::Painter, c: Pos2, mark: Mark, size: f32, colour: Color32) {
    let r = size / 2.0;
    match mark {
        Mark::Dot => {
            p.circle_filled(c, r * 0.8, colour);
        }
        Mark::TriUp => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x, c.y - r),
                    Pos2::new(c.x + r, c.y + r * 0.75),
                    Pos2::new(c.x - r, c.y + r * 0.75),
                ],
                colour,
                Stroke::NONE,
            ));
        }
        Mark::TriDown => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x, c.y + r * 0.75),
                    Pos2::new(c.x + r, c.y - r * 0.6),
                    Pos2::new(c.x - r, c.y - r * 0.6),
                ],
                colour,
                Stroke::NONE,
            ));
        }
        Mark::Diamond => {
            // Outline, not filled: it is a footnote marker, and a solid
            // diamond at 8px next to 11px text reads louder than the label.
            p.add(egui::Shape::closed_line(
                vec![
                    Pos2::new(c.x, c.y - r),
                    Pos2::new(c.x + r * 0.8, c.y),
                    Pos2::new(c.x, c.y + r),
                    Pos2::new(c.x - r * 0.8, c.y),
                ],
                crate::theme::hair(colour),
            ));
        }
        Mark::Cross => {
            let s = Stroke::new(1.4_f32, colour);
            let d = r * 0.8;
            p.line_segment([Pos2::new(c.x - d, c.y - d), Pos2::new(c.x + d, c.y + d)], s);
            p.line_segment([Pos2::new(c.x + d, c.y - d), Pos2::new(c.x - d, c.y + d)], s);
        }
        Mark::Bar => {
            p.rect_filled(
                egui::Rect::from_center_size(c, egui::vec2(r * 0.75, size)),
                0.0,
                colour,
            );
        }
        Mark::Query => {
            // Drawn with the mono face so it sits on the same baseline as the
            // label beside it; '?' is one of the few marks the fonts do have.
            p.text(c, egui::Align2::CENTER_CENTER, "?", crate::theme::mono_b(size + 2.0), colour);
        }
        Mark::Chevron => {
            let s = Stroke::new(1.4_f32, colour);
            p.line_segment([Pos2::new(c.x - r * 0.4, c.y - r), Pos2::new(c.x + r * 0.5, c.y)], s);
            p.line_segment([Pos2::new(c.x + r * 0.5, c.y), Pos2::new(c.x - r * 0.4, c.y + r)], s);
        }
    }
}

/// Mark plus label, laid out left to right. Returns the width used.
pub fn labelled(
    ui: &mut egui::Ui,
    mark: Mark,
    label: &str,
    font: egui::FontId,
    colour: Color32,
) -> f32 {
    let gap = 6.0;
    let mark_w = 9.0;
    let text_w = crate::theme::tracked_width(ui.ctx(), label, &font, 0.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(mark_w + gap + text_w, font.size + 6.0), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return rect.width();
    }
    let p = ui.painter();
    draw(p, Pos2::new(rect.left() + mark_w / 2.0, rect.center().y), mark, 8.0, colour);
    p.text(
        Pos2::new(rect.left() + mark_w + gap, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        colour,
    );
    rect.width()
}

/// The Alien mark: a broken ring with a heavy terminal node.
///
/// Solving the design's arc — `M16.8 66 A36 36 0 1 1 79.2 66` in a 96 box —
/// puts the centre at (48,48) and gives a 240° sweep from 150° to 30°, open at
/// the bottom, with the node sitting on the end point.
///
/// That is the same geometry as a [`crate::gauge`]: the mark is an instrument
/// dial with its needle parked. Worth keeping exact rather than eyeballed,
/// because the two appear side by side on the dashboard.
pub fn logo(p: &egui::Painter, centre: Pos2, size: f32) {
    let k = size / 96.0;
    let r = 36.0 * k;
    let w = 14.0 * k;

    let start = std::f32::consts::PI * 5.0 / 6.0;
    let sweep = std::f32::consts::PI * 4.0 / 3.0;

    let segs = 72;
    let pts: Vec<Pos2> = (0..=segs)
        .map(|i| {
            let a = start + sweep * (i as f32 / segs as f32);
            Pos2::new(centre.x + r * a.cos(), centre.y + r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(w, crate::theme::GREEN)));

    let end = start + sweep;
    let node = Pos2::new(centre.x + r * end.cos(), centre.y + r * end.sin());
    p.circle_filled(node, 9.5 * k, crate::theme::BRIGHT);
}
