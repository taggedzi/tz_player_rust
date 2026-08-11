//! Native, device-independent streaming Symphonia decoder.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::default::{get_codecs, get_probe};

use crate::{clamp_sample, DecodeError, PcmSource, PcmSpec};

pub struct NativePcmSource {
    spec: PcmSpec,
    inner: NativeInner,
    position_frame: u64,
    duration_frames: Option<u64>,
    source_path: Option<PathBuf>,
}

enum NativeInner {
    Buffered { samples: Vec<f32> },
    Streaming(StreamState),
}

struct StreamState {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    source_spec: PcmSpec,
    requested_spec: PcmSpec,
    resample_phase: u64,
    pending: Vec<f32>,
    pending_offset: usize,
    finished: bool,
}

impl NativePcmSource {
    pub fn from_interleaved(spec: PcmSpec, samples: Vec<f32>) -> Result<Self, DecodeError> {
        if !samples.len().is_multiple_of(spec.frame_samples()) {
            return Err(DecodeError::UnalignedBuffer);
        }
        let samples = samples
            .into_iter()
            .map(clamp_sample)
            .collect::<Result<Vec<_>, _>>()?;
        let duration_frames = Some((samples.len() / spec.frame_samples()) as u64);
        Ok(Self {
            spec,
            inner: NativeInner::Buffered { samples },
            position_frame: 0,
            duration_frames,
            source_path: None,
        })
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }
}

impl PcmSource for NativePcmSource {
    fn spec(&self) -> PcmSpec {
        self.spec
    }

    fn duration_frames(&self) -> Option<u64> {
        self.duration_frames
    }

    fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, DecodeError> {
        if !output.len().is_multiple_of(self.spec.frame_samples()) {
            return Err(DecodeError::UnalignedBuffer);
        }
        let read = match &mut self.inner {
            NativeInner::Buffered { samples } => {
                let start = usize::try_from(self.position_frame)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(self.spec.frame_samples());
                if start >= samples.len() {
                    0
                } else {
                    let count = output.len().min(samples.len() - start);
                    output[..count].copy_from_slice(&samples[start..start + count]);
                    count
                }
            }
            NativeInner::Streaming(stream) => stream.read(output)?,
        };
        self.position_frame = self
            .position_frame
            .saturating_add((read / self.spec.frame_samples()) as u64);
        Ok(read)
    }

    fn seek_to_frame(&mut self, frame: u64) -> Result<(), DecodeError> {
        match &self.inner {
            NativeInner::Buffered { samples } => {
                self.position_frame = frame.min((samples.len() / self.spec.frame_samples()) as u64);
                Ok(())
            }
            NativeInner::Streaming(_) => {
                let path = self
                    .source_path
                    .clone()
                    .ok_or_else(|| DecodeError::Message("stream source path is missing".into()))?;
                let mut replacement = decode_native(&path, self.spec)?;
                let mut remaining = frame;
                let mut discard =
                    vec![0.0; 16_384 / self.spec.frame_samples() * self.spec.frame_samples()];
                while remaining > 0 {
                    let requested_samples = usize::try_from(remaining)
                        .unwrap_or(usize::MAX)
                        .saturating_mul(self.spec.frame_samples())
                        .min(discard.len());
                    let read = replacement.read_interleaved(&mut discard[..requested_samples])?;
                    if read == 0 {
                        break;
                    }
                    remaining = remaining.saturating_sub((read / self.spec.frame_samples()) as u64);
                }
                *self = replacement;
                Ok(())
            }
        }
    }
}

impl StreamState {
    fn read(&mut self, output: &mut [f32]) -> Result<usize, DecodeError> {
        let mut written = 0;
        while written < output.len() {
            if self.pending_offset < self.pending.len() {
                let count = (self.pending.len() - self.pending_offset).min(output.len() - written);
                output[written..written + count].copy_from_slice(
                    &self.pending[self.pending_offset..self.pending_offset + count],
                );
                self.pending_offset += count;
                written += count;
                continue;
            }
            if self.finished {
                break;
            }
            self.decode_next_packet()?;
        }
        Ok(written)
    }

    fn decode_next_packet(&mut self) -> Result<(), DecodeError> {
        self.pending.clear();
        self.pending_offset = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.finished = true;
                    return Ok(());
                }
                Err(SymphoniaError::DecodeError(error)) => {
                    return Err(DecodeError::Message(format!("decode: {error}")));
                }
                Err(error) => return Err(DecodeError::Message(format!("read: {error}"))),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = self
                .decoder
                .decode(&packet)
                .map_err(|error| DecodeError::Message(error.to_string()))?;
            let actual = PcmSpec::new(decoded.spec().rate, decoded.spec().channels.count() as u16)?;
            if actual != self.source_spec {
                return Err(DecodeError::Message(
                    "native audio parameters changed during decode".into(),
                ));
            }
            let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
            samples.copy_interleaved_ref(decoded);
            self.convert_packet(samples.samples())?;
            if !self.pending.is_empty() {
                return Ok(());
            }
        }
    }

    fn convert_packet(&mut self, samples: &[f32]) -> Result<(), DecodeError> {
        for frame in samples.chunks_exact(self.source_spec.frame_samples()) {
            self.resample_phase = self
                .resample_phase
                .saturating_add(u64::from(self.requested_spec.sample_rate));
            while self.resample_phase >= u64::from(self.source_spec.sample_rate) {
                for channel in 0..self.requested_spec.channels {
                    let source_channel = usize::from(channel)
                        .min(usize::from(self.source_spec.channels.saturating_sub(1)));
                    self.pending.push(clamp_sample(frame[source_channel])?);
                }
                self.resample_phase -= u64::from(self.source_spec.sample_rate);
            }
        }
        Ok(())
    }
}

pub fn decode_native(path: &Path, requested: PcmSpec) -> Result<NativePcmSource, DecodeError> {
    let mut file = File::open(path)
        .map_err(|error| DecodeError::Message(format!("open {}: {error}", path.display())))?;
    let source_len = file.metadata().ok().map(|metadata| metadata.len());
    if let Some(family) = probe_helper_only_file(&mut file, path)? {
        return Err(DecodeError::Message(format!(
            "native decoder does not support recognized {family} content"
        )));
    }
    let media = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let probed = get_probe()
        .format(
            &hint,
            media,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| map_decode_error(error, source_len))?;
    let format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| DecodeError::Message("no audio stream".into()))?;
    let source_spec = PcmSpec::new(
        track.codec_params.sample_rate.unwrap_or(0),
        track
            .codec_params
            .channels
            .map(|channels| channels.count() as u16)
            .unwrap_or(0),
    )?;
    let track_id = track.id;
    let duration_frames = track.codec_params.n_frames.map(|frames| {
        frames.saturating_mul(u64::from(requested.sample_rate)) / u64::from(source_spec.sample_rate)
    });
    let decoder = get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| map_decode_error(error, source_len))?;
    Ok(NativePcmSource {
        spec: requested,
        inner: NativeInner::Streaming(StreamState {
            format,
            decoder,
            track_id,
            source_spec,
            requested_spec: requested,
            resample_phase: 0,
            pending: Vec::new(),
            pending_offset: 0,
            finished: false,
        }),
        position_frame: 0,
        duration_frames,
        source_path: Some(path.to_path_buf()),
    })
}

/// Bounded content probe for formats deliberately assigned to the helper.
/// This does not consult the filename extension.
pub fn probe_helper_only_content(path: &Path) -> Result<Option<&'static str>, DecodeError> {
    let mut file = File::open(path)
        .map_err(|error| DecodeError::Message(format!("open {}: {error}", path.display())))?;
    probe_helper_only_file(&mut file, path)
}

fn probe_helper_only_file(
    file: &mut File,
    path: &Path,
) -> Result<Option<&'static str>, DecodeError> {
    let mut prefix = [0_u8; 64 * 1024];
    let prefix_len = file
        .read(&mut prefix)
        .map_err(|error| DecodeError::Message(format!("read {}: {error}", path.display())))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| DecodeError::Message(format!("rewind {}: {error}", path.display())))?;
    Ok(helper_only_family(&prefix[..prefix_len]))
}

fn helper_only_family(prefix: &[u8]) -> Option<&'static str> {
    const ASF_HEADER: [u8; 16] = [
        0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce,
        0x6c,
    ];
    if prefix.starts_with(b"MAC ") {
        return Some("Monkey's Audio");
    }
    if prefix.starts_with(b"wvpk") {
        return Some("WavPack");
    }
    if prefix.starts_with(b"TTA1") {
        return Some("TTA");
    }
    if prefix.starts_with(b"MPCK") || prefix.starts_with(b"MP+") {
        return Some("Musepack");
    }
    if prefix.starts_with(&ASF_HEADER) {
        return Some("ASF/WMA");
    }
    if prefix.starts_with(b"OggS") {
        if prefix.windows(8).any(|window| window == b"OpusHead") {
            return Some("Ogg Opus");
        }
        if prefix.windows(8).any(|window| window == b"Speex   ") {
            return Some("Ogg Speex");
        }
    }
    if prefix.starts_with(&[0x0b, 0x77]) {
        return Some("AC-3/E-AC-3");
    }
    if [
        [0x7f, 0xfe, 0x80, 0x01],
        [0xfe, 0x7f, 0x01, 0x80],
        [0x1f, 0xff, 0xe8, 0x00],
        [0xff, 0x1f, 0x00, 0xe8],
    ]
    .iter()
    .any(|sync| prefix.starts_with(sync))
    {
        return Some("DTS");
    }
    None
}

fn map_decode_error(error: SymphoniaError, _source_len: Option<u64>) -> DecodeError {
    DecodeError::Message(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tz-playback/tests/fixtures")
            .join(name)
    }

    #[test]
    fn native_decode_streams_and_seeks_without_materializing_the_track() {
        let spec = PcmSpec::new(44_100, 2).unwrap();
        let mut source = decode_native(&fixture("tone.wav"), spec).unwrap();
        assert_eq!(source.source_path(), Some(fixture("tone.wav").as_path()));
        assert!(matches!(source.inner, NativeInner::Streaming(_)));

        let mut buffer = vec![0.0; 2_048];
        assert_eq!(source.read_interleaved(&mut buffer).unwrap(), buffer.len());
        source.seek_to_frame(4_410).unwrap();
        assert_eq!(source.position_frame, 4_410);
        assert_eq!(source.read_interleaved(&mut buffer).unwrap(), buffer.len());
        assert!(buffer.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn helper_only_detection_uses_content_not_filename_extensions() {
        let spec = PcmSpec::new(44_100, 2).unwrap();
        for name in [
            "tone-opus.ogg",
            "tone-wma.wma",
            "tone-wavpack.wv",
            "tone-ac3.ac3",
            "tone-eac3.eac3",
            "tone-dts.dts",
            "tone-tta.tta",
            "tone-speex.ogg",
            "tone-ape.ape",
            "tone-musepack7.mpc",
            "tone-musepack8.mpc",
        ] {
            let error = match decode_native(&fixture(name), spec) {
                Ok(_) => panic!("{name} unexpectedly selected the native decoder"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("recognized"), "{name}: {error}");
        }
    }
}
