//! Terminal UI frontend for tz-player (ratatui).

mod visualizers;

use std::io::{self, stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use tz_control::Command;
use tz_core::AppRuntime;
use visualizers::{VisualizerFrameInput, VisualizerHost};

/// Run the interactive TUI until quit.
pub async fn run_tui(mut runtime: AppRuntime) -> Result<(), TuiError> {
    enable_raw_mode().map_err(|e| TuiError::Io(e.to_string()))?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).map_err(|e| TuiError::Io(e.to_string()))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend).map_err(|e| TuiError::Io(e.to_string()))?;

    let mut viz = VisualizerHost::new(runtime.app_state.ansi_enabled)
        .with_plugin_id(Some(&runtime.visualizer_id));
    runtime.set_visualizer_id(viz.active_id());

    let result = ui_loop(&mut terminal, &mut runtime, &mut viz).await;

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    runtime.persist().await;
    result
}

async fn ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    runtime: &mut AppRuntime,
    viz: &mut VisualizerHost,
) -> Result<(), TuiError> {
    let mut scroll_offset = 0usize;

    loop {
        runtime.tick().await;
        let snap = runtime.snapshot().await;
        let count = runtime.playlist_count();
        let term_size = terminal.size().map_err(|e| TuiError::Io(e.to_string()))?;
        let area = Rect::new(0, 0, term_size.width, term_size.height);
        // header(3) + transport(4) + footer(2) + borders; main fills the rest
        let main_height = area.height.saturating_sub(9).max(3);
        let visible = main_height.saturating_sub(2) as usize; // list border

        if runtime.cursor_index < scroll_offset {
            scroll_offset = runtime.cursor_index;
        } else if runtime.cursor_index >= scroll_offset + visible.max(1) {
            scroll_offset = runtime.cursor_index + 1 - visible.max(1);
        }

        let rows = runtime
            .fetch_rows(scroll_offset, visible.max(1))
            .unwrap_or_default();

        // Precompute the same layout as the draw pass so the visualizer canvas
        // matches the block's *inner* rect exactly (avoids Paragraph wrap shifts
        // that make the geometric center appear to bounce). Sharing
        // main_layout() (rather than two separately-written Layout::split
        // calls) is what keeps this guarantee true after edits.
        let layout_root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(4),
                Constraint::Length(2),
            ])
            .split(area);
        let (_, viz_panel) = main_layout(layout_root[1], runtime.visualizer_hidden);
        // Frozen (not ticking) while hidden: skipping render() avoids
        // spending CPU animating something invisible, and since the host
        // itself isn't torn down, showing it again resumes instantly rather
        // than restarting from scratch.
        let viz_lines = if let Some(viz_panel) = viz_panel {
            let viz_w = viz_panel.width.saturating_sub(2).max(1); // borders
            let viz_h = viz_panel.height.saturating_sub(2).max(1);
            let frame_in = VisualizerFrameInput {
                frame_index: 0,
                width: viz_w,
                height: viz_h,
                status: snap.status.clone(),
                position_s: snap.position_ms as f64 / 1000.0,
                duration_s: if snap.duration_ms > 0 {
                    Some(snap.duration_ms as f64 / 1000.0)
                } else {
                    None
                },
                volume: f64::from(snap.volume) / 100.0,
                speed: snap.speed,
                title: snap.title.clone(),
                track_path: snap.track_path.clone(),
                level_left: snap.level_left,
                level_right: snap.level_right,
                level_source: snap.level_source.clone(),
                spectrum_bands: snap.spectrum_bands.clone(),
                spectrum_source: snap.spectrum_source.clone(),
                beat_strength: snap.beat_strength,
                beat_is_onset: snap.beat_is_onset,
                beat_bpm: snap.beat_bpm,
                beat_source: snap.beat_source.clone(),
                waveform_min_left: snap.waveform_min_left,
                waveform_max_left: snap.waveform_max_left,
                waveform_min_right: snap.waveform_min_right,
                waveform_max_right: snap.waveform_max_right,
                waveform_source: snap.waveform_source.clone(),
                waveform_history: snap.waveform_history.clone(),
            };
            viz.render(frame_in)
        } else {
            Vec::new()
        };

        terminal
            .draw(|f| {
                let root = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // header
                        Constraint::Min(5),    // main: playlist | visualizer
                        Constraint::Length(4), // transport
                        Constraint::Length(2), // footer
                    ])
                    .split(f.area());

                let (playlist_area, viz_area) = main_layout(root[1], runtime.visualizer_hidden);

                draw_header(f, root[0], &snap, viz.active_name());
                draw_playlist(
                    f,
                    playlist_area,
                    &rows,
                    PlaylistView {
                        cursor_index: runtime.cursor_index,
                        scroll_offset,
                        total: count,
                        find_query: &runtime.find_query,
                        playing_item_id: snap.item_id,
                    },
                );
                if let Some(viz_area) = viz_area {
                    draw_visualizer(f, viz_area, &viz_lines, viz.active_name());
                }
                draw_transport(f, root[2], &snap);
                draw_footer(
                    f,
                    root[3],
                    runtime.status_message.as_deref(),
                    runtime.status_level,
                    &runtime.input_mode,
                    &runtime.input_buffer,
                    runtime.confirm_clear,
                );
                if runtime.input_mode == "help" {
                    draw_help_overlay(f, f.area());
                }
            })
            .map_err(|e| TuiError::Io(e.to_string()))?;

        if event::poll(Duration::from_millis(80)).map_err(|e| TuiError::Io(e.to_string()))? {
            if let Event::Key(key) = event::read().map_err(|e| TuiError::Io(e.to_string()))? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if handle_key(runtime, viz, key.code, key.modifiers).await? {
                    break;
                }
            }
        }

        if runtime.quit_requested {
            break;
        }
    }
    Ok(())
}

/// Split the main row into (playlist, visualizer) areas. Returns `None` for
/// the visualizer half when hidden, instead of a zero-width `Rect` — that
/// makes "there's nothing to draw" a compile-time-checked case at call
/// sites rather than an index that happens to be out of bounds.
fn main_layout(area: Rect, visualizer_hidden: bool) -> (Rect, Option<Rect>) {
    if visualizer_hidden {
        return (area, None);
    }
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    (cols[0], Some(cols[1]))
}

fn draw_header(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    snap: &tz_control::TransportSnapshot,
    viz_name: &str,
) {
    let title = format!(
        " tz-player  backend={}  tracks={}  viz={} ",
        snap.backend, snap.playlist_count, viz_name
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().fg(Color::Cyan));
    let now_playing = match (&snap.title, &snap.artist) {
        (Some(t), Some(a)) => format!("{a} — {t}"),
        (Some(t), None) => t.clone(),
        (None, Some(a)) => a.clone(),
        _ => snap
            .track_path
            .as_deref()
            .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p))
            .unwrap_or("(nothing playing)")
            .to_string(),
    };
    let p = Paragraph::new(now_playing).block(block);
    f.render_widget(p, area);
}

/// View state for [`draw_playlist`], bundled to stay under clippy's
/// too-many-arguments limit.
struct PlaylistView<'a> {
    cursor_index: usize,
    scroll_offset: usize,
    total: usize,
    find_query: &'a str,
    playing_item_id: Option<i64>,
}

fn draw_playlist(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    rows: &[tz_db::PlaylistRow],
    view: PlaylistView<'_>,
) {
    let PlaylistView {
        cursor_index,
        scroll_offset,
        total,
        find_query,
        playing_item_id,
    } = view;
    let items: Vec<ListItem> = if rows.is_empty() {
        let hint = if !find_query.is_empty() {
            "  No matches — Esc clears find"
        } else {
            "  (empty)  press a to add files/folders, ? for help"
        };
        vec![ListItem::new(Line::from(Span::styled(
            hint,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )))]
    } else {
        rows.iter()
            .enumerate()
            .map(|(i, row)| {
                let abs = scroll_offset + i;
                let label = row
                    .title
                    .clone()
                    .or_else(|| {
                        row.path
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| row.path.display().to_string());
                let artist = row.artist.as_deref().unwrap_or("");
                let rest = if artist.is_empty() {
                    format!("{:>4}  {label}", abs + 1)
                } else {
                    format!("{:>4}  {artist} — {label}", abs + 1)
                };
                let is_cursor = abs == cursor_index;
                let is_playing = playing_item_id == Some(row.item_id);
                let marker = if is_playing { ">" } else { " " };
                let base_style = if is_cursor {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let marker_style = if is_cursor {
                    base_style
                } else if is_playing {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    base_style
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {marker} "), marker_style),
                    Span::styled(rest, base_style),
                ]))
            })
            .collect()
    };

    let title = if find_query.is_empty() {
        format!(" Playlist ({total}) ")
    } else {
        format!(" Find '{find_query}' ({total}) ")
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(list, area);
}

fn draw_visualizer(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    lines: &[Line<'static>],
    viz_name: &str,
) {
    let title = format!(" Visualizer · {viz_name} ");
    // No wrap: each canvas row is already sized to the inner width. Wrapping
    // would insert extra rows and make centered origins look like they bounce.
    let p = Paragraph::new(lines.to_vec()).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Magenta)),
    );
    f.render_widget(p, area);
}

/// Style for a togglable player-state indicator (repeat/shuffle): bold green
/// when active, dim gray when off, so it reads at a glance instead of
/// blending into the rest of the transport line.
fn state_style(active: bool) -> Style {
    if active {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn draw_transport(f: &mut ratatui::Frame<'_>, area: Rect, snap: &tz_control::TransportSnapshot) {
    let pos = format_time(snap.position_ms);
    let dur = format_time(snap.duration_ms);
    let bar_w = (area.width.saturating_sub(48) as usize).clamp(12, 40);
    let bar = progress_bar(snap.position_ms, snap.duration_ms, bar_w);
    let lvl = match (snap.level_left, snap.level_right, &snap.level_source) {
        (Some(l), Some(r), Some(s)) => format!(" L{l:.2} R{r:.2}[{s}]"),
        _ => String::new(),
    };
    let analysis = snap
        .analysis_status
        .as_deref()
        .map(|a| format!("  analysis:{a}"))
        .unwrap_or_default();
    let prefix = format!(
        " {}  {} {} {}  vol {}%  speed {:.2}x  ",
        snap.status.to_uppercase(),
        pos,
        bar,
        dur,
        snap.volume,
        snap.speed,
    );
    let suffix = format!("{lvl}{analysis} ");
    let shuffle_label = if snap.shuffle { "on" } else { "off" };
    let mut lines = vec![Line::from(vec![
        Span::raw(prefix),
        Span::styled(
            format!("rep {}", snap.repeat_mode),
            state_style(snap.repeat_mode != "off"),
        ),
        Span::raw("  "),
        Span::styled(
            format!("shuf {shuffle_label}"),
            state_style(snap.shuffle),
        ),
        Span::raw(suffix),
    ])];
    if let Some(err) = &snap.error {
        lines.push(Line::from(Span::styled(
            err.chars().take(120).collect::<String>(),
            Style::default().fg(Color::Red),
        )));
    }
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Transport "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_footer(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    msg: Option<&str>,
    status_level: tz_core::StatusLevel,
    input_mode: &str,
    input_buffer: &str,
    confirm_clear: bool,
) {
    let help = if confirm_clear {
        "Clear playlist? [y]es / [n]o".to_string()
    } else if input_mode == "find" {
        format!("Find: {input_buffer}_   (live — Enter=keep Esc=cancel)")
    } else if input_mode == "add_path" {
        format!("Add path: {input_buffer}_   (Enter=add Esc=cancel)")
    } else if input_mode == "help" {
        "Esc / q / any key — close help".into()
    } else {
        "↑/↓ Space n/p x ←/→ -/+ [] r/s f a d c m z i g  Z=hide-viz  ?=help  q quit".into()
    };
    let (line, style) = if input_mode == "help" {
        (
            help,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some(m) = msg {
        let (prefix, color) = match status_level {
            tz_core::StatusLevel::Error => ("[ERROR] ", Color::Red),
            tz_core::StatusLevel::Warn => ("[WARN] ", Color::Yellow),
            tz_core::StatusLevel::Info => ("", Color::DarkGray),
        };
        (
            format!("{help}  |  {prefix}{m}"),
            Style::default().fg(color),
        )
    } else {
        (help, Style::default().fg(Color::DarkGray))
    };
    let p = Paragraph::new(line).style(style);
    f.render_widget(p, area);
}

/// Center a fixed-size box within `r`, clamped so it never exceeds `r`.
/// Unlike a percentage-of-screen popup, this can't silently clip
/// fixed-length content on a small terminal — it only shrinks to fit.
fn centered_fixed_rect(width: u16, height: u16, r: Rect) -> Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);
    let x = r.x + (r.width.saturating_sub(width)) / 2;
    let y = r.y + (r.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

/// Full keybinding reference, grouped by category. Drawn as a centered
/// overlay (not squeezed into the 2-row footer) since the flat list is
/// long enough that a single non-wrapping line was both cramped and,
/// historically, incomplete.
const HELP_KEY_W: usize = 14;
const HELP_DESC_W: usize = 22;

/// Two mnemonic/description pairs packed onto one line, so the full
/// reference fits an 80x24 terminal without scrolling (verified by
/// `help_overlay_documents_every_previously_undocumented_key_on_a_standard_terminal`).
/// Only used for entries short enough that `d1` won't blow past
/// `HELP_DESC_W` and misalign the second key column.
fn help_entry2(
    key_style: Style,
    desc_style: Style,
    k1: &'static str,
    d1: &'static str,
    k2: &'static str,
    d2: &'static str,
) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{k1:<HELP_KEY_W$}"), key_style),
        Span::styled(format!("{d1:<HELP_DESC_W$}"), desc_style),
        Span::raw("  "),
        Span::styled(format!("{k2:<HELP_KEY_W$}"), key_style),
        Span::styled(d2, desc_style),
    ])
}

/// A single mnemonic/description, for entries whose description is too
/// long to pair without overflowing `HELP_DESC_W`.
fn help_entry1(key_style: Style, desc_style: Style, k: &'static str, d: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{k:<HELP_KEY_W$}"), key_style),
        Span::styled(d, desc_style),
    ])
}

/// Full keybinding reference, grouped by category. Drawn as a centered
/// overlay (not squeezed into the 2-row footer) since the flat list is
/// long enough that a single non-wrapping line was both cramped and,
/// historically, incomplete. Packed two entries per line (ASCII only —
/// no arrow/shift glyphs, which are ambiguous-width in many terminals and
/// would misalign the second column) to fit an 80x24 terminal in full.
fn help_lines() -> Vec<Line<'static>> {
    let heading = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc = Style::default().fg(Color::Gray);
    let e2 = |k1, d1, k2, d2| help_entry2(key, desc, k1, d1, k2, d2);
    let e1 = |k, d| help_entry1(key, desc, k, d);
    let section =
        |title: &'static str| Line::from(vec![Span::raw(" "), Span::styled(title, heading)]);
    vec![
        section("Playback"),
        e2("Space", "Play / Pause", "x", "Stop"),
        e2("n / p", "Next / Previous", "Enter", "Play selected"),
        e2("Left/Right", "Seek +/-5s", "Shift+L/R", "Seek +/-30s"),
        e2("- / +", "Volume +/-5%", "[ / ]", "Speed +/-0.25x"),
        e2("\\", "Reset speed to 1.0x", "r", "Cycle repeat mode"),
        e1("s", "Toggle shuffle"),
        section("Navigation"),
        e2("Up/Down", "Move cursor", "Home/End", "Top / Bottom"),
        e1("PgUp / PgDn", "Page up / down"),
        e1("g", "Locate now-playing track"),
        section("Playlist"),
        e2("a", "Add path", "d / Del", "Remove selected"),
        e2("c", "Clear playlist", "m", "Refresh metadata"),
        e2("f", "Find", "Shift+U/D", "Reorder up/down"),
        section("View"),
        e2("z", "Cycle visualizer", "i", "About / version"),
        e1("Shift+Z", "Hide/show visualizer pane"),
        Line::from(Span::styled(
            "Esc / q / any key - close",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )),
    ]
}

fn draw_help_overlay(f: &mut ratatui::Frame<'_>, area: Rect) {
    let lines = help_lines();
    // Size from actual content, not a screen percentage: a percentage popup
    // silently clips fixed-length content on a small terminal (verified on
    // an 80x24 fixture). This only ever shrinks to fit the screen.
    let content_width = lines.iter().map(Line::width).max().unwrap_or(40) as u16;
    let popup = centered_fixed_rect(content_width + 4, lines.len() as u16 + 2, area);
    f.render_widget(Clear, popup);
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Keyboard Shortcuts ")
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(p, popup);
}

fn format_time(ms: u64) -> String {
    let total_s = ms / 1000;
    format!("{:02}:{:02}", total_s / 60, total_s % 60)
}

fn progress_bar(pos: u64, dur: u64, width: usize) -> String {
    if dur == 0 || width == 0 {
        return format!("[{}]", " ".repeat(width));
    }
    let filled = ((pos as f64 / dur as f64) * width as f64).round() as usize;
    let filled = filled.min(width);
    let mut bar = String::from("[");
    for i in 0..width {
        bar.push(if i < filled { '#' } else { '-' });
    }
    bar.push(']');
    bar
}

fn toggle_visualizer_hidden(runtime: &mut AppRuntime) {
    runtime.visualizer_hidden = !runtime.visualizer_hidden;
    runtime.set_status(if runtime.visualizer_hidden {
        "Visualizer hidden — playlist maximized"
    } else {
        "Visualizer shown"
    });
}

async fn handle_key(
    runtime: &mut AppRuntime,
    viz: &mut VisualizerHost,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<bool, TuiError> {
    // Confirm clear
    if runtime.confirm_clear {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let _ = runtime.handle(Command::ConfirmClear { yes: true }).await;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                let _ = runtime.handle(Command::ConfirmClear { yes: false }).await;
            }
            _ => {}
        }
        return Ok(false);
    }

    // Help overlay (any key dismisses except another ?)
    if runtime.input_mode == "help" {
        match code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                runtime.input_mode = "normal".into();
                runtime.clear_status();
            }
            _ => {
                runtime.input_mode = "normal".into();
            }
        }
        return Ok(false);
    }

    // Text input modes
    if runtime.input_mode == "find" || runtime.input_mode == "add_path" {
        match code {
            KeyCode::Esc => {
                if runtime.input_mode == "find" {
                    let _ = runtime.handle(Command::ClearFind).await;
                }
                runtime.input_mode = "normal".into();
                runtime.input_buffer.clear();
                runtime.set_status("Cancelled");
            }
            KeyCode::Enter => {
                if runtime.input_mode == "find" {
                    let q = runtime.input_buffer.clone();
                    let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
                    runtime.input_mode = "normal".into();
                } else if runtime.input_mode == "add_path" {
                    let path = runtime.input_buffer.trim().to_string();
                    if path.is_empty() {
                        runtime.set_status("Empty path — cancelled");
                    } else {
                        let _ = runtime
                            .handle(Command::AddPaths { paths: vec![path] })
                            .await;
                    }
                    runtime.input_mode = "normal".into();
                    runtime.input_buffer.clear();
                }
            }
            KeyCode::Backspace => {
                runtime.input_buffer.pop();
                if runtime.input_mode == "find" {
                    let q = runtime.input_buffer.clone();
                    let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
                }
            }
            KeyCode::Char(c) => {
                runtime.input_buffer.push(c);
                if runtime.input_mode == "find" {
                    let q = runtime.input_buffer.clone();
                    let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    let shift = modifiers.contains(KeyModifiers::SHIFT);
    match code {
        KeyCode::Char('q') => {
            let _ = runtime.handle(Command::Quit).await;
            return Ok(true);
        }
        KeyCode::Char('?') => {
            runtime.input_mode = "help".into();
            runtime.set_status("Help — Esc to close");
        }
        KeyCode::Char('f') => {
            runtime.input_mode = "find".into();
            runtime.input_buffer = runtime.find_query.clone();
            runtime.set_status("Find mode");
        }
        KeyCode::Char('a') => {
            let _ = runtime.handle(Command::RequestAddPath).await;
        }
        // Shift+Z toggles pane visibility; must sit above the plain 'z'
        // cycle arm. Terminals vary on whether a shifted letter arrives as
        // a bare uppercase char or as Char('z') + the SHIFT modifier, so
        // both are handled.
        KeyCode::Char('Z') => toggle_visualizer_hidden(runtime),
        KeyCode::Char('z') if shift => toggle_visualizer_hidden(runtime),
        KeyCode::Char('z') => {
            let id = viz.cycle();
            runtime.set_visualizer_id(id);
            runtime.set_status(format!("Visualizer: {} ({id})", viz.active_name()));
        }
        KeyCode::Char('i') => {
            runtime.set_status(tz_core::about_info().tui_line());
        }
        KeyCode::Char('g') => {
            let _ = runtime.handle(Command::LocatePlaying).await;
        }
        KeyCode::Home => {
            runtime.cursor_index = 0;
        }
        KeyCode::End => {
            let n = runtime.playlist_count();
            if n > 0 {
                runtime.cursor_index = n - 1;
            }
        }
        KeyCode::Up if shift => {
            let _ = runtime.move_cursor_item(true);
        }
        KeyCode::Down if shift => {
            let _ = runtime.move_cursor_item(false);
        }
        KeyCode::Up => {
            let _ = runtime.handle(Command::CursorUp).await;
        }
        KeyCode::Down => {
            let _ = runtime.handle(Command::CursorDown).await;
        }
        KeyCode::PageUp => {
            let _ = runtime.handle(Command::PageUp).await;
        }
        KeyCode::PageDown => {
            let _ = runtime.handle(Command::PageDown).await;
        }
        KeyCode::Enter => {
            let _ = runtime.handle(Command::PlayCursor).await;
        }
        KeyCode::Char(' ') => {
            let _ = runtime.handle(Command::PlayPause).await;
        }
        KeyCode::Char('n') => {
            let _ = runtime.handle(Command::Next).await;
        }
        KeyCode::Char('p') => {
            let _ = runtime.handle(Command::Previous).await;
        }
        KeyCode::Char('x') => {
            let _ = runtime.handle(Command::Stop).await;
        }
        KeyCode::Left if shift => {
            let _ = runtime
                .handle(Command::SeekRelative { delta_ms: -30_000 })
                .await;
        }
        KeyCode::Right if shift => {
            let _ = runtime
                .handle(Command::SeekRelative { delta_ms: 30_000 })
                .await;
        }
        KeyCode::Left => {
            let _ = runtime
                .handle(Command::SeekRelative { delta_ms: -5_000 })
                .await;
        }
        KeyCode::Right => {
            let _ = runtime
                .handle(Command::SeekRelative { delta_ms: 5_000 })
                .await;
        }
        KeyCode::Char('-') => {
            let _ = runtime.handle(Command::VolumeDelta { delta: -5 }).await;
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let _ = runtime.handle(Command::VolumeDelta { delta: 5 }).await;
        }
        KeyCode::Char('[') => {
            let _ = runtime.handle(Command::SpeedDelta { delta: -0.25 }).await;
        }
        KeyCode::Char(']') => {
            let _ = runtime.handle(Command::SpeedDelta { delta: 0.25 }).await;
        }
        KeyCode::Char('\\') => {
            let _ = runtime.handle(Command::SetSpeed { speed: 1.0 }).await;
        }
        KeyCode::Char('r') => {
            let _ = runtime.handle(Command::CycleRepeat).await;
        }
        KeyCode::Char('s') => {
            let _ = runtime.handle(Command::ToggleShuffle).await;
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            let _ = runtime.handle(Command::RemoveSelected).await;
        }
        KeyCode::Char('c') => {
            let _ = runtime.handle(Command::ClearPlaylist).await;
        }
        KeyCode::Char('m') => {
            let _ = runtime.handle(Command::RefreshMetadata).await;
        }
        KeyCode::Esc => {
            if runtime.status_level == tz_core::StatusLevel::Error {
                // Dismiss the error on its own keypress; don't also clear
                // find in the same Esc, or the user can't tell which
                // happened.
                runtime.clear_status();
                return Ok(false);
            }
            if runtime.find_ids.is_some() {
                let _ = runtime.handle(Command::ClearFind).await;
            }
        }
        _ => {}
    }
    Ok(false)
}

#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    #[error("io: {0}")]
    Io(String),
    #[error("{0}")]
    Message(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::path::PathBuf;

    #[test]
    fn main_layout_gives_playlist_the_full_width_when_visualizer_is_hidden() {
        let area = Rect::new(0, 0, 100, 30);

        let (playlist, viz) = main_layout(area, true);

        assert_eq!(playlist, area);
        assert_eq!(viz, None, "no visualizer rect should be produced at all");
    }

    #[test]
    fn main_layout_splits_evenly_when_visualizer_is_visible() {
        let area = Rect::new(0, 0, 100, 30);

        let (playlist, viz) = main_layout(area, false);
        let viz = viz.expect("visualizer rect expected when not hidden");

        assert_eq!(playlist.width + viz.width, area.width);
        assert_eq!(playlist.height, area.height);
        assert_eq!(viz.height, area.height);
        assert!(viz.x >= playlist.x + playlist.width);
    }

    fn row(item_id: i64, title: &str) -> tz_db::PlaylistRow {
        tz_db::PlaylistRow {
            item_id,
            track_id: item_id,
            pos_key: item_id,
            path: PathBuf::from(format!("{title}.mp3")),
            title: Some(title.to_string()),
            artist: None,
            album: None,
            year: None,
            duration_ms: None,
            meta_valid: None,
            meta_error: None,
        }
    }

    fn render(
        rows: &[tz_db::PlaylistRow],
        cursor_index: usize,
        playing_item_id: Option<i64>,
    ) -> Buffer {
        let backend = TestBackend::new(40, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw_playlist(
                    f,
                    f.area(),
                    rows,
                    PlaylistView {
                        cursor_index,
                        scroll_offset: 0,
                        total: rows.len(),
                        find_query: "",
                        playing_item_id,
                    },
                );
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn marks_now_playing_row_with_distinct_glyph() {
        let rows = vec![row(1, "one"), row(2, "two"), row(3, "three")];
        let buf = render(&rows, 0, Some(2));

        // Border is y=0; list rows start at y=1, one per row in order.
        assert!(
            row_text(&buf, 2).contains('>'),
            "expected a marker on the now-playing row (item_id=2)"
        );
        assert!(
            !row_text(&buf, 1).contains('>'),
            "did not expect a marker on a non-playing row"
        );
    }

    #[test]
    fn marker_still_visible_when_cursor_is_on_playing_row() {
        let rows = vec![row(1, "one"), row(2, "two")];
        let buf = render(&rows, 1, Some(2));

        assert!(
            row_text(&buf, 2).contains('>'),
            "expected the marker even when the cursor highlight covers the same row"
        );
    }

    #[test]
    fn no_marker_when_nothing_has_played() {
        let rows = vec![row(1, "one"), row(2, "two")];
        let buf = render(&rows, 0, None);

        for y in 1..3 {
            assert!(
                !row_text(&buf, y).contains('>'),
                "did not expect any marker when playing_item_id is None"
            );
        }
    }

    fn buffer_text(buf: &Buffer) -> String {
        let mut text = String::new();
        for y in 0..buf.area.height {
            text.push_str(&row_text(buf, y));
            text.push('\n');
        }
        text
    }

    #[test]
    fn help_overlay_documents_every_previously_undocumented_key_on_a_standard_terminal() {
        // 80x24 is the common default terminal size, not a generous test
        // fixture — a percentage-of-screen popup with ~32 lines of fixed
        // content clips silently on a screen this size. Sizing the popup
        // from content length (not a screen percentage) is what's under
        // test here.
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_help_overlay(f, f.area()))
            .unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());

        for needle in [
            "Keyboard Shortcuts",
            "Play / Pause",
            "Cycle repeat mode",
            "Toggle shuffle",
            "Reset speed to 1.0x",
            "Locate now-playing track",
            "Remove selected",
            "Refresh metadata",
            "Reorder",
            "Cycle visualizer",
            "About",
            "Hide/show visualizer",
            "Esc / q / any key",
        ] {
            assert!(
                text.contains(needle),
                "expected help overlay to mention {needle:?} on an 80x24 terminal, got:\n{text}"
            );
        }
    }

    #[test]
    fn state_style_highlights_active_state() {
        let active = state_style(true);
        let inactive = state_style(false);

        assert_eq!(active.fg, Some(Color::Green));
        assert!(active.add_modifier.contains(Modifier::BOLD));
        assert_eq!(inactive.fg, Some(Color::DarkGray));
        assert_ne!(active.fg, inactive.fg);
    }

    /// A real `AppRuntime` (Fake backend, temp on-disk DB) with no tracks —
    /// for tests that only care about UI-dispatch state, not playlist
    /// contents.
    async fn bare_test_runtime(name: &str) -> AppRuntime {
        let dir = std::env::temp_dir().join(format!(
            "tz_tui_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = tz_core::AppPaths {
            data_dir: dir.clone(),
            config_dir: dir.clone(),
            log_dir: dir.join("logs"),
            state_file: dir.join("state.json"),
            db_file: dir.join("db.sqlite3"),
        };
        tz_core::open_runtime(paths, Some(tz_playback::BackendKind::Fake))
            .await
            .unwrap()
    }

    async fn find_test_runtime(name: &str) -> AppRuntime {
        let runtime = bare_test_runtime(name).await;
        let dir = &runtime.paths.data_dir;
        // Distinct filename substrings, no metadata upsert needed — mirrors
        // tz-db's search_by_path_and_metadata fixture, which is what proves
        // add_tracks alone populates the search index (FTS-or-LIKE) that
        // search_item_ids reads from.
        let names = ["alpha_song.mp3", "beta_song.mp3", "alphabet.mp3"];
        for name in names {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let dir_paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
        runtime
            .store
            .add_tracks(runtime.playlist_id, &dir_paths)
            .unwrap();
        runtime
    }

    #[tokio::test]
    async fn typing_in_find_mode_filters_the_playlist_before_enter_is_pressed() {
        let mut runtime = find_test_runtime("live").await;
        let mut viz = VisualizerHost::new(false);
        assert_eq!(runtime.playlist_count(), 3);

        handle_key(&mut runtime, &mut viz, KeyCode::Char('f'), KeyModifiers::NONE)
            .await
            .unwrap();
        for c in "alpha".chars() {
            handle_key(&mut runtime, &mut viz, KeyCode::Char(c), KeyModifiers::NONE)
                .await
                .unwrap();
        }

        // Live filtering means this is already true — Enter hasn't been
        // pressed yet. Assert through the same read path draw_playlist
        // uses, not the internal find_ids field.
        assert_eq!(
            runtime.playlist_count(),
            2,
            "expected 'alpha' to match alpha_song and alphabet without pressing Enter"
        );
        let visible = runtime.fetch_rows(0, 10).unwrap();
        assert!(visible
            .iter()
            .all(|r| r.path.to_string_lossy().contains("alpha")));

        handle_key(&mut runtime, &mut viz, KeyCode::Backspace, KeyModifiers::NONE)
            .await
            .unwrap();
        handle_key(&mut runtime, &mut viz, KeyCode::Backspace, KeyModifiers::NONE)
            .await
            .unwrap();
        handle_key(&mut runtime, &mut viz, KeyCode::Backspace, KeyModifiers::NONE)
            .await
            .unwrap();
        handle_key(&mut runtime, &mut viz, KeyCode::Backspace, KeyModifiers::NONE)
            .await
            .unwrap();
        handle_key(&mut runtime, &mut viz, KeyCode::Backspace, KeyModifiers::NONE)
            .await
            .unwrap();

        assert_eq!(
            runtime.playlist_count(),
            3,
            "expected backspacing back to an empty query to restore the full playlist"
        );

        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn shift_z_toggles_visualizer_hidden() {
        let mut runtime = bare_test_runtime("viz_toggle").await;
        let mut viz = VisualizerHost::new(false);
        assert!(!runtime.visualizer_hidden);

        // Terminals vary on whether a shifted letter arrives as a bare
        // uppercase char or as Char('z') + SHIFT — cover both.
        handle_key(&mut runtime, &mut viz, KeyCode::Char('Z'), KeyModifiers::NONE)
            .await
            .unwrap();
        assert!(runtime.visualizer_hidden);

        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Char('z'),
            KeyModifiers::SHIFT,
        )
        .await
        .unwrap();
        assert!(!runtime.visualizer_hidden);

        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn plain_z_still_cycles_visualizer_and_leaves_the_pane_visible() {
        // Regression guard for arm ordering: the new shift-guarded arms sit
        // right above the plain 'z' cycle arm in the match — make sure they
        // don't swallow it.
        let mut runtime = bare_test_runtime("viz_cycle").await;
        let mut viz = VisualizerHost::new(false);
        let before = viz.active_id();

        handle_key(&mut runtime, &mut viz, KeyCode::Char('z'), KeyModifiers::NONE)
            .await
            .unwrap();

        assert_ne!(viz.active_id(), before);
        assert!(!runtime.visualizer_hidden);

        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }
}
