//! Error types for sync operations

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Index error: {0}")]
    Index(#[from] landro_index::IndexError),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Conflict detected: {0}")]
    Conflict(String),

    #[error("Transfer failed: {0}")]
    TransferFailed(String),

    #[error("Peer not found: {0}")]
    PeerNotFound(String),

    #[error("Invalid state transition: from {from:?} to {to:?}")]
    InvalidStateTransition { from: String, to: String },

    #[error("Sync already in progress with peer: {0}")]
    AlreadySyncing(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Timeout waiting for: {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, SyncError>;
