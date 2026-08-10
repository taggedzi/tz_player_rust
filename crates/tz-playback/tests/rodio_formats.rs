use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rodio::{Decoder, Source};

const SUPPORTED_FIXTURES: &[(&str, &str)] = &[
    ("WAV/PCM", "tone.wav"),
    ("MP3", "tone.mp3"),
    ("FLAC", "tone.flac"),
    ("Ogg Vorbis", "tone.ogg"),
    ("AAC/M4A", "tone-aac.m4a"),
    ("ALAC/M4A", "tone-alac.m4a"),
    ("AIFF/PCM", "tone.aiff"),
    ("CAF/PCM", "tone.caf"),
    ("Matroska/FLAC", "tone.mka"),
];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn common_formats_decode_report_duration_and_seek_both_directions() {
    for &(label, name) in SUPPORTED_FIXTURES {
        let path = fixture(name);
        let file = File::open(&path).unwrap_or_else(|error| {
            panic!("could not open {label} fixture {}: {error}", path.display())
        });
        let mut decoder = Decoder::try_from(file).unwrap_or_else(|error| {
            panic!(
                "could not decode {label} fixture {}: {error}",
                path.display()
            )
        });

        let duration = decoder
            .total_duration()
            .unwrap_or_else(|| panic!("{label} did not report a duration"));
        assert!(
            (Duration::from_millis(850)..=Duration::from_millis(1_150)).contains(&duration),
            "{label} duration {duration:?} was outside the fixture tolerance"
        );
        assert!(decoder.next().is_some(), "{label} produced no samples");

        decoder
            .try_seek(Duration::from_millis(700))
            .unwrap_or_else(|error| panic!("{label} forward seek failed: {error}"));
        assert!(
            decoder.next().is_some(),
            "{label} produced no samples after a forward seek"
        );

        decoder
            .try_seek(Duration::from_millis(100))
            .unwrap_or_else(|error| panic!("{label} backward seek failed: {error}"));
        assert!(
            decoder.next().is_some(),
            "{label} produced no samples after a backward seek"
        );
    }
}

#[test]
fn unsupported_and_corrupt_inputs_fail_then_supported_decode_still_works() {
    for name in ["tone-opus.ogg", "corrupt.bin"] {
        let path = fixture(name);
        let file = File::open(&path).unwrap();
        assert!(
            Decoder::try_from(file).is_err(),
            "{} unexpectedly decoded",
            path.display()
        );
    }

    let mut decoder = Decoder::try_from(File::open(fixture("tone.wav")).unwrap()).unwrap();
    assert!(decoder.next().is_some());
}
