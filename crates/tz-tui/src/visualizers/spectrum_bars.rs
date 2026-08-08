//! Vertical bar spectrum visualizer (colored).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::host::{heat_color, VisualizerContext, VisualizerFrameInput, VisualizerPlugin};

#[derive(Debug, Default)]
pub struct SpectrumBarsVisualizer {
    smooth: Vec<f32>,
    color: bool,
}

impl VisualizerPlugin for SpectrumBarsVisualizer {
    fn plugin_id(&self) -> &'static str {
        "spectrum.bars"
    }

    fn display_name(&self) -> &'static str {
        "Spectrum Bars"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
        self.smooth.clear();
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(8) as usize;
        let height = frame.height.max(4) as usize;
        let source = frame.spectrum_source.as_deref().unwrap_or("none");

        let levels = bands_to_levels(frame.spectrum_bands.as_deref(), width.saturating_sub(2));
        if self.smooth.len() != levels.len() {
            self.smooth = levels.clone();
        } else {
            for (i, target) in levels.iter().enumerate() {
                let cur = self.smooth[i];
                let alpha = if *target > cur { 0.55 } else { 0.28 };
                self.smooth[i] = (cur + (target - cur) * alpha).clamp(0.0, 1.0);
            }
        }

        let plot_h = height.saturating_sub(2).max(2);
        let plot_w = self.smooth.len().max(1);
        let mut lines = vec![Line::from(vec![
            Span::styled(
                "SPECTRUM ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("[{source}]"), Style::default().fg(Color::Yellow)),
        ])];

        for row in 0..plot_h {
            let threshold = 1.0 - ((row + 1) as f32 / (plot_h as f32 + 0.25));
            let mut spans = vec![Span::styled("|", Style::default().fg(Color::DarkGray))];
            for &v in &self.smooth {
                let (ch, fg) = if v >= threshold {
                    (bar_char(v), heat_color(v, self.color))
                } else if row + 1 == plot_h {
                    ('_', Color::DarkGray)
                } else {
                    (' ', Color::Reset)
                };
                spans.push(Span::styled(ch.to_string(), Style::default().fg(fg)));
            }
            spans.push(Span::styled("|", Style::default().fg(Color::DarkGray)));
            // pad if needed
            let _ = plot_w;
            lines.push(Line::from(spans));
        }

        lines.push(Line::from(Span::styled(
            format!(
                "{}  {:.1}s  bands={}",
                frame.status,
                frame.position_s,
                frame.spectrum_bands.as_ref().map(|b| b.len()).unwrap_or(0)
            ),
            Style::default().fg(Color::Gray),
        )));
        lines.truncate(height.max(1));
        lines
    }
}

fn bands_to_levels(bands: Option<&[u8]>, target_cols: usize) -> Vec<f32> {
    let target_cols = target_cols.clamp(8, 64);
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return (0..target_cols)
            .map(|i| {
                let t = i as f32 * 0.35;
                (0.08 + 0.12 * (0.5 + 0.5 * t.sin())).clamp(0.0, 1.0)
            })
            .collect();
    };
    let mut out = Vec::with_capacity(target_cols);
    for col in 0..target_cols {
        let start = (col * bands.len()) / target_cols;
        let end = (((col + 1) * bands.len()) / target_cols).max(start + 1);
        let slice = &bands[start..end.min(bands.len())];
        let avg = if slice.is_empty() {
            0.0
        } else {
            let sum: u32 = slice.iter().map(|b| u32::from(*b)).sum();
            (sum as f32 / slice.len() as f32) / 255.0
        };
        out.push(avg.sqrt().clamp(0.0, 1.0));
    }
    out
}

fn bar_char(level: f32) -> char {
    const CHARS: [char; 6] = ['.', ':', '-', '=', '#', '#'];
    let idx = ((level * (CHARS.len() - 1) as f32).round() as usize).min(CHARS.len() - 1);
    CHARS[idx]
}
