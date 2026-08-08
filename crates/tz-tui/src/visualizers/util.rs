//! Shared helpers for terminal visualizers (canvas, energy, glyphs).

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::host::{heat_color, VisualizerFrameInput};

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Reset,
        }
    }
}

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    cells: Vec<Cell>,
}

/// Map known double-width / ambiguous glyphs to single-column stand-ins so
/// every canvas cell stays one terminal column (prevents row wrap / vertical jump).
fn ascii_cell(ch: char) -> char {
    match ch {
        // Explicitly wide or often double-width in common terminals
        '◉' | '◎' | '○' | '◐' | '◑' | '◒' | '◓' => 'O',
        '█' | '▓' | '▒' | '░' | '▌' | '▐' | '▀' | '▄' => '#',
        '═' | '║' => '=',
        // Keep ASCII and common single-column punctuation/symbols as-is.
        c => c,
    }
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
        }
    }

    pub fn set(&mut self, x: i32, y: i32, ch: char, fg: Color) {
        if x < 0 || y < 0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if x < self.width && y < self.height {
            // Force single-column glyphs so row display width == canvas width
            // (wide Unicode makes Paragraph layout shift vertically).
            self.cells[y * self.width + x] = Cell {
                ch: ascii_cell(ch),
                fg,
            };
        }
    }

    pub fn set_if_empty(&mut self, x: i32, y: i32, ch: char, fg: Color) {
        if x < 0 || y < 0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if x < self.width && y < self.height {
            let i = y * self.width + x;
            if self.cells[i].ch == ' ' {
                self.cells[i] = Cell {
                    ch: ascii_cell(ch),
                    fg,
                };
            }
        }
    }

    pub fn into_lines(self) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.height);
        for y in 0..self.height {
            let mut spans = Vec::with_capacity(self.width);
            for x in 0..self.width {
                let c = self.cells[y * self.width + x];
                if c.ch == ' ' {
                    spans.push(Span::raw(" "));
                } else {
                    spans.push(Span::styled(c.ch.to_string(), Style::default().fg(c.fg)));
                }
            }
            lines.push(Line::from(spans));
        }
        lines
    }
}

pub fn mono_level(frame: &VisualizerFrameInput) -> f32 {
    match (frame.level_left, frame.level_right) {
        (Some(l), Some(r)) if l.is_finite() && r.is_finite() => ((l + r) * 0.5).clamp(0.0, 1.0),
        (Some(l), _) if l.is_finite() => l.clamp(0.0, 1.0),
        (_, Some(r)) if r.is_finite() => r.clamp(0.0, 1.0),
        _ if matches!(frame.status.as_str(), "playing") => {
            let t = frame.position_s * frame.speed.max(0.1);
            (0.2 + 0.35 * (0.5 + 0.5 * (t * 4.2).sin()) as f32).clamp(0.0, 1.0)
        }
        _ => 0.0,
    }
}

pub fn bass_energy(bands: Option<&[u8]>) -> f32 {
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return 0.0;
    };
    let n = (bands.len() / 4).max(1);
    let sum: u32 = bands.iter().take(n).map(|b| u32::from(*b)).sum();
    (sum as f32 / (n as f32 * 255.0)).clamp(0.0, 1.0)
}

pub fn high_energy(bands: Option<&[u8]>) -> f32 {
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return 0.0;
    };
    let start = bands.len() * 3 / 4;
    let chunk = &bands[start..];
    if chunk.is_empty() {
        return 0.0;
    }
    let sum: u32 = chunk.iter().map(|b| u32::from(*b)).sum();
    (sum as f32 / (chunk.len() as f32 * 255.0)).clamp(0.0, 1.0)
}

pub fn mid_energy(bands: Option<&[u8]>) -> f32 {
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return 0.0;
    };
    let a = bands.len() / 4;
    let b = bands.len() * 3 / 4;
    let chunk = &bands[a..b.max(a + 1).min(bands.len())];
    if chunk.is_empty() {
        return 0.0;
    }
    let sum: u32 = chunk.iter().map(|b| u32::from(*b)).sum();
    (sum as f32 / (chunk.len() as f32 * 255.0)).clamp(0.0, 1.0)
}

pub fn beat_onset(frame: &VisualizerFrameInput) -> bool {
    frame.beat_is_onset == Some(true)
}

pub fn stable_seed(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

pub fn particle_count(width: usize, height: usize, mono: f32, bass: f32) -> usize {
    let base = (width * height) / 18;
    let boost = ((mono * 0.55 + bass * 0.45) * 40.0) as usize;
    (base + boost).clamp(12, 180)
}

pub fn particle_glyph(idx: usize, high: f32, onset: bool) -> char {
    if onset {
        return ['*', '+', 'x', '#'][idx % 4];
    }
    if high > 0.55 {
        return ['·', ':', ';', '.'][idx % 4];
    }
    ['.', 'o', '*', '+'][idx % 4]
}

pub fn energy_color(level: f32, color: bool) -> Color {
    heat_color(level.clamp(0.0, 1.0), color)
}

pub fn track_label(frame: &VisualizerFrameInput) -> String {
    frame
        .title
        .clone()
        .or_else(|| {
            frame
                .track_path
                .as_ref()
                .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string())
        })
        .unwrap_or_else(|| "Unknown".into())
}

pub fn fit_lines(mut lines: Vec<Line<'static>>, height: usize) -> Vec<Line<'static>> {
    lines.truncate(height.max(1));
    lines
}

pub fn band_level(bands: Option<&[u8]>, index: usize, count: usize) -> f32 {
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return 0.0;
    };
    let count = count.max(1);
    let start = (index * bands.len()) / count;
    let mut end = ((index + 1) * bands.len()) / count;
    if end <= start {
        end = (start + 1).min(bands.len());
    }
    let chunk = bands.get(start..end.min(bands.len())).unwrap_or(&[]);
    if chunk.is_empty() {
        return 0.0;
    }
    let max = chunk.iter().copied().max().unwrap_or(0);
    f32::from(max) / 255.0
}

/// Terminal cell aspect: characters are taller than wide, so Y is scaled when
/// mapping polar coordinates into the grid.
pub const Y_ASPECT: f32 = 0.5;

/// Geometric center of a `width`×`height` character field (panel-centered).
///
/// Uses integer cell indices so the origin stays on a single stable cell
/// (no half-cell float jitter frame-to-frame).
pub fn field_center(width: usize, height: usize) -> (f32, f32) {
    ((width / 2) as f32, (height / 2) as f32)
}

/// Largest polar radius that stays inside the field (symmetric edge margins).
pub fn max_radius_for_field(width: usize, height: usize) -> f32 {
    let (cx, cy) = field_center(width, height);
    let max_x = width.saturating_sub(1) as f32;
    let max_y = height.saturating_sub(1) as f32;
    // Distance from center to nearest left/right and top/bottom edge.
    let rx = cx.min(max_x - cx).max(0.5);
    let ry = cy.min(max_y - cy).max(0.5) / Y_ASPECT;
    rx.min(ry).max(1.0)
}

/// Polar → grid with consistent Y aspect (circular figures look circular).
/// Result is clamped conceptually by callers via `max_radius_for_field`.
pub fn polar_xy(cx: f32, cy: f32, angle: f32, radius: f32) -> (i32, i32) {
    let x = (cx + angle.cos() * radius).round() as i32;
    let y = (cy + angle.sin() * radius * Y_ASPECT).round() as i32;
    (x, y)
}

/// Integer cell of the field center (same as `field_center`, for glyph pins).
pub fn field_center_i(width: usize, height: usize) -> (i32, i32) {
    ((width / 2) as i32, (height / 2) as i32)
}

pub fn draw_ring(canvas: &mut Canvas, cx: f32, cy: f32, radius: f32, ch: char, fg: Color) {
    if radius < 0.5 {
        return;
    }
    let steps = ((radius * 14.0) as i32).clamp(16, 128);
    for i in 0..steps {
        let a = std::f32::consts::TAU * (i as f32) / (steps as f32);
        let (x, y) = polar_xy(cx, cy, a, radius);
        canvas.set(x, y, ch, fg);
    }
}

pub fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Deterministic mix for per-particle noise (stable across frames).
pub fn mix_u64(seed: u64, salt: u64) -> u64 {
    let mut h = seed ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// Unit float in [0, 1) from seed+salt.
pub fn unit_noise(seed: u64, salt: u64) -> f32 {
    (mix_u64(seed, salt) as f32) / (u64::MAX as f32)
}
