use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::errors::{QuicError, Result};

/// Simplified transfer configuration for v1.0
#[derive(Debug, Clone)]
pub struct TransferConfig {
    /// Number of parallel streams (fixed for v1.0)
    pub stream_count: usize,
    /// Chunk size in bytes
    pub chunk_size: usize,
    /// Request timeout
    pub timeout: Duration,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            stream_count: 4,
            chunk_size: 64 * 1024, // 64KB
            timeout: Duration::from_secs(30),
        }
    }
}

/// Simple chunk information
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub hash: String,
    pub size: u64,
    pub offset: u64,
}

/// Basic transfer manager for v1.0
pub struct TransferManager {
    config: TransferConfig,
    active_transfers: Arc<RwLock<HashMap<String, TransferSession>>>,
}

#[derive(Debug)]
struct TransferSession {
    file_path: String,
    chunks: Vec<ChunkInfo>,
    completed_chunks: usize,
    total_size: u64,
    transferred: u64,
}

impl TransferManager {
    /// Create new transfer manager
    pub fn new(config: TransferConfig) -> Self {
        Self {
            config,
            active_transfers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start a file transfer
    pub async fn start_transfer(
        &self,
        transfer_id: String,
        file_path: String,
        chunks: Vec<ChunkInfo>,
    ) -> Result<()> {
        info!(
            "Starting transfer: {} ({} chunks)",
            transfer_id,
            chunks.len()
        );

        let total_size = chunks.iter().map(|c| c.size).sum();
        let session = TransferSession {
            file_path,
            chunks,
            completed_chunks: 0,
            total_size,
            transferred: 0,
        };

        let mut transfers = self.active_transfers.write().await;
        transfers.insert(transfer_id.clone(), session);

        // For v1.0, just log the transfer - actual QUIC streaming will be implemented later
        info!("Transfer {} queued: {} bytes", transfer_id, total_size);

        Ok(())
    }

    /// Get transfer progress
    pub async fn get_progress(&self, transfer_id: &str) -> Option<(u64, u64)> {
        let transfers = self.active_transfers.read().await;
        transfers
            .get(transfer_id)
            .map(|s| (s.transferred, s.total_size))
    }

    /// Complete a transfer
    pub async fn complete_transfer(&self, transfer_id: &str) -> Result<()> {
        let mut transfers = self.active_transfers.write().await;
        if let Some(session) = transfers.remove(transfer_id) {
            info!(
                "Transfer completed: {} ({} bytes)",
                transfer_id, session.total_size
            );
        }
        Ok(())
    }

    /// Cancel a transfer
    pub async fn cancel_transfer(&self, transfer_id: &str) -> Result<()> {
        let mut transfers = self.active_transfers.write().await;
        if transfers.remove(transfer_id).is_some() {
            info!("Transfer cancelled: {}", transfer_id);
        }
        Ok(())
    }
}

/// Simple chunk provider interface
pub trait ChunkProvider: Send + Sync {
    /// Get chunk data by hash
    fn get_chunk(&self, hash: &str) -> Result<Option<Bytes>>;

    /// Store chunk data
    fn put_chunk(&self, hash: &str, data: Bytes) -> Result<()>;
}

/// Basic in-memory chunk provider for testing
pub struct MemoryChunkProvider {
    chunks: Arc<RwLock<HashMap<String, Bytes>>>,
}

impl MemoryChunkProvider {
    pub fn new() -> Self {
        Self {
            chunks: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl ChunkProvider for MemoryChunkProvider {
    fn get_chunk(&self, hash: &str) -> Result<Option<Bytes>> {
        let chunks = futures::executor::block_on(self.chunks.read());
        Ok(chunks.get(hash).cloned())
    }

    fn put_chunk(&self, hash: &str, data: Bytes) -> Result<()> {
        let mut chunks = futures::executor::block_on(self.chunks.write());
        chunks.insert(hash.to_string(), data);
        Ok(())
    }
}

impl Default for MemoryChunkProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_transfer_manager() {
        let config = TransferConfig::default();
        let manager = TransferManager::new(config);

        let chunks = vec![
            ChunkInfo {
                hash: "hash1".to_string(),
                size: 1024,
                offset: 0,
            },
            ChunkInfo {
                hash: "hash2".to_string(),
                size: 2048,
                offset: 1024,
            },
        ];

        // Start transfer
        manager
            .start_transfer(
                "test-transfer".to_string(),
                "/test/file.txt".to_string(),
                chunks,
            )
            .await
            .unwrap();

        // Check progress
        let progress = manager.get_progress("test-transfer").await;
        assert!(progress.is_some());
        let (transferred, total) = progress.unwrap();
        assert_eq!(transferred, 0);
        assert_eq!(total, 3072);

        // Complete transfer
        manager.complete_transfer("test-transfer").await.unwrap();

        // Should be removed after completion
        let progress = manager.get_progress("test-transfer").await;
        assert!(progress.is_none());
    }

    #[test]
    fn test_chunk_provider() {
        let provider = MemoryChunkProvider::new();
        let data = Bytes::from("test data");

        // Store chunk
        provider.put_chunk("test-hash", data.clone()).unwrap();

        // Retrieve chunk
        let retrieved = provider.get_chunk("test-hash").unwrap();
        assert_eq!(retrieved, Some(data));

        // Non-existent chunk
        let missing = provider.get_chunk("missing").unwrap();
        assert_eq!(missing, None);
    }
}
