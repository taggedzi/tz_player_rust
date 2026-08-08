//! Built-in terminal visualizers.

mod basic;
mod cover_ascii;
mod hackscope;
mod host;
mod matrix;
mod neon;
mod particles;
mod radial;
mod spectrum_bars;
mod terrain;
mod typography;
mod util;
mod vu;
mod waterfall;
mod waveproxy;

pub use basic::BasicVisualizer;
pub use cover_ascii::{CoverAsciiMotionVisualizer, CoverAsciiStaticVisualizer};
pub use hackscope::HackScopeVisualizer;
pub use host::{VisualizerFrameInput, VisualizerHost};
pub use matrix::{MatrixBlueVisualizer, MatrixGreenVisualizer, MatrixRedVisualizer};
pub use neon::WaveformNeonVisualizer;
pub use particles::{
    AudioTornadoVisualizer, ConstellationVisualizer, DataCoreFragVisualizer, EmberFieldVisualizer,
    GravityWellVisualizer, MagneticGridVisualizer, OrbitalSystemVisualizer,
    ParticleReactorVisualizer, PlasmaStreamVisualizer, ReactiveRainVisualizer,
    ShockwaveRingsVisualizer,
};
pub use radial::RadialSpectrumVisualizer;
pub use spectrum_bars::SpectrumBarsVisualizer;
pub use terrain::AudioTerrainVisualizer;
pub use typography::TypographyGlitchVisualizer;
pub use vu::VuReactiveVisualizer;
pub use waterfall::SpectrogramWaterfallVisualizer;
pub use waveproxy::WaveformProxyVisualizer;
