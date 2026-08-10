//! Visualizer plugin contract and host (registry + lifecycle).

use ratatui::style::Color;
use ratatui::text::Line;

/// Shared activation context.
#[derive(Debug, Clone, Copy)]
pub struct VisualizerContext {
    pub ansi_enabled: bool,
}

/// Per-frame input (mirrors Python VisualizerFrameInput subset).
#[derive(Debug, Clone)]
pub struct VisualizerFrameInput {
    pub frame_index: u64,
    pub width: u16,
    pub height: u16,
    pub status: String,
    pub position_s: f64,
    pub duration_s: Option<f64>,
    pub volume: f64,
    pub speed: f64,
    pub title: Option<String>,
    pub track_path: Option<String>,
    pub level_left: Option<f32>,
    pub level_right: Option<f32>,
    pub level_source: Option<String>,
    pub spectrum_bands: Option<Vec<u8>>,
    pub spectrum_source: Option<String>,
    pub beat_strength: Option<f32>,
    pub beat_is_onset: Option<bool>,
    pub beat_bpm: Option<f32>,
    pub beat_source: Option<String>,
    pub waveform_min_left: Option<f32>,
    pub waveform_max_left: Option<f32>,
    pub waveform_min_right: Option<f32>,
    pub waveform_max_right: Option<f32>,
    pub waveform_source: Option<String>,
    /// Recent (min_left, max_left, min_right, max_right) buckets, oldest first.
    pub waveform_history: Option<Vec<(f32, f32, f32, f32)>>,
}

/// Plugin trait for built-in visualizers.
///
/// Renders return ratatui `Line`s with styles (not ANSI strings — ratatui does
/// not interpret SGR escapes).
pub trait VisualizerPlugin: Send {
    fn plugin_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn on_activate(&mut self, context: VisualizerContext);
    fn on_deactivate(&mut self);
    fn render(&mut self, frame: &VisualizerFrameInput) -> Vec<Line<'static>>;
}

/// Host that owns the active plugin and cycles through built-ins.
pub struct VisualizerHost {
    plugins: Vec<Box<dyn VisualizerPlugin>>,
    active: usize,
    frame_index: u64,
    ansi_enabled: bool,
}

impl VisualizerHost {
    pub fn new(ansi_enabled: bool) -> Self {
        use crate::visualizers::*;
        let mut host = Self {
            plugins: vec![
                // Core
                Box::new(BasicVisualizer::default()),
                Box::new(VuReactiveVisualizer::default()),
                Box::new(SpectrumBarsVisualizer::default()),
                Box::new(SpectrogramWaterfallVisualizer::default()),
                Box::new(AudioTerrainVisualizer::default()),
                Box::new(RadialSpectrumVisualizer::default()),
                // Matrix themes
                Box::new(MatrixGreenVisualizer::default()),
                Box::new(MatrixBlueVisualizer::default()),
                Box::new(MatrixRedVisualizer::default()),
                // Waveform
                Box::new(WaveformProxyVisualizer::default()),
                Box::new(WaveformNeonVisualizer::default()),
                // Ops / typography / cover
                Box::new(HackScopeVisualizer::default()),
                Box::new(TypographyGlitchVisualizer::default()),
                Box::new(CoverAsciiStaticVisualizer::default()),
                Box::new(CoverAsciiMotionVisualizer::default()),
                // Particle pack
                Box::new(ParticleReactorVisualizer::default()),
                Box::new(GravityWellVisualizer::default()),
                Box::new(ShockwaveRingsVisualizer::default()),
                Box::new(ReactiveRainVisualizer::default()),
                Box::new(OrbitalSystemVisualizer::default()),
                Box::new(EmberFieldVisualizer::default()),
                Box::new(MagneticGridVisualizer::default()),
                Box::new(AudioTornadoVisualizer::default()),
                Box::new(ConstellationVisualizer::default()),
                Box::new(DataCoreFragVisualizer::default()),
                Box::new(PlasmaStreamVisualizer::default()),
            ],
            active: 0,
            frame_index: 0,
            ansi_enabled,
        };
        host.activate_current();
        host
    }

    pub fn with_plugin_id(mut self, id: Option<&str>) -> Self {
        if let Some(id) = id {
            if let Some(idx) = self.plugins.iter().position(|p| p.plugin_id() == id) {
                self.plugins[self.active].on_deactivate();
                self.active = idx;
                self.activate_current();
            }
        }
        self
    }

    fn activate_current(&mut self) {
        let ctx = VisualizerContext {
            ansi_enabled: self.ansi_enabled,
        };
        self.plugins[self.active].on_activate(ctx);
    }

    pub fn active_id(&self) -> &'static str {
        self.plugins[self.active].plugin_id()
    }

    pub fn active_name(&self) -> &'static str {
        self.plugins[self.active].display_name()
    }

    pub fn cycle(&mut self) -> &'static str {
        self.plugins[self.active].on_deactivate();
        self.active = (self.active + 1) % self.plugins.len();
        self.activate_current();
        self.active_id()
    }

    pub fn render(&mut self, mut frame: VisualizerFrameInput) -> Vec<Line<'static>> {
        self.frame_index = self.frame_index.saturating_add(1);
        frame.frame_index = self.frame_index;
        // Panel chrome already shows the visualizer name — give plugins the full
        // content box so centered fields (radial / particles) sit on the true midpoint.
        let max_h = frame.height.max(1) as usize;
        let mut lines = self.plugins[self.active].render(&frame);
        if lines.len() > max_h {
            lines.truncate(max_h);
        }
        lines
    }
}

/// Heatmap color for intensity 0.0..=1.0 (green → yellow → red/cyan accents).
pub fn heat_color(level: f32, color: bool) -> Color {
    if !color {
        return Color::Gray;
    }
    let t = level.clamp(0.0, 1.0);
    if t < 0.33 {
        let u = t / 0.33;
        Color::Rgb(20, (80.0 + 150.0 * u) as u8, (60.0 + 80.0 * u) as u8)
    } else if t < 0.66 {
        let u = (t - 0.33) / 0.33;
        Color::Rgb(
            (40.0 + 200.0 * u) as u8,
            (200.0 - 40.0 * u) as u8,
            (40.0 - 20.0 * u) as u8,
        )
    } else {
        let u = (t - 0.66) / 0.34;
        Color::Rgb(255, (160.0 - 100.0 * u) as u8, (30.0 + 20.0 * u) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_inventory_preserves_every_python_visualizer_id() {
        let host = VisualizerHost::new(false);
        let mut ids: Vec<_> = host
            .plugins
            .iter()
            .map(|plugin| plugin.plugin_id())
            .collect();
        ids.sort_unstable();

        assert_eq!(
            ids,
            [
                "basic",
                "cover.ascii.motion",
                "cover.ascii.static",
                "matrix.blue",
                "matrix.green",
                "matrix.red",
                "ops.hackscope",
                "spectrum.bars",
                "viz.particle.audio_tornado",
                "viz.particle.constellation",
                "viz.particle.data_core_frag",
                "viz.particle.ember_field",
                "viz.particle.gravity_well",
                "viz.particle.magnetic_grid",
                "viz.particle.orbital_system",
                "viz.particle.plasma_stream",
                "viz.particle.rain_reactive",
                "viz.particle.shockwave_rings",
                "viz.reactor.particles",
                "viz.spectrogram.waterfall",
                "viz.spectrum.radial",
                "viz.spectrum.terrain",
                "viz.typography.glitch",
                "viz.waveform.neon",
                "viz.waveform.proxy",
                "vu.reactive",
            ]
        );
    }
}
