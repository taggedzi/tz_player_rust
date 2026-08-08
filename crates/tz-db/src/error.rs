//! Database errors.

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io: {0}")]
    Io(String),

    #[error("schema: {0}")]
    Schema(String),

    #[error("unsupported schema version {found} (supported max {supported})")]
    UnsupportedVersion { found: i32, supported: i32 },
}
