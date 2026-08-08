//! Rolling spectrogram waterfall driven by cached spectrum bands (colored).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::host::{heat_color, VisualizerContext, VisualizerFrameInput, VisualizerPlugin};

const ASCII_RAMP: &[u8] = b" .:-=+*#%@";

#[derive(Debug, Default)]
pub struct SpectrogramWaterfallVisualizer {
    history: Vec<Vec<u8>>,
    color: bool,
}

impl VisualizerPlugin for SpectrogramWaterfallVisualizer {
    fn plugin_id(&self) -> &'static str {
        "viz.spectrogram.waterfall"
    }

    fn display_name(&self) -> &'static str {
        "Spectrogram Waterfall"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
        self.history.clear();
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;
        let grid_height = height.saturating_sub(2).max(1);
        let grid_width = width.saturating_sub(1).max(1);

        let mut newest = collapse_bands(frame.spectrum_bands.as_deref(), grid_width);
        if frame.beat_is_onset == Some(true) {
            for v in &mut newest {
                *v = v.saturating_add(40);
            }
        }
        self.history.insert(0, newest);
        if self.history.len() > grid_height {
            self.history.truncate(grid_height);
        }

        let src = frame.spectrum_source.as_deref().unwrap_or("missing");
        let beat = if frame.beat_is_onset == Some(true) {
            "ONSET"
        } else {
            "idle"
        };
        let bpm = frame
            .beat_bpm
            .map(|b| format!("{b:.0}"))
            .unwrap_or_else(|| "--".into());
        let bsrc = frame.beat_source.as_deref().unwrap_or("-");

        let mut lines = vec![
            Line::from(Span::styled(
                "SPECTRO WATERFALL",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(format!("FFT [{src}]  "), Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("BEAT {beat} "),
                    Style::default().fg(if frame.beat_is_onset == Some(true) {
                        Color::Yellow
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::styled(
                    format!("bpm={bpm} [{bsrc}]"),
                    Style::default().fg(Color::Gray),
                ),
            ]),
        ];

        for row_idx in 0..grid_height {
            let empty = vec![0u8; grid_width];
            let row = if row_idx < self.history.len() {
                &self.history[row_idx]
            } else {
                &empty
            };
            let mut spans = Vec::with_capacity(grid_width + 1);
            let marker = if row_idx == 0 && frame.beat_is_onset == Some(true) {
                ('>', Color::Yellow)
            } else {
                (' ', Color::Reset)
            };
            spans.push(Span::styled(
                marker.0.to_string(),
                Style::default().fg(marker.1),
            ));
            // Fade older rows slightly darker via intensity scale
            let age_scale = 1.0 - (row_idx as f32 / (grid_height as f32 + 2.0)) * 0.35;
            for &v in row {
                let level = (f32::from(v) / 255.0) * age_scale;
                let ch = cell_glyph(v);
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default().fg(heat_color(level, self.color)),
                ));
            }
            lines.push(Line::from(spans));
        }
        lines.truncate(height.max(1));
        lines
    }
}

fn collapse_bands(bands: Option<&[u8]>, width: usize) -> Vec<u8> {
    let width = width.max(1);
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return vec![0; width];
    };
    let mut columns = Vec::with_capacity(width);
    for idx in 0..width {
        let start = (idx * bands.len()) / width;
        let mut end = ((idx + 1) * bands.len()) / width;
        if end <= start {
            end = (start + 1).min(bands.len());
        }
        let peak = bands
            .get(start..end.min(bands.len()))
            .and_then(|s| s.iter().max().copied())
            .unwrap_or(0);
        columns.push(peak);
    }
    columns
}

fn cell_glyph(level_u8: u8) -> char {
    let idx = ((f32::from(level_u8) / 255.0) * (ASCII_RAMP.len() - 1) as f32).round() as usize;
    ASCII_RAMP[idx.min(ASCII_RAMP.len() - 1)] as char
}
