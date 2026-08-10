//! Full-height stereo waveform-envelope visualizer.

use ratatui::style::Color;
use ratatui::text::Line;

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};
use super::util::Canvas;

#[derive(Debug)]
pub struct WaveformProxyVisualizer {
    color: bool,
    gain_left: f32,
    gain_right: f32,
    displayed_left: Vec<(f32, f32)>,
    displayed_right: Vec<(f32, f32)>,
}

impl Default for WaveformProxyVisualizer {
    fn default() -> Self {
        Self {
            color: false,
            gain_left: 1.0,
            gain_right: 1.0,
            displayed_left: Vec::new(),
            displayed_right: Vec::new(),
        }
    }
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
        self.gain_left = 1.0;
        self.gain_right = 1.0;
        self.displayed_left.clear();
        self.displayed_right.clear();
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;
        let history = resolved_history(frame);

        let left_peak = channel_peak(&history, Channel::Left);
        let right_peak = channel_peak(&history, Channel::Right);
        self.gain_left = smooth_gain(self.gain_left, desired_gain(left_peak));
        self.gain_right = smooth_gain(self.gain_right, desired_gain(right_peak));

        let mut canvas = Canvas::new(width, height);
        if height < 5 {
            // Very shallow panes cannot show two useful centered scopes. Keep
            // both channels visible as compact amplitude lanes instead.
            draw_compact_lane(&mut canvas, 0, &history, Channel::Left, self.color);
            if height > 1 {
                draw_compact_lane(
                    &mut canvas,
                    height - 1,
                    &history,
                    Channel::Right,
                    self.color,
                );
            }
            return canvas.into_lines();
        }

        // A single shared centerline gives each channel the full half-pane as
        // vertical range: left grows upward and right grows downward.
        let center_y = height / 2;
        let chart_start = usize::from(width > 2) * 2;
        let chart_width = width.saturating_sub(chart_start).max(1);
        let bar_count = bar_count_for_width(chart_width);
        let target_left = aggregate_envelope(&history, bar_count, Channel::Left);
        let target_right = aggregate_envelope(&history, bar_count, Channel::Right);
        update_envelope(&mut self.displayed_left, &target_left);
        update_envelope(&mut self.displayed_right, &target_right);

        for x in 0..width {
            canvas.set(x as i32, center_y as i32, '─', Color::DarkGray);
        }

        draw_half_waveform(
            &mut canvas,
            HalfWaveformParams {
                center_y,
                radius: center_y,
                direction: -1,
                envelope: &self.displayed_left,
                channel: Channel::Left,
                gain: self.gain_left,
                color: self.color,
            },
        );
        draw_half_waveform(
            &mut canvas,
            HalfWaveformParams {
                center_y,
                radius: height.saturating_sub(center_y + 1),
                direction: 1,
                envelope: &self.displayed_right,
                channel: Channel::Right,
                gain: self.gain_right,
                color: self.color,
            },
        );

        if center_y > 0 {
            canvas.set(
                0,
                (center_y - 1) as i32,
                'L',
                if self.color {
                    channel_color(Channel::Left, 0.85)
                } else {
                    Color::Gray
                },
            );
        }
        if center_y + 1 < height {
            canvas.set(
                0,
                (center_y + 1) as i32,
                'R',
                if self.color {
                    channel_color(Channel::Right, 0.85)
                } else {
                    Color::Gray
                },
            );
        }
        canvas.set(0, center_y as i32, '┤', Color::DarkGray);

        // Keep source metadata unobtrusive on the shared centerline.
        if let Some(source) = frame.waveform_source.as_deref() {
            let label: Vec<char> = format!("[{source}]").chars().take(width).collect();
            let label_x = width.saturating_sub(label.len());
            for (offset, ch) in label.into_iter().enumerate() {
                canvas.set(
                    (label_x + offset) as i32,
                    center_y as i32,
                    ch,
                    Color::DarkGray,
                );
            }
        }

        canvas.into_lines()
    }
}

#[derive(Clone, Copy)]
enum Channel {
    Left,
    Right,
}

fn resolved_history(frame: &VisualizerFrameInput) -> Vec<(f32, f32, f32, f32)> {
    if let Some(history) = frame.waveform_history.as_ref().filter(|h| !h.is_empty()) {
        return history
            .iter()
            .map(|&(min_l, max_l, min_r, max_r)| {
                (clamp(min_l), clamp(max_l), clamp(min_r), clamp(max_r))
            })
            .collect();
    }

    let (min_l, max_l, min_r, max_r) = resolve_ranges(frame);
    vec![(min_l, max_l, min_r, max_r)]
}

fn channel_range(bucket: (f32, f32, f32, f32), channel: Channel) -> (f32, f32) {
    match channel {
        Channel::Left => (bucket.0, bucket.1),
        Channel::Right => (bucket.2, bucket.3),
    }
}

fn channel_peak(history: &[(f32, f32, f32, f32)], channel: Channel) -> f32 {
    history.iter().fold(0.0, |peak, &bucket| {
        let (minimum, maximum) = channel_range(bucket, channel);
        peak.max(minimum.abs().max(maximum.abs()))
    })
}

fn desired_gain(peak: f32) -> f32 {
    // Aim the loudest recent peak below the hard ceiling. Unlike the previous
    // 1.0 minimum, this permits attenuation of heavily mastered material.
    (0.92 / peak.max(0.01)).clamp(0.75, 8.0)
}

fn expressive_amplitude(amplitude: f32, gain: f32) -> f32 {
    // Peak envelopes from loud masters cluster near 1.0. A curve above 1.0
    // spreads those high values across more rows while retaining true peaks.
    (amplitude.clamp(0.0, 1.0) * gain)
        .clamp(0.0, 1.0)
        .powf(1.65)
}

fn smooth_gain(current: f32, target: f32) -> f32 {
    // Gain moves much more slowly than the waveform itself so the entire scope
    // does not pulse whenever the loudest history bucket enters or leaves.
    let response = if target < current { 0.24 } else { 0.06 };
    current + (target - current) * response
}

fn aggregate_envelope(
    history: &[(f32, f32, f32, f32)],
    bar_count: usize,
    channel: Channel,
) -> Vec<(f32, f32)> {
    (0..bar_count)
        .map(|bar| aggregate_column(history, bar, bar_count, channel))
        .collect()
}

fn update_envelope(displayed: &mut Vec<(f32, f32)>, target: &[(f32, f32)]) {
    if displayed.len() != target.len() {
        displayed.clear();
        displayed.extend_from_slice(target);
        return;
    }

    for ((current_min, current_max), &(target_min, target_max)) in displayed.iter_mut().zip(target)
    {
        // Peaks arrive promptly but relax gently. Persistent per-column state
        // removes the hard snapping caused by re-binning history every frame.
        let min_response = if target_min < *current_min {
            0.45
        } else {
            0.18
        };
        let max_response = if target_max > *current_max {
            0.45
        } else {
            0.18
        };
        *current_min += (target_min - *current_min) * min_response;
        *current_max += (target_max - *current_max) * max_response;
    }
}

struct HalfWaveformParams<'a> {
    center_y: usize,
    radius: usize,
    direction: i32,
    envelope: &'a [(f32, f32)],
    channel: Channel,
    gain: f32,
    color: bool,
}

fn draw_half_waveform(canvas: &mut Canvas, params: HalfWaveformParams<'_>) {
    let HalfWaveformParams {
        center_y,
        radius,
        direction,
        envelope,
        channel,
        gain,
        color,
    } = params;

    if radius == 0 || canvas.width == 0 {
        return;
    }

    let chart_start = usize::from(canvas.width > 2) * 2;
    let chart_width = canvas.width.saturating_sub(chart_start).max(1);
    let bar_count = envelope.len().max(1);

    for bar in 0..bar_count {
        let col = if bar_count <= 1 {
            0
        } else {
            ((bar as f32 * chart_width.saturating_sub(1) as f32)
                / bar_count.saturating_sub(1) as f32)
                .round() as usize
        };
        let (minimum, maximum) = envelope.get(bar).copied().unwrap_or((0.0, 0.0));
        let peak = expressive_amplitude(minimum.abs().max(maximum.abs()), gain);
        let extent = (peak * radius as f32).round() as usize;

        for distance in 0..=extent {
            let y = (center_y as i32 + direction * distance as i32) as usize;
            let edge = distance == extent;
            let relative = distance as f32 / radius as f32;
            let intensity = (0.34 + peak * 0.38 + relative * 0.28).clamp(0.0, 1.0);
            let glyph = if extent == 0 {
                '•'
            } else if edge {
                '●'
            } else if distance == 0 {
                '┼'
            } else {
                '│'
            };
            canvas.set(
                (chart_start + col) as i32,
                y as i32,
                glyph,
                if distance == 0 {
                    Color::DarkGray
                } else if color {
                    channel_color(channel, intensity)
                } else if intensity > 0.72 {
                    Color::White
                } else {
                    Color::Gray
                },
            );
        }
    }
}

fn bar_count_for_width(width: usize) -> usize {
    width.max(1)
}

fn aggregate_column(
    history: &[(f32, f32, f32, f32)],
    col: usize,
    width: usize,
    channel: Channel,
) -> (f32, f32) {
    let start = col * history.len() / width;
    let end = ((col + 1) * history.len() / width)
        .max(start + 1)
        .min(history.len());
    let mut minimum = 1.0f32;
    let mut maximum = -1.0f32;
    for &bucket in &history[start..end] {
        let (bucket_min, bucket_max) = channel_range(bucket, channel);
        minimum = minimum.min(bucket_min);
        maximum = maximum.max(bucket_max);
    }
    (minimum, maximum)
}

fn channel_color(channel: Channel, intensity: f32) -> Color {
    let light = intensity.clamp(0.0, 1.0);
    match channel {
        // Left: deep blue through electric cyan-white.
        Channel::Left => Color::Rgb(
            (8.0 + light * 145.0) as u8,
            (48.0 + light * 195.0) as u8,
            (105.0 + light * 150.0) as u8,
        ),
        // Right: violet through hot magenta-white.
        Channel::Right => Color::Rgb(
            (85.0 + light * 170.0) as u8,
            (12.0 + light * 135.0) as u8,
            (90.0 + light * 165.0) as u8,
        ),
    }
}

fn draw_compact_lane(
    canvas: &mut Canvas,
    y: usize,
    history: &[(f32, f32, f32, f32)],
    channel: Channel,
    color: bool,
) {
    let peak = channel_peak(history, channel);
    let filled = (peak * canvas.width as f32).round() as usize;
    for x in 0..canvas.width {
        let active = x < filled;
        canvas.set(
            x as i32,
            y as i32,
            if active { '━' } else { '─' },
            if active && color {
                channel_color(channel, 0.75)
            } else if active {
                Color::Gray
            } else {
                Color::DarkGray
            },
        );
    }
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

fn clamp(value: f32) -> f32 {
    value.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_frame() -> VisualizerFrameInput {
        VisualizerFrameInput {
            frame_index: 0,
            width: 40,
            height: 15,
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

    fn row_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn renders_two_full_height_channel_scopes() {
        let mut frame = base_frame();
        frame.waveform_history = Some(vec![
            (-0.1, 0.2, -0.2, 0.3),
            (-0.8, 0.7, -0.5, 0.9),
            (-0.3, 0.4, -0.7, 0.6),
        ]);
        let mut viz = WaveformProxyVisualizer::default();
        let lines = viz.render(&frame);

        assert_eq!(lines.len(), frame.height as usize);
        let upper = lines[..7].iter().map(row_text).collect::<String>();
        let lower = lines[8..].iter().map(row_text).collect::<String>();
        assert!(upper.contains('L') && upper.contains('│'));
        assert!(lower.contains('R') && lower.contains('│'));
    }

    #[test]
    fn adaptive_gain_expands_quiet_waveforms() {
        assert_eq!(desired_gain(0.05), 8.0);
        assert!((desired_gain(0.5) - 1.84).abs() < 0.001);
        assert!((desired_gain(1.0) - 0.92).abs() < 0.001);
    }

    #[test]
    fn loud_amplitudes_are_spread_across_more_vertical_range() {
        let ordinary_loud = expressive_amplitude(0.80, 0.92);
        let true_peak = expressive_amplitude(1.0, 0.92);
        assert!(ordinary_loud < 0.65);
        assert!(true_peak > 0.85);
        assert!(true_peak - ordinary_loud > 0.25);
    }

    #[test]
    fn waveform_restores_one_bar_per_available_column() {
        assert_eq!(bar_count_for_width(100), 100);
        assert_eq!(bar_count_for_width(40), 40);
        assert_eq!(bar_count_for_width(1), 1);
    }

    #[test]
    fn mirrored_channels_can_reach_both_pane_edges() {
        let mut frame = base_frame();
        frame.waveform_history = Some(vec![(-1.0, 1.0, -1.0, 1.0); 40]);
        let mut viz = WaveformProxyVisualizer::default();
        let lines = viz.render(&frame);
        assert!(row_text(&lines[0]).contains('●'));
        assert!(row_text(lines.last().expect("bottom row")).contains('●'));
    }

    #[test]
    fn waveform_endpoints_glide_instead_of_snapping() {
        let mut displayed = vec![(-0.2, 0.2)];
        update_envelope(&mut displayed, &[(-0.8, 0.8)]);
        assert!(displayed[0].0 < -0.2 && displayed[0].0 > -0.8);
        assert!(displayed[0].1 > 0.2 && displayed[0].1 < 0.8);

        let expanded = displayed[0];
        update_envelope(&mut displayed, &[(-0.1, 0.1)]);
        assert!(displayed[0].0 > expanded.0 && displayed[0].0 < -0.1);
        assert!(displayed[0].1 < expanded.1 && displayed[0].1 > 0.1);
    }

    #[test]
    fn fallback_ranges_still_render_both_channels() {
        let mut viz = WaveformProxyVisualizer::default();
        let lines = viz.render(&base_frame());
        let text = lines.iter().map(row_text).collect::<String>();
        assert!(text.contains('L') && text.contains('R'));
    }
}
