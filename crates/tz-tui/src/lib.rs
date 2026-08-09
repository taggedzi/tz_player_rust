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
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
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
        // that make the geometric center appear to bounce).
        let layout_root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(4),
                Constraint::Length(2),
            ])
            .split(area);
        let layout_main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(layout_root[1]);
        let viz_panel = layout_main[1];
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
        let viz_lines = viz.render(frame_in);

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

                let main = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(50), // playlist
                        Constraint::Percentage(50), // visualizer
                    ])
                    .split(root[1]);

                draw_header(f, root[0], &snap, viz.active_name());
                draw_playlist(
                    f,
                    main[0],
                    &rows,
                    PlaylistView {
                        cursor_index: runtime.cursor_index,
                        scroll_offset,
                        total: count,
                        find_query: &runtime.find_query,
                        playing_item_id: snap.item_id,
                    },
                );
                draw_visualizer(f, main[1], &viz_lines, viz.active_name());
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
    let text = format!(
        " {}  {} {} {}  vol {}%  speed {:.2}x  rep {}  shuf {}{lvl}{analysis} ",
        snap.status.to_uppercase(),
        pos,
        bar,
        dur,
        snap.volume,
        snap.speed,
        snap.repeat_mode,
        if snap.shuffle { "on" } else { "off" }
    );
    let mut lines = vec![Line::from(text)];
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
        format!("Find: {input_buffer}_   (Enter=apply Esc=cancel)")
    } else if input_mode == "add_path" {
        format!("Add path: {input_buffer}_   (Enter=add Esc=cancel)")
    } else if input_mode == "help" {
        "HELP: ↑↓ cursor  Space play  n/p next/prev  ←→ seek  -/+ vol  [] speed  f find  a add  z viz  i about  g locate playing  Esc/q close"
            .into()
    } else {
        "↑/↓ Space n/p x ←/→ -/+ [] r/s f a d c m z i g  ?=help  q quit".into()
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
            }
            KeyCode::Char(c) => {
                runtime.input_buffer.push(c);
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
}
