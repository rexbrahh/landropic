use thiserror::Error;
use landro_chunker::ContentHash;

#[derive(Error, Debug)]
pub enum CasError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Object not found: {0}")]
    ObjectNotFound(ContentHash),
    
    #[error("Hash mismatch: expected {expected}, got {actual}")]
    HashMismatch {
        expected: ContentHash,
        actual: ContentHash,
    },
    
    #[error("Corrupt object: {0}")]
    CorruptObject(String),
    
    #[error("Storage path error: {0}")]
    StoragePath(String),
    
    #[error("Atomic write failed: {0}")]
    AtomicWriteFailed(String),
}

pub type Result<T> = std::result::Result<T, CasError>;