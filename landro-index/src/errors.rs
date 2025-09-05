use thiserror::Error;

#[derive(Error, Debug)]
pub enum IndexError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Manifest not found: {0}")]
    ManifestNotFound(String),

    #[error("Invalid manifest: {0}")]
    InvalidManifest(String),

    #[error("Schema version mismatch: expected {expected}, got {actual}")]
    SchemaVersionMismatch { expected: u32, actual: u32 },

    #[error("Chunker error: {0}")]
    ChunkerError(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Invalid file type: {0}")]
    InvalidFileType(std::path::PathBuf),

    #[error("Watcher error: {0}")]
    WatcherError(String),

    #[error("Database error: {0}")]
    DatabaseError(String),
}

pub type Result<T> = std::result::Result<T, IndexError>;
