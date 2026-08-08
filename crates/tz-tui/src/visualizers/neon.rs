//! Neon ribbon waveform visualizer — vertically centered, fixed midline.

use ratatui::style::Color;
use ratatui::text::Line;

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};
use super::util::{clamp01, fit_lines, mono_level, Canvas};

#[derive(Debug, Default)]
pub struct WaveformNeonVisualizer {
    color: bool,
}

impl VisualizerPlugin for WaveformNeonVisualizer {
    fn plugin_id(&self) -> &'static str {
        "viz.waveform.neon"
    }

    fn display_name(&self) -> &'static str {
        "Waveform Neon"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        // Full panel; geometric zero-line at vertical midpoint (no header offset).
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;
        let (l_amp, r_amp) = resolve_amps(frame);
        // Soft floor so quiet tracks still show a thin ribbon.
        let l_amp = l_amp.max(0.04);
        let r_amp = r_amp.max(0.04);
        let mono = mono_level(frame);
        let cy = (height.saturating_sub(1) as f32) * 0.5;
        // Map amplitude so full-scale signal reaches ~90% of half-height.
        let half = (height.saturating_sub(1) as f32 * 0.5).max(1.0);
        let scale = half * 0.9;

        let mut canvas = Canvas::new(width, height);
        // Steady midline guide
        for x in 0..width {
            if x % 4 == 0 {
                canvas.set(x as i32, cy.round() as i32, '·', Color::DarkGray);
            }
        }

        for x in 0..width {
            let phase = frame.frame_index as f32 * 0.23 + x as f32 * 0.21;
            // Always centered on 0 — amplitude only; no DC offset bounce.
            let left = (phase.sin() * l_amp).clamp(-1.0, 1.0);
            let right = ((phase * 0.92 + 0.7).cos() * r_amp).clamp(-1.0, 1.0);
            let ly = (cy - left * scale).round() as i32;
            let ry = (cy - right * scale).round() as i32;

            let lc = if self.color {
                Color::Rgb(80, 220, 255)
            } else {
                Color::White
            };
            let rc = if self.color {
                Color::Rgb(255, 90, 220)
            } else {
                Color::Gray
            };

            // Soft glow trails
            canvas.set_if_empty(x as i32, ly - 1, '·', Color::DarkGray);
            canvas.set_if_empty(x as i32, ly + 1, '·', Color::DarkGray);
            canvas.set_if_empty(x as i32, ry - 1, '·', Color::DarkGray);
            canvas.set_if_empty(x as i32, ry + 1, '·', Color::DarkGray);

            if ly == ry {
                canvas.set(
                    x as i32,
                    ly,
                    if mono > 0.7 { '◆' } else { '●' },
                    if self.color {
                        Color::Yellow
                    } else {
                        Color::White
                    },
                );
            } else {
                canvas.set(x as i32, ly, '●', lc);
                canvas.set(x as i32, ry, '■', rc);
            }
        }

        fit_lines(canvas.into_lines(), height)
    }
}

/// Peak-to-zero amplitude per channel (never shifts the zero line).
fn resolve_amps(frame: &VisualizerFrameInput) -> (f32, f32) {
    if let (Some(a), Some(b), Some(c), Some(d)) = (
        frame.waveform_min_left,
        frame.waveform_max_left,
        frame.waveform_min_right,
        frame.waveform_max_right,
    ) {
        let l = ((b - a) * 0.5).abs().clamp(0.0, 1.0);
        let r = ((d - c) * 0.5).abs().clamp(0.0, 1.0);
        return (l, r);
    }
    let left = clamp01(frame.level_left.unwrap_or(0.0));
    let right = clamp01(frame.level_right.unwrap_or(0.0));
    (left, right)
}
