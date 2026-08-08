//! Minimal progress-bar visualizer (fallback).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};

#[derive(Debug, Default)]
pub struct BasicVisualizer {
    color: bool,
}

impl VisualizerPlugin for BasicVisualizer {
    fn plugin_id(&self) -> &'static str {
        "basic"
    }

    fn display_name(&self) -> &'static str {
        "Basic"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let fg = if self.color { Color::Cyan } else { Color::Gray };
        let dim = if self.color {
            Color::DarkGray
        } else {
            Color::Gray
        };

        if frame.status == "error" {
            return vec![Line::from(Span::styled(
                "Error",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ))];
        }
        if !matches!(frame.status.as_str(), "playing" | "paused") {
            let msg = if matches!(frame.status.as_str(), "idle" | "stopped") {
                "Idle"
            } else {
                "Loading"
            };
            return vec![Line::from(Span::styled(msg, Style::default().fg(dim)))];
        }
        if frame.status == "paused" {
            return vec![Line::from(Span::styled(
                "Paused",
                Style::default().fg(Color::Yellow),
            ))];
        }

        let bar_width = width.clamp(8, 40);
        let pct = match frame.duration_s {
            Some(d) if d > 0.0 => (frame.position_s / d).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let fill = ((bar_width as f64) * pct).round() as usize;
        let mut spans = vec![Span::styled("[", Style::default().fg(dim))];
        for i in 0..bar_width {
            let ch = if i < fill { '#' } else { '-' };
            let c = if i < fill {
                if self.color {
                    let t = i as f32 / bar_width as f32;
                    super::host::heat_color(t, true)
                } else {
                    Color::White
                }
            } else {
                dim
            };
            spans.push(Span::styled(ch.to_string(), Style::default().fg(c)));
        }
        spans.push(Span::styled("]", Style::default().fg(dim)));
        spans.push(Span::styled(
            format!(" {:3}%", (pct * 100.0) as i32),
            Style::default().fg(fg),
        ));

        let title = frame
            .title
            .clone()
            .or_else(|| {
                frame
                    .track_path
                    .as_ref()
                    .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string())
            })
            .unwrap_or_else(|| "Unknown track".into());

        vec![
            Line::from(spans),
            Line::from(Span::styled(title, Style::default().fg(Color::White))),
        ]
    }
}
