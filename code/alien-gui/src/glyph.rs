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
//! meaning. The project logo is the deliberate exception: one canonical
//! project-owned image is shared with the window and Linux desktop packages.

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
            p.line_segment(
                [Pos2::new(c.x - d, c.y - d), Pos2::new(c.x + d, c.y + d)],
                s,
            );
            p.line_segment(
                [Pos2::new(c.x + d, c.y - d), Pos2::new(c.x - d, c.y + d)],
                s,
            );
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
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                "?",
                crate::theme::mono_b(size + 2.0),
                colour,
            );
        }
        Mark::Chevron => {
            let s = Stroke::new(1.4_f32, colour);
            p.line_segment(
                [
                    Pos2::new(c.x - r * 0.4, c.y - r),
                    Pos2::new(c.x + r * 0.5, c.y),
                ],
                s,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + r * 0.5, c.y),
                    Pos2::new(c.x - r * 0.4, c.y + r),
                ],
                s,
            );
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
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(mark_w + gap + text_w, font.size + 6.0),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return rect.width();
    }
    let p = ui.painter();
    draw(
        p,
        Pos2::new(rect.left() + mark_w / 2.0, rect.center().y),
        mark,
        8.0,
        colour,
    );
    p.text(
        Pos2::new(rect.left() + mark_w + gap, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        colour,
    );
    rect.width()
}

/// Draw the canonical Alien logo texture, centred in a square.
pub fn logo(p: &egui::Painter, texture: egui::TextureId, centre: Pos2, size: f32) {
    let rect = egui::Rect::from_center_size(centre, egui::vec2(size, size));
    p.image(
        texture,
        rect,
        egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
}
