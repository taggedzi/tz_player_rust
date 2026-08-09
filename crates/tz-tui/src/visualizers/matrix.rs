//! Stateful, music-reactive matrix rain in green / blue / red themes.

use ratatui::style::Color;
use ratatui::text::Line;

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};
use super::util::{
    bass_energy, beat_onset, high_energy, mid_energy, mix_u64, mono_level, stable_seed, unit_noise,
    Canvas,
};

// Half-width Katakana are deliberately used here: unlike full-width Japanese
// glyphs, they occupy one terminal cell in terminals/fonts that support them.
const GLYPHS: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'K', 'M', 'N',
    'R', 'T', 'X', 'Z', '$', '#', '@', '%', '&', '*', '+', '=', '-', ':', 'ｱ', 'ｲ', 'ｳ', 'ｴ', 'ｵ',
    'ｶ', 'ｷ', 'ｸ', 'ｹ', 'ｺ', 'ｻ', 'ｼ', 'ｽ', 'ｾ', 'ｿ', 'ﾀ', 'ﾁ', 'ﾂ', 'ﾃ', 'ﾄ', 'ﾅ', 'ﾆ', 'ﾇ', 'ﾈ',
    'ﾉ', 'ﾏ', 'ﾐ', 'ﾑ', 'ﾒ', 'ﾓ', 'ﾗ', 'ﾘ', 'ﾙ', 'ﾚ', 'ﾛ',
];
const MATRIX_SPEED_BOOST: f32 = 1.15;
const MATRIX_DENSITY_BOOST: f32 = 1.20;

#[derive(Debug, Clone, Copy, Default)]
enum MatrixTheme {
    #[default]
    Green,
    Blue,
    Red,
}

#[derive(Debug, Clone)]
struct MatrixStream {
    x: f32,
    y: f32,
    base_speed: f32,
    trail_len: u8,
    weight: f32,
    brightness: f32,
    glyph_seed: u64,
}

#[derive(Debug)]
struct MatrixRain {
    theme: MatrixTheme,
    color: bool,
    streams: Vec<MatrixStream>,
    seed: u64,
    serial: u64,
    width: usize,
    height: usize,
    smoothed_energy: f32,
    smoothed_high: f32,
    smoothed_fall: f32,
    smoothed_wind: f32,
    beat_flash: f32,
}

impl MatrixRain {
    fn new(theme: MatrixTheme) -> Self {
        Self {
            theme,
            color: false,
            streams: Vec::new(),
            seed: 0,
            serial: 0,
            width: 0,
            height: 0,
            smoothed_energy: 0.0,
            smoothed_high: 0.0,
            smoothed_fall: 0.8,
            smoothed_wind: 0.0,
            beat_flash: 0.0,
        }
    }

    fn reset(&mut self, seed: u64, width: usize, height: usize) {
        self.streams.clear();
        self.seed = seed;
        self.serial = 0;
        self.width = width;
        self.height = height;
        self.smoothed_energy = 0.0;
        self.smoothed_high = 0.0;
        self.smoothed_fall = 0.8;
        self.smoothed_wind = 0.0;
        self.beat_flash = 0.0;
    }

    fn make_stream(
        &mut self,
        width: usize,
        height: usize,
        bass: f32,
        energy: f32,
        fill: bool,
    ) -> MatrixStream {
        let id = self.serial;
        self.serial = self.serial.wrapping_add(1);
        let salt = mix_u64(self.seed, id.wrapping_mul(0x9e37).wrapping_add(29));
        let weight = (unit_noise(self.seed, salt ^ 0x71) * 0.55 + bass * 0.45).clamp(0.0, 1.0);
        MatrixStream {
            x: unit_noise(self.seed, salt ^ 0x19) * width.saturating_sub(1) as f32,
            y: if fill {
                unit_noise(self.seed, salt ^ 0x33) * height.saturating_sub(1) as f32
            } else {
                -1.0 - unit_noise(self.seed, salt ^ 0x44) * 5.0
            },
            base_speed: 0.36 + unit_noise(self.seed, salt ^ 0x55) * 0.52,
            trail_len: (4.0 + weight * 5.0 + unit_noise(self.seed, salt ^ 0x81) * 2.0).round()
                as u8,
            weight,
            brightness: (0.18 + energy * 0.35 + unit_noise(self.seed, salt ^ 0x91) * 0.16)
                .clamp(0.16, 0.95),
            glyph_seed: salt,
        }
    }

    fn target_density(width: usize, height: usize, energy: f32, playing: bool) -> usize {
        if !playing || energy < 0.015 {
            return 0;
        }
        let area = width.saturating_mul(height);
        let minimum = (width / 5).clamp(8, 24);
        let maximum = (area / 40).clamp(minimum, 88);
        let musical_target =
            minimum + ((maximum - minimum) as f32 * energy.powf(0.68)).round() as usize;
        // Matrix rain intentionally occupies more of the pane than the shorter,
        // more naturalistic Reactive Rain drops.
        ((musical_target as f32) * MATRIX_DENSITY_BOOST).round() as usize
    }

    fn glyph(stream: &MatrixStream, trail: u8, frame_index: u64, high: f32) -> char {
        // Higher frequencies make the code mutate more quickly, while the head
        // still advances through a coherent per-stream sequence.
        let cadence = if high > 0.66 {
            1
        } else if high > 0.32 {
            2
        } else {
            4
        };
        let tick = frame_index / cadence;
        let mixed = mix_u64(
            stream.glyph_seed,
            tick.wrapping_add(u64::from(trail).wrapping_mul(17)),
        );
        GLYPHS[(mixed as usize) % GLYPHS.len()]
    }

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;
        let seed = stable_seed(
            frame
                .track_path
                .as_deref()
                .or(frame.title.as_deref())
                .unwrap_or("MATRIX RAIN"),
        );
        if seed != self.seed || width != self.width || height != self.height {
            self.reset(seed, width, height);
        }

        let mono = mono_level(frame);
        let bands = frame.spectrum_bands.as_deref();
        let bass = bass_energy(bands);
        let mid = mid_energy(bands);
        let high = high_energy(bands);
        let playing = frame.status == "playing";
        let raw_energy = if playing {
            (mono * 0.52 + bass * 0.20 + mid * 0.18 + high * 0.10).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.smoothed_energy += (raw_energy - self.smoothed_energy) * 0.22;
        self.smoothed_high += (high - self.smoothed_high) * 0.28;

        if beat_onset(frame) {
            self.beat_flash = 1.0;
        } else {
            self.beat_flash *= 0.62;
        }

        let beat = frame.beat_strength.unwrap_or(0.0).clamp(0.0, 1.0);
        let fall_target = if playing {
            (0.76 + mono * 0.58 + mid * 0.28 + beat * 0.15) * 1.10 * MATRIX_SPEED_BOOST
        } else {
            0.0
        };
        self.smoothed_fall += (fall_target - self.smoothed_fall) * 0.18;

        let stereo = match (frame.level_left, frame.level_right) {
            (Some(left), Some(right)) => (right - left).clamp(-1.0, 1.0),
            _ => 0.0,
        };
        let high_wind = (frame.frame_index as f32 * 0.055).sin() * self.smoothed_high * 0.10;
        let wind_target = stereo * (0.07 + self.smoothed_high * 0.10) + high_wind;
        self.smoothed_wind += (wind_target - self.smoothed_wind) * 0.18;

        let target = Self::target_density(width, height, self.smoothed_energy, playing);
        if self.streams.is_empty() && target > 0 {
            for _ in 0..target {
                let stream = self.make_stream(width, height, bass, self.smoothed_energy, true);
                self.streams.push(stream);
            }
        }

        if playing {
            for stream in &mut self.streams {
                stream.y += stream.base_speed * self.smoothed_fall;
                stream.x = (stream.x + self.smoothed_wind * (0.75 + stream.weight * 0.35))
                    .clamp(0.0, width.saturating_sub(1) as f32);
            }
        }

        // Streams simply disappear at the bottom; Matrix rain has no splash.
        self.streams
            .retain(|stream| stream.y - f32::from(stream.trail_len) < height as f32);

        let spawn_count = target.saturating_sub(self.streams.len()).min(4);
        for _ in 0..spawn_count {
            let stream = self.make_stream(width, height, bass, self.smoothed_energy, false);
            self.streams.push(stream);
        }

        let mut canvas = Canvas::new(width, height);
        let onset_flash = self.beat_flash * 0.42;
        for stream in &self.streams {
            let x = stream.x.round() as i32;
            let head_y = stream.y.round() as i32;
            for trail in 0..=stream.trail_len {
                let y = head_y - i32::from(trail);
                if y < 0 || y >= height as i32 {
                    continue;
                }
                let fade = 1.0 - f32::from(trail) / (f32::from(stream.trail_len) + 1.0);
                let brightness = (stream.brightness * (0.20 + fade * 0.80)
                    + self.smoothed_high * (0.10 + fade * 0.24)
                    + onset_flash * fade)
                    .clamp(0.0, 1.0);
                let glyph = Self::glyph(stream, trail, frame.frame_index, self.smoothed_high);
                let color = matrix_color(
                    self.theme,
                    brightness,
                    stream.weight,
                    self.smoothed_high,
                    trail == 0,
                    self.color,
                );
                if trail == 0 {
                    canvas.set(x, y, glyph, color);
                } else {
                    canvas.set_if_empty(x, y, glyph, color);
                }
            }
        }
        canvas.into_lines()
    }
}

fn matrix_color(
    theme: MatrixTheme,
    brightness: f32,
    bass: f32,
    high: f32,
    head: bool,
    color: bool,
) -> Color {
    // Keep the lead glyph clearly visible even before high-frequency and beat
    // flashes arrive. Trails retain their existing, darker falloff.
    let light = (brightness + if head { 0.31 } else { 0.0 }).clamp(0.0, 1.0);
    if !color {
        return if light > 0.72 {
            Color::White
        } else if light > 0.34 {
            Color::Gray
        } else {
            Color::DarkGray
        };
    }

    match theme {
        MatrixTheme::Green => Color::Rgb(
            (4.0 + light * 170.0 + high * 30.0) as u8,
            (42.0 + light * 205.0) as u8,
            (12.0 + light * 135.0 + high * 40.0 - bass * 8.0).clamp(0.0, 255.0) as u8,
        ),
        MatrixTheme::Blue => Color::Rgb(
            (8.0 + light * 145.0 + high * 35.0) as u8,
            (30.0 + light * 175.0 + high * 35.0) as u8,
            (85.0 + light * 165.0 - bass * 10.0).clamp(0.0, 255.0) as u8,
        ),
        MatrixTheme::Red => Color::Rgb(
            (65.0 + light * 190.0) as u8,
            (8.0 + light * 140.0 + high * 25.0) as u8,
            (8.0 + light * 115.0 + high * 22.0 - bass * 7.0).clamp(0.0, 255.0) as u8,
        ),
    }
}

macro_rules! matrix_plugin {
    ($ty:ident, $theme:expr, $id:expr, $name:expr) => {
        #[derive(Debug)]
        pub struct $ty {
            rain: MatrixRain,
        }

        impl Default for $ty {
            fn default() -> Self {
                Self {
                    rain: MatrixRain::new($theme),
                }
            }
        }

        impl VisualizerPlugin for $ty {
            fn plugin_id(&self) -> &'static str {
                $id
            }

            fn display_name(&self) -> &'static str {
                $name
            }

            fn on_activate(&mut self, context: VisualizerContext) {
                self.rain.color = context.ansi_enabled;
                self.rain.streams.clear();
            }

            fn on_deactivate(&mut self) {}

            fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
                self.rain.render(frame)
            }
        }
    };
}

matrix_plugin!(
    MatrixGreenVisualizer,
    MatrixTheme::Green,
    "matrix.green",
    "Matrix Rain (Green)"
);
matrix_plugin!(
    MatrixBlueVisualizer,
    MatrixTheme::Blue,
    "matrix.blue",
    "Matrix Rain (Blue)"
);
matrix_plugin!(
    MatrixRedVisualizer,
    MatrixTheme::Red,
    "matrix.red",
    "Matrix Rain (Red)"
);

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(frame_index: u64, level: f32) -> VisualizerFrameInput {
        VisualizerFrameInput {
            frame_index,
            width: 80,
            height: 20,
            status: "playing".into(),
            position_s: frame_index as f64 * 0.05,
            duration_s: Some(180.0),
            volume: 1.0,
            speed: 1.0,
            title: Some("Matrix Test".into()),
            track_path: Some("matrix-test.mp3".into()),
            level_left: Some(level),
            level_right: Some(level),
            level_source: None,
            spectrum_bands: Some(vec![(level * 255.0) as u8; 48]),
            spectrum_source: None,
            beat_strength: Some(level),
            beat_is_onset: Some(false),
            beat_bpm: Some(120.0),
            beat_source: None,
            waveform_min_left: None,
            waveform_max_left: None,
            waveform_min_right: None,
            waveform_max_right: None,
            waveform_source: None,
            waveform_history: None,
        }
    }

    #[test]
    fn streams_keep_moving_down_when_music_energy_changes() {
        let mut rain = MatrixRain::new(MatrixTheme::Green);
        rain.render(&frame(0, 0.35));
        let before: Vec<(u64, f32)> = rain
            .streams
            .iter()
            .map(|stream| (stream.glyph_seed, stream.y))
            .collect();

        rain.render(&frame(1, 0.95));
        for (id, old_y) in before {
            if let Some(stream) = rain.streams.iter().find(|stream| stream.glyph_seed == id) {
                assert!(
                    stream.y > old_y,
                    "stream moved backward after energy change"
                );
            }
        }
    }

    #[test]
    fn matrix_is_denser_than_reactive_rain_but_silent_when_stopped() {
        assert_eq!(MatrixRain::target_density(80, 20, 0.8, false), 0);
        // The unboosted musical target for this pane/energy is 37 streams.
        assert_eq!(MatrixRain::target_density(80, 20, 0.8, true), 44);
    }

    #[test]
    fn all_themes_share_the_same_motion_model() {
        let green = MatrixRain::new(MatrixTheme::Green);
        let blue = MatrixRain::new(MatrixTheme::Blue);
        let red = MatrixRain::new(MatrixTheme::Red);
        assert_eq!(green.smoothed_fall, blue.smoothed_fall);
        assert_eq!(blue.smoothed_fall, red.smoothed_fall);
    }
}
