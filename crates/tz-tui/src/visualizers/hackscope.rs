//! Cyberpunk ops visualizer — multi-layer movie-style hacking HUD.
//!
//! Layers run concurrently (not a flat text crawl): ICE walls, packet streams,
//! scan reticle, decrypt cascade, breach flashes — all driven by audio energy.

use ratatui::style::Color;
use ratatui::text::Line;

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};
use super::util::{
    bass_energy, beat_onset, fit_lines, high_energy, mid_energy, mix_u64, mono_level, stable_seed,
    track_label, unit_noise, Canvas,
};

/// Ops phase — cycles with time + snaps forward on beats.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Recon,
    Ice,
    Breach,
    Decrypt,
    Extract,
    Own,
}

impl Phase {
    fn from_progress(p: f32) -> Self {
        let p = p.rem_euclid(1.0);
        if p < 0.18 {
            Phase::Recon
        } else if p < 0.38 {
            Phase::Ice
        } else if p < 0.55 {
            Phase::Breach
        } else if p < 0.75 {
            Phase::Decrypt
        } else if p < 0.90 {
            Phase::Extract
        } else {
            Phase::Own
        }
    }

    fn label(self) -> &'static str {
        match self {
            Phase::Recon => "RECON",
            Phase::Ice => "ICE//WALL",
            Phase::Breach => "BREACH",
            Phase::Decrypt => "DECRYPT",
            Phase::Extract => "EXTRACT",
            Phase::Own => "OWNED",
        }
    }

    fn color(self, color: bool) -> Color {
        if !color {
            return Color::Gray;
        }
        match self {
            Phase::Recon => Color::Cyan,
            Phase::Ice => Color::Rgb(80, 160, 255),
            Phase::Breach => Color::Red,
            Phase::Decrypt => Color::Yellow,
            Phase::Extract => Color::Magenta,
            Phase::Own => Color::Green,
        }
    }
}

#[derive(Debug, Default)]
pub struct HackScopeVisualizer {
    color: bool,
}

impl VisualizerPlugin for HackScopeVisualizer {
    fn plugin_id(&self) -> &'static str {
        "ops.hackscope"
    }

    fn display_name(&self) -> &'static str {
        "HackScope (Fictional)"
    }

    fn on_activate(&mut self, context: VisualizerContext) {
        self.color = context.ansi_enabled;
    }

    fn on_deactivate(&mut self) {}

    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
        let width = frame.width.max(1) as usize;
        let height = frame.height.max(1) as usize;
        let seed = stable_seed(
            frame
                .track_path
                .as_deref()
                .or(frame.title.as_deref())
                .unwrap_or("hack"),
        );
        let title = track_label(frame);
        let mono = mono_level(frame);
        let bands = frame.spectrum_bands.as_deref();
        let bass = bass_energy(bands);
        let mid = mid_energy(bands);
        let high = high_energy(bands);
        let onset = beat_onset(frame);
        let t = frame.frame_index as f32;
        // Phase advances with time; beats punch it forward.
        let beat_boost = if onset { 0.04 } else { 0.0 };
        let progress =
            ((t * 0.0045) + mono * 0.08 + bass * 0.05 + beat_boost * (t % 17.0)).rem_euclid(1.0);
        let phase = if matches!(frame.status.as_str(), "playing" | "paused") {
            Phase::from_progress(progress)
        } else {
            Phase::Recon
        };

        let mut canvas = Canvas::new(width, height);

        // --- Layer 0: dark grid / scanlines ---
        for y in 0..height {
            if y % 3 == 0 {
                for x in 0..width {
                    if x % 6 == 0 {
                        canvas.set(x as i32, y as i32, '·', Color::DarkGray);
                    }
                }
            }
        }

        // --- Layer 1: matrix rain (background packet stream) ---
        let cols = (width / 2).max(8);
        for c in 0..cols {
            let x = (c * 2 + (seed as usize % 2)) % width.max(1);
            let speed = 0.5 + unit_noise(seed, c as u64 * 13) * 1.8 + high * 0.8;
            let head = ((t * speed + unit_noise(seed, c as u64) * height as f32) as i32)
                .rem_euclid(height as i32 + 6)
                - 2;
            let trail = 3 + (mono * 6.0) as i32;
            for d in 0..trail {
                let y = head - d;
                if y < 0 || y >= height as i32 {
                    continue;
                }
                let n = mix_u64(seed, (c as u64) << 8 | y as u64);
                let ch = b"01ABCDEF#%$"[((n >> 4) % 11) as usize] as char;
                let fg = if d == 0 {
                    if self.color {
                        Color::White
                    } else {
                        Color::Gray
                    }
                } else if self.color {
                    Color::Green
                } else {
                    Color::DarkGray
                };
                canvas.set_if_empty(x as i32, y, ch, fg);
            }
        }

        // --- Layer 2: phase-specific main action ---
        match phase {
            Phase::Recon => layer_recon(&mut canvas, width, height, t, seed, mono, self.color),
            Phase::Ice => layer_ice(&mut canvas, width, height, t, seed, bass, onset, self.color),
            Phase::Breach => {
                layer_breach(&mut canvas, width, height, t, mono, bass, onset, self.color)
            }
            Phase::Decrypt => layer_decrypt(
                &mut canvas,
                width,
                height,
                t,
                seed,
                progress,
                mid,
                self.color,
            ),
            Phase::Extract => {
                layer_extract(&mut canvas, width, height, t, progress, mono, self.color)
            }
            Phase::Own => layer_own(&mut canvas, width, height, t, onset, self.color),
        }

        // --- Layer 3: targeting reticle (center) — always present ---
        let cx = width as i32 / 2;
        let cy = height as i32 / 2;
        let r = 2 + (bass * 3.0) as i32;
        for dx in -r..=r {
            canvas.set(
                cx + dx,
                cy,
                if dx == 0 { '+' } else { '-' },
                Color::DarkGray,
            );
        }
        for dy in -r..=r {
            canvas.set(
                cx,
                cy + dy,
                if dy == 0 { '+' } else { '|' },
                Color::DarkGray,
            );
        }
        if onset {
            canvas.set(
                cx,
                cy,
                '◎',
                if self.color {
                    Color::Yellow
                } else {
                    Color::White
                },
            );
        }

        // --- Layer 4: horizontal scan beam ---
        let beam_y = ((t * (0.4 + mono * 0.8)) as i32).rem_euclid(height.max(1) as i32);
        for x in 0..width {
            canvas.set_if_empty(
                x as i32,
                beam_y,
                '=',
                if self.color {
                    Color::Rgb(0, 200, 180)
                } else {
                    Color::Gray
                },
            );
        }

        // --- Layer 5: HUD chrome (top + bottom) ---
        let stage = format!("{:08X}", seed as u32);
        let hud_top = format!(
            "HACK//SCOPE  PH:{:<8}  {:>3.0}%  E:{:.0}%",
            phase.label(),
            progress * 100.0,
            mono * 100.0
        );
        put_str(&mut canvas, 0, 0, &hud_top, width, phase.color(self.color));
        let target: String = format!("TGT>{title}").chars().take(width).collect();
        put_str(&mut canvas, 0, 1, &target, width, Color::Cyan);

        let alert = if onset {
            "// ALERT: SIGNAL SPIKE //"
        } else if bass > 0.65 {
            "// ICE PRESSURE HIGH //"
        } else {
            "// LINK STABLE //"
        };
        put_str(
            &mut canvas,
            0,
            height.saturating_sub(1) as i32,
            &format!("{alert}  id={stage}"),
            width,
            if onset && self.color {
                Color::Red
            } else {
                Color::DarkGray
            },
        );

        // Side integrity bars (left/right)
        let bar_h = ((height as f32) * (0.3 + mono * 0.6)).round() as i32;
        for y in 0..bar_h.min(height as i32) {
            let yy = height as i32 - 1 - y;
            canvas.set(
                0,
                yy,
                '▌',
                if self.color {
                    Color::Green
                } else {
                    Color::Gray
                },
            );
            canvas.set(
                width.saturating_sub(1) as i32,
                yy,
                '▐',
                if self.color {
                    Color::Magenta
                } else {
                    Color::Gray
                },
            );
        }

        fit_lines(canvas.into_lines(), height)
    }
}

fn put_str(canvas: &mut Canvas, x: i32, y: i32, s: &str, max_w: usize, fg: Color) {
    for (i, ch) in s.chars().take(max_w).enumerate() {
        canvas.set(x + i as i32, y, ch, fg);
    }
}

fn layer_recon(canvas: &mut Canvas, w: usize, h: usize, t: f32, seed: u64, mono: f32, color: bool) {
    // Expanding probe rings + node map
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    for ring in 0..4 {
        let r = ((t * 0.3 + ring as f32 * 2.5) % (w.min(h) as f32 * 0.45 + 1.0)).max(1.0);
        let steps = 24;
        for i in 0..steps {
            let a = std::f32::consts::TAU * (i as f32) / steps as f32;
            let x = (cx + a.cos() * r).round() as i32;
            let y = (cy + a.sin() * r * 0.5).round() as i32;
            canvas.set(
                x,
                y,
                if ring == 0 { 'o' } else { '.' },
                if color { Color::Cyan } else { Color::Gray },
            );
        }
    }
    // Network nodes
    let nodes = 6 + (mono * 8.0) as usize;
    for n in 0..nodes {
        let u = unit_noise(seed, n as u64 * 3);
        let v = unit_noise(seed, n as u64 * 7 + 1);
        let x = (u * (w.saturating_sub(1) as f32)).round() as i32;
        let y = (2.0 + v * (h.saturating_sub(4) as f32)).round() as i32;
        canvas.set(x, y, '*', if color { Color::Yellow } else { Color::White });
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_ice(
    canvas: &mut Canvas,
    w: usize,
    h: usize,
    t: f32,
    seed: u64,
    bass: f32,
    onset: bool,
    color: bool,
) {
    // Cracking ICE wall blocks
    let crack = if onset { 0.55 } else { 0.25 + bass * 0.35 };
    for y in 2..h.saturating_sub(1) {
        for x in 0..w {
            let n = mix_u64(seed, (y as u64) << 12 | x as u64);
            let v = (n % 1000) as f32 / 1000.0;
            let wave = ((x as f32 * 0.3 + t * 0.2).sin() * 0.5 + 0.5) * 0.2;
            if v < crack + wave {
                let ch = if v < crack * 0.4 {
                    ' '
                } else if onset {
                    '#'
                } else {
                    ['█', '▓', '▒', '░'][((n >> 3) % 4) as usize]
                };
                if ch != ' ' {
                    canvas.set(
                        x as i32,
                        y as i32,
                        ch,
                        if color {
                            Color::Rgb(100, 160, 255)
                        } else {
                            Color::Gray
                        },
                    );
                }
            }
        }
    }
    // Breach wedge
    let tip = ((t * 0.4) as usize % w.max(1)).min(w.saturating_sub(1));
    for y in 2..h.saturating_sub(1) {
        let span = ((y as f32 / h as f32) * 4.0) as i32;
        for dx in -span..=span {
            let x = tip as i32 + dx;
            if x >= 0 && x < w as i32 {
                canvas.set(
                    x,
                    y as i32,
                    if dx == 0 { '>' } else { '·' },
                    if color { Color::Yellow } else { Color::White },
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_breach(
    canvas: &mut Canvas,
    w: usize,
    h: usize,
    t: f32,
    mono: f32,
    bass: f32,
    onset: bool,
    color: bool,
) {
    // Explosion / glitch shatter from center
    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    let force = 3.0 + mono * 10.0 + bass * 6.0 + if onset { 5.0 } else { 0.0 };
    let shards = 40 + (mono * 40.0) as i32;
    for i in 0..shards {
        let a = (i as f32 * 0.7 + t * 0.1) % std::f32::consts::TAU;
        let dist = ((t * 0.5 + i as f32 * 0.3) % force).max(0.5);
        let x = cx + (a.cos() * dist).round() as i32;
        let y = cy + (a.sin() * dist * 0.5).round() as i32;
        let ch = if onset {
            ['*', '#', '+', 'x'][(i as usize) % 4]
        } else {
            ['/', '\\', '|', '-'][(i as usize) % 4]
        };
        canvas.set(
            x,
            y,
            ch,
            if color {
                if onset {
                    Color::Red
                } else {
                    Color::Rgb(255, 100, 80)
                }
            } else {
                Color::White
            },
        );
    }
    // Flash bar
    if onset {
        for x in 0..w {
            canvas.set(x as i32, cy, '═', Color::White);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_decrypt(
    canvas: &mut Canvas,
    w: usize,
    h: usize,
    t: f32,
    seed: u64,
    progress: f32,
    mid: f32,
    color: bool,
) {
    // Cascading hex that "locks" into plaintext bands
    let lock_rows = ((h as f32) * (0.3 + (progress - 0.55).max(0.0) * 2.0).min(0.7)) as usize;
    for y in 2..h.saturating_sub(1) {
        let locked = y < lock_rows + 2;
        for x in 0..w {
            let n = mix_u64(seed, y as u64 * 31 + x as u64 + (t as u64));
            let ch = if locked {
                // "Solved" blocks
                if (x + y) % 5 == 0 {
                    '█'
                } else {
                    '='
                }
            } else {
                let scramble = ((t as u64).wrapping_add(n)) % 16;
                b"0123456789ABCDEF"[scramble as usize] as char
            };
            canvas.set(
                x as i32,
                y as i32,
                ch,
                if color {
                    if locked {
                        Color::Green
                    } else if mid > 0.5 {
                        Color::Yellow
                    } else {
                        Color::Rgb(0, 220, 100)
                    }
                } else {
                    Color::Gray
                },
            );
        }
    }
}

fn layer_extract(
    canvas: &mut Canvas,
    w: usize,
    h: usize,
    t: f32,
    progress: f32,
    mono: f32,
    color: bool,
) {
    // Data packets flying to a sink on the right
    let packets = 12 + (mono * 20.0) as usize;
    let sink_x = w.saturating_sub(3) as i32;
    for i in 0..packets {
        let lane = 2 + (i % (h.saturating_sub(3).max(1)));
        let speed = 0.8 + (i % 5) as f32 * 0.25 + mono;
        let x = (((t * speed * 1.5 + i as f32 * 7.0) as i32).rem_euclid(w as i32 + 4)) - 2;
        if x >= 0 && x < w as i32 {
            let ch = if i % 3 == 0 { '#' } else { '>' };
            canvas.set(
                x,
                lane as i32,
                ch,
                if color { Color::Magenta } else { Color::White },
            );
        }
        // Trail
        if x > 0 {
            canvas.set_if_empty(x - 1, lane as i32, '-', Color::DarkGray);
        }
    }
    // Sink / vault
    for y in 2..h.saturating_sub(1) {
        canvas.set(
            sink_x,
            y as i32,
            '▌',
            if color { Color::Cyan } else { Color::Gray },
        );
        canvas.set(
            sink_x + 1,
            y as i32,
            '█',
            if color { Color::Cyan } else { Color::Gray },
        );
    }
    let fill = ((h.saturating_sub(4) as f32) * ((progress - 0.75) / 0.15).clamp(0.0, 1.0)) as i32;
    for y in 0..fill {
        let yy = h as i32 - 2 - y;
        canvas.set(
            sink_x + 1,
            yy,
            '#',
            if color { Color::Yellow } else { Color::White },
        );
    }
}

fn layer_own(canvas: &mut Canvas, w: usize, h: usize, t: f32, onset: bool, color: bool) {
    // Victory cascade + "ROOT" stamp
    for y in 2..h.saturating_sub(1) {
        for x in 0..w {
            if (x + y + (t as usize / 2)) % 7 == 0 {
                canvas.set(
                    x as i32,
                    y as i32,
                    if onset { '*' } else { '+' },
                    if color { Color::Green } else { Color::Gray },
                );
            }
        }
    }
    let stamp = if onset {
        ">> ROOT ACCESS <<"
    } else {
        ">> SYSTEM OWNED <<"
    };
    let y = h / 2;
    let x0 = w.saturating_sub(stamp.len()) / 2;
    put_str(
        canvas,
        x0 as i32,
        y as i32,
        stamp,
        w,
        if color { Color::Green } else { Color::White },
    );
}
