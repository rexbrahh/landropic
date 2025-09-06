use landro_chunker::ContentHash;
use thiserror::Error;

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

    #[error("Invalid object size: expected {expected}, got {actual}")]
    InvalidObjectSize { expected: u64, actual: u64 },

    #[error("Inconsistent store state: {0}")]
    InconsistentState(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Storage quota exceeded: {0}")]
    QuotaExceeded(String),

    #[error("Concurrent modification detected: {0}")]
    ConcurrentModification(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, CasError>;
