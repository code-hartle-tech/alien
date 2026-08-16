//! The Alien look: palette, type, and the handful of shapes the design is
//! built from.
//!
//! # Why this is not PredatorSense's palette any more
//!
//! It used to be. The first version matched the vendor deliberately — teal
//! `#00AEC7`, the same tab names in the same order — on the theory that
//! looking familiar was the point.
//!
//! This is a phosphor-green instrument panel instead, and that is a better
//! answer for three reasons. It is honestly *not* the vendor's software, so
//! dressing up as it invites the user to expect vendor behaviour we do not
//! have. Trade dress is the part of a UI that is actually risky to copy, and
//! this repo is published. And green-on-near-black is simply easier to read
//! at the small mono sizes this much telemetry needs.
//!
//! What carries over is the discipline: every shape here is drawn from
//! primitives. No extracted icons, no bitmaps, no vendor artwork.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Stroke, StrokeKind,
};

// ── Palette ─────────────────────────────────────────────────────────────────

/// Window and content background.
pub const BG: Color32 = Color32::from_rgb(0x07, 0x0A, 0x08);
/// Chrome and cards.
pub const PANEL: Color32 = Color32::from_rgb(0x0C, 0x11, 0x0D);
/// Raised fill: selected nav row, idle chip.
pub const RAISED: Color32 = Color32::from_rgb(0x12, 0x1A, 0x14);
/// Borders and rules.
pub const LINE: Color32 = Color32::from_rgb(0x1F, 0x2C, 0x22);

/// Dimmest legible text; also the disabled foreground.
pub const DIM: Color32 = Color32::from_rgb(0x46, 0x56, 0x4B);
/// Secondary text.
pub const MUTED: Color32 = Color32::from_rgb(0x64, 0x7A, 0x6B);
/// Body text.
pub const TEXT: Color32 = Color32::from_rgb(0xCF, 0xE0, 0xD2);

/// Primary accent.
pub const GREEN: Color32 = Color32::from_rgb(0x3F, 0xE8, 0x6C);
/// Highlight, for the single most important thing on screen.
pub const BRIGHT: Color32 = Color32::from_rgb(0xA6, 0xFF, 0xC0);
/// "Reading is in the normal band."
pub const OK: Color32 = Color32::from_rgb(0x49, 0xC9, 0x7A);

pub const AMBER: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x00);
pub const AMBER_LT: Color32 = Color32::from_rgb(0xFF, 0xD9, 0x8A);
/// Background behind an amber-outlined chip.
pub const AMBER_BG: Color32 = Color32::from_rgb(0x1E, 0x18, 0x09);
pub const RED: Color32 = Color32::from_rgb(0xFF, 0x52, 0x38);

/// Temperature → colour, shared with the TUI so both frontends agree on what
/// "hot" looks like.
pub fn temp_colour(c: u16) -> Color32 {
    match c {
        0..=69 => OK,
        70..=84 => AMBER,
        _ => RED,
    }
}

// ── Type ────────────────────────────────────────────────────────────────────
//
// Archivo for labels, IBM Plex Mono for anything numeric. The split is not
// decoration: readings change every second, and a proportional font makes a
// four-digit RPM jitter sideways as the digits change width.

const SANS: &str = "alien-sans";
const SANS_B: &str = "alien-sans-b";
const MONO: &str = "alien-mono";
const MONO_B: &str = "alien-mono-b";

pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SANS.into()))
}
pub fn sans_b(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SANS_B.into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO.into()))
}
pub fn mono_b(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO_B.into()))
}

/// Register the bundled faces.
///
/// Each family keeps egui's stock fonts appended behind it. The bundled faces
/// are unmodified upstream static fonts; the stock faces remain fallback for
/// any glyph they do not cover, such as a translated model name read from DMI.
fn install_fonts(ctx: &egui::Context) {
    use std::sync::Arc;

    let mut f = egui::FontDefinitions::default();

    // Whatever egui shipped with, in order, to fall back to.
    let stock_prop = f
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let stock_mono = f
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();

    for (name, bytes) in [
        (
            SANS,
            &include_bytes!("../assets/fonts/Archivo-Regular.ttf")[..],
        ),
        (
            SANS_B,
            &include_bytes!("../assets/fonts/Archivo-SemiBold.ttf")[..],
        ),
        (
            MONO,
            &include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")[..],
        ),
        (
            MONO_B,
            &include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf")[..],
        ),
    ] {
        f.font_data.insert(
            name.to_owned(),
            Arc::new(egui::FontData::from_static(bytes)),
        );
    }

    let with = |first: &str, stock: &[String]| {
        let mut v = vec![first.to_owned()];
        v.extend(stock.iter().cloned());
        v
    };

    f.families
        .insert(FontFamily::Name(SANS.into()), with(SANS, &stock_prop));
    f.families
        .insert(FontFamily::Name(SANS_B.into()), with(SANS_B, &stock_prop));
    f.families
        .insert(FontFamily::Name(MONO.into()), with(MONO, &stock_mono));
    f.families
        .insert(FontFamily::Name(MONO_B.into()), with(MONO_B, &stock_mono));

    // Built-in widgets (tooltips, the colour picker) should match too.
    f.families
        .insert(FontFamily::Proportional, with(SANS, &stock_prop));
    f.families
        .insert(FontFamily::Monospace, with(MONO, &stock_mono));

    ctx.set_fonts(f);
}

/// Draw letter-spaced text, returning the width consumed.
///
/// egui has no tracking control, and the design uses it structurally: the
/// wordmark is 10px apart, section headings 3px. The old trick of interleaving
/// spaces only produces one width — a full space — which is far too wide at
/// 11px. Advancing per glyph gives the real thing.
pub fn tracked(
    ctx: &egui::Context,
    painter: &egui::Painter,
    left_centre: Pos2,
    text: &str,
    font: FontId,
    colour: Color32,
    tracking: f32,
) -> f32 {
    let mut x = left_centre.x;
    for ch in text.chars() {
        let w = ctx.fonts(|f| f.glyph_width(&font, ch));
        painter.text(
            Pos2::new(x, left_centre.y),
            egui::Align2::LEFT_CENTER,
            ch,
            font.clone(),
            colour,
        );
        x += w + tracking;
    }
    (x - tracking - left_centre.x).max(0.0)
}

/// Measure what [`tracked`] would consume, without drawing.
pub fn tracked_width(ctx: &egui::Context, text: &str, font: &FontId, tracking: f32) -> f32 {
    let n = text.chars().count();
    if n == 0 {
        return 0.0;
    }
    let glyphs: f32 = text
        .chars()
        .map(|c| ctx.fonts(|f| f.glyph_width(font, c)))
        .sum();
    glyphs + tracking * (n as f32 - 1.0)
}

// ── Shapes ──────────────────────────────────────────────────────────────────

/// A one-pixel stroke.
///
/// Nearly every line in this UI is one pixel, so this both shortens the call
/// sites and pins the literal's type — `Stroke::new` takes `impl Into<f32>`,
/// which leaves a bare `1.0` ambiguous enough to warn.
pub fn hair(colour: Color32) -> Stroke {
    Stroke::new(1.0_f32, colour)
}

/// A vertical rule that occupies layout space.
///
/// Painting a line at `ui.cursor()` mid-row does not work — the cursor is the
/// next widget's origin, and nothing reserves the column, so the following
/// widget lands on top of it. Allocating a one-pixel widget and painting
/// inside its own rect is the version that survives a reflow.
pub fn vrule(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .vline(rect.center().x, rect.y_range(), hair(LINE));
    }
}

/// A card: flat fill, one-pixel border, no rounding.
pub fn card(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, PANEL);
    painter.rect_stroke(rect, 0.0, hair(LINE), StrokeKind::Inside);
}

/// The 8px L-brackets that mark the primary panel on each screen.
pub fn corner_ticks(painter: &egui::Painter, rect: Rect, colour: Color32) {
    let s = 8.0;
    let w = Stroke::new(2.0_f32, colour);
    // Top-left.
    painter.hline(rect.left()..=rect.left() + s, rect.top() + 1.0, w);
    painter.vline(rect.left() + 1.0, rect.top()..=rect.top() + s, w);
    // Bottom-right.
    painter.hline(rect.right() - s..=rect.right(), rect.bottom() - 1.0, w);
    painter.vline(rect.right() - 1.0, rect.bottom() - s..=rect.bottom(), w);
}

/// The faint CRT raster over content areas.
///
/// One-pixel lines every three, at ~2% alpha. Subtle enough that it reads as
/// texture rather than banding, and it costs one mesh.
pub fn scanlines(painter: &egui::Painter, rect: Rect) {
    let tint = Color32::from_rgba_unmultiplied(0x3F, 0xE8, 0x6C, 5);
    let mut y = rect.top().ceil();
    while y < rect.bottom() {
        painter.hline(rect.left()..=rect.right(), y, hair(tint));
        y += 3.0;
    }
}

/// Diagonal hazard stripes, used to head a panel describing something the
/// machine cannot do.
pub fn hazard(painter: &egui::Painter, rect: Rect, colour: Color32) {
    let pitch = 10.0;
    let mut x = rect.left() - rect.height();
    while x < rect.right() {
        let a = Pos2::new(x, rect.bottom());
        let b = Pos2::new(x + rect.height(), rect.top());
        painter.line_segment([a, b], Stroke::new(4.0_f32, colour.gamma_multiply(0.5)));
        x += pitch;
    }
}

/// Polygon for a chip with the bottom-right corner cut away.
fn notched(rect: Rect, cut: f32) -> Vec<Pos2> {
    vec![
        rect.left_top(),
        rect.right_top(),
        Pos2::new(rect.right(), rect.bottom() - cut),
        Pos2::new(rect.right() - cut, rect.bottom()),
        rect.left_bottom(),
    ]
}

/// How a [`chip`] is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChipStyle {
    /// The option currently in force.
    Active,
    /// Selectable, not selected.
    Idle,
    /// Selectable and outlined — "this is the one you are editing".
    Outline,
    /// Selectable but consequential.
    Warn,
    /// Not selectable at all.
    Disabled,
}

impl ChipStyle {
    fn colours(self) -> (Color32, Color32, Color32) {
        // (fill, border, text)
        match self {
            ChipStyle::Active => (GREEN, GREEN, BG),
            ChipStyle::Idle => (RAISED, LINE, TEXT),
            ChipStyle::Outline => (RAISED, GREEN, BRIGHT),
            ChipStyle::Warn => (AMBER_BG, AMBER, AMBER),
            ChipStyle::Disabled => (PANEL, LINE, DIM),
        }
    }
}

/// A notched chip button — the design's primary control.
pub fn chip(ui: &mut egui::Ui, label: &str, style: ChipStyle, size: f32) -> egui::Response {
    let font = mono(size);
    let tracking = 1.0;
    let w = tracked_width(ui.ctx(), label, &font, tracking) + 32.0;
    let h = size + 18.0;

    let sense = if style == ChipStyle::Disabled {
        egui::Sense::hover()
    } else {
        egui::Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), sense);
    if !ui.is_rect_visible(rect) {
        return resp;
    }

    let (fill, border, text) = style.colours();
    // Hover brightens the border only. Moving the fill would make the active
    // chip and a hovered idle chip look alike, which is the one distinction
    // this row exists to make.
    let border = if resp.hovered() && style != ChipStyle::Disabled {
        BRIGHT
    } else {
        border
    };

    let p = ui.painter();
    p.add(egui::Shape::convex_polygon(
        notched(rect, 8.0),
        fill,
        hair(border),
    ));
    let cx = rect.left() + (w - tracked_width(ui.ctx(), label, &font, tracking)) / 2.0;
    tracked(
        ui.ctx(),
        ui.painter(),
        Pos2::new(cx, rect.center().y),
        label,
        font,
        text,
        tracking,
    );
    resp
}

/// A square-cornered chip, used for the effect grid where seven of them sit in
/// a block and the notches would read as noise.
///
/// `marked` appends the design's ◇ — "this effect uses the firmware's own
/// palette". The chip reserves room for it rather than having the caller paint
/// one over the label, which is what collided with `NEON` and `RIPPLE`.
pub fn tag(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    enabled: bool,
    marked: bool,
) -> egui::Response {
    let font = mono(11.0);
    let tw = tracked_width(ui.ctx(), label, &font, 0.5);
    let mark_w = if marked { 15.0 } else { 0.0 };
    let w = tw + mark_w + 26.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(w, 29.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let (fill, border, text) = match (selected, enabled) {
        (true, _) => (RAISED, GREEN, BRIGHT),
        (false, true) => (PANEL, LINE, TEXT),
        (false, false) => (PANEL, LINE, DIM),
    };
    let border = if resp.hovered() && enabled {
        BRIGHT
    } else {
        border
    };
    let p = ui.painter();
    p.rect_filled(rect, 0.0, fill);
    p.rect_stroke(rect, 0.0, hair(border), StrokeKind::Inside);
    let cx = rect.left() + (w - tw - mark_w) / 2.0;
    tracked(
        ui.ctx(),
        ui.painter(),
        Pos2::new(cx, rect.center().y),
        label,
        font,
        text,
        0.5,
    );
    if marked {
        crate::glyph::draw(
            ui.painter(),
            Pos2::new(cx + tw + 8.0, rect.center().y),
            crate::glyph::Mark::Diamond,
            7.0,
            text,
        );
    }
    resp
}

/// A section heading: tracked caps, an optional part code, then a rule out to
/// the right edge.
pub fn section(ui: &mut egui::Ui, title: &str, meta: Option<&str>) {
    let font = sans_b(11.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 14.0), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let y = rect.center().y;
    let mut x = rect.left();
    x += tracked(
        ui.ctx(),
        ui.painter(),
        Pos2::new(x, y),
        title,
        font,
        MUTED,
        3.0,
    ) + 12.0;

    if let Some(m) = meta {
        let mf = mono(9.0);
        let mw = tracked_width(ui.ctx(), m, &mf, 0.0);
        ui.painter()
            .text(Pos2::new(x, y), egui::Align2::LEFT_CENTER, m, mf, DIM);
        x += mw + 10.0;
    }
    if x < rect.right() {
        ui.painter().hline(x..=rect.right(), y, hair(LINE));
    }
}

/// A horizontal 4px track with a square handle. Returns the response so the
/// caller can act on release rather than on every pixel of drag.
///
/// `enabled = false` draws the design's dotted, dashed-handle variant: the
/// control stays visible and in place, with the explanation next to it,
/// instead of vanishing and leaving the panel to reflow.
pub fn slider(
    ui: &mut egui::Ui,
    value: &mut u8,
    range: std::ops::RangeInclusive<u8>,
    width: f32,
    enabled: bool,
) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(
        egui::vec2(width, 16.0),
        if enabled {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::hover()
        },
    );
    let (lo, hi) = (*range.start() as f32, *range.end() as f32);

    if enabled {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let want = (lo + t * (hi - lo)).round() as u8;
            if want != *value {
                *value = want;
                resp.mark_changed();
            }
        }
    }
    if !ui.is_rect_visible(rect) {
        return resp;
    }

    let y = rect.center().y;
    let p = ui.painter();
    let t = ((*value as f32 - lo) / (hi - lo)).clamp(0.0, 1.0);
    let hx = rect.left() + t * rect.width();

    if enabled {
        p.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.left(), y - 2.0),
                egui::vec2(rect.width(), 4.0),
            ),
            0.0,
            LINE,
        );
        p.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.left(), y - 2.0),
                egui::vec2(hx - rect.left(), 4.0),
            ),
            0.0,
            GREEN,
        );
        let h = Rect::from_center_size(Pos2::new(hx, y), egui::vec2(14.0, 14.0));
        p.rect_filled(h, 0.0, RAISED);
        p.rect_stroke(h, 0.0, hair(BRIGHT), StrokeKind::Inside);
    } else {
        // Dotted track: 6 on, 4 off.
        let mut x = rect.left();
        while x < rect.right() {
            let end = (x + 6.0).min(rect.right());
            p.rect_filled(
                Rect::from_min_max(Pos2::new(x, y - 2.0), Pos2::new(end, y + 2.0)),
                0.0,
                LINE,
            );
            x += 10.0;
        }
        let h = Rect::from_center_size(Pos2::new(rect.left() + 7.0, y), egui::vec2(14.0, 14.0));
        p.rect_filled(h, 0.0, PANEL);
        dashed_rect(p, h, DIM);
    }
    resp
}

/// A rectangle outlined with a dashed stroke — the design's "inert control"
/// and "unverified" cue.
pub fn dashed_rect(painter: &egui::Painter, rect: Rect, colour: Color32) {
    let s = hair(colour);
    let dash = 3.0;
    let gap = 3.0;
    let mut x = rect.left();
    while x < rect.right() {
        let e = (x + dash).min(rect.right());
        painter.hline(x..=e, rect.top(), s);
        painter.hline(x..=e, rect.bottom(), s);
        x += dash + gap;
    }
    let mut y = rect.top();
    while y < rect.bottom() {
        let e = (y + dash).min(rect.bottom());
        painter.vline(rect.left(), y..=e, s);
        painter.vline(rect.right(), y..=e, s);
        y += dash + gap;
    }
}

/// A small outlined badge: `✕ UNSUPPORTED`, `? ACCEPTED, UNVERIFIED`.
pub fn badge(
    ui: &mut egui::Ui,
    mark: crate::glyph::Mark,
    label: &str,
    colour: Color32,
    dashed: bool,
) {
    let font = mono_b(10.0);
    let w = tracked_width(ui.ctx(), label, &font, 0.0) + 30.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    if dashed {
        dashed_rect(p, rect, colour);
    } else {
        p.rect_stroke(rect, 0.0, hair(colour), StrokeKind::Inside);
    }
    crate::glyph::draw(
        p,
        Pos2::new(rect.left() + 12.0, rect.center().y),
        mark,
        8.0,
        colour,
    );
    p.text(
        Pos2::new(rect.left() + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        colour,
    );
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;

    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = RAISED;

    // Hard corners throughout. This is an instrument panel.
    let r = CornerRadius::ZERO;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }

    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = hair(LINE);
    v.widgets.noninteractive.fg_stroke = hair(TEXT);

    v.widgets.inactive.bg_fill = RAISED;
    v.widgets.inactive.weak_bg_fill = RAISED;
    v.widgets.inactive.bg_stroke = hair(LINE);
    v.widgets.inactive.fg_stroke = hair(TEXT);

    v.widgets.hovered.bg_fill = RAISED;
    v.widgets.hovered.weak_bg_fill = RAISED;
    v.widgets.hovered.bg_stroke = hair(GREEN);
    v.widgets.hovered.fg_stroke = hair(BRIGHT);

    v.widgets.active.bg_fill = RAISED;
    v.widgets.active.weak_bg_fill = RAISED;
    v.widgets.active.bg_stroke = hair(BRIGHT);
    v.widgets.active.fg_stroke = hair(BRIGHT);

    v.selection.bg_fill = RAISED;
    v.selection.stroke = hair(GREEN);

    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(13.0, 8.0);
    style.spacing.interact_size.y = 22.0;

    ctx.set_style(style);
}
