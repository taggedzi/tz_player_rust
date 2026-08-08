//! Radial spectrum spokes around a fixed panel-centered core.

use ratatui::style::Color;
use ratatui::text::Line;

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};
use super::util::{
    band_level, beat_onset, draw_ring, energy_color, field_center, field_center_i, fit_lines,
    max_radius_for_field, polar_xy, Canvas,
};

#[derive(Debug, Default)]
pub struct RadialSpectrumVisualizer {
    color: bool,
}

impl VisualizerPlugin for RadialSpectrumVisualizer {
    fn plugin_id(&self) -> &'static str {
        "viz.spectrum.radial"
    }

    fn display_name(&self) -> &'static str {
        "Radial Spectrum"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;
        let mut canvas = Canvas::new(width, height);
        // Fixed geometric center + fixed outer envelope (spokes grow inward→out, never shift origin).
        let (cx, cy) = field_center(width, height);
        let (core_x, core_y) = field_center_i(width, height);
        let max_r = max_radius_for_field(width, height);
        let base_r = (max_r * 0.22).max(1.0);
        let bands = frame.spectrum_bands.as_deref();
        let spoke_count = bands.map(|b| b.len().clamp(16, 64)).unwrap_or(32).max(16);
        let spin = (frame.frame_index % 360) as f32 * std::f32::consts::PI / 720.0;
        let onset = beat_onset(frame);

        for i in 0..spoke_count {
            let level = band_level(bands, i, spoke_count);
            // Length only; origin and max envelope stay fixed.
            let len = base_r + (max_r - base_r) * level;
            let angle = std::f32::consts::TAU * (i as f32) / (spoke_count as f32) + spin;
            let steps = len.round().max(1.0) as i32;
            for step in 1..=steps {
                let (x, y) = polar_xy(cx, cy, angle, step as f32);
                let ch = if step == steps {
                    if onset {
                        '*'
                    } else {
                        '+'
                    }
                } else if step as f32 > len * 0.7 {
                    ':'
                } else {
                    '.'
                };
                canvas.set(x, y, ch, energy_color(level, self.color));
            }
        }

        if onset {
            draw_ring(
                &mut canvas,
                cx,
                cy,
                base_r + 1.0,
                'o',
                if self.color {
                    Color::Yellow
                } else {
                    Color::White
                },
            );
        }
        let core = if onset { '@' } else { 'O' };
        canvas.set(
            core_x,
            core_y,
            core,
            if self.color {
                Color::White
            } else {
                Color::Gray
            },
        );

        fit_lines(canvas.into_lines(), height)
    }
}
