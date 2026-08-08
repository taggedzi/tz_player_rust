//! Oscilloscope-style waveform proxy visualizer (colored).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};

#[derive(Debug, Default)]
pub struct WaveformProxyVisualizer {
    color: bool,
}

impl VisualizerPlugin for WaveformProxyVisualizer {
    fn plugin_id(&self) -> &'static str {
        "viz.waveform.proxy"
    }

    fn display_name(&self) -> &'static str {
        "Waveform Proxy"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(12) as usize;
        let src = frame.waveform_source.as_deref().unwrap_or("fallback");
        let mut lines = vec![Line::from(vec![
            Span::styled(
                "WaveformProxy ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("[{src}]"), Style::default().fg(Color::Yellow)),
        ])];
        match frame.waveform_history.as_ref().filter(|h| h.len() >= 2) {
            Some(history) => {
                let chart_width = width.saturating_sub(4).max(6);
                lines.push(render_trace(
                    "L",
                    history,
                    chart_width,
                    Channel::Left,
                    Color::Green,
                    self.color,
                ));
                lines.push(render_trace(
                    "R",
                    history,
                    chart_width,
                    Channel::Right,
                    Color::Magenta,
                    self.color,
                ));
            }
            None => {
                let (lmin, lmax, rmin, rmax) = resolve_ranges(frame);
                lines.push(render_lane(
                    "L",
                    lmin,
                    lmax,
                    width,
                    Color::Green,
                    self.color,
                ));
                lines.push(render_lane(
                    "R",
                    rmin,
                    rmax,
                    width,
                    Color::Magenta,
                    self.color,
                ));
            }
        }
        if let Some(h) = frame.height.checked_sub(1) {
            lines.truncate(h.max(1) as usize);
        }
        lines
    }
}

#[derive(Clone, Copy)]
enum Channel {
    Left,
    Right,
}

/// Scrolling amplitude sparkline built from recent waveform-proxy buckets
/// (oldest first); each column shows the peak |amplitude| for its time slice.
fn render_trace(
    label: &str,
    history: &[(f32, f32, f32, f32)],
    width: usize,
    channel: Channel,
    accent: Color,
    color: bool,
) -> Line<'static> {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let chart_width = width.max(1);
    let fg = if color { accent } else { Color::Gray };
    let mut spans = vec![Span::styled(format!("{label}: "), Style::default().fg(fg))];
    for col in 0..chart_width {
        let start = col * history.len() / chart_width;
        let end = ((col + 1) * history.len() / chart_width)
            .max(start + 1)
            .min(history.len());
        let mut peak = 0.0f32;
        for &(min_l, max_l, min_r, max_r) in &history[start..end] {
            let (mn, mx) = match channel {
                Channel::Left => (min_l, max_l),
                Channel::Right => (min_r, max_r),
            };
            peak = peak.max(mn.abs().max(mx.abs()));
        }
        let idx = (peak.clamp(0.0, 1.0) * (BLOCKS.len() - 1) as f32).round() as usize;
        let idx = idx.min(BLOCKS.len() - 1);
        spans.push(Span::styled(
            BLOCKS[idx].to_string(),
            Style::default().fg(fg),
        ));
    }
    Line::from(spans)
}

fn resolve_ranges(frame: &VisualizerFrameInput) -> (f32, f32, f32, f32) {
    if let (Some(a), Some(b), Some(c), Some(d)) = (
        frame.waveform_min_left,
        frame.waveform_max_left,
        frame.waveform_min_right,
        frame.waveform_max_right,
    ) {
        return (clamp(a), clamp(b), clamp(c), clamp(d));
    }
    let left = clamp(frame.level_left.unwrap_or(0.0));
    let right = clamp(frame.level_right.unwrap_or(0.0));
    (-left, left, -right, right)
}

fn render_lane(
    label: &str,
    minimum: f32,
    maximum: f32,
    width: usize,
    accent: Color,
    color: bool,
) -> Line<'static> {
    let chart_width = width.saturating_sub(4).max(6);
    let center = chart_width / 2;
    let left_col = to_column(minimum, chart_width);
    let right_col = to_column(maximum, chart_width);
    let start = left_col.min(right_col);
    let end = left_col.max(right_col);

    let mut spans = vec![Span::styled(
        format!("{label}: "),
        Style::default().fg(if color { accent } else { Color::Gray }),
    )];
    for i in 0..chart_width {
        let (ch, fg) = if i == center && start <= center && center <= end {
            ('┼', if color { Color::White } else { Color::Gray })
        } else if i == center {
            ('|', Color::DarkGray)
        } else if i >= start && i <= end {
            ('─', if color { accent } else { Color::Gray })
        } else {
            (' ', Color::Reset)
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(fg)));
    }
    Line::from(spans)
}

fn to_column(value: f32, width: usize) -> usize {
    let normalized = (clamp(value) + 1.0) * 0.5;
    let col = (normalized * (width.saturating_sub(1) as f32)).round() as isize;
    col.clamp(0, width.saturating_sub(1) as isize) as usize
}

fn clamp(v: f32) -> f32 {
    v.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_frame() -> VisualizerFrameInput {
        VisualizerFrameInput {
            frame_index: 0,
            width: 40,
            height: 6,
            status: "playing".into(),
            position_s: 1.0,
            duration_s: Some(10.0),
            volume: 1.0,
            speed: 1.0,
            title: None,
            track_path: None,
            level_left: None,
            level_right: None,
            level_source: None,
            spectrum_bands: None,
            spectrum_source: None,
            beat_strength: None,
            beat_is_onset: None,
            beat_bpm: None,
            beat_source: None,
            waveform_min_left: Some(-0.2),
            waveform_max_left: Some(0.2),
            waveform_min_right: Some(-0.3),
            waveform_max_right: Some(0.3),
            waveform_source: Some("cache".into()),
            waveform_history: None,
        }
    }

    #[test]
    fn falls_back_to_bar_when_no_history() {
        let mut viz = WaveformProxyVisualizer::default();
        let lines = viz.render(&base_frame());
        let l_line: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(l_line.contains('┼') || l_line.contains('─'));
    }

    #[test]
    fn uses_scrolling_trace_when_history_present() {
        let mut frame = base_frame();
        frame.waveform_history = Some(vec![
            (-0.1, 0.1, -0.1, 0.1),
            (-0.9, 0.9, -0.9, 0.9),
            (-0.05, 0.05, -0.05, 0.05),
        ]);
        let mut viz = WaveformProxyVisualizer::default();
        let lines = viz.render(&frame);
        let l_line: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        // No bar-chart glyphs; sparkline blocks only.
        assert!(!l_line.contains('┼') && !l_line.contains('─'));
        assert!(l_line.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)));
    }
}
