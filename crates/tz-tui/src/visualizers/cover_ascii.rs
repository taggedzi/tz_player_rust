//! Cover-art ASCII visualizers — render embedded album art when present.

use std::io::Cursor;
use std::path::Path;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView};
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use ratatui::style::Color;
use ratatui::text::Line;

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};
use super::util::{energy_color, fit_lines, mono_level, track_label, Canvas};

const RAMP: &[char] = &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

#[derive(Debug, Default)]
struct CoverCache {
    path: Option<String>,
    /// Cached source luminance at a moderate resolution (re-sampled per frame size).
    src_w: u32,
    src_h: u32,
    /// Row-major luma 0–255
    luma: Vec<u8>,
    /// Row-major source color corresponding to each luminance sample.
    rgb: Vec<(u8, u8, u8)>,
    has_art: bool,
}

impl CoverCache {
    fn ensure(&mut self, track_path: Option<&str>) {
        let key = track_path.map(str::to_string);
        if self.path == key {
            return;
        }
        self.path = key;
        self.luma.clear();
        self.rgb.clear();
        self.src_w = 0;
        self.src_h = 0;
        self.has_art = false;

        let Some(path) = track_path.filter(|p| !p.is_empty()) else {
            return;
        };
        if let Some(img) = load_cover_image(Path::new(path)) {
            // Downscale once to a working buffer (fast frame resampling later).
            let max_side = 160u32;
            let (w, h) = img.dimensions();
            let scale = (max_side as f32 / w.max(h).max(1) as f32).min(1.0);
            let tw = ((w as f32) * scale).round().max(1.0) as u32;
            let th = ((h as f32) * scale).round().max(1.0) as u32;
            let small = img.resize_exact(tw, th, FilterType::Triangle).to_rgb8();
            self.luma.reserve((tw * th) as usize);
            self.rgb.reserve((tw * th) as usize);
            for pixel in small.pixels() {
                let r = u64::from(pixel[0]);
                let g = u64::from(pixel[1]);
                let b = u64::from(pixel[2]);
                // Rec. 601 luma drives the glyph; original RGB drives its color.
                self.luma.push(((77 * r + 150 * g + 29 * b) / 256) as u8);
                self.rgb.push((pixel[0], pixel[1], pixel[2]));
            }
            self.src_w = tw;
            self.src_h = th;
            self.has_art = !self.luma.is_empty() && self.luma.len() == self.rgb.len();
        }
    }

    fn sample(&self, u: f32, v: f32) -> Option<(u8, (u8, u8, u8))> {
        if !self.has_art || self.src_w == 0 || self.src_h == 0 {
            return None;
        }
        let u = u.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);
        let x = ((u * (self.src_w.saturating_sub(1) as f32)).round() as u32).min(self.src_w - 1);
        let y = ((v * (self.src_h.saturating_sub(1) as f32)).round() as u32).min(self.src_h - 1);
        let i = (y * self.src_w + x) as usize;
        self.luma.get(i).copied().zip(self.rgb.get(i).copied())
    }
}

fn load_cover_image(path: &Path) -> Option<DynamicImage> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let mut pics: Vec<_> = tagged
        .tags()
        .iter()
        .flat_map(|t| t.pictures().iter())
        .collect();
    if pics.is_empty() {
        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            pics = tag.pictures().iter().collect();
        }
    }
    if pics.is_empty() {
        return None;
    }
    // Prefer CoverFront
    pics.sort_by_key(|p| match p.pic_type() {
        lofty::picture::PictureType::CoverFront => 0u8,
        lofty::picture::PictureType::Other => 1,
        _ => 2,
    });
    for pic in pics {
        let data = pic.data();
        if data.is_empty() {
            continue;
        }
        if let Ok(img) = image::load_from_memory(data) {
            return Some(img);
        }
        let reader = image::ImageReader::new(Cursor::new(data));
        if let Ok(fmt) = reader.with_guessed_format() {
            if let Ok(img) = fmt.decode() {
                return Some(img);
            }
        }
    }
    None
}

#[derive(Debug, Default)]
pub struct CoverAsciiStaticVisualizer {
    color: bool,
    cache: CoverCache,
}

impl VisualizerPlugin for CoverAsciiStaticVisualizer {
    fn plugin_id(&self) -> &'static str {
        "cover.ascii.static"
    }

    fn display_name(&self) -> &'static str {
        "Cover ASCII (Static)"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        self.cache.ensure(frame.track_path.as_deref());
        render_cover(frame, self.color, false, &self.cache)
    }
}

#[derive(Debug, Default)]
pub struct CoverAsciiMotionVisualizer {
    color: bool,
    cache: CoverCache,
}

impl VisualizerPlugin for CoverAsciiMotionVisualizer {
    fn plugin_id(&self) -> &'static str {
        "cover.ascii.motion"
    }

    fn display_name(&self) -> &'static str {
        "Cover ASCII (Motion)"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        self.cache.ensure(frame.track_path.as_deref());
        render_cover(frame, self.color, true, &self.cache)
    }
}

fn render_cover(
    frame: &VisualizerFrameInput,
    color: bool,
    motion: bool,
    cache: &CoverCache,
) -> Vec<Line<'static>> {
    let width = frame.width.max(1) as usize;
    let height = frame.height.max(1) as usize;
    let title = track_label(frame);
    let mono = mono_level(frame);
    let mut canvas = Canvas::new(width, height);

    if cache.has_art {
        // Letterbox cover into panel while preserving aspect.
        let art_aspect = cache.src_w as f32 / cache.src_h.max(1) as f32;
        let (draw_w, draw_h) = fit_cover_dimensions(width, height, art_aspect);
        let x0 = width.saturating_sub(draw_w) / 2;
        let y0 = height.saturating_sub(draw_h) / 2;

        let t = if motion {
            frame.frame_index as f32 * 0.04 + mono * 1.5
        } else {
            0.0
        };
        // Mild zoom pulse on motion / beat energy
        let zoom = if motion {
            1.0 + mono * 0.06 + (t * 0.5).sin() * 0.03
        } else {
            1.0
        };

        for dy in 0..draw_h {
            for dx in 0..draw_w {
                let mut u = (dx as f32 + 0.5) / draw_w as f32;
                let mut v = (dy as f32 + 0.5) / draw_h as f32;
                // Center zoom
                u = 0.5 + (u - 0.5) / zoom;
                v = 0.5 + (v - 0.5) / zoom;
                if motion {
                    // Soft liquid warp
                    u += (v * 6.0 + t).sin() * 0.015 * (0.5 + mono);
                    v += (u * 5.0 - t * 0.8).cos() * 0.02 * (0.5 + mono);
                }
                let Some((luma, rgb)) = cache.sample(u, v) else {
                    continue;
                };
                let mut level = f32::from(luma) / 255.0;
                if motion {
                    level = (level + 0.05 * (t + dx as f32 * 0.1).sin() * mono).clamp(0.0, 1.0);
                }
                let gi = (level * (RAMP.len() - 1) as f32).round() as usize;
                let ch = RAMP[gi.min(RAMP.len() - 1)];
                if ch == ' ' {
                    continue;
                }
                let fg = if color {
                    cover_pixel_color(rgb)
                } else {
                    energy_color(level, false)
                };
                canvas.set((x0 + dx) as i32, (y0 + dy) as i32, ch, fg);
            }
        }

        // Title strip overlaid at bottom of art
        if draw_h > 1 {
            let label: String = title
                .chars()
                .take(draw_w.saturating_sub(2).max(1))
                .collect();
            let lx = x0 + draw_w.saturating_sub(label.chars().count()) / 2;
            let ly = y0 + draw_h - 1;
            for (i, ch) in label.chars().enumerate() {
                canvas.set(
                    (lx + i) as i32,
                    ly as i32,
                    ch,
                    if color { Color::White } else { Color::Gray },
                );
            }
        }
    } else {
        // Explicit empty state — not a fake fingerprint.
        let msg1 = "NO EMBEDDED COVER ART";
        let msg2 = title
            .chars()
            .take(width.saturating_sub(2))
            .collect::<String>();
        let msg3 = frame
            .track_path
            .as_deref()
            .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p))
            .unwrap_or("");
        let mid_y = height / 2;
        center_text(
            &mut canvas,
            width,
            mid_y.saturating_sub(1),
            msg1,
            Color::Yellow,
        );
        center_text(&mut canvas, width, mid_y, &msg2, Color::Cyan);
        if !msg3.is_empty() && height > 3 {
            let short: String = msg3.chars().take(width.saturating_sub(2)).collect();
            center_text(&mut canvas, width, mid_y + 1, &short, Color::DarkGray);
        }
        // Dim border
        for x in 0..width {
            canvas.set(x as i32, 0, '·', Color::DarkGray);
            canvas.set(x as i32, (height - 1) as i32, '·', Color::DarkGray);
        }
        for y in 0..height {
            canvas.set(0, y as i32, '·', Color::DarkGray);
            canvas.set((width - 1) as i32, y as i32, '·', Color::DarkGray);
        }
    }

    fit_lines(canvas.into_lines(), height)
}

fn fit_cover_dimensions(width: usize, height: usize, art_aspect: f32) -> (usize, usize) {
    // A terminal cell is approximately half as wide as it is tall. Therefore a
    // square image needs about two character columns per row to look square.
    let cell_width_over_height = 0.5f32;
    let art_aspect = art_aspect.max(0.01);
    let panel_aspect = width as f32 * cell_width_over_height / height.max(1) as f32;
    if art_aspect > panel_aspect {
        let draw_w = width.max(1);
        let draw_h = (draw_w as f32 * cell_width_over_height / art_aspect)
            .round()
            .max(1.0) as usize;
        (draw_w, draw_h.min(height.max(1)))
    } else {
        let draw_h = height.max(1);
        let draw_w = (draw_h as f32 * art_aspect / cell_width_over_height)
            .round()
            .max(1.0) as usize;
        (draw_w.min(width.max(1)), draw_h)
    }
}

fn center_text(canvas: &mut Canvas, width: usize, y: usize, text: &str, fg: Color) {
    let chars: Vec<char> = text.chars().take(width).collect();
    let x0 = width.saturating_sub(chars.len()) / 2;
    for (i, ch) in chars.into_iter().enumerate() {
        canvas.set((x0 + i) as i32, y as i32, ch, fg);
    }
}

fn cover_pixel_color(rgb: (u8, u8, u8)) -> Color {
    // Preserve the source palette. A tiny floor prevents saturated dark colors
    // from disappearing completely against a black terminal background.
    Color::Rgb(rgb.0.max(8), rgb.1.max(8), rgb.2.max(8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_cover_uses_two_columns_per_terminal_row() {
        assert_eq!(fit_cover_dimensions(60, 20, 1.0), (40, 20));
        assert_eq!(fit_cover_dimensions(30, 20, 1.0), (30, 15));
    }

    #[test]
    fn cover_fit_never_exceeds_the_panel() {
        for aspect in [0.5, 1.0, 1.5, 2.0] {
            let (width, height) = fit_cover_dimensions(57, 19, aspect);
            assert!((1..=57).contains(&width));
            assert!((1..=19).contains(&height));
        }
    }

    #[test]
    fn cover_characters_preserve_source_pixel_color() {
        assert_eq!(cover_pixel_color((220, 35, 90)), Color::Rgb(220, 35, 90));
        assert_eq!(cover_pixel_color((0, 4, 7)), Color::Rgb(8, 8, 8));
    }
}
