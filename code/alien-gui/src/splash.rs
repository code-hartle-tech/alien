//! Startup splash timing and rendering.
//!
//! The splash is deliberately coupled to the first daemon connection result:
//! it is not a decorative sleep that hides an unresponsive startup. The
//! poller owns every socket/firmware call; this module only observes whether
//! that background attempt has completed and paints its current state.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Stroke, TextureId};

use crate::theme;

const MIN_VISIBLE: Duration = Duration::from_millis(850);
const RESULT_HOLD: Duration = Duration::from_millis(420);
const FADE: Duration = Duration::from_millis(280);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Contacting,
    Connected,
    Unreachable,
    LinkLost,
}

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    elapsed: Duration,
    opacity: f32,
    pub reduced_motion: bool,
}

/// A one-shot startup timeline. `completed_after` records the first observed
/// daemon result relative to `started`, which keeps all decisions deterministic
/// even if egui happens to skip a frame.
pub struct Splash {
    started: Instant,
    completed_after: Option<Duration>,
    reduced_motion: bool,
}

impl Splash {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            started: Instant::now(),
            completed_after: None,
            reduced_motion,
        }
    }

    /// Return the current presentation, or `None` once normal UI may take over.
    pub fn frame(&mut self, now: Instant, daemon_attempt_finished: bool) -> Option<Frame> {
        let elapsed = now.saturating_duration_since(self.started);
        if daemon_attempt_finished && self.completed_after.is_none() {
            self.completed_after = Some(elapsed);
        }
        opacity_at(elapsed, self.completed_after, self.reduced_motion).map(|opacity| Frame {
            elapsed,
            opacity,
            reduced_motion: self.reduced_motion,
        })
    }
}

/// Pure timeline used by both the runtime and tests.
fn opacity_at(
    elapsed: Duration,
    completed_after: Option<Duration>,
    reduced_motion: bool,
) -> Option<f32> {
    let Some(completed_after) = completed_after else {
        // A slow or wedged connection attempt is still active. Keep showing
        // the honest contacting state until the poller reports a result.
        return Some(1.0);
    };
    let fade_at = MIN_VISIBLE.max(completed_after.saturating_add(RESULT_HOLD));
    if elapsed < fade_at {
        return Some(1.0);
    }
    if reduced_motion {
        return None;
    }

    let fade_elapsed = elapsed.saturating_sub(fade_at);
    if fade_elapsed >= FADE {
        None
    } else {
        Some(1.0 - fade_elapsed.as_secs_f32() / FADE.as_secs_f32())
    }
}

pub fn show(ctx: &egui::Context, logo: TextureId, frame: Frame, outcome: Outcome) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(theme::BG))
        .show(ctx, |ui| {
            let full = ui.max_rect();
            let p = ui.painter();
            theme::scanlines(p, full);

            let compact = full.width() < 720.0 || full.height() < 560.0;
            let logo_size = (full.width() * 0.24)
                .min(full.height() * 0.34)
                .clamp(126.0, 208.0);
            let centre = Pos2::new(
                full.center().x,
                full.center().y - if compact { 48.0 } else { 58.0 },
            );
            let alpha = frame.opacity.clamp(0.0, 1.0);
            let seconds = frame.elapsed.as_secs_f32();

            // A quiet phosphor halo and two instrument rings frame the mark.
            // Reduced motion keeps their composition but removes every phase
            // change, including the logo's two-percent breathing scale.
            let breath = if frame.reduced_motion {
                1.0
            } else {
                1.0 + (seconds * 2.8).sin() * 0.018
            };
            p.circle_filled(
                centre,
                logo_size * 0.58,
                fade(Color32::from_rgba_unmultiplied(0x3F, 0xE8, 0x6C, 8), alpha),
            );
            p.circle_stroke(
                centre,
                logo_size * 0.66,
                Stroke::new(1.0_f32, fade(theme::LINE, alpha)),
            );
            p.circle_stroke(
                centre,
                logo_size * 0.74,
                Stroke::new(1.0_f32, fade(theme::DIM, alpha * 0.45)),
            );

            let orbit_phase = if frame.reduced_motion {
                -0.72
            } else {
                seconds * 1.15 - 0.72
            };
            p.add(egui::Shape::line(
                arc(centre, logo_size * 0.74, orbit_phase, 1.12, 28),
                Stroke::new(2.0_f32, fade(theme::GREEN, alpha)),
            ));
            p.add(egui::Shape::line(
                arc(
                    centre,
                    logo_size * 0.66,
                    -orbit_phase * 0.58 + 2.3,
                    0.62,
                    18,
                ),
                Stroke::new(1.0_f32, fade(theme::BRIGHT, alpha * 0.72)),
            ));

            let node = point_on_circle(centre, logo_size * 0.74, orbit_phase + 1.12);
            p.circle_filled(node, 3.0, fade(theme::BRIGHT, alpha));

            let image_rect = egui::Rect::from_center_size(
                centre,
                egui::vec2(logo_size * breath, logo_size * breath),
            );
            p.image(
                logo,
                image_rect,
                egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                fade(Color32::WHITE, alpha),
            );

            if !frame.reduced_motion {
                let scan_t = (seconds * 0.42).fract();
                let scan_y = centre.y - logo_size * 0.48 + scan_t * logo_size * 0.96;
                p.line_segment(
                    [
                        Pos2::new(centre.x - logo_size * 0.48, scan_y),
                        Pos2::new(centre.x + logo_size * 0.48, scan_y),
                    ],
                    Stroke::new(1.0_f32, fade(theme::BRIGHT, alpha * 0.18)),
                );
            }

            let word_y = centre.y + logo_size * 0.88;
            let title_font = theme::sans_b(if compact { 18.0 } else { 21.0 });
            let title_tracking = if compact { 7.0 } else { 9.0 };
            let title_w = theme::tracked_width(ctx, "ALIEN", &title_font, title_tracking);
            theme::tracked(
                ctx,
                p,
                Pos2::new(centre.x - title_w / 2.0, word_y),
                "ALIEN",
                title_font,
                fade(theme::BRIGHT, alpha),
                title_tracking,
            );

            let (status, detail, status_colour) = outcome.copy();
            let status_font = theme::mono_b(if compact { 10.0 } else { 11.0 });
            let status_tracking = 1.7;
            let status_w = theme::tracked_width(ctx, status, &status_font, status_tracking);
            theme::tracked(
                ctx,
                p,
                Pos2::new(centre.x - status_w / 2.0, word_y + 37.0),
                status,
                status_font,
                fade(status_colour, alpha),
                status_tracking,
            );
            p.text(
                Pos2::new(centre.x, word_y + 57.0),
                egui::Align2::CENTER_CENTER,
                detail,
                theme::mono(if compact { 9.0 } else { 10.0 }),
                fade(theme::MUTED, alpha),
            );

            progress(
                p,
                Pos2::new(centre.x, word_y + 82.0),
                if compact { 220.0 } else { 286.0 },
                frame,
                outcome,
                status_colour,
            );

            p.text(
                Pos2::new(full.center().x, full.bottom() - 24.0),
                egui::Align2::CENTER_CENTER,
                format!("CONTROL SURFACE // {}", env!("CARGO_PKG_VERSION")),
                theme::mono(8.0),
                fade(theme::DIM, alpha),
            );
        });
}

impl Outcome {
    fn copy(self) -> (&'static str, &'static str, Color32) {
        match self {
            Self::Contacting => (
                "CONTACTING ALIEN-DAEMON",
                "opening privileged control link",
                theme::GREEN,
            ),
            Self::Connected => (
                "CONTROL LINK ESTABLISHED",
                "loading live capabilities",
                theme::GREEN,
            ),
            Self::Unreachable => ("DAEMON NOT REACHABLE", "opening guided setup", theme::AMBER),
            Self::LinkLost => (
                "CONTROL LINK INTERRUPTED",
                "opening frozen telemetry",
                theme::AMBER,
            ),
        }
    }
}

fn progress(
    p: &egui::Painter,
    centre: Pos2,
    width: f32,
    frame: Frame,
    outcome: Outcome,
    colour: Color32,
) {
    const SEGMENTS: usize = 18;
    let gap = 3.0;
    let segment_w = (width - gap * (SEGMENTS as f32 - 1.0)) / SEGMENTS as f32;
    let left = centre.x - width / 2.0;
    let runner = ((frame.elapsed.as_secs_f32() * 13.0) as usize) % SEGMENTS;

    for i in 0..SEGMENTS {
        let rect = egui::Rect::from_min_size(
            Pos2::new(left + i as f32 * (segment_w + gap), centre.y - 2.0),
            egui::vec2(segment_w, 4.0),
        );
        let strength = match outcome {
            Outcome::Contacting if frame.reduced_motion => {
                if i == 0 || i + 1 == SEGMENTS {
                    0.62
                } else {
                    0.18
                }
            }
            Outcome::Contacting => {
                let behind = (runner + SEGMENTS - i) % SEGMENTS;
                match behind {
                    0 => 1.0,
                    1 => 0.58,
                    2 => 0.3,
                    _ => 0.12,
                }
            }
            _ => 0.78,
        };
        p.rect_filled(rect, 0.0, fade(colour, frame.opacity * strength));
    }
}

fn arc(centre: Pos2, radius: f32, start: f32, sweep: f32, segments: usize) -> Vec<Pos2> {
    (0..=segments)
        .map(|i| {
            let angle = start + sweep * i as f32 / segments as f32;
            point_on_circle(centre, radius, angle)
        })
        .collect()
}

fn point_on_circle(centre: Pos2, radius: f32, angle: f32) -> Pos2 {
    Pos2::new(
        centre.x + angle.cos() * radius,
        centre.y + angle.sin() * radius,
    )
}

fn fade(colour: Color32, opacity: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        colour.r(),
        colour.g(),
        colour.b(),
        (colour.a() as f32 * opacity.clamp(0.0, 1.0)).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_both_minimum_intro_and_daemon_result() {
        assert_eq!(opacity_at(Duration::from_secs(30), None, false), Some(1.0));

        let early = Some(Duration::from_millis(40));
        assert_eq!(
            opacity_at(Duration::from_millis(849), early, false),
            Some(1.0)
        );
        assert_eq!(
            opacity_at(Duration::from_millis(850), early, false),
            Some(1.0)
        );

        let late = Some(Duration::from_secs(5));
        assert_eq!(
            opacity_at(Duration::from_millis(5_419), late, false),
            Some(1.0)
        );
        assert_eq!(
            opacity_at(Duration::from_millis(5_560), late, false),
            Some(0.5)
        );
        assert!(opacity_at(Duration::from_millis(5_700), late, false).is_none());
    }

    #[test]
    fn reduced_motion_keeps_the_result_hold_but_skips_fade() {
        let completed = Some(Duration::from_millis(100));
        assert_eq!(
            opacity_at(Duration::from_millis(849), completed, true),
            Some(1.0)
        );
        assert!(opacity_at(Duration::from_millis(850), completed, true).is_none());
    }

    #[test]
    fn every_outcome_has_truthful_transition_copy() {
        assert!(Outcome::Contacting.copy().0.contains("CONTACTING"));
        assert!(Outcome::Connected.copy().0.contains("ESTABLISHED"));
        assert!(Outcome::Unreachable.copy().1.contains("setup"));
        assert!(Outcome::LinkLost.copy().1.contains("frozen"));
    }
}
