use std::io::Write;
use std::path::Path;

use ffmpeg::{codec, format, frame, media, software};
use ffmpeg_next as ffmpeg;
use ffmpeg_sys_next as sys;
use tz_audio::{write_decode_header, DecodeHeader, ExitCode, PcmSpec, SampleFormat};

use crate::avio::LocalInput;

struct LeadingTrim {
    target_frame: i128,
    next_frame: Option<i128>,
    timeline_origin: i64,
    time_base: ffmpeg::Rational,
    sample_rate: u32,
    seek_padding_frames: i128,
}

impl LeadingTrim {
    fn new(
        start_frame: u64,
        timeline_origin: i64,
        time_base: ffmpeg::Rational,
        sample_rate: u32,
        seek_padding_frames: u64,
    ) -> Self {
        Self {
            target_frame: i128::from(start_frame),
            next_frame: None,
            timeline_origin,
            time_base,
            sample_rate,
            seek_padding_frames: i128::from(seek_padding_frames),
        }
    }

    fn begin_decoded_frame(&mut self, decoded: &frame::Audio) -> Result<(), (ExitCode, String)> {
        if self.next_frame.is_some() {
            return Ok(());
        }
        let timestamp = decoded.timestamp().or_else(|| decoded.pts()).ok_or((
            ExitCode::Decode,
            "FFmpeg did not provide timestamps needed for an exact seek".into(),
        ))?;
        let relative_timestamp = i128::from(timestamp.saturating_sub(self.timeline_origin));
        let numerator = relative_timestamp
            .saturating_mul(i128::from(self.time_base.0))
            .saturating_mul(i128::from(self.sample_rate));
        self.next_frame = Some(
            div_floor(numerator, i128::from(self.time_base.1).max(1))
                .saturating_sub(self.seek_padding_frames),
        );
        Ok(())
    }

    fn skip_for(&mut self, frame_count: usize) -> usize {
        let next_frame = self.next_frame.unwrap_or(self.target_frame);
        let available = i128::try_from(frame_count).unwrap_or(i128::MAX);
        let skip = self
            .target_frame
            .saturating_sub(next_frame)
            .clamp(0, available);
        self.next_frame = Some(next_frame.saturating_add(available));
        usize::try_from(skip).unwrap_or(frame_count)
    }
}

fn div_floor(numerator: i128, denominator: i128) -> i128 {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder != 0 && numerator < 0 {
        quotient - 1
    } else {
        quotient
    }
}

pub fn decode(
    path: &Path,
    start_ms: u64,
    sample_rate: u32,
    channels: u16,
) -> Result<(), (ExitCode, String)> {
    let spec = PcmSpec::new(sample_rate, channels)
        .map_err(|error| (ExitCode::InvalidArguments, error.to_string()))?;
    ffmpeg::init().map_err(|error| {
        (
            ExitCode::LibraryCompatibility,
            format!("FFmpeg initialization failed: {error}"),
        )
    })?;
    let mut input = LocalInput::open(path)?;
    let stream = input.streams().best(media::Type::Audio).ok_or((
        ExitCode::UnsupportedMedia,
        "input has no audio stream".into(),
    ))?;
    let stream_index = stream.index();
    let time_base = stream.time_base();
    let stream_start = if stream.start_time() == sys::AV_NOPTS_VALUE {
        0
    } else {
        stream.start_time()
    };
    let duration = stream.duration();
    let parameters = stream.parameters();
    let codec_name = parameters.id().name();

    // Establish frame zero from the first sample the decoder can actually
    // publish. Some codecs (notably WMA and Opus) begin their decoded timeline
    // after the demuxer's stream start because of codec delay or pre-skip.
    // Calibrating once lets every later seek use the same output-frame timeline
    // as a decode from the beginning. A separate context prevents the probe
    // from disturbing demuxers with coarse or unreliable backward seeking.
    let (timeline_origin, seek_padding_frames) = {
        let mut probe_input = LocalInput::open(path)?;
        let probe_stream = probe_input.streams().best(media::Type::Audio).ok_or((
            ExitCode::UnsupportedMedia,
            "input has no audio stream".into(),
        ))?;
        let probe_index = probe_stream.index();
        let probe_context = codec::context::Context::from_parameters(probe_stream.parameters())
            .map_err(|error| (ExitCode::UnsupportedMedia, error.to_string()))?;
        let mut probe_decoder = probe_context
            .decoder()
            .audio()
            .map_err(|error| (ExitCode::UnsupportedMedia, error.to_string()))?;
        let mut first_decoded = frame::Audio::empty();
        let mut origin = None;
        let mut first_samples = None;
        let mut nominal_samples = 0usize;
        let mut decoded_rate = 1u32;
        'probe: for (packet_stream, packet) in probe_input.packets() {
            if packet_stream.index() != probe_index {
                continue;
            }
            probe_decoder
                .send_packet(&packet)
                .map_err(|error| (ExitCode::Decode, error.to_string()))?;
            while probe_decoder.receive_frame(&mut first_decoded).is_ok() {
                if origin.is_none() {
                    origin = first_decoded.timestamp().or_else(|| first_decoded.pts());
                    first_samples = Some(first_decoded.samples());
                    decoded_rate = first_decoded.rate().max(1);
                }
                nominal_samples = nominal_samples.max(first_decoded.samples());
                if first_samples.is_some() && nominal_samples > first_samples.unwrap_or(0) {
                    break 'probe;
                }
            }
        }
        let origin = origin.ok_or((
            ExitCode::UnsupportedMedia,
            "audio stream produced no timestamped frames".into(),
        ))?;
        let leading_samples = nominal_samples.saturating_sub(first_samples.unwrap_or(0));
        // MPEG audio's first packet carries encoder-delay side data that is
        // absent after a demuxer seek. Account for the samples the normal
        // decode removes. Opus pre-skip remains represented by its timestamps
        // after seek, so applying the same correction there would double-trim.
        let output_padding = if matches!(codec_name, "mp1" | "mp2" | "mp3") {
            u64::try_from(leading_samples)
                .unwrap_or(u64::MAX)
                .saturating_mul(u64::from(spec.sample_rate))
                .saturating_add(u64::from(decoded_rate) - 1)
                / u64::from(decoded_rate)
        } else {
            0
        };
        (origin, output_padding)
    };

    let context = codec::context::Context::from_parameters(parameters)
        .map_err(|error| (ExitCode::UnsupportedMedia, error.to_string()))?;
    let mut decoder = context
        .decoder()
        .audio()
        .map_err(|error| (ExitCode::UnsupportedMedia, error.to_string()))?;

    // FFmpeg uses both AV_NOPTS_VALUE and, for some demuxers such as Musepack
    // SV8, zero to mean that the stream duration is unavailable. Preserve that
    // distinction in the protocol instead of advertising a zero-length stream.
    // With anonymous custom AVIO, some demuxers (notably MP3) expose the final
    // timestamp in `duration` while also reporting a positive stream start.
    // Convert that pair to the playable interval. Negative starts represent
    // codec pre-roll and must not lengthen or shorten the output timeline.
    let effective_duration = duration.saturating_sub(stream_start.max(0));
    let duration_frames = (duration > 0).then(|| {
        let seconds = effective_duration as f64 * f64::from(time_base.0) / f64::from(time_base.1);
        (seconds.max(0.0) * f64::from(sample_rate)) as u64
    });
    let start_frame = start_ms.saturating_mul(u64::from(spec.sample_rate)) / 1_000;
    let timestamp = (start_ms as i64).saturating_mul(i64::from(time_base.1))
        / (1_000_i64.saturating_mul(i64::from(time_base.0).max(1)));
    if start_ms > 0 {
        let target_timestamp = timeline_origin.saturating_add(timestamp);
        // SAFETY: the input context and selected stream remain alive for this call.
        // Supplying the stream index makes all three timestamps use its time base.
        let result = unsafe {
            sys::avformat_seek_file(
                input.as_mut_ptr(),
                i32::try_from(stream_index).unwrap_or(i32::MAX),
                i64::MIN,
                target_timestamp,
                target_timestamp,
                sys::AVSEEK_FLAG_BACKWARD,
            )
        };
        if result < 0 {
            return Err((
                ExitCode::Decode,
                format!("FFmpeg seek failed: {}", ffmpeg::Error::from(result)),
            ));
        }
    }
    let mut resampler = None;
    let mut leading_trim = (start_ms > 0).then(|| {
        LeadingTrim::new(
            start_frame,
            timeline_origin,
            time_base,
            spec.sample_rate,
            seek_padding_frames,
        )
    });

    let header = DecodeHeader {
        protocol_major: 1,
        protocol_minor: 0,
        sample_format: SampleFormat::F32le,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        duration_frames,
        start_frame,
    };
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    write_decode_header(&mut output, &header)
        .map_err(|error| (ExitCode::ProtocolIo, error.to_string()))?;

    let mut decoded = frame::Audio::empty();
    let mut converted = frame::Audio::empty();
    for (packet_stream, packet) in input.packets() {
        if packet_stream.index() != stream_index {
            continue;
        }
        decoder
            .send_packet(&packet)
            .map_err(|error| (ExitCode::Decode, error.to_string()))?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            resample_and_write(
                &mut resampler,
                &mut decoded,
                &mut converted,
                &mut output,
                spec,
                &mut leading_trim,
            )?;
        }
    }
    decoder
        .send_eof()
        .map_err(|error| (ExitCode::Decode, error.to_string()))?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        resample_and_write(
            &mut resampler,
            &mut decoded,
            &mut converted,
            &mut output,
            spec,
            &mut leading_trim,
        )?;
    }
    if let Some(resampler) = resampler.as_mut() {
        while let Some(delay) = resampler.delay() {
            prepare_output_frame(
                &mut converted,
                usize::try_from(delay.output.max(1)).unwrap_or(usize::MAX),
                spec,
            )?;
            let remaining = resampler
                .flush(&mut converted)
                .map_err(|error| (ExitCode::Decode, error.to_string()))?;
            write_stereo_frame(&mut output, &converted, &mut leading_trim)?;
            if remaining.is_none() {
                break;
            }
        }
    }
    output.flush().map_err(map_output_error)?;
    Ok(())
}

fn resample_and_write(
    resampler: &mut Option<software::resampling::Context>,
    decoded: &mut frame::Audio,
    converted: &mut frame::Audio,
    output: &mut impl Write,
    spec: PcmSpec,
    leading_trim: &mut Option<LeadingTrim>,
) -> Result<(), (ExitCode, String)> {
    if let Some(trim) = leading_trim.as_mut() {
        trim.begin_decoded_frame(decoded)?;
    }
    if decoded.channel_layout().is_empty() {
        decoded.set_channel_layout(ffmpeg::channel_layout::ChannelLayout::default(i32::from(
            decoded.channels(),
        )));
    }
    if resampler.is_none() {
        *resampler = Some(create_resampler(decoded, spec)?);
    }
    let input_rate = u64::from(decoded.rate().max(1));
    let scaled_samples = (u64::try_from(decoded.samples())
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(spec.sample_rate))
        .saturating_add(input_rate - 1))
        / input_rate;
    let delay_samples = resampler
        .as_ref()
        .and_then(software::resampling::Context::delay)
        .map(|delay| delay.output.max(0) as u64)
        .unwrap_or(0);
    let capacity = scaled_samples
        .saturating_add(delay_samples)
        .saturating_add(32)
        .try_into()
        .unwrap_or(usize::MAX);
    prepare_output_frame(converted, capacity, spec)?;
    let result = resampler
        .as_mut()
        .expect("resampler was initialized")
        .run(decoded, converted);
    if matches!(
        result,
        Err(ffmpeg::Error::InputChanged | ffmpeg::Error::OutputChanged)
    ) {
        *resampler = Some(create_resampler(decoded, spec)?);
        *converted = frame::Audio::empty();
        resampler
            .as_mut()
            .expect("resampler was reinitialized")
            .run(decoded, converted)
            .map_err(|error| (ExitCode::Decode, error.to_string()))?;
    } else {
        result.map_err(|error| (ExitCode::Decode, error.to_string()))?;
    }
    write_stereo_frame(output, converted, leading_trim)
}

fn prepare_output_frame(
    converted: &mut frame::Audio,
    capacity: usize,
    spec: PcmSpec,
) -> Result<(), (ExitCode, String)> {
    if capacity > i32::MAX as usize {
        return Err((
            ExitCode::Decode,
            "FFmpeg requested an oversized PCM frame".into(),
        ));
    }
    *converted = frame::Audio::empty();
    unsafe {
        converted.alloc(
            format::Sample::F32(format::sample::Type::Packed),
            capacity.max(1),
            ffmpeg::channel_layout::ChannelLayout::STEREO,
        );
    }
    converted.set_rate(spec.sample_rate);
    Ok(())
}

fn create_resampler(
    decoded: &frame::Audio,
    spec: PcmSpec,
) -> Result<software::resampling::Context, (ExitCode, String)> {
    software::resampling::Context::get(
        decoded.format(),
        decoded.channel_layout(),
        decoded.rate(),
        format::Sample::F32(format::sample::Type::Packed),
        ffmpeg::channel_layout::ChannelLayout::STEREO,
        spec.sample_rate,
    )
    .map_err(|error| {
        (
            ExitCode::Decode,
            format!("FFmpeg resampler initialization failed: {error}"),
        )
    })
}

fn write_stereo_frame(
    output: &mut impl Write,
    converted: &frame::Audio,
    leading_trim: &mut Option<LeadingTrim>,
) -> Result<(), (ExitCode, String)> {
    let samples = converted.plane::<(f32, f32)>(0);
    let skip = leading_trim
        .as_mut()
        .map_or(0, |trim| trim.skip_for(samples.len()));
    for &(left, right) in &samples[skip..] {
        output
            .write_all(&left.to_le_bytes())
            .and_then(|()| output.write_all(&right.to_le_bytes()))
            .map_err(map_output_error)?;
    }
    Ok(())
}

fn map_output_error(error: std::io::Error) -> (ExitCode, String) {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        (ExitCode::Ok, String::new())
    } else {
        (ExitCode::ProtocolIo, error.to_string())
    }
}
