//! User-configurable semantic TUI palette.
//!
//! Widgets continue to render with the built-in semantic colors; this module
//! remaps the completed frame from a user-owned config file. Playback and core
//! state therefore have no dependency on presentation themes.

use std::fs;
use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde::Deserialize;

const MAX_THEME_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiTheme {
    accent: Color,
    accent_alt: Color,
    text: Color,
    muted: Color,
    success: Color,
    warning: Color,
    error: Color,
    bright: Color,
    selection_fg: Color,
    selection_bg: Color,
    selection_bold: Option<bool>,
    muted_dim: Option<bool>,
}

impl Default for TuiTheme {
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
            accent_alt: Color::Magenta,
            text: Color::Gray,
            muted: Color::DarkGray,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            bright: Color::White,
            selection_fg: Color::Black,
            selection_bg: Color::Cyan,
            selection_bold: None,
            muted_dim: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ThemeConfig {
    accent: Option<String>,
    accent_alt: Option<String>,
    text: Option<String>,
    muted: Option<String>,
    success: Option<String>,
    warning: Option<String>,
    error: Option<String>,
    bright: Option<String>,
    selection_fg: Option<String>,
    selection_bg: Option<String>,
    selection_bold: Option<bool>,
    muted_dim: Option<bool>,
}

impl TuiTheme {
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("Theme could not be read; using defaults: {error}")),
                );
            }
        };
        if metadata.len() > MAX_THEME_BYTES {
            return (
                Self::default(),
                Some(format!(
                    "Theme exceeds {} KiB; using defaults",
                    MAX_THEME_BYTES / 1024
                )),
            );
        }
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("Theme could not be read; using defaults: {error}")),
                );
            }
        };
        let config: ThemeConfig = match serde_json::from_str(&raw) {
            Ok(config) => config,
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("Theme is invalid JSON; using defaults: {error}")),
                );
            }
        };
        match Self::from_config(config) {
            Ok(theme) => (theme, None),
            Err(error) => (
                Self::default(),
                Some(format!("Theme is invalid; using defaults: {error}")),
            ),
        }
    }

    fn from_config(config: ThemeConfig) -> Result<Self, String> {
        let defaults = Self::default();
        Ok(Self {
            accent: configured_color("accent", config.accent, defaults.accent)?,
            accent_alt: configured_color("accent_alt", config.accent_alt, defaults.accent_alt)?,
            text: configured_color("text", config.text, defaults.text)?,
            muted: configured_color("muted", config.muted, defaults.muted)?,
            success: configured_color("success", config.success, defaults.success)?,
            warning: configured_color("warning", config.warning, defaults.warning)?,
            error: configured_color("error", config.error, defaults.error)?,
            bright: configured_color("bright", config.bright, defaults.bright)?,
            selection_fg: configured_color(
                "selection_fg",
                config.selection_fg,
                defaults.selection_fg,
            )?,
            selection_bg: configured_color(
                "selection_bg",
                config.selection_bg,
                defaults.selection_bg,
            )?,
            selection_bold: config.selection_bold,
            muted_dim: config.muted_dim,
        })
    }

    pub fn apply_buffer(&self, buffer: &mut Buffer) {
        for cell in &mut buffer.content {
            let original_fg = cell.fg;
            let original_bg = cell.bg;
            cell.fg = self.map_foreground(original_fg);
            if original_bg == Color::Cyan {
                cell.bg = self.selection_bg;
                if let Some(selection_bold) = self.selection_bold {
                    if selection_bold {
                        cell.modifier.insert(Modifier::BOLD);
                    } else {
                        cell.modifier.remove(Modifier::BOLD);
                    }
                }
            }
            if original_fg == Color::DarkGray {
                if let Some(muted_dim) = self.muted_dim {
                    if muted_dim {
                        cell.modifier.insert(Modifier::DIM);
                    } else {
                        cell.modifier.remove(Modifier::DIM);
                    }
                }
            }
        }
    }

    fn map_foreground(&self, color: Color) -> Color {
        match color {
            Color::Cyan => self.accent,
            Color::Magenta => self.accent_alt,
            Color::Gray => self.text,
            Color::DarkGray => self.muted,
            Color::Green => self.success,
            Color::Yellow => self.warning,
            Color::Red => self.error,
            Color::White => self.bright,
            Color::Black => self.selection_fg,
            other => other,
        }
    }
}

fn configured_color(
    field: &str,
    configured: Option<String>,
    default: Color,
) -> Result<Color, String> {
    configured
        .map(|value| parse_color(&value).map_err(|error| format!("{field}: {error}")))
        .transpose()
        .map(|color| color.unwrap_or(default))
}

fn parse_color(value: &str) -> Result<Color, String> {
    let normalized = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let named = match normalized.as_str() {
        "reset" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "dark_gray" | "dark_grey" => Some(Color::DarkGray),
        "light_red" => Some(Color::LightRed),
        "light_green" => Some(Color::LightGreen),
        "light_yellow" => Some(Color::LightYellow),
        "light_blue" => Some(Color::LightBlue),
        "light_magenta" => Some(Color::LightMagenta),
        "light_cyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    };
    if let Some(color) = named {
        return Ok(color);
    }
    if let Some(hex) = normalized.strip_prefix('#') {
        if hex.len() == 6 && hex.chars().all(|character| character.is_ascii_hexdigit()) {
            let red = u8::from_str_radix(&hex[0..2], 16).unwrap();
            let green = u8::from_str_radix(&hex[2..4], 16).unwrap();
            let blue = u8::from_str_radix(&hex[4..6], 16).unwrap();
            return Ok(Color::Rgb(red, green, blue));
        }
    }
    if let Some(index) = normalized.strip_prefix("ansi:") {
        return index
            .parse::<u8>()
            .map(Color::Indexed)
            .map_err(|_| format!("expected ansi:0..255, got {value:?}"));
    }
    Err(format!(
        "unknown color {value:?}; use a named color, #RRGGBB, or ansi:0..255"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_theme() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "tz_player_theme_{}_{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn missing_theme_uses_defaults_without_a_warning() {
        let path = temp_theme();
        let _ = fs::remove_file(&path);
        let (theme, notice) = TuiTheme::load(&path);
        assert_eq!(theme, TuiTheme::default());
        assert!(notice.is_none());
    }

    #[test]
    fn custom_colors_and_formatting_remap_semantic_cells() {
        let path = temp_theme();
        fs::write(
            &path,
            r##"{
                "accent": "#102030",
                "muted": "ansi:240",
                "selection_bg": "blue",
                "selection_fg": "white",
                "selection_bold": false,
                "muted_dim": true
            }"##,
        )
        .unwrap();
        let (theme, notice) = TuiTheme::load(&path);
        assert!(notice.is_none());

        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 1));
        buffer[(0, 0)].fg = Color::Cyan;
        buffer[(1, 0)].fg = Color::DarkGray;
        buffer[(2, 0)].fg = Color::Black;
        buffer[(2, 0)].bg = Color::Cyan;
        buffer[(2, 0)].modifier.insert(Modifier::BOLD);
        theme.apply_buffer(&mut buffer);

        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(buffer[(1, 0)].fg, Color::Indexed(240));
        assert!(buffer[(1, 0)].modifier.contains(Modifier::DIM));
        assert_eq!(buffer[(2, 0)].fg, Color::White);
        assert_eq!(buffer[(2, 0)].bg, Color::Blue);
        assert!(!buffer[(2, 0)].modifier.contains(Modifier::BOLD));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_theme_falls_back_with_an_actionable_notice() {
        let path = temp_theme();
        fs::write(&path, r#"{"accent":"not-a-color"}"#).unwrap();
        let (theme, notice) = TuiTheme::load(&path);
        assert_eq!(theme, TuiTheme::default());
        assert!(notice.unwrap().contains("accent"));
        let _ = fs::remove_file(path);
    }
}
