use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../native/ffmpeg/manifest.toml");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
        panic!(
            "could not read FFmpeg manifest {}: {error}",
            manifest_path.display()
        )
    });
    emit_manifest_value(&manifest, "version", "TZ_FFMPEG_EXPECTED_VERSION");
    emit_manifest_value(&manifest, "ffmpeg_release_commit", "TZ_FFMPEG_COMMIT");
    emit_manifest_array(&manifest, "demuxers", "TZ_FFMPEG_DEMUXERS");
    emit_manifest_array(&manifest, "decoders", "TZ_FFMPEG_DECODERS");

    println!("cargo:rerun-if-env-changed=TZ_FFMPEG_LIB_DIR");
    println!("cargo:rerun-if-env-changed=TZ_FFMPEG_INCLUDE_DIR");
    if env::var_os("CARGO_FEATURE_FFMPEG_NATIVE").is_none() {
        return;
    }
    let lib_dir = env::var_os("TZ_FFMPEG_LIB_DIR")
        .map(PathBuf::from)
        .expect("TZ_FFMPEG_LIB_DIR is required for ffmpeg-native builds");
    if !lib_dir.is_dir() {
        panic!(
            "TZ_FFMPEG_LIB_DIR is not a directory: {}",
            lib_dir.display()
        );
    }
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    for library in ["avcodec", "avformat", "avutil", "swresample"] {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
    if let Some(include_dir) = env::var_os("TZ_FFMPEG_INCLUDE_DIR") {
        println!(
            "cargo:rustc-env=TZ_FFMPEG_INCLUDE_DIR={}",
            PathBuf::from(include_dir).display()
        );
    }
    if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
    }
}

fn emit_manifest_value(manifest: &str, name: &str, environment_name: &str) {
    let prefix = format!("{name} = \"");
    let value = manifest
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|line| line.strip_suffix('"'))
        .unwrap_or_else(|| panic!("FFmpeg manifest value is missing: {name}"));
    println!("cargo:rustc-env={environment_name}={value}");
}

fn emit_manifest_array(manifest: &str, name: &str, environment_name: &str) {
    let prefix = format!("{name} = [");
    let values = manifest
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|line| line.strip_suffix(']'))
        .unwrap_or_else(|| panic!("FFmpeg manifest array is missing: {name}"))
        .split(',')
        .map(|value| value.trim().trim_matches('"'))
        .collect::<Vec<_>>()
        .join(",");
    println!("cargo:rustc-env={environment_name}={values}");
}
