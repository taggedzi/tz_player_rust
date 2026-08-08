//! Spectral landscape / terrain visualizer (colored).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::host::{heat_color, VisualizerContext, VisualizerFrameInput, VisualizerPlugin};

#[derive(Debug, Default)]
pub struct AudioTerrainVisualizer {
    color: bool,
}

impl VisualizerPlugin for AudioTerrainVisualizer {
    fn plugin_id(&self) -> &'static str {
        "viz.spectrum.terrain"
    }

    fn display_name(&self) -> &'static str {
        "Audio Terrain"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;
        let chart_rows = height.saturating_sub(2).max(1);
        let columns = collapse_bands(frame.spectrum_bands.as_deref(), width);
        let peaks = terrain_heights(&columns, chart_rows, frame.beat_is_onset);

        let src = frame.spectrum_source.as_deref().unwrap_or("missing");
        let beat = if frame.beat_is_onset == Some(true) {
            "ONSET"
        } else {
            "idle"
        };

        let mut lines = vec![
            Line::from(Span::styled(
                "AUDIO TERRAIN",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(format!("FFT [{src}]  "), Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("BEAT {beat}"),
                    Style::default().fg(if frame.beat_is_onset == Some(true) {
                        Color::Yellow
                    } else {
                        Color::DarkGray
                    }),
                ),
            ]),
        ];
        lines.extend(render_terrain(&peaks, chart_rows, self.color));
        lines.truncate(height.max(1));
        lines
    }
}

fn collapse_bands(bands: Option<&[u8]>, width: usize) -> Vec<u8> {
    let width = width.max(1);
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return vec![0; width];
    };
    let mut out = Vec::with_capacity(width);
    for idx in 0..width {
        let start = (idx * bands.len()) / width;
        let mut end = ((idx + 1) * bands.len()) / width;
        if end <= start {
            end = (start + 1).min(bands.len());
        }
        let chunk = bands.get(start..end.min(bands.len())).unwrap_or(&[]);
        if chunk.is_empty() {
            out.push(0);
        } else {
            let avg = chunk.iter().map(|b| u32::from(*b)).sum::<u32>() / chunk.len() as u32;
            out.push(avg as u8);
        }
    }
    out
}

fn terrain_heights(values: &[u8], rows: usize, beat_onset: Option<bool>) -> Vec<usize> {
    if values.is_empty() || rows == 0 {
        return Vec::new();
    }
    let beat_lift = if beat_onset == Some(true) && rows > 1 {
        1
    } else {
        0
    };
    values
        .iter()
        .enumerate()
        .map(|(idx, &value)| {
            let left = if idx > 0 { values[idx - 1] } else { value };
            let right = values.get(idx + 1).copied().unwrap_or(value);
            let smoothed = ((u32::from(left) + u32::from(value) * 2 + u32::from(right)) / 4) as f32;
            let height =
                ((smoothed / 255.0) * (rows.saturating_sub(1) as f32)).round() as usize + beat_lift;
            height.min(rows.saturating_sub(1))
        })
        .collect()
}

fn render_terrain(peaks: &[usize], rows: usize, color: bool) -> Vec<Line<'static>> {
    let width = peaks.len();
    let mut lines = Vec::with_capacity(rows);
    for row_idx in 0..rows {
        let threshold = rows.saturating_sub(1).saturating_sub(row_idx);
        let mut spans = Vec::with_capacity(width);
        for (col_idx, &peak) in peaks.iter().enumerate() {
            if peak < threshold {
                spans.push(Span::raw(" "));
                continue;
            }
            let level = if rows > 1 {
                peak as f32 / (rows - 1) as f32
            } else {
                1.0
            };
            let (ch, fg) = if peak == threshold {
                (
                    if level > 0.7 { '^' } else { '/' },
                    heat_color(level, color),
                )
            } else {
                let ch = if (col_idx + peak) % 3 == 0 { '#' } else { '=' };
                (ch, heat_color(level * 0.85, color))
            };
            spans.push(Span::styled(ch.to_string(), Style::default().fg(fg)));
        }
        lines.push(Line::from(spans));
    }
    lines
}
