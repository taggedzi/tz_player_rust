//! Safe rendering for untrusted text written directly to a terminal.

use std::path::Path;

/// Escape terminal controls and bidirectional text controls in untrusted text.
///
/// Ratatui writes cells rather than replaying text as a byte stream, but plain
/// CLI output does not have that protection. This function makes every C0/C1
/// control visible, including ESC/BEL used by ANSI and OSC sequences, and
/// escapes directional controls that can visually reorder diagnostics.
pub fn terminal_safe(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if is_c0_or_c1(character) => {
                let code = character as u32;
                if code <= 0xFF {
                    output.push_str(&format!("\\x{code:02X}"));
                } else {
                    output.push_str(&format!("\\u{{{code:04X}}}"));
                }
            }
            character if is_directional_or_line_control(character) => {
                output.push_str(&format!("\\u{{{:04X}}}", character as u32));
            }
            character => output.push(character),
        }
    }
    output
}

/// Lossily render a path and escape any terminal-active characters it carries.
pub fn terminal_safe_path(path: &Path) -> String {
    terminal_safe(path.as_os_str().to_string_lossy())
}

fn is_c0_or_c1(character: char) -> bool {
    matches!(character as u32, 0x00..=0x1F | 0x7F..=0x9F)
}

fn is_directional_or_line_control(character: char) -> bool {
    matches!(
        character as u32,
        0x061C | 0x200E | 0x200F | 0x2028..=0x202E | 0x2066..=0x206F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_ansi_osc_newlines_c1_and_bidi_payloads() {
        let payload = concat!(
            "safe",
            "\x1B[31mred\x1B[0m",
            "\x1B]0;owned\x07",
            "line\r\nnext\t",
            "\u{009B}31m",
            "\u{202E}txt\u{2066}end"
        );
        let escaped = terminal_safe(payload);
        assert_eq!(
            escaped,
            concat!(
                "safe",
                "\\x1B[31mred\\x1B[0m",
                "\\x1B]0;owned\\x07",
                "line\\r\\nnext\\t",
                "\\x9B31m",
                "\\u{202E}txt\\u{2066}end"
            )
        );
        assert!(!escaped.chars().any(is_c0_or_c1));
        assert!(!escaped.chars().any(is_directional_or_line_control));
    }

    #[test]
    fn preserves_normal_unicode_text() {
        assert_eq!(terminal_safe("Björk — Jóga 🎵"), "Björk — Jóga 🎵");
    }
}
