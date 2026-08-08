//! Audio-reactive stereo VU meter (colored).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::host::{heat_color, VisualizerContext, VisualizerFrameInput, VisualizerPlugin};

#[derive(Debug)]
pub struct VuReactiveVisualizer {
    left_smooth: f32,
    right_smooth: f32,
    norm_peak: f32,
    history: Vec<f32>,
    color: bool,
}

impl Default for VuReactiveVisualizer {
    fn default() -> Self {
        Self {
            left_smooth: 0.0,
            right_smooth: 0.0,
            norm_peak: 0.35,
            history: Vec::new(),
            color: true,
        }
    }
}

impl VisualizerPlugin for VuReactiveVisualizer {
    fn plugin_id(&self) -> &'static str {
        "vu.reactive"
    }

    fn display_name(&self) -> &'static str {
        "VU Meter (Reactive)"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
        self.left_smooth = 0.0;
        self.right_smooth = 0.0;
        self.norm_peak = 0.35;
        self.history.clear();
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;

        let (left_t, right_t, source) = match (frame.level_left, frame.level_right) {
            (Some(l), Some(r)) if l.is_finite() && r.is_finite() => (
                clamp(l),
                clamp(r),
                frame.level_source.as_deref().unwrap_or("env"),
            ),
            _ => {
                let (l, r) = fallback_levels(frame);
                (l, r, "sim")
            }
        };
        let (left_t, right_t) = self.normalize(left_t, right_t);
        self.left_smooth = smooth(self.left_smooth, left_t);
        self.right_smooth = smooth(self.right_smooth, right_t);
        let mono = (self.left_smooth + self.right_smooth) * 0.5;
        self.history.push(mono);
        let max_hist = (width.saturating_sub(4)).max(10);
        if self.history.len() > max_hist {
            let drop = self.history.len() - max_hist;
            self.history.drain(0..drop);
        }

        let meter_w = (width.saturating_sub(6)).clamp(8, 48);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    "VU REACTIVE ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("[{source}]"), Style::default().fg(Color::Cyan)),
            ]),
            meter_line("L", self.left_smooth, meter_w, self.color),
            meter_line("R", self.right_smooth, meter_w, self.color),
            meter_line("M", mono, meter_w, self.color),
            status_line(frame),
        ];
        lines.extend(history_block(&self.history, width.min(48), 3, self.color));
        lines.truncate(height.max(1));
        lines
    }
}

impl VuReactiveVisualizer {
    fn normalize(&mut self, left: f32, right: f32) -> (f32, f32) {
        let peak = left.max(right);
        self.norm_peak = peak.max(self.norm_peak * 0.94).max(0.12);
        let gain = 1.0 / self.norm_peak;
        (boost(left, gain), boost(right, gain))
    }
}

fn fallback_levels(frame: &VisualizerFrameInput) -> (f32, f32) {
    if !matches!(frame.status.as_str(), "playing") {
        return (0.0, 0.0);
    }
    if frame.volume <= 0.0 {
        return (0.0, 0.0);
    }
    let t = frame.position_s * frame.speed.max(0.1) + (frame.frame_index as f64 / 14.0);
    let left = 0.10 + 0.70 * (0.30 + 0.70 * (0.5 + 0.5 * (t * 5.4 + 0.3).sin()));
    let right = 0.10 + 0.70 * (0.30 + 0.70 * (0.5 + 0.5 * (t * 6.1 + 1.2).sin()));
    (clamp(left as f32), clamp(right as f32))
}

fn smooth(current: f32, target: f32) -> f32 {
    let alpha = if target > current { 0.62 } else { 0.34 };
    clamp(current + (target - current) * alpha)
}

fn boost(value: f32, gain: f32) -> f32 {
    let raw = (value - 0.01).max(0.0);
    clamp(raw * gain * 1.15)
}

fn clamp(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn meter_line(label: &str, level: f32, width: usize, color: bool) -> Line<'static> {
    let fill = ((width as f32) * clamp(level)).round() as usize;
    let mut spans = vec![
        Span::styled(
            format!("{label} "),
            Style::default().fg(if color { Color::White } else { Color::Gray }),
        ),
        Span::styled("|", Style::default().fg(Color::DarkGray)),
    ];
    for i in 0..width {
        let filled = i < fill;
        let ch = if filled { '#' } else { '-' };
        let t = (i as f32 + 1.0) / width as f32;
        let fg = if filled {
            heat_color(t, color)
        } else {
            Color::DarkGray
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(fg)));
    }
    spans.push(Span::styled("|", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled(
        format!(" {:3}%", (level * 100.0) as i32),
        Style::default().fg(heat_color(level, color)),
    ));
    Line::from(spans)
}

fn status_line(frame: &VisualizerFrameInput) -> Line<'static> {
    let beat = if frame.beat_is_onset == Some(true) {
        "ONSET"
    } else {
        "beat"
    };
    let bpm = frame
        .beat_bpm
        .map(|b| format!("{b:.0}"))
        .unwrap_or_else(|| "--".into());
    let strength = frame
        .beat_strength
        .map(|s| format!("{s:.2}"))
        .unwrap_or_else(|| "-".into());
    let bsrc = frame.beat_source.as_deref().unwrap_or("-");
    let beat_color = if frame.beat_is_onset == Some(true) {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    Line::from(vec![
        Span::styled(
            format!(
                "{}  {:.1}x  vol {:.0}%  ",
                frame.status,
                frame.speed,
                frame.volume * 100.0
            ),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!("{beat} "),
            Style::default()
                .fg(beat_color)
                .add_modifier(if frame.beat_is_onset == Some(true) {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        Span::styled(
            format!("bpm={bpm} str={strength} [{bsrc}]"),
            Style::default().fg(Color::Cyan),
        ),
    ])
}

fn history_block(history: &[f32], width: usize, rows: usize, color: bool) -> Vec<Line<'static>> {
    if history.is_empty() || width == 0 {
        return vec![Line::from(Span::styled(
            "trail:",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let mut lines = vec![Line::from(Span::styled(
        "trail:",
        Style::default().fg(Color::DarkGray),
    ))];
    let chars = ['.', ':', '-', '=', '#', '#'];
    for row in 0..rows {
        let threshold = 1.0 - ((row + 1) as f32 / (rows as f32 + 0.5));
        let mut spans = Vec::new();
        for (i, &v) in history.iter().enumerate() {
            if i >= width {
                break;
            }
            if v >= threshold {
                let ch = chars[(v * (chars.len() - 1) as f32) as usize];
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default().fg(heat_color(v, color)),
                ));
            } else {
                spans.push(Span::raw(" "));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}
