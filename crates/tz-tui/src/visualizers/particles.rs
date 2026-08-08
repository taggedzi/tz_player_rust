//! Particle-field visualizers (Python advanced pack parity).
//!
//! Centered styles keep a **fixed geometric origin and outer envelope**.
//! Music drives speed, density, brightness, and local jitter — not diameter bounce.

use ratatui::style::Color;
use ratatui::text::Line;

use super::host::{VisualizerContext, VisualizerFrameInput, VisualizerPlugin};
use super::util::{
    band_level, bass_energy, beat_onset, draw_ring, energy_color, field_center, field_center_i,
    fit_lines, high_energy, max_radius_for_field, mid_energy, mix_u64, mono_level, particle_count,
    particle_glyph, polar_xy, stable_seed, unit_noise, Canvas,
};

#[derive(Clone, Copy)]
enum ParticleStyle {
    Reactor,
    GravityWell,
    Shockwave,
    Rain,
    Orbital,
    Ember,
    Magnetic,
    Tornado,
    Constellation,
    DataCore,
    Plasma,
}

fn render_particles(
    style: ParticleStyle,
    title: &str,
    color: bool,
    frame: &VisualizerFrameInput,
) -> Vec<Line<'static>> {
    let width = frame.width.max(1) as usize;
    let height = frame.height.max(1) as usize;
    let rows = height.max(1);
    let mono = mono_level(frame);
    let bands = frame.spectrum_bands.as_deref();
    let bass = bass_energy(bands);
    let high = high_energy(bands);
    let mid = mid_energy(bands);
    let onset = beat_onset(frame);
    let seed = stable_seed(
        frame
            .track_path
            .as_deref()
            .or(frame.title.as_deref())
            .unwrap_or(title),
    );
    // Faithful ports of the Python advanced-pack plugins the user tuned.
    match style {
        ParticleStyle::Reactor => return render_reactor_py(frame, color, seed),
        ParticleStyle::GravityWell => return render_gravity_well_py(frame, color, seed),
        ParticleStyle::Orbital => return render_orbital_py(frame, color, seed),
        ParticleStyle::Constellation => return render_constellation(frame, color, seed),
        _ => {}
    }

    let count = particle_count(width, rows, mono, bass);
    let (cx, cy) = field_center(width, rows);
    let (core_x, core_y) = field_center_i(width, rows);
    // Fixed envelope — never scaled by energy (steady-state diameter).
    let max_r = max_radius_for_field(width, rows);
    let t = frame.frame_index as f32;
    let mut canvas = Canvas::new(width, rows);

    match style {
        ParticleStyle::Reactor
        | ParticleStyle::GravityWell
        | ParticleStyle::Orbital
        | ParticleStyle::Constellation => unreachable!("handled above"),
        ParticleStyle::Shockwave => {
            // Expanding rings fill the pane (no static outer guide ring).
            const RINGS: usize = 5;
            for r_idx in 0..RINGS {
                let phase = (t * (0.28 + bass * 0.25) + r_idx as f32 * (max_r / RINGS as f32))
                    .rem_euclid(max_r);
                let ch = if onset && r_idx == 0 {
                    '#'
                } else if r_idx % 2 == 0 {
                    'o'
                } else {
                    '.'
                };
                let heat = 0.25 + (1.0 - phase / max_r) * 0.55 + mono * 0.2;
                draw_ring(
                    &mut canvas,
                    cx,
                    cy,
                    phase.max(0.5),
                    ch,
                    energy_color(heat.clamp(0.0, 1.0), color),
                );
            }
            for idx in 0..(count / 3).max(8) {
                let a = (idx as f32 * 0.7 + t * 0.04).to_radians();
                let r = max_r * (0.1 + unit_noise(seed, idx as u64) * 0.88);
                let (px, py) = polar_xy(cx, cy, a, r);
                canvas.set(px, py, '.', energy_color(high * 0.6 + 0.2, color));
            }
            canvas.set(core_x, core_y, '*', Color::Yellow);
        }
        ParticleStyle::Rain => {
            // Organic rain: independent drops, random x/speed/length, mild wind, ground splash.
            let density = (count as f32 * 1.4).round() as usize + 24;
            let wind = (t * 0.03).sin() * (0.15 + high * 0.25);
            for idx in 0..density {
                let drop_seed = mix_u64(seed, idx as u64 * 0x9e37 + 11);
                let x0 = unit_noise(seed, drop_seed) * width as f32;
                let speed = 0.35 + unit_noise(seed, drop_seed ^ 0x55) * 1.4 + bass * 0.9;
                let length = 1 + (unit_noise(seed, drop_seed ^ 0xaa) * 3.0) as i32;
                // Stagger start so drops don't share a phase lattice.
                let phase = unit_noise(seed, drop_seed ^ 0x33) * rows as f32 * 3.0;
                let fall = (t * speed + phase).rem_euclid((rows as f32) + 4.0) - 2.0;
                let y = fall.round() as i32;
                let x = (x0 + wind * fall * 0.15 + (drop_seed % 3) as f32 * 0.1).round() as i32;
                if y < 0 || y >= rows as i32 {
                    // Splash near ground when leaving bottom
                    if y >= rows as i32 && y < rows as i32 + 2 && mono > 0.15 {
                        canvas.set_if_empty(x, rows as i32 - 1, '.', Color::DarkGray);
                        canvas.set_if_empty(x - 1, rows as i32 - 1, '.', Color::DarkGray);
                        canvas.set_if_empty(x + 1, rows as i32 - 1, '.', Color::DarkGray);
                    }
                    continue;
                }
                let ch = if (onset && high > 0.45) || speed > 1.2 {
                    '|'
                } else if speed > 0.8 {
                    ':'
                } else {
                    '.'
                };
                let heat = (0.2 + mono * 0.5 + bass * 0.2).clamp(0.15, 1.0);
                canvas.set(x, y, ch, energy_color(heat, color));
                for d in 1..=length {
                    canvas.set_if_empty(x, y - d, '\'', Color::DarkGray);
                }
            }
        }
        ParticleStyle::Ember => {
            // Burning log: solid base coals + rising flames driven by spectrum bands.
            let log_y = rows.saturating_sub(2) as i32;
            let log_h = 2i32.min(rows as i32);
            // Log / coals
            for dy in 0..log_h {
                for x in 0..width {
                    let n = unit_noise(seed, (x as u64) + dy as u64 * 99);
                    let ch = if n > 0.7 {
                        '#'
                    } else if n > 0.4 {
                        '='
                    } else {
                        '-'
                    };
                    let heat = 0.35 + n * 0.25 + bass * 0.3;
                    canvas.set(
                        x as i32,
                        log_y - dy,
                        ch,
                        energy_color(heat.clamp(0.0, 1.0), color),
                    );
                }
            }
            // Flame columns from band energy
            let cols = width.max(8);
            let flame_max = (rows.saturating_sub(3) as f32).max(2.0);
            for x in 0..cols {
                let level = if bands.is_some() {
                    band_level(bands, x, cols)
                } else {
                    mono * (0.5 + unit_noise(seed, x as u64) * 0.5)
                };
                let height_f = flame_max * (0.15 + level * 0.85 + if onset { 0.12 } else { 0.0 });
                let h = height_f.round() as i32;
                // Slight horizontal flicker
                let flicker = ((t * (0.4 + level) + x as f32 * 0.7).sin() * (0.4 + high * 0.6))
                    .round() as i32;
                let px = (x as i32 + flicker).clamp(0, width.saturating_sub(1) as i32);
                for dy in 0..h {
                    let y = log_y - 1 - dy;
                    if y < 0 {
                        break;
                    }
                    let frac = dy as f32 / h.max(1) as f32;
                    // Hot base → cooler tips
                    let heat = (1.0 - frac * 0.85 + bass * 0.15).clamp(0.15, 1.0);
                    let ch = if frac < 0.2 {
                        if onset {
                            '*'
                        } else {
                            '#'
                        }
                    } else if frac < 0.45 {
                        '%'
                    } else if frac < 0.7 {
                        '+'
                    } else if frac < 0.88 {
                        ':'
                    } else {
                        '.'
                    };
                    // Tips lean with high energy
                    let tip_x = if frac > 0.6 {
                        px + ((t * 0.5 + x as f32).sin() * high * 1.5).round() as i32
                    } else {
                        px
                    };
                    canvas.set(
                        tip_x.clamp(0, width.saturating_sub(1) as i32),
                        y,
                        ch,
                        if color {
                            flame_color(heat)
                        } else {
                            energy_color(heat, false)
                        },
                    );
                }
            }
            // Rising sparks
            let sparks = (8.0 + mono * 24.0 + if onset { 12.0 } else { 0.0 }) as usize;
            for idx in 0..sparks {
                let u = unit_noise(seed, idx as u64 * 13 + frame.frame_index / 3);
                let x = (u * width as f32).round() as i32;
                let rise = 0.4 + unit_noise(seed, idx as u64 + 99) * 1.2 + mono;
                let y = log_y
                    - 1
                    - ((t * rise + idx as f32 * 2.1).rem_euclid(flame_max + 2.0)).round() as i32;
                if y >= 0 && y < log_y {
                    canvas.set_if_empty(
                        x,
                        y,
                        if onset { '*' } else { '.' },
                        if color {
                            Color::Rgb(255, 220, 120)
                        } else {
                            Color::White
                        },
                    );
                }
            }
        }
        ParticleStyle::Magnetic => {
            // Dense field: every row, every ~2 cols — no empty "newline" gaps.
            let step_x = if width > 60 { 2 } else { 1 };
            for y in 0..rows {
                for x in (0..width).step_by(step_x) {
                    let nx = x as f32 / width.max(1) as f32;
                    let ny = y as f32 / rows.max(1) as f32;
                    let field = (nx * 7.0 + t * 0.09).sin()
                        + (ny * 5.5 + bass * 2.8).cos()
                        + (nx * ny * 4.0 + mono).sin() * 0.5;
                    let f = field * 0.35;
                    let ch = if f.abs() > 0.45 {
                        if f > 0.0 {
                            '/'
                        } else {
                            '\\'
                        }
                    } else if f.abs() > 0.2 {
                        if f > 0.0 {
                            ')'
                        } else {
                            '('
                        }
                    } else {
                        '-'
                    };
                    canvas.set(
                        x as i32,
                        y as i32,
                        if onset && f.abs() > 0.3 { '+' } else { ch },
                        energy_color(f.abs().clamp(0.15, 1.0), color),
                    );
                }
            }
            // Flow particles along field center ring
            for idx in 0..(count / 4).max(6) {
                let a = (t * 0.08 + idx as f32 * 0.9).to_radians();
                let r = max_r * (0.35 + mid * 0.25);
                let (px, py) = polar_xy(cx, cy, a, r);
                canvas.set(px, py, 'o', if color { Color::Cyan } else { Color::White });
            }
        }
        ParticleStyle::Tornado => {
            for idx in 0..count {
                let layer = (idx % 8) as f32 / 8.0;
                let y = (layer * (rows.saturating_sub(1) as f32)).round() as i32;
                let twist = t * (0.8 + bass) + layer * 8.0 + idx as f32 * 0.3;
                let radius = max_r * (0.15 + layer * 0.7) * (0.75 + mono * 0.25);
                let x = (cx + twist.cos() * radius).round() as i32;
                canvas.set(
                    x,
                    y,
                    particle_glyph(idx, high, onset),
                    energy_color(0.3 + layer * 0.5, color),
                );
                canvas.set_if_empty(
                    (cx + (twist + 1.2).cos() * radius * 0.7).round() as i32,
                    y,
                    '.',
                    Color::DarkGray,
                );
            }
        }
        ParticleStyle::DataCore => {
            // Cyberpunk defrag: fragmented sector blocks that migrate into sorted lanes.
            render_data_core(
                &mut canvas,
                width,
                rows,
                seed,
                t,
                mono,
                bass,
                mid,
                onset,
                color,
                cx,
                cy,
            );
        }
        ParticleStyle::Plasma => {
            for y in 0..rows {
                for x in 0..width {
                    let nx = x as f32 / width as f32;
                    let ny = y as f32 / rows as f32;
                    let v = ((nx * 8.0 + t * 0.12).sin()
                        + (ny * 6.0 - t * 0.09).cos()
                        + (nx + ny + bass).sin() * 0.7
                        + mono * 0.5)
                        * 0.35
                        + 0.5;
                    if v < 0.42 {
                        continue;
                    }
                    let ch = if v > 0.85 {
                        '#'
                    } else if v > 0.7 {
                        '='
                    } else if v > 0.55 {
                        '-'
                    } else {
                        '.'
                    };
                    canvas.set(
                        x as i32,
                        y as i32,
                        if onset && v > 0.6 { '*' } else { ch },
                        energy_color(v.clamp(0.0, 1.0), color),
                    );
                }
            }
        }
    }

    fit_lines(canvas.into_lines(), height)
}

// ---------------------------------------------------------------------------
// Python parity ports (reactor.py / gravity_well.py / orbital_system.py)
// ---------------------------------------------------------------------------

/// Python max_radius: `max(1, min(width, field_rows*2) / 2)`.
fn py_max_radius(width: usize, field_rows: usize) -> f32 {
    let w = width as f32;
    let h2 = (field_rows * 2) as f32;
    (w.min(h2) / 2.0).max(1.0)
}

/// Python mono: average of available levels, else volume (0..1).
fn py_mono_level(frame: &VisualizerFrameInput) -> f32 {
    match (frame.level_left, frame.level_right) {
        (Some(l), Some(r)) if l >= 0.0 && r >= 0.0 => ((l + r) * 0.5).clamp(0.0, 1.0),
        (Some(l), _) if l >= 0.0 => l.clamp(0.0, 1.0),
        (_, Some(r)) if r >= 0.0 => r.clamp(0.0, 1.0),
        _ => (frame.volume as f32).clamp(0.0, 1.0),
    }
}

fn py_bass_energy(bands: Option<&[u8]>) -> f32 {
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return 0.0;
    };
    let size = (bands.len() / 4).max(1);
    let sum: u32 = bands.iter().take(size).map(|b| u32::from(*b)).sum();
    sum as f32 / (size as f32 * 255.0)
}

fn py_high_energy(bands: Option<&[u8]>, start_frac: f32) -> f32 {
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return 0.0;
    };
    let start = ((bands.len() as f32) * start_frac) as usize;
    let start = start.min(bands.len().saturating_sub(1));
    let chunk = &bands[start..];
    if chunk.is_empty() {
        return 0.0;
    }
    let sum: u32 = chunk.iter().map(|b| u32::from(*b)).sum();
    sum as f32 / (chunk.len() as f32 * 255.0)
}

fn py_center(width: usize, rows: usize) -> (f32, f32) {
    (
        (width.saturating_sub(1) as f32) / 2.0,
        (rows.saturating_sub(1) as f32) / 2.0,
    )
}

fn py_polar(cx: f32, cy: f32, angle_rad: f32, radius: f32, y_scale: f32) -> (i32, i32) {
    let px = (cx + angle_rad.cos() * radius).round() as i32;
    let py = (cy + angle_rad.sin() * radius * y_scale).round() as i32;
    (px, py)
}

fn header_lines(title: &str, status: String, color: bool, accent: Color) -> Vec<Line<'static>> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;
    vec![
        Line::from(Span::styled(
            title.to_string(),
            Style::default()
                .fg(if color { accent } else { Color::White })
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            status,
            Style::default().fg(if color {
                Color::Rgb(160, 200, 255)
            } else {
                Color::Gray
            }),
        )),
    ]
}

/// Port of Python `reactor.py`.
fn render_reactor_py(frame: &VisualizerFrameInput, color: bool, seed: u64) -> Vec<Line<'static>> {
    let width = frame.width.max(1) as usize;
    let height = frame.height.max(1) as usize;
    let field_rows = height.saturating_sub(2).max(1);
    let mono = py_mono_level(frame);
    let bass = py_bass_energy(frame.spectrum_bands.as_deref());
    let high = py_high_energy(frame.spectrum_bands.as_deref(), 0.75);
    let beat = beat_onset(frame);
    let fi = frame.frame_index as f32;

    // count = budget * (0.20 + mono*0.55 + bass*0.25), budget = clamp(area//3, 20, 320)
    let area = (width * field_rows).max(1);
    let budget = (area / 3).clamp(20, 320);
    let intensity = 0.20 + mono * 0.55 + bass * 0.25;
    let count = ((budget as f32) * intensity).round().clamp(8.0, 320.0) as usize;

    let (cx, cy) = py_center(width, field_rows);
    let max_radius = py_max_radius(width, field_rows);
    let pulse_scale = if beat { 1.8 } else { 1.0 };
    let angular_speed = 0.8 + bass * 2.5;

    let mut canvas = Canvas::new(width, field_rows);
    for idx in 0..count {
        let angle_deg =
            ((seed % 360) as f32 + idx as f32 * 137.5 + fi * angular_speed).rem_euclid(360.0);
        let angle = angle_deg.to_radians();
        let speed = 0.45 + (((idx * 17) % 101) as f32 / 100.0) * (1.0 + bass);
        let mut radius =
            ((fi * speed * 0.22) + ((idx % 11) as f32 * 0.31)).rem_euclid(max_radius) * pulse_scale;
        radius = radius.min(max_radius);
        let (px, py) = py_polar(cx, cy, angle, radius, 0.55);
        if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < field_rows {
            let glyph = reactor_glyph(idx, high, beat);
            canvas.set(px, py, glyph, reactor_color(glyph, beat, color));
        }
    }
    let core = if beat { '@' } else { 'O' };
    canvas.set(
        cx.round() as i32,
        cy.round() as i32,
        core,
        if color {
            if beat {
                Color::Rgb(255, 90, 54)
            } else {
                Color::Rgb(160, 220, 255)
            }
        } else {
            Color::White
        },
    );

    let beat_s = if beat { "ONSET" } else { "IDLE" };
    let mut lines = header_lines(
        "PARTICLE REACTOR",
        format!(
            "BEAT {beat_s} | RMS {:3}% | BASS {:3}% | HIGH {:3}%",
            (mono * 100.0).round() as i32,
            (bass * 100.0).round() as i32,
            (high * 100.0).round() as i32
        ),
        color,
        Color::Rgb(242, 201, 76),
    );
    lines.extend(canvas.into_lines());
    fit_lines(lines, height)
}

fn reactor_glyph(idx: usize, high: f32, beat: bool) -> char {
    let glyphs = if beat {
        ".*+o#@"
    } else if high >= 0.65 {
        ".:*+o#"
    } else {
        ".:*+o"
    };
    glyphs.as_bytes()[idx % glyphs.len()] as char
}

fn reactor_color(glyph: char, beat: bool, color: bool) -> Color {
    if !color {
        return Color::Gray;
    }
    match glyph {
        '.' | ':' => Color::Rgb(53, 230, 138),
        '*' | '+' | 'o' => Color::Rgb(242, 201, 76),
        '@' | '#' if beat => Color::Rgb(255, 90, 54),
        _ => Color::Rgb(160, 220, 255),
    }
}

/// Port of Python `gravity_well.py`.
fn render_gravity_well_py(
    frame: &VisualizerFrameInput,
    color: bool,
    seed: u64,
) -> Vec<Line<'static>> {
    let width = frame.width.max(1) as usize;
    let height = frame.height.max(1) as usize;
    let field_rows = height.saturating_sub(2).max(1);
    let rms = py_mono_level(frame);
    let bass = py_bass_energy(frame.spectrum_bands.as_deref());
    let highs = py_high_energy(frame.spectrum_bands.as_deref(), 0.72);
    let beat = beat_onset(frame);
    let fi = frame.frame_index as f32;

    // base_particles: budget = clamp(area//4, 40, 220); count = budget * (0.25 + rms*0.75)
    let area = (width * field_rows).max(1);
    let budget = (area / 4).clamp(40, 220);
    let base_particles = ((budget as f32) * (0.25 + rms * 0.75))
        .round()
        .clamp(20.0, 220.0) as usize;

    let (cx, cy) = py_center(width, field_rows);
    let max_radius = py_max_radius(width, field_rows);
    let gravity_strength = 0.35 + bass * 0.95;

    let mut canvas = Canvas::new(width, field_rows);
    for idx in 0..base_particles {
        let orbit = 0.18 + (((idx * 23) % 97) as f32 / 100.0);
        let angle_deg =
            ((seed % 360) as f32 + idx as f32 * 137.5 + fi * (0.7 + orbit * 1.2)).rem_euclid(360.0);
        let angle = angle_deg.to_radians();
        let distance_phase = fi * 0.12 * orbit + idx as f32 * 0.19;
        let collapse = (1.0 - gravity_strength * 0.55).max(0.12);
        let mut radius = ((distance_phase.sin() + 1.0) * 0.5) * max_radius * collapse;
        if beat {
            radius = (radius + max_radius * (0.28 + bass * 0.20)).min(max_radius);
        }
        let (px, py) = py_polar(cx, cy, angle, radius, 0.56);
        if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < field_rows {
            let glyph = gravity_glyph(idx, highs, beat);
            canvas.set(px, py, glyph, gravity_color(glyph, beat, color));
        }
    }
    if beat {
        let ring_r = max_radius * (0.44 + bass * 0.22);
        let samples = ((ring_r * 10.0) as i32).max(32);
        for i in 0..samples {
            let a = std::f32::consts::TAU * (i as f32) / (samples as f32);
            let (px, py) = py_polar(cx, cy, a, ring_r, 0.56);
            canvas.set_if_empty(
                px,
                py,
                'o',
                if color {
                    Color::Rgb(255, 92, 92)
                } else {
                    Color::White
                },
            );
        }
    }
    // Core: @ on onset, O otherwise (◉ in Python; single-column for terminal safety).
    canvas.set(
        cx.round() as i32,
        cy.round() as i32,
        if beat { '@' } else { 'O' },
        if color {
            if beat {
                Color::Rgb(255, 248, 140)
            } else {
                Color::Rgb(210, 230, 255)
            }
        } else {
            Color::White
        },
    );

    let mode = if beat { "BURST" } else { "COLLAPSE" };
    let mut lines = header_lines(
        "GRAVITY WELL REACTOR",
        format!(
            "{mode} | RMS {:3}% | BASS {:3}% | HIGH {:3}%",
            (rms * 100.0).round() as i32,
            (bass * 100.0).round() as i32,
            (highs * 100.0).round() as i32
        ),
        color,
        Color::Rgb(64, 216, 255),
    );
    lines.extend(canvas.into_lines());
    fit_lines(lines, height)
}

fn gravity_glyph(idx: usize, highs: f32, beat: bool) -> char {
    let glyphs = if beat {
        "·:*+xX#"
    } else if highs >= 0.60 {
        ".:*+xX#"
    } else {
        ".:*+xX"
    };
    // · may pass through; canvas keeps single-column common dots.
    glyphs
        .chars()
        .nth(idx % glyphs.chars().count())
        .unwrap_or('.')
}

fn gravity_color(glyph: char, beat: bool, color: bool) -> Color {
    if !color {
        return Color::Gray;
    }
    match glyph {
        '.' | '·' | ':' => Color::Rgb(64, 216, 255),
        '*' | '+' | 'x' => Color::Rgb(255, 188, 66),
        'X' | '#' | 'o' => Color::Rgb(255, 92, 92),
        _ if beat => Color::Rgb(255, 248, 140),
        _ => Color::Rgb(210, 230, 255),
    }
}

/// Port of Python `orbital_system.py`.
fn render_orbital_py(frame: &VisualizerFrameInput, color: bool, seed: u64) -> Vec<Line<'static>> {
    let width = frame.width.max(1) as usize;
    let height = frame.height.max(1) as usize;
    let field_rows = height.saturating_sub(2).max(1);
    let rms = py_mono_level(frame);
    let (bass, mids, highs) = band_triplet(frame.spectrum_bands.as_deref());
    let beat = beat_onset(frame);
    let fi = frame.frame_index as f32;

    let (cx, cy) = py_center(width, field_rows);
    let max_radius = py_max_radius(width, field_rows).max(2.0);
    let beat_boost = if beat { 1.35 } else { 1.0 };

    // Three orbits: bass / mid / high with distinct glyphs & energy-scaled radius.
    let orbit_specs: [(f32, f32, char, Color); 3] = [
        (
            (max_radius * 0.28).max(1.8),
            bass,
            'o', // Python ●
            Color::Rgb(255, 164, 86),
        ),
        (
            (max_radius * 0.48).max(2.2),
            mids,
            '+', // Python ◆
            Color::Rgb(90, 210, 255),
        ),
        (
            (max_radius * 0.72).max(2.6),
            highs,
            '#', // Python ■
            Color::Rgb(170, 255, 132),
        ),
    ];

    let mut canvas = Canvas::new(width, field_rows);
    for (orbit_idx, &(base_radius, energy, glyph, fg)) in orbit_specs.iter().enumerate() {
        let radius = (base_radius * (0.88 + energy * 0.34)).min(max_radius);
        // count = clamp(base + dynamic, 10, 120); base = radius*3.2+8; dyn = energy*16 + rms*10
        let base = (radius * 3.2 + 8.0).round() as i32;
        let dynamic = (energy * 16.0 + rms * 10.0).round() as i32;
        let count = (base + dynamic).clamp(10, 120) as usize;
        let velocity = (0.45 + energy * 1.45 + orbit_idx as f32 * 0.18) * beat_boost;
        for particle_idx in 0..count {
            let angle_deg = ((seed % 360) as f32
                + particle_idx as f32 * (360.0 / count.max(1) as f32)
                + fi * velocity
                + orbit_idx as f32 * 57.0)
                .rem_euclid(360.0);
            let angle = angle_deg.to_radians();
            let (px, py) = py_polar(cx, cy, angle, radius, 0.56);
            if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < field_rows {
                canvas.set(px, py, glyph, if color { fg } else { Color::Gray });
            }
        }
    }
    canvas.set(
        cx.round() as i32,
        cy.round() as i32,
        if beat { '@' } else { 'O' },
        if color {
            if beat {
                Color::Rgb(255, 238, 128)
            } else {
                Color::Rgb(220, 232, 255)
            }
        } else {
            Color::White
        },
    );

    let mode = if beat { "PULSE" } else { "STABLE" };
    let mut lines = header_lines(
        "ORBITAL AUDIO SYSTEM",
        format!(
            "{mode} | BASS {:3}% | MID {:3}% | HIGH {:3}%",
            (bass * 100.0).round() as i32,
            (mids * 100.0).round() as i32,
            (highs * 100.0).round() as i32
        ),
        color,
        Color::Rgb(90, 210, 255),
    );
    lines.extend(canvas.into_lines());
    fit_lines(lines, height)
}

/// Port of Python `constellation.py` — star links that pulse with spectrum/beat.
fn render_constellation(
    frame: &VisualizerFrameInput,
    color: bool,
    seed: u64,
) -> Vec<Line<'static>> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;

    let width = frame.width.max(1) as usize;
    let height = frame.height.max(1) as usize;
    // Original reserves 2 rows for title + status.
    let field_rows = height.saturating_sub(2).max(1);
    let (bass, mids, highs) = band_triplet(frame.spectrum_bands.as_deref());
    let beat_onset = beat_onset(frame);
    let frame_index = frame.frame_index;

    // star_count = 18 + round(mids*36 + highs*22) [+8 onset], clamp 14..90
    let mut star_count = 18 + ((mids * 36.0) + (highs * 22.0)).round() as usize;
    if beat_onset {
        star_count += 8;
    }
    let star_count = star_count.clamp(14, 90);

    let stars = constellation_stars(width, field_rows, frame_index, seed, star_count);
    let max_link_dist = 5 + (bass * 8.0).round() as i32;
    let link_stride = (7.0 - (highs * 3.0)).round().max(2.0) as usize;

    let link_ch = if beat_onset {
        '='
    } else if bass > 0.45 {
        '-'
    } else {
        '.'
    };
    let link_fg = if color {
        if beat_onset {
            Color::Rgb(255, 148, 148)
        } else {
            Color::Rgb(110, 172, 255)
        }
    } else {
        Color::Gray
    };

    let star_ch = if beat_onset {
        '*' // Python ✶ — single-column stand-in
    } else if highs > 0.55 {
        '+' // Python ✦
    } else {
        '*'
    };
    let star_fg = if color {
        if beat_onset {
            Color::Rgb(255, 238, 152)
        } else {
            Color::Rgb(184, 236, 255)
        }
    } else {
        Color::White
    };

    let mut canvas = Canvas::new(width, field_rows);

    // Links first (only empty cells), then stars on top — same order as Python.
    for (idx, &(x1, y1)) in stars.iter().enumerate() {
        if idx % link_stride != 0 {
            continue;
        }
        if let Some((x2, y2)) = nearest_star(idx, &stars, max_link_dist) {
            draw_bresenham(&mut canvas, x1, y1, x2, y2, link_ch, link_fg);
        }
    }
    for &(x, y) in &stars {
        canvas.set(x, y, star_ch, star_fg);
    }

    let mode = if beat_onset {
        "CLUSTER BURST"
    } else {
        "STAR LINK"
    };
    let mut lines = vec![
        Line::from(Span::styled(
            "CONSTELLATION MODE",
            Style::default()
                .fg(if color {
                    Color::Rgb(184, 236, 255)
                } else {
                    Color::White
                })
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "{mode} | BASS {:3}% | MID {:3}% | HIGH {:3}%",
                (bass * 100.0).round() as i32,
                (mids * 100.0).round() as i32,
                (highs * 100.0).round() as i32
            ),
            Style::default().fg(if color {
                Color::Rgb(110, 172, 255)
            } else {
                Color::Gray
            }),
        )),
    ];
    lines.extend(canvas.into_lines());
    fit_lines(lines, height)
}

/// Equal-third bass/mid/high split — matches Python `_band_triplet`.
fn band_triplet(bands: Option<&[u8]>) -> (f32, f32, f32) {
    let Some(bands) = bands.filter(|b| !b.is_empty()) else {
        return (0.0, 0.0, 0.0);
    };
    let size = bands.len();
    let third = (size / 3).max(1);
    let bass = &bands[..third.min(size)];
    let mids = if third * 2 <= size {
        &bands[third..third * 2]
    } else {
        bass
    };
    let highs = if third * 2 < size {
        &bands[third * 2..]
    } else {
        mids
    };
    let avg = |chunk: &[u8]| {
        if chunk.is_empty() {
            0.0
        } else {
            let sum: u32 = chunk.iter().map(|b| u32::from(*b)).sum();
            sum as f32 / (chunk.len() as f32 * 255.0)
        }
    };
    (avg(bass), avg(mids), avg(highs))
}

/// Python `_stars_for_frame`: slow orbital drift with per-star radius shells.
fn constellation_stars(
    width: usize,
    rows: usize,
    frame_index: u64,
    seed: u64,
    count: usize,
) -> Vec<(i32, i32)> {
    let cx = (width.saturating_sub(1) as f32) * 0.5;
    let cy = (rows.saturating_sub(1) as f32) * 0.5;
    let seed_i = seed as i64;
    let mut stars = Vec::with_capacity(count);
    for idx in 0..count {
        let angle =
            (frame_index as f32 * 0.09) + (idx as f32 * 0.63) + ((seed % 360) as f32 * 0.01745);
        let radius = 0.22 + (((idx as i64 * 37 + seed_i).rem_euclid(100)) as f32) / 140.0;
        let x = (cx + angle.cos() * width as f32 * 0.38 * radius).round() as i32;
        // Note: sin(angle * 1.3) — not pure circular; original elliptic drift.
        let y = (cy + (angle * 1.3).sin() * rows as f32 * 0.36 * radius).round() as i32;
        let x = x.clamp(0, width.saturating_sub(1) as i32);
        let y = y.clamp(0, rows.saturating_sub(1) as i32);
        stars.push((x, y));
    }
    stars
}

fn nearest_star(index: usize, stars: &[(i32, i32)], max_dist: i32) -> Option<(i32, i32)> {
    let (x1, y1) = stars[index];
    let mut best: Option<(i32, i32)> = None;
    let mut best_d2 = max_dist * max_dist;
    for (j, &(x2, y2)) in stars.iter().enumerate() {
        if j == index {
            continue;
        }
        let dx = x2 - x1;
        let dy = y2 - y1;
        let d2 = dx * dx + dy * dy;
        if d2 == 0 || d2 > best_d2 {
            continue;
        }
        best = Some((x2, y2));
        best_d2 = d2;
    }
    best
}

/// Bresenham line — only paints empty cells (Python `_draw_line`).
fn draw_bresenham(canvas: &mut Canvas, x1: i32, y1: i32, x2: i32, y2: i32, ch: char, fg: Color) {
    let mut x = x1;
    let mut y = y1;
    let dx = (x2 - x1).abs();
    let dy = -(y2 - y1).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        canvas.set_if_empty(x, y, ch, fg);
        if x == x2 && y == y2 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn flame_color(heat: f32) -> Color {
    // Dark red coals → orange → yellow → near-white tips
    if heat > 0.85 {
        Color::Rgb(255, 245, 200)
    } else if heat > 0.65 {
        Color::Rgb(255, 200, 60)
    } else if heat > 0.45 {
        Color::Rgb(255, 120, 20)
    } else if heat > 0.25 {
        Color::Rgb(200, 50, 10)
    } else {
        Color::Rgb(120, 30, 10)
    }
}

#[allow(clippy::too_many_arguments)]
fn render_data_core(
    canvas: &mut Canvas,
    width: usize,
    rows: usize,
    seed: u64,
    t: f32,
    mono: f32,
    bass: f32,
    mid: f32,
    onset: bool,
    color: bool,
    cx: f32,
    cy: f32,
) {
    // Sector grid: each row is a "track" being defragged.
    let sectors = (width / 4).clamp(6, 24);
    let tracks = rows.saturating_sub(1).max(3);
    let progress = (0.15 + mono * 0.55 + bass * 0.25).clamp(0.0, 0.95);
    let head_track = ((t * (0.15 + mid * 0.25)) as usize) % tracks.max(1);

    for ty in 0..tracks {
        let done = ((sectors as f32) * progress
            + unit_noise(seed, ty as u64) * 2.0
            + if ty == head_track { 1.0 } else { 0.0 })
        .round() as usize;
        let done = done.min(sectors);

        for sx in 0..sectors {
            let cell_w = (width / sectors).max(1);
            let x0 = sx * cell_w;
            // Fragments still "seeking": slide with time
            let sorted = sx < done;
            let frag_phase = if sorted {
                0.0
            } else {
                let u = unit_noise(seed, (ty * 31 + sx) as u64);
                ((t * (0.4 + u) + sx as f32 * 1.7 + ty as f32).sin() * 0.5 + 0.5)
                    * (cell_w.saturating_sub(1) as f32)
            };
            let x = if sorted {
                x0 as i32
            } else {
                // Scatter then drift toward home
                let home = x0 as f32;
                let scatter = unit_noise(seed, (sx * 17 + ty * 9) as u64) * width as f32;
                let blend = ((t * 0.02 + sx as f32 * 0.05 + progress).sin() * 0.5 + 0.5)
                    * (0.3 + progress * 0.7);
                (scatter * (1.0 - blend) + home * blend + frag_phase * 0.2).round() as i32
            };

            let n = mix_u64(seed, (ty as u64) << 16 | sx as u64);
            let glyph = if sorted {
                if onset && ty == head_track {
                    '#'
                } else {
                    ['█', '▓', '▒', '░'][(n % 4) as usize]
                }
            } else {
                b"0123456789ABCDEF"[((n >> 8) % 16) as usize] as char
            };

            let heat = if sorted {
                0.35 + progress * 0.4 + if ty == head_track { 0.2 } else { 0.0 }
            } else {
                0.55 + mid * 0.3
            };

            let fg = if color {
                if sorted {
                    if ty == head_track {
                        Color::Cyan
                    } else {
                        Color::Green
                    }
                } else if onset {
                    Color::Yellow
                } else {
                    Color::Rgb(180, 80, 255)
                }
            } else {
                energy_color(heat.clamp(0.0, 1.0), false)
            };

            canvas.set(
                x.clamp(0, width.saturating_sub(1) as i32),
                ty as i32,
                glyph,
                fg,
            );
            // Fill rest of sorted sector solidly
            if sorted && cell_w > 1 {
                for dx in 1..cell_w {
                    if x0 + dx < width {
                        canvas.set(
                            (x0 + dx) as i32,
                            ty as i32,
                            if dx + 1 == cell_w { '|' } else { glyph },
                            fg,
                        );
                    }
                }
            }
        }
    }

    // Read/write head indicator
    let head_x = ((progress * (width.saturating_sub(1) as f32))
        + (t * 2.0 + bass * 5.0).sin() * 2.0)
        .round() as i32;
    canvas.set(
        head_x.clamp(0, width.saturating_sub(1) as i32),
        head_track as i32,
        if onset { '@' } else { '>' },
        if color { Color::White } else { Color::Gray },
    );

    // Core checksum block at center (overdrawn lightly)
    let label = format!(
        "{:04X}",
        (seed as u32 ^ (t as u32).wrapping_mul(13)) & 0xFFFF
    );
    let lx = (cx as i32) - (label.len() as i32 / 2);
    let ly = cy.round() as i32;
    if ly >= 0 && (ly as usize) < rows {
        for (i, ch) in label.chars().enumerate() {
            canvas.set(
                lx + i as i32,
                ly,
                ch,
                if color { Color::Yellow } else { Color::White },
            );
        }
    }

    // Status strip
    if rows > 0 {
        let pct = (progress * 100.0).round() as i32;
        let status = format!("DEFRAG {pct:3}%  SEC {sectors}  TRK {tracks}");
        for (i, ch) in status.chars().take(width).enumerate() {
            canvas.set(
                i as i32,
                (rows - 1) as i32,
                ch,
                if color { Color::DarkGray } else { Color::Gray },
            );
        }
    }
}

macro_rules! particle_plugin {
    ($ty:ident, $style:expr, $id:expr, $name:expr, $title:expr) => {
        #[derive(Debug, Default)]
        pub struct $ty {
            color: bool,
        }

        impl VisualizerPlugin for $ty {
            fn plugin_id(&self) -> &'static str {
                $id
            }
            fn display_name(&self) -> &'static str {
                $name
            }
            fn on_activate(&mut self, context: VisualizerContext) {
                self.color = context.ansi_enabled;
            }
            fn on_deactivate(&mut self) {}
            fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>> {
                render_particles($style, $title, self.color, frame)
            }
        }
    };
}

particle_plugin!(
    ParticleReactorVisualizer,
    ParticleStyle::Reactor,
    "viz.reactor.particles",
    "Particle Reactor",
    "PARTICLE REACTOR"
);
particle_plugin!(
    GravityWellVisualizer,
    ParticleStyle::GravityWell,
    "viz.particle.gravity_well",
    "Gravity Well Reactor",
    "GRAVITY WELL REACTOR"
);
particle_plugin!(
    ShockwaveRingsVisualizer,
    ParticleStyle::Shockwave,
    "viz.particle.shockwave_rings",
    "Shockwave Rings",
    "SHOCKWAVE RINGS"
);
particle_plugin!(
    ReactiveRainVisualizer,
    ParticleStyle::Rain,
    "viz.particle.rain_reactive",
    "Reactive Rain",
    "REACTIVE RAIN"
);
particle_plugin!(
    OrbitalSystemVisualizer,
    ParticleStyle::Orbital,
    "viz.particle.orbital_system",
    "Orbital Audio System",
    "ORBITAL SYSTEM"
);
particle_plugin!(
    EmberFieldVisualizer,
    ParticleStyle::Ember,
    "viz.particle.ember_field",
    "Ember Field",
    "EMBER FIELD"
);
particle_plugin!(
    MagneticGridVisualizer,
    ParticleStyle::Magnetic,
    "viz.particle.magnetic_grid",
    "Magnetic Grid",
    "MAGNETIC GRID"
);
particle_plugin!(
    AudioTornadoVisualizer,
    ParticleStyle::Tornado,
    "viz.particle.audio_tornado",
    "Audio Tornado",
    "AUDIO TORNADO"
);
particle_plugin!(
    ConstellationVisualizer,
    ParticleStyle::Constellation,
    "viz.particle.constellation",
    "Constellation Mode",
    "CONSTELLATION"
);
particle_plugin!(
    DataCoreFragVisualizer,
    ParticleStyle::DataCore,
    "viz.particle.data_core_frag",
    "Data Core Frag",
    "DATA CORE FRAG"
);
particle_plugin!(
    PlasmaStreamVisualizer,
    ParticleStyle::Plasma,
    "viz.particle.plasma_stream",
    "Plasma Stream",
    "PLASMA STREAM"
);
