//! Product info surfaced to users across every interface (CLI, TUI, and any
//! future headless client). Deliberately carries no user-specific data —
//! no local file paths, no environment variables, no machine identifiers.
//! For that kind of debug info, see `tz-player doctor`.

use std::fmt;

/// Fixed product identity (not derived from this crate's own Cargo.toml,
/// since `tz-core`'s package metadata describes the library, not the product).
pub const PRODUCT_NAME: &str = "tz-player";
pub const PRODUCT_DESCRIPTION: &str = "TaggedZ's terminal music player (Rust rewrite)";

/// Snapshot of product/version/build info, safe to display or log anywhere.
#[derive(Debug, Clone)]
pub struct AboutInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub version: &'static str,
    pub repository: &'static str,
    pub license: &'static str,
    pub schema_version: i32,
    pub target: String,
    pub profile: &'static str,
}

/// Build the current [`AboutInfo`] snapshot.
///
/// The `env!("CARGO_PKG_*")` calls below resolve to *this crate's*
/// (`tz-core`'s) own Cargo.toml metadata, not `tz-player`'s — env! is
/// resolved per-compilation-unit, and there's no way to read another
/// crate's package metadata at compile time. This only produces correct
/// values because every crate in the workspace inherits version/repository/
/// license from `[workspace.package]`. If any crate ever overrides one of
/// those fields, this drifts from the product's actual metadata silently.
pub fn about_info() -> AboutInfo {
    AboutInfo {
        name: PRODUCT_NAME,
        description: PRODUCT_DESCRIPTION,
        version: env!("CARGO_PKG_VERSION"),
        repository: env!("CARGO_PKG_REPOSITORY"),
        license: env!("CARGO_PKG_LICENSE"),
        schema_version: tz_db::SCHEMA_VERSION,
        target: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    }
}

impl AboutInfo {
    /// Condensed single-line form for status bars / footers.
    pub fn tui_line(&self) -> String {
        format!(
            "{} v{} — {}  (see: tz-player about)",
            self.name, self.version, self.repository
        )
    }
}

impl fmt::Display for AboutInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} v{}", self.name, self.version)?;
        writeln!(f, "{}", self.description)?;
        writeln!(f, "Repository: {}", self.repository)?;
        writeln!(f, "License:    {}", self.license)?;
        writeln!(f, "Schema:     v{}", self.schema_version)?;
        write!(f, "Target:     {} ({})", self.target, self.profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_info_carries_product_identity() {
        let info = about_info();
        assert_eq!(info.name, "tz-player");
        assert!(!info.version.is_empty());
        assert!(info.repository.starts_with("https://"));
        assert!(!info.license.is_empty());
        assert!(info.schema_version > 0);
    }

    #[test]
    fn display_includes_key_fields() {
        let info = about_info();
        let rendered = info.to_string();
        assert!(rendered.contains(info.name));
        assert!(rendered.contains(info.version));
        assert!(rendered.contains(info.repository));
        assert!(rendered.contains(info.license));
    }

    #[test]
    fn tui_line_is_single_line() {
        let line = about_info().tui_line();
        assert!(!line.contains('\n'));
        assert!(line.contains(about_info().version));
    }
}
