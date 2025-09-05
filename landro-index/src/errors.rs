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
    SchemaVersionMismatch {
        expected: u32,
        actual: u32,
    },
}

pub type Result<T> = std::result::Result<T, IndexError>;