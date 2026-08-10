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
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Terminal;
use tz_control::Command;
use tz_core::{AppRuntime, EditorFocus, EditorOverlay};
use tz_db::PlaylistSort;
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
    let mut browse_scroll_offset = 0usize;
    let mut editor_was_active = runtime.input_mode == "editor";

    loop {
        runtime.tick().await;
        let editor_is_active = runtime.input_mode == "editor";
        if editor_transitioned(editor_was_active, &runtime.input_mode) {
            // On some terminals (notably Windows Terminal), Ratatui's logical
            // back buffer can diverge from cells physically left on screen.
            // A resize repairs that by issuing a terminal-level clear; do the
            // same once whenever the full-screen editor is entered or exited.
            terminal
                .clear()
                .map_err(|error| TuiError::Io(error.to_string()))?;
            editor_was_active = editor_is_active;
        }
        let snap = runtime.snapshot().await;
        let count = runtime.playlist_count();
        let term_size = terminal.size().map_err(|e| TuiError::Io(e.to_string()))?;
        let area = Rect::new(0, 0, term_size.width, term_size.height);
        // header(3) + transport(5) + footer(2); main fills the rest
        let main_height = area.height.saturating_sub(10).max(3);
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
                Constraint::Length(5),
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
                if runtime.input_mode == "editor" {
                    draw_editor_screen(f, f.area(), runtime);
                    return;
                }
                let root = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // header
                        Constraint::Min(5),    // main: playlist | visualizer
                        Constraint::Length(5), // three-row transport + borders
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
                        sort: runtime.playlist_sort,
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
                if runtime.input_mode == "browse" {
                    draw_browse_overlay(f, f.area(), runtime, &mut browse_scroll_offset);
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

fn editor_transitioned(was_active: bool, input_mode: &str) -> bool {
    was_active != (input_mode == "editor")
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
    sort: PlaylistSort,
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
        sort,
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
                let rest = playlist_row_columns(row, abs, area.width);
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
        format!(
            " Playlist ({total})  Track | Artist | Album  sort:{} ",
            sort.as_str()
        )
    } else {
        format!(
            " Find '{find_query}' ({total})  Track | Artist | Album  sort:{} ",
            sort.as_str()
        )
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(list, area);
}

fn playlist_row_columns(row: &tz_db::PlaylistRow, index: usize, area_width: u16) -> String {
    let track = row
        .title
        .clone()
        .or_else(|| {
            row.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| row.path.display().to_string());
    let prefix = format!("{:>4}  ", index + 1);
    // Borders consume two cells and the now-playing marker span consumes
    // three. Keep a compact Track-only fallback for very narrow terminals.
    let available = (area_width.saturating_sub(2) as usize)
        .saturating_sub(3)
        .saturating_sub(prefix.len());
    if available < 18 {
        return format!("{prefix}{}", fit_cell(&track, available));
    }

    let cell_space = available.saturating_sub(6); // two " | " separators
    let track_width = cell_space / 2;
    let artist_width = (cell_space - track_width) / 2;
    let album_width = cell_space - track_width - artist_width;
    format!(
        "{prefix}{} | {} | {}",
        fit_cell(&track, track_width),
        fit_cell(row.artist.as_deref().unwrap_or(""), artist_width),
        fit_cell(row.album.as_deref().unwrap_or(""), album_width),
    )
}

fn fit_cell(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if Line::from(value).width() <= width {
        let padding = width - Line::from(value).width();
        return format!("{value}{}", " ".repeat(padding));
    }

    let content_width = width.saturating_sub(1);
    let mut out = String::new();
    for ch in value.chars() {
        let mut candidate = out.clone();
        candidate.push(ch);
        if Line::from(candidate.as_str()).width() > content_width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    let padding = width.saturating_sub(Line::from(out.as_str()).width());
    out.push_str(&" ".repeat(padding));
    out
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
    let block = Block::default().borders(Borders::ALL).title(" Transport ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let position = format_time(snap.position_ms);
    let duration = format_time(snap.duration_ms);
    let time_value = format!("{position}/{duration}");
    let time_ratio = if snap.duration_ms > 0 {
        snap.position_ms as f64 / snap.duration_ms as f64
    } else {
        0.0
    };
    f.render_widget(
        Paragraph::new(slider_line(
            "TIME",
            time_value,
            time_ratio,
            rows[0].width,
            Color::Cyan,
        )),
        rows[0],
    );

    // Keep these as independent rectangles: future mouse handling can map
    // clicks directly to volume and speed without reworking transport layout.
    let controls = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    let volume_area = Rect {
        width: controls[0].width.saturating_sub(1),
        ..controls[0]
    };
    f.render_widget(
        Paragraph::new(slider_line(
            "VOL",
            format!("{}%", snap.volume),
            f64::from(snap.volume) / 100.0,
            volume_area.width,
            Color::Green,
        )),
        volume_area,
    );
    let speed_ratio = ((snap.speed - 0.5) / (4.0 - 0.5)).clamp(0.0, 1.0);
    f.render_widget(
        Paragraph::new(slider_line(
            "SPD",
            format!("{:.2}x", snap.speed),
            speed_ratio,
            controls[1].width,
            Color::Cyan,
        )),
        controls[1],
    );

    f.render_widget(
        Paragraph::new(transport_status_line(snap, rows[2].width)),
        rows[2],
    );
}

fn slider_line(
    label: &'static str,
    value: String,
    ratio: f64,
    width: u16,
    active_color: Color,
) -> Line<'static> {
    let fixed_width = label.chars().count() + value.chars().count() + 2;
    let track_width = (width as usize).saturating_sub(fixed_width);
    let mut spans = vec![Span::styled(
        format!("{label} "),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )];
    if track_width > 0 {
        let marker = if track_width == 1 {
            0
        } else {
            (ratio.clamp(0.0, 1.0) * (track_width - 1) as f64).round() as usize
        };
        if marker > 0 {
            spans.push(Span::styled(
                "━".repeat(marker),
                Style::default().fg(active_color),
            ));
        }
        spans.push(Span::styled("●", Style::default().fg(Color::White)));
        if marker + 1 < track_width {
            spans.push(Span::styled(
                "─".repeat(track_width - marker - 1),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    spans.push(Span::styled(
        format!(" {value}"),
        Style::default().fg(Color::Gray),
    ));
    Line::from(spans)
}

fn transport_status_line(snap: &tz_control::TransportSnapshot, width: u16) -> Line<'static> {
    let shuffle = if snap.shuffle { "on" } else { "off" };
    if width >= 52 {
        Line::from(vec![
            Span::styled(
                "Status: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(snap.status.clone(), state_style(snap.status == "playing")),
            Span::raw("  |  "),
            Span::styled(
                "Repeat: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                snap.repeat_mode.clone(),
                state_style(snap.repeat_mode != "off"),
            ),
            Span::raw("  |  "),
            Span::styled(
                "Shuffle: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(shuffle, state_style(snap.shuffle)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                snap.status.to_uppercase(),
                state_style(snap.status == "playing"),
            ),
            Span::raw("  REP:"),
            Span::styled(
                snap.repeat_mode.clone(),
                state_style(snap.repeat_mode != "off"),
            ),
            Span::raw("  SHUF:"),
            Span::styled(shuffle, state_style(snap.shuffle)),
        ])
    }
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
    } else if input_mode == "browse" {
        "Browse: Enter=open/add file  a/Space=add folder  Backspace=up  Esc=cancel".into()
    } else if input_mode == "help" {
        "Esc / q / any key — close help".into()
    } else {
        "↑/↓ Space n/p x ←/→ -/+ [] r/s f o a d c m z i g  Z=hide-viz  ?=help  q quit".into()
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    let help_style = if input_mode == "help" {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(help).style(help_style), rows[0]);

    if let Some(message) = msg.filter(|_| status_level != tz_core::StatusLevel::Info) {
        let (prefix, color) = match status_level {
            tz_core::StatusLevel::Error => ("[ERROR] ", Color::Red),
            tz_core::StatusLevel::Warn => ("[WARN] ", Color::Yellow),
            tz_core::StatusLevel::Info => unreachable!("informational messages are not rendered"),
        };
        f.render_widget(
            Paragraph::new(format!("{prefix}{message}")).style(Style::default().fg(color)),
            rows[1],
        );
    }
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
fn help_entry1(
    key_style: Style,
    desc_style: Style,
    k: &'static str,
    d: &'static str,
) -> Line<'static> {
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
        e2(
            "a",
            "Open playlist editor",
            "d / Del",
            "Edit playlist in editor",
        ),
        e2("c", "Open editor", "m", "Refresh metadata"),
        e2("f", "Find", "o", "Cycle view sort"),
        e2(
            "F10",
            "Apply editor changes",
            "Ctrl+Up/Down",
            "Reorder in editor",
        ),
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

fn draw_editor_screen(f: &mut ratatui::Frame<'_>, area: Rect, runtime: &AppRuntime) {
    // The editor replaces the normal app chrome rather than overlaying it.
    // Clear every cell first so sparse lists cannot leave playlist/visualizer
    // glyphs from the previous frame visible in their unused rows.
    f.render_widget(Clear, area);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(" Playlist editor  |  Tab: switch pane  F10: Apply  Esc: cancel"),
        root[0],
    );
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(root[1]);

    let left_title = runtime
        .browse_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Drives".into());
    let left_visible = panes[0].height.saturating_sub(2).max(1) as usize;
    let left_start = runtime
        .browse_cursor
        .saturating_sub(left_visible.saturating_sub(1));
    let left_items = runtime
        .browse_entries
        .iter()
        .enumerate()
        .skip(left_start)
        .take(left_visible)
        .map(|(i, e)| {
            let marker = if e.is_dir { "/" } else { "" };
            let style = if runtime.editor_focus == EditorFocus::Files && i == runtime.browse_cursor
            {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("{}{}", e.name, marker)).style(style)
        })
        .collect::<Vec<_>>();
    f.render_widget(
        List::new(left_items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Files — {left_title} ")),
        ),
        panes[0],
    );

    let count = runtime.editor_draft_count().unwrap_or(0);
    let right_visible = panes[1].height.saturating_sub(2).max(1) as usize;
    let right_start = runtime
        .editor_playlist_cursor
        .saturating_sub(right_visible.saturating_sub(1));
    let rows = runtime
        .editor_fetch_rows(right_start, right_visible)
        .unwrap_or_default();
    let right_items = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let absolute_index = right_start + i;
            let mut label = row
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            if row.missing {
                label.push_str(" [missing]");
            }
            let style = if runtime.editor_focus == EditorFocus::Playlist
                && absolute_index == runtime.editor_playlist_cursor
            {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("{:>4} {}", absolute_index + 1, label)).style(style)
        })
        .collect::<Vec<_>>();
    f.render_widget(
        List::new(right_items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Staged playlist ({count}) ")),
        ),
        panes[1],
    );
    let footer = match runtime.editor_overlay {
        EditorOverlay::SaveName | EditorOverlay::Rename => format!("Name: {}", runtime.input_buffer),
        EditorOverlay::Load => "Load: Up/Down choose, Enter load, Esc cancel".into(),
        EditorOverlay::DeleteConfirm => "Delete selected saved playlist? y/n".into(),
        EditorOverlay::PartialScanConfirm => "Add partial scan result? y/n".into(),
        _ => "i insert  a append  d delete  Ctrl+Up/Down move  s save  l load  r rename  D delete saved".into(),
    };
    f.render_widget(
        Paragraph::new(footer).block(Block::default().borders(Borders::TOP)),
        root[2],
    );
    if runtime.editor_overlay == EditorOverlay::Help {
        draw_help_overlay(f, area);
        return;
    }
    if runtime.editor_overlay == EditorOverlay::Load {
        let lists = runtime.editor_playlist_summaries().unwrap_or_default();
        let items = lists
            .iter()
            .enumerate()
            .map(|(i, p)| {
                ListItem::new(format!(
                    "{} {} ({})",
                    if i == runtime.editor_load_cursor {
                        ">"
                    } else {
                        " "
                    },
                    p.name,
                    p.track_count
                ))
            })
            .collect::<Vec<_>>();
        let popup = centered_fixed_rect(
            (area.width * 2 / 3).max(24).min(area.width),
            (area.height * 2 / 3).max(6).min(area.height),
            area,
        );
        f.render_widget(Clear, popup);
        f.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Load playlist "),
            ),
            popup,
        );
    }
}

/// Folder-browser modal: a single-pane directory listing (dirs then media
/// files, per `list_dir`), with the same manual cursor-highlight convention
/// as `draw_playlist` (no `ListState`). `scroll_offset` is clamped here,
/// against the same popup height used to render — computing both in one
/// place avoids the clamp/render size mismatch that bit `main_layout`
/// before it existed as a single shared function.
fn draw_browse_overlay(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    runtime: &AppRuntime,
    scroll_offset: &mut usize,
) {
    let title = match &runtime.browse_dir {
        Some(dir) => format!(" Add — {} ", dir.display()),
        None => " Add — select a drive ".to_string(),
    };
    let popup_w = (area.width * 7 / 10).clamp(20.min(area.width), area.width);
    let popup_h = (area.height * 7 / 10).clamp(6.min(area.height), area.height);
    let popup = centered_fixed_rect(popup_w, popup_h, area);
    let visible = popup.height.saturating_sub(2).max(1) as usize; // borders

    let cursor = runtime.browse_cursor;
    if cursor < *scroll_offset {
        *scroll_offset = cursor;
    } else if cursor >= *scroll_offset + visible {
        *scroll_offset = cursor + 1 - visible;
    }
    let offset = *scroll_offset;

    let items: Vec<ListItem> = if runtime.browse_entries.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  (empty)",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )))]
    } else {
        runtime
            .browse_entries
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
            .map(|(i, entry)| {
                let is_cursor = i == cursor;
                let label = if entry.is_dir {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                };
                let style = if is_cursor {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if entry.is_dir {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Gray)
                };
                ListItem::new(Line::from(Span::styled(format!(" {label}"), style)))
            })
            .collect()
    };

    f.render_widget(Clear, popup);
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(list, popup);
}

fn format_time(ms: u64) -> String {
    let total_s = ms / 1000;
    format!("{:02}:{:02}", total_s / 60, total_s % 60)
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

    if runtime.input_mode == "editor" {
        if runtime.editor_overlay == EditorOverlay::Help {
            runtime.editor_overlay = EditorOverlay::None;
            return Ok(false);
        }
        match runtime.editor_overlay {
            EditorOverlay::SaveName | EditorOverlay::Rename => {
                match code {
                    KeyCode::Esc => runtime.editor_overlay = EditorOverlay::None,
                    KeyCode::Backspace => {
                        runtime.input_buffer.pop();
                    }
                    KeyCode::Enter => {
                        let name = runtime.input_buffer.clone();
                        let result = if runtime.editor_overlay == EditorOverlay::Rename {
                            runtime.editor_commit_rename(name)
                        } else {
                            runtime.editor_commit_name(name, runtime.editor_save_as)
                        };
                        if let Err(e) = result {
                            runtime.set_warning(e);
                        }
                    }
                    KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                        runtime.input_buffer.push(c)
                    }
                    _ => {}
                }
                return Ok(false);
            }
            EditorOverlay::Load => {
                match code {
                    KeyCode::Esc => runtime.editor_overlay = EditorOverlay::None,
                    KeyCode::Up | KeyCode::Char('k') => runtime.editor_move_load_cursor(-1),
                    KeyCode::Down | KeyCode::Char('j') => runtime.editor_move_load_cursor(1),
                    KeyCode::Enter => {
                        if let Err(e) = runtime.editor_load_selected() {
                            runtime.set_warning(e);
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            EditorOverlay::DeleteConfirm
            | EditorOverlay::PartialScanConfirm
            | EditorOverlay::DiscardConfirm => {
                match code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let _ = runtime.handle(Command::EditorConfirm { yes: true }).await;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        let _ = runtime.handle(Command::EditorConfirm { yes: false }).await;
                    }
                    _ => {}
                }
                return Ok(false);
            }
            EditorOverlay::Help => {}
            EditorOverlay::None => {}
        }
        if code == KeyCode::Char('?') && runtime.editor_overlay == EditorOverlay::None {
            runtime.editor_overlay = EditorOverlay::Help;
            return Ok(false);
        }
        let cmd = match code {
            KeyCode::Esc => Some(Command::EditorCancel),
            KeyCode::Tab => Some(Command::EditorTab),
            KeyCode::Up if modifiers.contains(KeyModifiers::CONTROL) => Some(Command::EditorMoveUp),
            KeyCode::Down if modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Command::EditorMoveDown)
            }
            KeyCode::Up | KeyCode::Char('k') => Some(Command::EditorUp),
            KeyCode::Down | KeyCode::Char('j') => Some(Command::EditorDown),
            KeyCode::PageUp => Some(Command::EditorPageUp),
            KeyCode::PageDown => Some(Command::EditorPageDown),
            KeyCode::Home => Some(Command::EditorHome),
            KeyCode::End => Some(Command::EditorEnd),
            KeyCode::Backspace => Some(Command::EditorParent),
            KeyCode::Enter if modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Command::EditorApply)
            }
            KeyCode::Enter => Some(Command::EditorEnter),
            KeyCode::Char('~') => Some(Command::EditorDrives),
            KeyCode::Char('i') => Some(Command::EditorInsert),
            KeyCode::Char('a') => Some(Command::EditorAppend),
            KeyCode::Char('d') if !modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Command::EditorRemove)
            }
            KeyCode::Char('c') => Some(Command::EditorClear),
            KeyCode::Delete => Some(Command::EditorRemove),
            KeyCode::Char('s') => Some(Command::EditorSave),
            KeyCode::Char('S') => Some(Command::EditorSaveAs),
            KeyCode::Char('l') => Some(Command::EditorLoad),
            KeyCode::Char('r') => Some(Command::EditorRename),
            KeyCode::Char('D') => Some(Command::EditorDelete),
            KeyCode::Char('q') => Some(Command::EditorCancel),
            KeyCode::F(10) => Some(Command::EditorApply),
            _ => None,
        };
        if let Some(cmd) = cmd {
            let _ = runtime.handle(cmd).await;
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

    // Folder-browser modal
    if runtime.input_mode == "browse" {
        match code {
            KeyCode::Esc => {
                let _ = runtime.handle(Command::BrowseCancel).await;
            }
            KeyCode::Up => {
                let _ = runtime.handle(Command::BrowseUp).await;
            }
            KeyCode::Down => {
                let _ = runtime.handle(Command::BrowseDown).await;
            }
            KeyCode::Enter => {
                let _ = runtime.handle(Command::BrowseEnter).await;
            }
            KeyCode::Char('a') | KeyCode::Char(' ') => {
                let _ = runtime.handle(Command::BrowseSelect).await;
            }
            KeyCode::Backspace | KeyCode::Left => {
                let _ = runtime.handle(Command::BrowseParent).await;
            }
            _ => {}
        }
        return Ok(false);
    }

    // Text input mode (find)
    if runtime.input_mode == "find" {
        match code {
            KeyCode::Esc => {
                let _ = runtime.handle(Command::ClearFind).await;
                runtime.input_mode = "normal".into();
                runtime.input_buffer.clear();
                runtime.set_status("Cancelled");
            }
            KeyCode::Enter => {
                let q = runtime.input_buffer.clone();
                let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
                runtime.input_mode = "normal".into();
            }
            KeyCode::Backspace => {
                runtime.input_buffer.pop();
                let q = runtime.input_buffer.clone();
                let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
            }
            KeyCode::Char(c) => {
                runtime.input_buffer.push(c);
                let q = runtime.input_buffer.clone();
                let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
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
            let _ = runtime.handle(Command::EditorOpen).await;
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
        KeyCode::Char('o') => {
            let _ = runtime.handle(Command::CyclePlaylistSort).await;
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
            let _ = runtime.handle(Command::EditorOpen).await;
            runtime.editor_focus = EditorFocus::Playlist;
        }
        KeyCode::Char('c') => {
            let _ = runtime.handle(Command::EditorOpen).await;
            runtime.editor_focus = EditorFocus::Playlist;
            let _ = runtime.handle(Command::EditorClear).await;
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
                        sort: PlaylistSort::Playlist,
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

    #[test]
    fn playlist_rows_show_track_artist_and_album_columns() {
        let mut item = row(1, "Track One");
        item.artist = Some("Artist".into());
        item.album = Some("Album".into());

        let buf = render(&[item], 0, None);
        let text = row_text(&buf, 1);
        assert!(text.contains("Track One"), "row was: {text:?}");
        assert!(text.contains("Artist"), "row was: {text:?}");
        assert!(text.contains("Album"), "row was: {text:?}");
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
        terminal.draw(|f| draw_help_overlay(f, f.area())).unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());

        for needle in [
            "Keyboard Shortcuts",
            "Play / Pause",
            "Cycle repeat mode",
            "Toggle shuffle",
            "Reset speed to 1.0x",
            "Locate now-playing track",
            "Edit playlist in editor",
            "Refresh metadata",
            "Cycle view sort",
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

    #[tokio::test]
    async fn browse_overlay_shows_current_dir_and_highlights_cursor_entry() {
        let mut runtime = bare_test_runtime("browse_render").await;
        let dir = &runtime.paths.data_dir.clone();
        std::fs::write(dir.join("alpha.mp3"), b"").unwrap();
        std::fs::write(dir.join("beta.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(dir.clone());
        runtime.handle(Command::RequestAddFolder).await.unwrap();
        // Note: `bare_test_runtime` nests `log_dir` under `data_dir`, so
        // `browse_entries` is actually [logs/, alpha.mp3, beta.mp3] (dirs
        // sort first) and this second cursor step lands on alpha.mp3, not
        // beta.mp3 as the name suggests. Left as-is (matches the task
        // brief's test verbatim) since the assertions below don't depend
        // on which entry the cursor highlights — only that both filenames
        // are rendered.
        runtime.handle(Command::BrowseDown).await.unwrap();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut scroll = 0usize;
        terminal
            .draw(|f| draw_browse_overlay(f, f.area(), &runtime, &mut scroll))
            .unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());

        assert!(
            text.contains("alpha.mp3"),
            "expected both entries listed:\n{text}"
        );
        assert!(
            text.contains("beta.mp3"),
            "expected both entries listed:\n{text}"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn editor_screen_renders_both_files_and_staged_playlist_panes() {
        let mut runtime = bare_test_runtime("editor_render").await;
        let dir = runtime.paths.data_dir.clone();
        std::fs::write(dir.join("alpha.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(dir.clone());
        runtime.handle(Command::EditorOpen).await.unwrap();
        runtime.editor_focus = EditorFocus::Files;
        runtime.browse_cursor = runtime
            .browse_entries
            .iter()
            .position(|entry| entry.name == "alpha.mp3")
            .unwrap();
        runtime.handle(Command::EditorAppend).await.unwrap();

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_editor_screen(f, f.area(), &runtime))
            .unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(text.contains("Files"));
        assert!(text.contains("Staged playlist (1)"));
        assert!(text.contains("alpha.mp3"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn editor_screen_clears_previous_frame_content() {
        let mut runtime = bare_test_runtime("editor_clears_frame").await;
        runtime.handle(Command::EditorOpen).await.unwrap();

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let dirty = (0..24)
            .map(|_| "STALE_BACKGROUND")
            .collect::<Vec<_>>()
            .join("\n");
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new(dirty.as_str()), frame.area()))
            .unwrap();
        terminal
            .draw(|frame| draw_editor_screen(frame, frame.area(), &runtime))
            .unwrap();

        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(
            !text.contains("STALE_BACKGROUND"),
            "editor leaked content from the previous frame:\n{text}"
        );

        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[test]
    fn entering_and_exiting_editor_require_terminal_level_clear() {
        assert!(editor_transitioned(false, "editor"));
        assert!(editor_transitioned(true, "normal"));
        assert!(!editor_transitioned(true, "editor"));
        assert!(!editor_transitioned(false, "normal"));
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

    fn transport_snapshot_fixture() -> tz_control::TransportSnapshot {
        tz_control::TransportSnapshot {
            status: "playing".into(),
            position_ms: 107_000,
            duration_ms: 211_000,
            volume: 45,
            speed: 1.0,
            repeat_mode: "off".into(),
            shuffle: false,
            level_left: Some(0.09),
            level_right: Some(0.08),
            level_source: Some("Envelope".into()),
            analysis_status: Some("ESBW".into()),
            ..Default::default()
        }
    }

    fn render_transport_buffer(width: u16) -> Buffer {
        let backend = TestBackend::new(width, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let snapshot = transport_snapshot_fixture();
        terminal
            .draw(|frame| draw_transport(frame, frame.area(), &snapshot))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn transport_uses_three_stable_rows_without_analysis_diagnostics() {
        let buffer = render_transport_buffer(100);
        let time = row_text(&buffer, 1);
        let controls = row_text(&buffer, 2);
        let status = row_text(&buffer, 3);
        let all = buffer_text(&buffer);

        assert!(time.contains("TIME") && time.contains("01:47/03:31"));
        assert!(controls.contains("VOL") && controls.contains("SPD"));
        assert!(status.contains("Status: playing"));
        assert!(status.contains("Repeat: off"));
        assert!(status.contains("Shuffle: off"));
        assert!(!all.contains("Envelope"));
        assert!(!all.contains("ESBW"));
        assert!(!all.contains("L0.09"));
    }

    #[test]
    fn transport_abbreviates_status_without_wrapping_on_narrow_terminals() {
        let buffer = render_transport_buffer(36);
        assert!(row_text(&buffer, 1).contains("TIME"));
        let controls = row_text(&buffer, 2);
        assert!(controls.contains("VOL") && controls.contains("SPD"));
        let status = row_text(&buffer, 3);
        assert!(status.contains("PLAYING"));
        assert!(status.contains("REP:off"));
        assert!(status.contains("SHUF:off"));
    }

    #[test]
    fn footer_suppresses_chatter_but_keeps_actionable_warnings() {
        let backend = TestBackend::new(100, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_footer(
                    frame,
                    frame.area(),
                    Some("Playlist edits cancelled"),
                    tz_core::StatusLevel::Info,
                    "normal",
                    "",
                    false,
                )
            })
            .unwrap();
        let info = buffer_text(&terminal.backend().buffer().clone());
        assert!(!info.contains("Playlist edits cancelled"));

        terminal
            .draw(|frame| {
                draw_footer(
                    frame,
                    frame.area(),
                    Some("Metadata refresh failed"),
                    tz_core::StatusLevel::Warn,
                    "normal",
                    "",
                    false,
                )
            })
            .unwrap();
        let warning = buffer_text(&terminal.backend().buffer().clone());
        assert!(warning.contains("[WARN] Metadata refresh failed"));
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

        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Char('f'),
            KeyModifiers::NONE,
        )
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

        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )
        .await
        .unwrap();
        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )
        .await
        .unwrap();
        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )
        .await
        .unwrap();
        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )
        .await
        .unwrap();
        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )
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
        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Char('Z'),
            KeyModifiers::NONE,
        )
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
    async fn pressing_a_opens_the_browser_instead_of_the_old_text_prompt() {
        let mut runtime = bare_test_runtime("browse_open_key").await;
        let mut viz = VisualizerHost::new(false);

        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        )
        .await
        .unwrap();

        assert_eq!(runtime.input_mode, "editor");

        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn editor_opens_and_cancels_via_keys() {
        let mut runtime = bare_test_runtime("browse_keys_flow").await;
        let mut viz = VisualizerHost::new(false);

        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        )
        .await
        .unwrap();
        handle_key(&mut runtime, &mut viz, KeyCode::Esc, KeyModifiers::NONE)
            .await
            .unwrap();
        assert_eq!(runtime.input_mode, "normal");
        assert_eq!(runtime.playlist_count(), 0);
    }

    #[tokio::test]
    async fn esc_cancels_the_browser_without_adding_anything() {
        let mut runtime = bare_test_runtime("browse_keys_cancel").await;
        let dir = runtime.paths.data_dir.clone();
        std::fs::write(dir.join("track.mp3"), b"").unwrap();
        runtime.last_browse_dir = Some(dir.clone());
        let mut viz = VisualizerHost::new(false);

        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        )
        .await
        .unwrap();
        assert_eq!(runtime.browse_dir, Some(dir.clone()));
        handle_key(&mut runtime, &mut viz, KeyCode::Esc, KeyModifiers::NONE)
            .await
            .unwrap();

        assert_eq!(runtime.input_mode, "normal");
        assert_eq!(runtime.playlist_count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn plain_z_still_cycles_visualizer_and_leaves_the_pane_visible() {
        // Regression guard for arm ordering: the new shift-guarded arms sit
        // right above the plain 'z' cycle arm in the match — make sure they
        // don't swallow it.
        let mut runtime = bare_test_runtime("viz_cycle").await;
        let mut viz = VisualizerHost::new(false);
        let before = viz.active_id();

        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Char('z'),
            KeyModifiers::NONE,
        )
        .await
        .unwrap();

        assert_ne!(viz.active_id(), before);
        assert!(!runtime.visualizer_hidden);

        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }

    #[tokio::test]
    async fn o_cycles_the_playlist_view_sort_from_the_keyboard() {
        let mut runtime = bare_test_runtime("playlist_sort_key").await;
        let mut viz = VisualizerHost::new(false);
        assert_eq!(runtime.playlist_sort, PlaylistSort::Playlist);

        handle_key(
            &mut runtime,
            &mut viz,
            KeyCode::Char('o'),
            KeyModifiers::NONE,
        )
        .await
        .unwrap();

        assert_eq!(runtime.playlist_sort, PlaylistSort::Track);
        assert_eq!(runtime.app_state.playlist_sort, "track");
        let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
    }
}
