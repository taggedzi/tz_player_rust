//! Headless rendering support for the workspace benchmark runner.
//!
//! This module is feature-gated so benchmark plumbing does not become part of
//! the normal application build. It deliberately exercises the same private
//! draw functions as the interactive loop.

use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Terminal;
use tz_control::TransportSnapshot;
use tz_db::{PlaylistRow, PlaylistSort};

use crate::theme::TuiTheme;
use crate::visualizers::{VisualizerFrameInput, VisualizerHost};
use crate::{
    draw_footer, draw_header, draw_playlist, draw_transport, draw_visualizer, main_layout,
    PlaylistView,
};

/// Reusable, terminal-independent representation of an idle TUI frame.
pub struct IdleFrameBenchmark {
    terminal: Terminal<TestBackend>,
    rows: Vec<PlaylistRow>,
    snapshot: TransportSnapshot,
    theme: TuiTheme,
    visualizer: VisualizerHost,
    visualizer_hidden: bool,
}

impl IdleFrameBenchmark {
    pub fn new(
        width: u16,
        height: u16,
        playlist_count: usize,
        visualizer_id: &str,
        visualizer_hidden: bool,
    ) -> Result<Self, String> {
        let terminal =
            Terminal::new(TestBackend::new(width, height)).map_err(|error| error.to_string())?;
        let visible_rows = usize::from(height.saturating_sub(12).max(1));
        let rows = (0..visible_rows)
            .map(|index| PlaylistRow {
                item_id: index as i64 + 1,
                track_id: index as i64 + 1,
                pos_key: (index as i64 + 1) * 10_000,
                path: PathBuf::from(format!(
                    "benchmark/artist_{:03}/album_{:02}/track_{index:05}.flac",
                    index % 100,
                    index % 20
                )),
                title: Some(format!("Benchmark Track {index:05}")),
                artist: Some(format!("Artist {:03}", index % 100)),
                album: Some(format!("Album {:02}", index % 20)),
                year: Some(2026),
                duration_ms: Some(240_000),
                meta_valid: Some(true),
                meta_error: None,
            })
            .collect();

        let snapshot = TransportSnapshot {
            status: "playing".into(),
            position_ms: 61_250,
            duration_ms: 240_000,
            volume: 72,
            speed: 1.0,
            repeat_mode: "off".into(),
            shuffle: false,
            item_id: Some(4),
            track_path: Some("benchmark/artist_003/album_03/track_00003.flac".into()),
            title: Some("Benchmark Track 00003".into()),
            artist: Some("Artist 003".into()),
            album: Some("Album 03".into()),
            backend: "fake".into(),
            playlist_id: Some(1),
            playlist_count,
            cursor_index: 3,
            level_left: Some(0.42),
            level_right: Some(0.36),
            level_source: Some("benchmark".into()),
            spectrum_bands: Some(
                (0..48)
                    .map(|index| ((index * 37 + 19) % 256) as u8)
                    .collect(),
            ),
            spectrum_source: Some("benchmark".into()),
            beat_strength: Some(0.71),
            beat_is_onset: Some(true),
            beat_bpm: Some(120.0),
            beat_source: Some("benchmark".into()),
            waveform_min_left: Some(-0.6),
            waveform_max_left: Some(0.7),
            waveform_min_right: Some(-0.5),
            waveform_max_right: Some(0.6),
            waveform_source: Some("benchmark".into()),
            waveform_history: Some(
                (0..64)
                    .map(|index| {
                        let phase = index as f32 / 64.0;
                        (-phase, phase, -phase * 0.8, phase * 0.8)
                    })
                    .collect(),
            ),
            analysis_status: Some("ESBW".into()),
            visualizer_id: Some(visualizer_id.into()),
            ..TransportSnapshot::default()
        };

        Ok(Self {
            terminal,
            rows,
            snapshot,
            theme: TuiTheme::default(),
            visualizer: VisualizerHost::new(true).with_plugin_id(Some(visualizer_id)),
            visualizer_hidden,
        })
    }

    /// Render one complete frame and return a checksum consumed by the caller.
    pub fn render(&mut self) -> Result<u64, String> {
        let size = self.terminal.size().map_err(|error| error.to_string())?;
        let area = Rect::new(0, 0, size.width, size.height);
        let layout_root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(5),
                Constraint::Length(2),
            ])
            .split(area);
        let (playlist_panel, visualizer_panel) =
            main_layout(layout_root[1], self.visualizer_hidden);

        let visualizer_lines = visualizer_panel.map_or_else(Vec::new, |panel| {
            self.visualizer.render(VisualizerFrameInput {
                frame_index: 0,
                width: panel.width.saturating_sub(2).max(1),
                height: panel.height.saturating_sub(2).max(1),
                status: self.snapshot.status.clone(),
                position_s: self.snapshot.position_ms as f64 / 1000.0,
                duration_s: Some(self.snapshot.duration_ms as f64 / 1000.0),
                volume: f64::from(self.snapshot.volume) / 100.0,
                speed: self.snapshot.speed,
                title: self.snapshot.title.clone(),
                track_path: self.snapshot.track_path.clone(),
                level_left: self.snapshot.level_left,
                level_right: self.snapshot.level_right,
                level_source: self.snapshot.level_source.clone(),
                spectrum_bands: self.snapshot.spectrum_bands.clone(),
                spectrum_source: self.snapshot.spectrum_source.clone(),
                beat_strength: self.snapshot.beat_strength,
                beat_is_onset: self.snapshot.beat_is_onset,
                beat_bpm: self.snapshot.beat_bpm,
                beat_source: self.snapshot.beat_source.clone(),
                waveform_min_left: self.snapshot.waveform_min_left,
                waveform_max_left: self.snapshot.waveform_max_left,
                waveform_min_right: self.snapshot.waveform_min_right,
                waveform_max_right: self.snapshot.waveform_max_right,
                waveform_source: self.snapshot.waveform_source.clone(),
                waveform_history: self.snapshot.waveform_history.clone(),
            })
        });

        let rows = &self.rows;
        let snapshot = &self.snapshot;
        let theme = &self.theme;
        let visualizer_name = self.visualizer.active_name();
        self.terminal
            .draw(|frame| {
                draw_header(frame, layout_root[0], snapshot, visualizer_name);
                draw_playlist(
                    frame,
                    playlist_panel,
                    rows,
                    PlaylistView {
                        cursor_index: snapshot.cursor_index,
                        scroll_offset: 0,
                        total: snapshot.playlist_count,
                        find_query: "",
                        playing_item_id: snapshot.item_id,
                        sort: PlaylistSort::Playlist,
                    },
                );
                if let Some(panel) = visualizer_panel {
                    draw_visualizer(frame, panel, &visualizer_lines, visualizer_name);
                }
                draw_transport(frame, layout_root[2], snapshot);
                draw_footer(
                    frame,
                    layout_root[3],
                    None,
                    tz_core::StatusLevel::Info,
                    "normal",
                    "",
                    false,
                );
                theme.apply_buffer(frame.buffer_mut());
            })
            .map_err(|error| error.to_string())?;

        Ok(self
            .terminal
            .backend()
            .buffer()
            .content
            .iter()
            .flat_map(|cell| cell.symbol().bytes())
            .fold(0u64, |checksum, byte| {
                checksum.wrapping_mul(16_777_619) ^ u64::from(byte)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_benchmark_frame_renders() {
        let mut benchmark =
            IdleFrameBenchmark::new(100, 30, 10_000, "spectrum.bars", false).unwrap();
        assert_ne!(benchmark.render().unwrap(), 0);
    }
}
