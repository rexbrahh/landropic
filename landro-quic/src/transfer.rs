//! Resumable file transfer implementation over QUIC streams

use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::connection::Connection;
use crate::errors::{QuicError, Result};
use crate::protocol::{BatchTransferManager, StreamProtocol};
use crate::resumable::{ResumableTransferManager, TransferCheckpoint, TransferStatus};

/// Integrated transfer engine that combines QUIC operations with resumable transfers
pub struct QuicTransferEngine {
    connection: Arc<Connection>,
    stream_protocol: StreamProtocol,
    batch_manager: BatchTransferManager,
    resumable_manager: ResumableTransferManager,
    active_transfers: Arc<RwLock<HashMap<String, TransferSession>>>,
}

/// Active transfer session state
#[derive(Debug, Clone)]
struct TransferSession {
    transfer_id: String,
    peer_id: String,
    file_path: String,
    total_size: u64,
    chunks_needed: Vec<Vec<u8>>,
    chunks_received: HashMap<Vec<u8>, landro_proto::ChunkData>,
    status: TransferStatus,
}

impl QuicTransferEngine {
    /// Create a new transfer engine for a connection
    pub fn new(connection: Arc<Connection>) -> Self {
        let stream_protocol = StreamProtocol::new(connection.clone());
        let batch_manager = BatchTransferManager::new(connection.clone());
        let resumable_manager = ResumableTransferManager::default();

        Self {
            connection: connection.clone(),
            stream_protocol,
            batch_manager,
            resumable_manager,
            active_transfers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start a resumable file transfer
    pub async fn start_transfer(
        &mut self,
        transfer_id: String,
        peer_id: String,
        file_path: String,
        total_size: u64,
        chunks_needed: Vec<Vec<u8>>,
    ) -> Result<()> {
        info!(
            "Starting resumable transfer: {} ({} bytes, {} chunks)",
            transfer_id,
            total_size,
            chunks_needed.len()
        );

        // Create transfer session
        let session = TransferSession {
            transfer_id: transfer_id.clone(),
            peer_id: peer_id.clone(),
            file_path: file_path.clone(),
            total_size,
            chunks_needed: chunks_needed.clone(),
            chunks_received: HashMap::new(),
            status: TransferStatus::Active,
        };

        // Register with resumable manager
        self.resumable_manager
            .start_transfer(
                transfer_id.clone(),
                peer_id.clone(),
                file_path,
                total_size,
                self.connection.clone(),
            )
            .await?;

        // Store session
        {
            let mut active_transfers = self.active_transfers.write().await;
            active_transfers.insert(transfer_id.clone(), session);
        }

        // Start chunk transfer process
        self.transfer_chunks_resumable(transfer_id).await?;

        Ok(())
    }

    /// Resume a paused or failed transfer
    pub async fn resume_transfer(&mut self, transfer_id: &str) -> Result<()> {
        info!("Resuming transfer: {}", transfer_id);

        // Get transfer checkpoint
        if let Some((status, progress, _bandwidth)) = self
            .resumable_manager
            .get_transfer_status(transfer_id)
            .await
        {
            match status {
                TransferStatus::Paused | TransferStatus::Failed => {
                    // Resume with resumable manager
                    self.resumable_manager
                        .resume_transfer(transfer_id, self.connection.clone())
                        .await?;

                    // Continue chunk transfer
                    self.transfer_chunks_resumable(transfer_id.to_string())
                        .await?;
                }
                TransferStatus::Active => {
                    warn!("Transfer {} is already active", transfer_id);
                }
                TransferStatus::Completed => {
                    info!("Transfer {} is already completed", transfer_id);
                }
                TransferStatus::Cancelled => {
                    return Err(QuicError::Protocol(format!(
                        "Cannot resume cancelled transfer: {}",
                        transfer_id
                    )));
                }
            }
        } else {
            return Err(QuicError::Protocol(format!(
                "Transfer not found: {}",
                transfer_id
            )));
        }

        Ok(())
    }

    /// Pause an active transfer
    pub async fn pause_transfer(&mut self, transfer_id: &str) -> Result<()> {
        info!("Pausing transfer: {}", transfer_id);

        // Update session status
        {
            let mut active_transfers = self.active_transfers.write().await;
            if let Some(session) = active_transfers.get_mut(transfer_id) {
                session.status = TransferStatus::Paused;
            }
        }

        // Pause in resumable manager
        self.resumable_manager.pause_transfer(transfer_id).await?;

        Ok(())
    }

    /// Cancel a transfer
    pub async fn cancel_transfer(&mut self, transfer_id: &str) -> Result<()> {
        info!("Cancelling transfer: {}", transfer_id);

        // Remove from active transfers
        {
            let mut active_transfers = self.active_transfers.write().await;
            active_transfers.remove(transfer_id);
        }

        // Mark as failed in resumable manager (which handles cleanup)
        self.resumable_manager
            .fail_transfer(transfer_id, "Transfer cancelled by user")
            .await?;

        Ok(())
    }

    /// Get transfer progress information
    pub async fn get_transfer_progress(&self, transfer_id: &str) -> Option<TransferProgress> {
        if let Some((status, progress, bandwidth)) = self
            .resumable_manager
            .get_transfer_status(transfer_id)
            .await
        {
            let active_transfers = self.active_transfers.read().await;
            if let Some(session) = active_transfers.get(transfer_id) {
                return Some(TransferProgress {
                    transfer_id: transfer_id.to_string(),
                    status,
                    progress,
                    bandwidth_bytes_per_sec: bandwidth,
                    chunks_received: session.chunks_received.len(),
                    chunks_total: session.chunks_needed.len(),
                    bytes_transferred: (session.total_size as f32 * progress) as u64,
                    bytes_total: session.total_size,
                });
            }
        }
        None
    }

    /// List all active transfers
    pub async fn list_active_transfers(&self) -> Vec<TransferProgress> {
        let active_transfers = self.active_transfers.read().await;
        let mut transfers = Vec::new();

        for transfer_id in active_transfers.keys() {
            if let Some(progress) = self.get_transfer_progress(transfer_id).await {
                transfers.push(progress);
            }
        }

        transfers
    }

    /// Internal method to handle chunk transfer with resumption support
    async fn transfer_chunks_resumable(&mut self, transfer_id: String) -> Result<()> {
        let chunks_to_request = {
            let active_transfers = self.active_transfers.read().await;
            let session = active_transfers.get(&transfer_id).ok_or_else(|| {
                QuicError::Protocol(format!("Transfer session not found: {}", transfer_id))
            })?;

            // Find chunks we still need (haven't received yet)
            session
                .chunks_needed
                .iter()
                .filter(|hash| !session.chunks_received.contains_key(*hash))
                .cloned()
                .collect::<Vec<_>>()
        };

        if chunks_to_request.is_empty() {
            info!("All chunks received for transfer: {}", transfer_id);
            self.complete_transfer(&transfer_id).await?;
            return Ok(());
        }

        info!(
            "Requesting {} chunks for transfer: {}",
            chunks_to_request.len(),
            transfer_id
        );

        // Use batch manager to request chunks efficiently
        match self
            .batch_manager
            .transfer_chunks(chunks_to_request.clone())
            .await
        {
            Ok(received_chunks) => {
                // Update session with received chunks
                {
                    let mut active_transfers = self.active_transfers.write().await;
                    if let Some(session) = active_transfers.get_mut(&transfer_id) {
                        for chunk in received_chunks {
                            let hash = blake3::hash(&chunk.data).as_bytes().to_vec();
                            session.chunks_received.insert(hash, chunk);
                        }
                    }
                }

                // Update progress in resumable manager
                let progress = self.calculate_progress(&transfer_id).await;
                self.resumable_manager
                    .update_progress(
                        &transfer_id,
                        progress.bytes_transferred,
                        progress.chunks_received as u64,
                    )
                    .await?;

                // Check if transfer is complete
                if progress.chunks_received == progress.chunks_total {
                    self.complete_transfer(&transfer_id).await?;
                } else {
                    debug!(
                        "Transfer progress: {:.1}% ({}/{})",
                        progress.progress * 100.0,
                        progress.chunks_received,
                        progress.chunks_total
                    );
                }
            }
            Err(e) => {
                error!("Chunk transfer failed for {}: {}", transfer_id, e);
                self.resumable_manager
                    .fail_transfer(&transfer_id, &format!("Chunk transfer failed: {}", e))
                    .await?;
                return Err(e);
            }
        }

        Ok(())
    }

    /// Complete a transfer successfully
    async fn complete_transfer(&mut self, transfer_id: &str) -> Result<()> {
        info!("Completing transfer: {}", transfer_id);

        // Update session status
        {
            let mut active_transfers = self.active_transfers.write().await;
            if let Some(session) = active_transfers.get_mut(transfer_id) {
                session.status = TransferStatus::Completed;
            }
        }

        // Mark as completed in resumable manager
        self.resumable_manager
            .complete_transfer(transfer_id)
            .await?;

        Ok(())
    }

    /// Calculate current transfer progress
    async fn calculate_progress(&self, transfer_id: &str) -> TransferProgress {
        let active_transfers = self.active_transfers.read().await;

        if let Some(session) = active_transfers.get(transfer_id) {
            let chunks_received = session.chunks_received.len();
            let chunks_total = session.chunks_needed.len();
            let progress = if chunks_total > 0 {
                chunks_received as f32 / chunks_total as f32
            } else {
                0.0
            };

            TransferProgress {
                transfer_id: transfer_id.to_string(),
                status: session.status.clone(),
                progress,
                bandwidth_bytes_per_sec: 0.0, // Will be calculated by resumable manager
                chunks_received,
                chunks_total,
                bytes_transferred: (session.total_size as f32 * progress) as u64,
                bytes_total: session.total_size,
            }
        } else {
            TransferProgress {
                transfer_id: transfer_id.to_string(),
                status: TransferStatus::Failed,
                progress: 0.0,
                bandwidth_bytes_per_sec: 0.0,
                chunks_received: 0,
                chunks_total: 0,
                bytes_transferred: 0,
                bytes_total: 0,
            }
        }
    }
}

/// Transfer progress information
#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub transfer_id: String,
    pub status: TransferStatus,
    pub progress: f32, // 0.0 to 1.0
    pub bandwidth_bytes_per_sec: f32,
    pub chunks_received: usize,
    pub chunks_total: usize,
    pub bytes_transferred: u64,
    pub bytes_total: u64,
}

impl TransferProgress {
    /// Get human-readable transfer status
    pub fn status_text(&self) -> &'static str {
        match self.status {
            TransferStatus::Active => "Active",
            TransferStatus::Paused => "Paused",
            TransferStatus::Completed => "Completed",
            TransferStatus::Failed => "Failed",
            TransferStatus::Cancelled => "Cancelled",
        }
    }

    /// Get progress as percentage
    pub fn progress_percent(&self) -> f32 {
        self.progress * 100.0
    }

    /// Get human-readable bandwidth
    pub fn bandwidth_text(&self) -> String {
        if self.bandwidth_bytes_per_sec < 1024.0 {
            format!("{:.1} B/s", self.bandwidth_bytes_per_sec)
        } else if self.bandwidth_bytes_per_sec < 1024.0 * 1024.0 {
            format!("{:.1} KB/s", self.bandwidth_bytes_per_sec / 1024.0)
        } else {
            format!(
                "{:.1} MB/s",
                self.bandwidth_bytes_per_sec / (1024.0 * 1024.0)
            )
        }
    }

    /// Get human-readable size text
    pub fn size_text(&self) -> String {
        fn format_bytes(bytes: u64) -> String {
            if bytes < 1024 {
                format!("{} B", bytes)
            } else if bytes < 1024 * 1024 {
                format!("{:.1} KB", bytes as f64 / 1024.0)
            } else if bytes < 1024 * 1024 * 1024 {
                format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
            } else {
                format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
            }
        }

        format!(
            "{} / {}",
            format_bytes(self.bytes_transferred),
            format_bytes(self.bytes_total)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_progress_formatting() {
        let progress = TransferProgress {
            transfer_id: "test-transfer".to_string(),
            status: TransferStatus::Active,
            progress: 0.65,
            bandwidth_bytes_per_sec: 1536.0, // 1.5 KB/s
            chunks_received: 65,
            chunks_total: 100,
            bytes_transferred: 65 * 1024,
            bytes_total: 100 * 1024,
        };

        assert_eq!(progress.status_text(), "Active");
        assert_eq!(progress.progress_percent(), 65.0);
        assert_eq!(progress.bandwidth_text(), "1.5 KB/s");
        assert_eq!(progress.size_text(), "65.0 KB / 100.0 KB");
    }

    #[test]
    fn test_transfer_progress_large_sizes() {
        let progress = TransferProgress {
            transfer_id: "large-transfer".to_string(),
            status: TransferStatus::Active,
            progress: 0.33,
            bandwidth_bytes_per_sec: 5.0 * 1024.0 * 1024.0, // 5 MB/s
            chunks_received: 330,
            chunks_total: 1000,
            bytes_transferred: 330 * 1024 * 1024,
            bytes_total: 1024 * 1024 * 1024,
        };

        assert_eq!(progress.bandwidth_text(), "5.0 MB/s");
        assert_eq!(progress.size_text(), "330.0 MB / 1.0 GB");
    }
}
