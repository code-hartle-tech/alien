//! The Alien look.
//!
//! # Where the palette comes from, and where it stops
//!
//! Decompiling PredatorSense 3.00.3152 for interoperability gives us the facts
//! that make a familiar UI possible: it is a dark WPF app whose primary accent
//! is `#00AEC7` with `#00DFFF` for highlights, and whose tabs are Dashboard,
//! Lighting, Overclocking, Fan Control and Monitoring.
//!
//! Those are facts about an interface, and matching them is what makes this
//! feel like the tool users already know. What we do **not** do is ship Acer's
//! artwork: no extracted icons, no bitmaps, no logo. Every shape here is drawn
//! with primitives. That line matters because this repo is published, and
//! "we reimplemented the layout" and "we copied their PNGs" are different
//! things legally as well as ethically.

use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub const BG: Color32 = Color32::from_rgb(0x0A, 0x0D, 0x10);
pub const PANEL: Color32 = Color32::from_rgb(0x11, 0x16, 0x1B);
pub const PANEL_HI: Color32 = Color32::from_rgb(0x18, 0x1F, 0x26);
pub const LINE: Color32 = Color32::from_rgb(0x22, 0x2B, 0x33);

/// PredatorSense's primary.
pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0xAE, 0xC7);
/// Its brighter highlight, used for the active element only.
pub const HILITE: Color32 = Color32::from_rgb(0x00, 0xDF, 0xFF);

pub const TEXT: Color32 = Color32::from_rgb(0xD6, 0xDF, 0xE6);
pub const MUTED: Color32 = Color32::from_rgb(0x6C, 0x7A, 0x85);
pub const OK: Color32 = Color32::from_rgb(0x5A, 0xD8, 0x9A);
pub const WARN: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x00);
pub const HOT: Color32 = Color32::from_rgb(0xFF, 0x4D, 0x3A);

/// Temperature → colour, shared with the TUI so both frontends agree on what
/// "hot" looks like.
pub fn temp_colour(c: u16) -> Color32 {
    match c {
        0..=69 => OK,
        70..=84 => WARN,
        _ => HOT,
    }
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;

    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = PANEL_HI;

    // Square-ish corners. The vendor UI is angular, and rounded rectangles
    // read as a generic settings app rather than a gaming utility.
    let r = CornerRadius::same(2);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;

    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);

    v.widgets.inactive.bg_fill = PANEL_HI;
    v.widgets.inactive.weak_bg_fill = PANEL_HI;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);

    v.widgets.hovered.bg_fill = Color32::from_rgb(0x1E, 0x28, 0x30);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x1E, 0x28, 0x30);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, HILITE);

    v.widgets.active.bg_fill = Color32::from_rgb(0x0B, 0x35, 0x3D);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x0B, 0x35, 0x3D);
    v.widgets.active.bg_stroke = Stroke::new(1.0, HILITE);
    v.widgets.active.fg_stroke = Stroke::new(1.0, HILITE);

    v.selection.bg_fill = Color32::from_rgb(0x0B, 0x35, 0x3D);
    v.selection.stroke = Stroke::new(1.0, HILITE);

    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);

    ctx.set_style(style);
}

/// Draw a panel with a clipped top-left corner.
///
/// The angular corner is the single strongest cue of the vendor's visual
/// language, and it costs one polygon rather than an image asset.
pub fn chamfered_panel(ui: &mut egui::Ui, rect: egui::Rect, fill: Color32, stroke: Color32) {
    let c = 12.0_f32.min(rect.width() * 0.15).min(rect.height() * 0.4);
    let pts = vec![
        egui::pos2(rect.left() + c, rect.top()),
        egui::pos2(rect.right(), rect.top()),
        egui::pos2(rect.right(), rect.bottom()),
        egui::pos2(rect.left(), rect.bottom()),
        egui::pos2(rect.left(), rect.top() + c),
    ];
    ui.painter().add(egui::Shape::convex_polygon(
        pts.clone(),
        fill,
        Stroke::new(1.0, stroke),
    ));
}

/// A section heading: small, spaced, muted — the vendor's own idiom.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    let spaced: String = text.to_uppercase().chars().flat_map(|c| [c, ' ']).collect();
    ui.label(
        egui::RichText::new(spaced.trim_end())
            .color(MUTED)
            .size(11.0)
            .strong(),
    );
}
