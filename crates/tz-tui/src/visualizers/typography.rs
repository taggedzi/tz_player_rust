//! Typography glitch visualizer (metadata + beat pulse).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};
use super::util::{bass_energy, beat_onset, fit_lines, mono_level, track_label};

const GLITCH: &[char] = &['@', '#', '$', '%', '&', '*', '+', '=', '?', '!'];

#[derive(Debug, Default)]
pub struct TypographyGlitchVisualizer {
    color: bool,
}

impl VisualizerPlugin for TypographyGlitchVisualizer {
    fn plugin_id(&self) -> &'static str {
        "viz.typography.glitch"
    }

    fn display_name(&self) -> &'static str {
        "Typography Glitch"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;
        let mono = mono_level(frame);
        let bass = bass_energy(frame.spectrum_bands.as_deref());
        let onset = beat_onset(frame);
        let title = track_label(frame);
        let detail = frame
            .track_path
            .as_ref()
            .map(|p| p.rsplit(['/', '\\']).nth(1).unwrap_or("Local").to_string())
            .unwrap_or_else(|| "Unknown Artist".into());

        let wobble = ((mono * 3.0 + bass * 2.0) * (1.0 + (frame.frame_index % 7) as f32 * 0.1))
            .round() as isize
            % 3;
        let title_g = glitch_text(&title, frame.frame_index, onset);
        let detail_g = glitch_text(&detail, frame.frame_index + 11, onset);
        let border_ch = if onset { '=' } else { '-' };
        let top = border_ch.to_string().repeat(width);
        let breath = ((1.0 - mono) * 4.0).round() as usize;
        let mid = format!(
            "{}{}{}",
            " ".repeat(breath.min(width / 3)),
            "·".repeat(width.saturating_sub(breath * 2).max(1)),
            " ".repeat(breath.min(width / 3))
        );
        let mid = pad(&mid, width);
        let status = format!(
            "{} | BEAT {} | RMS {:3.0}%",
            frame.status.to_uppercase(),
            if onset { "ONSET" } else { "IDLE" },
            mono * 100.0
        );

        let lines = vec![
            Line::from(Span::styled(
                pad(&top, width),
                Style::default().fg(if self.color {
                    Color::Magenta
                } else {
                    Color::Gray
                }),
            )),
            Line::from(Span::styled(
                pad(&status, width),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                mid.clone(),
                Style::default().fg(Color::DarkGray),
            )),
            styled_center(&title_g, width, wobble, self.color, true),
            styled_center(&detail_g, width, -wobble, self.color, false),
            Line::from(Span::styled(mid, Style::default().fg(Color::DarkGray))),
            Line::from(Span::styled(
                pad(&top, width),
                Style::default().fg(if self.color {
                    Color::Magenta
                } else {
                    Color::Gray
                }),
            )),
        ];
        fit_lines(lines, height)
    }
}

fn glitch_text(text: &str, frame: u64, onset: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return "?".into();
    }
    let mut out = String::new();
    for (i, ch) in chars.iter().enumerate() {
        let mutate = onset || ((frame as usize + i * 3) % 17 == 0);
        if mutate && ch.is_alphanumeric() {
            out.push(GLITCH[(i + frame as usize) % GLITCH.len()]);
        } else {
            out.push(*ch);
        }
    }
    out
}

fn pad(s: &str, width: usize) -> String {
    let mut t: String = s.chars().take(width).collect();
    while t.chars().count() < width {
        t.push(' ');
    }
    t
}

fn styled_center(
    text: &str,
    width: usize,
    wobble: isize,
    color: bool,
    bold: bool,
) -> Line<'static> {
    let len = text.chars().count();
    let mut pad_left = width.saturating_sub(len) / 2;
    if wobble > 0 {
        pad_left = pad_left
            .saturating_add(wobble as usize)
            .min(width.saturating_sub(1));
    } else if wobble < 0 {
        pad_left = pad_left.saturating_sub((-wobble) as usize);
    }
    let mut line = " ".repeat(pad_left);
    line.push_str(text);
    line = pad(&line, width);
    let mut style = Style::default().fg(if color {
        if bold {
            Color::Cyan
        } else {
            Color::White
        }
    } else {
        Color::Gray
    });
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(Span::styled(line, style))
}
