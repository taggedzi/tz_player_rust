//! Deterministic matrix-rain visualizers (green / blue / red themes).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};

const GLYPHS: &[u8] = b"0123456789ABCDEF$#@%&*+=-";

#[derive(Debug, Clone, Copy)]
enum MatrixTheme {
    Green,
    Blue,
    Red,
}

fn render_matrix(
    theme: MatrixTheme,
    color: bool,
    frame: &VisualizerFrameInput,
) -> Vec<Line<'static>> {
    let width = frame.width.max(1) as usize;
    let height = frame.height.max(1) as usize;
    let t = if matches!(frame.status.as_str(), "playing") {
        frame.position_s * frame.speed.max(0.1) + (frame.frame_index as f64) * 0.05
    } else {
        frame.frame_index as f64 * 0.03
    };
    let energy = frame
        .level_left
        .zip(frame.level_right)
        .map(|(l, r)| ((l + r) * 0.5).clamp(0.0, 1.0))
        .unwrap_or(0.25);
    let base_rows_per_second = 7.5 + f64::from(energy) * 6.0;

    let head_color = match theme {
        MatrixTheme::Green => Color::Rgb(210, 255, 220),
        MatrixTheme::Blue => Color::Rgb(200, 230, 255),
        MatrixTheme::Red => Color::Rgb(255, 210, 210),
    };

    let mut lines = Vec::with_capacity(height);
    for y in 0..height {
        let mut spans = Vec::with_capacity(width);
        for x in 0..width {
            let speed_scale = 0.75 + ((x * 11) % 4) as f64 * 0.2;
            let period = (height + 14 + (x % 7)) as f64;
            let head = (t * base_rows_per_second * speed_scale + (x * 17) as f64) % period - 7.0;
            let trail = 5.0 + (x % 6) as f64;
            let distance = head - y as f64;
            if distance < 0.0 || distance >= trail {
                spans.push(Span::raw(" "));
                continue;
            }
            let gi = (x * 13 + y * 7 + frame.frame_index as usize) % GLYPHS.len();
            let glyph = GLYPHS[gi] as char;
            let (ch, style) = if !color {
                (glyph, Style::default().fg(Color::Gray))
            } else if distance < 1.0 {
                (
                    glyph,
                    Style::default().fg(head_color).add_modifier(Modifier::BOLD),
                )
            } else {
                let fade = (distance / trail).clamp(0.0, 1.0) as f32;
                let g = (255.0 * (0.85 - fade * 0.75)).round() as u8;
                let trail_color = match theme {
                    MatrixTheme::Green => Color::Rgb(0, g.max(40), (g / 3).max(20)),
                    MatrixTheme::Blue => Color::Rgb((g / 4).max(20), (g / 2).max(40), g.max(50)),
                    MatrixTheme::Red => Color::Rgb(g.max(50), (g / 4).max(15), (g / 5).max(15)),
                };
                let ch = if fade > 0.55 {
                    '.'
                } else if glyph.is_ascii_alphanumeric() && fade > 0.25 {
                    glyph.to_ascii_lowercase()
                } else {
                    glyph
                };
                (ch, Style::default().fg(trail_color))
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

#[derive(Debug, Default)]
pub struct MatrixGreenVisualizer {
    color: bool,
}

impl VisualizerPlugin for MatrixGreenVisualizer {
    fn plugin_id(&self) -> &'static str {
        "matrix.green"
    }
    fn display_name(&self) -> &'static str {
        "Matrix Rain (Green)"
    }
    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }
    fn on_deactivate(&mut self) {}
    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        render_matrix(MatrixTheme::Green, self.color, frame)
    }
}

#[derive(Debug, Default)]
pub struct MatrixBlueVisualizer {
    color: bool,
}

impl VisualizerPlugin for MatrixBlueVisualizer {
    fn plugin_id(&self) -> &'static str {
        "matrix.blue"
    }
    fn display_name(&self) -> &'static str {
        "Matrix Rain (Blue)"
    }
    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }
    fn on_deactivate(&mut self) {}
    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        render_matrix(MatrixTheme::Blue, self.color, frame)
    }
}

#[derive(Debug, Default)]
pub struct MatrixRedVisualizer {
    color: bool,
}

impl VisualizerPlugin for MatrixRedVisualizer {
    fn plugin_id(&self) -> &'static str {
        "matrix.red"
    }
    fn display_name(&self) -> &'static str {
        "Matrix Rain (Red)"
    }
    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }
    fn on_deactivate(&mut self) {}
    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        render_matrix(MatrixTheme::Red, self.color, frame)
    }
}
