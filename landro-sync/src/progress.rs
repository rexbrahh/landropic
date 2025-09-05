//! Progress tracking for sync operations

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Overall sync progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProgress {
    pub peer_id: String,
    pub total_files: usize,
    pub completed_files: usize,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub start_time: DateTime<Utc>,
    pub estimated_completion: Option<DateTime<Utc>>,
}

impl SyncProgress {
    pub fn new(peer_id: String) -> Self {
        Self {
            peer_id,
            total_files: 0,
            completed_files: 0,
            total_bytes: 0,
            transferred_bytes: 0,
            start_time: Utc::now(),
            estimated_completion: None,
        }
    }

    pub fn percentage(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.transferred_bytes as f32 / self.total_bytes as f32) * 100.0
    }

    pub fn is_complete(&self) -> bool {
        self.completed_files == self.total_files && self.transferred_bytes == self.total_bytes
    }
}

/// Progress for individual transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferProgress {
    pub file_path: String,
    pub chunk_hash: String,
    pub size: u64,
    pub transferred: u64,
    pub start_time: DateTime<Utc>,
}

impl TransferProgress {
    pub fn new(file_path: String, chunk_hash: String, size: u64) -> Self {
        Self {
            file_path,
            chunk_hash,
            size,
            transferred: 0,
            start_time: Utc::now(),
        }
    }

    pub fn percentage(&self) -> f32 {
        if self.size == 0 {
            return 100.0;
        }
        (self.transferred as f32 / self.size as f32) * 100.0
    }
}