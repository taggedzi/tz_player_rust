use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperLocation {
    pub path: PathBuf,
    pub package_root: PathBuf,
}

pub fn package_helper_path(executable: &Path) -> PathBuf {
    package_root_path(executable)
        .join("audio")
        .join(if cfg!(windows) {
            "tz-audio-decoder.exe"
        } else {
            "tz-audio-decoder"
        })
}

pub fn resolve_package_helper() -> Result<HelperLocation, String> {
    let executable =
        env::current_exe().map_err(|error| format!("cannot resolve player executable: {error}"))?;
    let path = package_helper_path(&executable);
    if !path.is_absolute() || !path.is_file() {
        return Err(format!(
            "bundled audio helper is missing: {}",
            path.display()
        ));
    }
    let package_root = package_root_path(&executable);
    Ok(HelperLocation { path, package_root })
}

pub fn package_root_path(executable: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(contents) = executable.parent().and_then(Path::parent) {
            if contents.file_name().is_some_and(|name| name == "Contents") {
                return contents.join("Resources");
            }
        }
    }
    executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_is_relative_to_executable_audio_directory() {
        let executable = if cfg!(target_os = "macos") {
            Path::new("/package/tz-player.app/Contents/MacOS/tz-player")
        } else {
            Path::new(r"C:\package\tz-player.exe")
        };
        let path = package_helper_path(executable);
        assert!(path.ends_with(Path::new("audio").join(if cfg!(windows) {
            "tz-audio-decoder.exe"
        } else {
            "tz-audio-decoder"
        })));
        if cfg!(target_os = "macos") {
            assert!(path.starts_with("/package/tz-player.app/Contents/Resources"));
        }
    }
}
