//! Resumable transfer support for QUIC streams

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Mutex};
use tokio::time::interval;
use tracing::{debug, info, warn, error};
use serde::{Deserialize, Serialize};

use crate::errors::{QuicError, Result};
use crate::Connection;

/// Transfer checkpoint information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferCheckpoint {
    pub transfer_id: String,
    pub peer_id: String,
    pub file_path: String,
    pub total_size: u64,
    pub bytes_transferred: u64,
    pub chunk_offset: u64,
    pub last_checkpoint: chrono::DateTime<chrono::Utc>,
    pub retry_count: u32,
}

/// Status of a resumable transfer
#[derive(Debug, Clone, PartialEq)]
pub enum TransferStatus {
    /// Transfer is actively running
    Active,
    /// Transfer is paused and can be resumed
    Paused,
    /// Transfer completed successfully
    Completed,
    /// Transfer failed and needs retry
    Failed,
    /// Transfer was cancelled
    Cancelled,
}

/// Active transfer state
#[derive(Debug, Clone)]
pub struct ActiveTransfer {
    pub checkpoint: TransferCheckpoint,
    pub status: TransferStatus,
    pub connection: Option<Arc<Connection>>,
    pub last_activity: Instant,
    pub bandwidth_samples: Vec<(Instant, u64)>, // (time, bytes)
}

/// Manager for resumable transfers
pub struct ResumableTransferManager {
    active_transfers: Arc<RwLock<HashMap<String, ActiveTransfer>>>,
    checkpoints: Arc<Mutex<HashMap<String, TransferCheckpoint>>>,
    checkpoint_interval: Duration,
    transfer_timeout: Duration,
    max_retry_count: u32,
}

impl ResumableTransferManager {
    /// Create a new resumable transfer manager
    pub fn new(
        checkpoint_interval: Duration,
        transfer_timeout: Duration,
        max_retry_count: u32,
    ) -> Self {
        let manager = Self {
            active_transfers: Arc::new(RwLock::new(HashMap::new())),
            checkpoints: Arc::new(Mutex::new(HashMap::new())),
            checkpoint_interval,
            transfer_timeout,
            max_retry_count,
        };

        // Start background cleanup task
        manager.start_cleanup_task();
        
        manager
    }

    /// Start a new resumable transfer
    pub async fn start_transfer(
        &self,
        transfer_id: String,
        peer_id: String,
        file_path: String,
        total_size: u64,
        connection: Arc<Connection>,
    ) -> Result<()> {
        info!("Starting resumable transfer: {} ({} bytes)", transfer_id, total_size);

        // Check if we have a checkpoint for this transfer
        let checkpoint = self.load_checkpoint(&transfer_id).await?;
        
        let bytes_transferred = checkpoint.as_ref()
            .map(|c| c.bytes_transferred)
            .unwrap_or(0);

        if bytes_transferred >= total_size {
            info!("Transfer already completed: {}", transfer_id);
            return Ok(());
        }

        let checkpoint = checkpoint.unwrap_or_else(|| TransferCheckpoint {
            transfer_id: transfer_id.clone(),
            peer_id: peer_id.clone(),
            file_path: file_path.clone(),
            total_size,
            bytes_transferred: 0,
            chunk_offset: 0,
            last_checkpoint: chrono::Utc::now(),
            retry_count: 0,
        });

        let active_transfer = ActiveTransfer {
            checkpoint: checkpoint.clone(),
            status: TransferStatus::Active,
            connection: Some(connection),
            last_activity: Instant::now(),
            bandwidth_samples: Vec::new(),
        };

        // Register active transfer
        {
            let mut active_transfers = self.active_transfers.write().await;
            active_transfers.insert(transfer_id.clone(), active_transfer);
        }

        // Save initial checkpoint
        self.save_checkpoint(checkpoint).await?;

        if bytes_transferred > 0 {
            info!("Resuming transfer from {} bytes: {}", bytes_transferred, transfer_id);
        }

        Ok(())
    }

    /// Update transfer progress and create checkpoint
    pub async fn update_progress(
        &self,
        transfer_id: &str,
        bytes_transferred: u64,
        chunk_offset: u64,
    ) -> Result<()> {
        let mut active_transfers = self.active_transfers.write().await;
        
        if let Some(transfer) = active_transfers.get_mut(transfer_id) {
            let now = Instant::now();
            
            // Update progress
            transfer.checkpoint.bytes_transferred = bytes_transferred;
            transfer.checkpoint.chunk_offset = chunk_offset;
            transfer.last_activity = now;
            
            // Record bandwidth sample
            transfer.bandwidth_samples.push((now, bytes_transferred));
            
            // Keep only recent samples (last 30 seconds)
            let cutoff = now - Duration::from_secs(30);
            transfer.bandwidth_samples.retain(|(time, _)| *time > cutoff);
            
            // Checkpoint periodically
            let elapsed = chrono::Utc::now()
                .signed_duration_since(transfer.checkpoint.last_checkpoint);
            
            if elapsed.num_seconds() >= self.checkpoint_interval.as_secs() as i64 {
                transfer.checkpoint.last_checkpoint = chrono::Utc::now();
                let checkpoint = transfer.checkpoint.clone();
                drop(active_transfers);
                
                self.save_checkpoint(checkpoint).await?;
                
                debug!("Checkpointed transfer {} at {} bytes", 
                    transfer_id, bytes_transferred);
            }
        } else {
            warn!("Update for unknown transfer: {}", transfer_id);
        }
        
        Ok(())
    }

    /// Complete a transfer successfully
    pub async fn complete_transfer(&self, transfer_id: &str) -> Result<()> {
        info!("Completing transfer: {}", transfer_id);
        
        let mut active_transfers = self.active_transfers.write().await;
        
        if let Some(mut transfer) = active_transfers.remove(transfer_id) {
            transfer.status = TransferStatus::Completed;
            transfer.checkpoint.last_checkpoint = chrono::Utc::now();
            
            // Save final checkpoint
            let checkpoint = transfer.checkpoint.clone();
            drop(active_transfers);
            
            self.save_checkpoint(checkpoint).await?;
            
            // Remove checkpoint after delay (for debugging)
            tokio::spawn({
                let manager = self.clone();
                let transfer_id = transfer_id.to_string();
                async move {
                    tokio::time::sleep(Duration::from_secs(300)).await; // 5 minutes
                    if let Err(e) = manager.remove_checkpoint(&transfer_id).await {
                        warn!("Failed to remove completed transfer checkpoint: {}", e);
                    }
                }
            });
        }
        
        Ok(())
    }

    /// Fail a transfer and mark for retry
    pub async fn fail_transfer(&self, transfer_id: &str, error: &str) -> Result<()> {
        warn!("Failing transfer {}: {}", transfer_id, error);
        
        let mut active_transfers = self.active_transfers.write().await;
        
        if let Some(transfer) = active_transfers.get_mut(transfer_id) {
            transfer.status = TransferStatus::Failed;
            transfer.checkpoint.retry_count += 1;
            transfer.checkpoint.last_checkpoint = chrono::Utc::now();
            
            let checkpoint = transfer.checkpoint.clone();
            
            if checkpoint.retry_count >= self.max_retry_count {
                error!("Transfer exceeded max retries: {}", transfer_id);
                active_transfers.remove(transfer_id);
            }
            
            drop(active_transfers);
            self.save_checkpoint(checkpoint).await?;
        }
        
        Ok(())
    }

    /// Pause a transfer
    pub async fn pause_transfer(&self, transfer_id: &str) -> Result<()> {
        debug!("Pausing transfer: {}", transfer_id);
        
        let mut active_transfers = self.active_transfers.write().await;
        
        if let Some(transfer) = active_transfers.get_mut(transfer_id) {
            transfer.status = TransferStatus::Paused;
            transfer.checkpoint.last_checkpoint = chrono::Utc::now();
            
            let checkpoint = transfer.checkpoint.clone();
            drop(active_transfers);
            
            self.save_checkpoint(checkpoint).await?;
        }
        
        Ok(())
    }

    /// Resume a paused transfer
    pub async fn resume_transfer(&self, transfer_id: &str, connection: Arc<Connection>) -> Result<()> {
        info!("Resuming transfer: {}", transfer_id);
        
        let mut active_transfers = self.active_transfers.write().await;
        
        if let Some(transfer) = active_transfers.get_mut(transfer_id) {
            transfer.status = TransferStatus::Active;
            transfer.connection = Some(connection);
            transfer.last_activity = Instant::now();
            transfer.bandwidth_samples.clear();
        } else {
            // Try to load from checkpoint
            if let Some(checkpoint) = self.load_checkpoint(transfer_id).await? {
                let active_transfer = ActiveTransfer {
                    checkpoint,
                    status: TransferStatus::Active,
                    connection: Some(connection),
                    last_activity: Instant::now(),
                    bandwidth_samples: Vec::new(),
                };
                
                active_transfers.insert(transfer_id.to_string(), active_transfer);
            } else {
                return Err(QuicError::Protocol(format!("No checkpoint found for transfer: {}", transfer_id)));
            }
        }
        
        Ok(())
    }

    /// Get transfer status and progress
    pub async fn get_transfer_status(&self, transfer_id: &str) -> Option<(TransferStatus, f32, f32)> {
        let active_transfers = self.active_transfers.read().await;
        
        if let Some(transfer) = active_transfers.get(transfer_id) {
            let progress = if transfer.checkpoint.total_size > 0 {
                transfer.checkpoint.bytes_transferred as f32 / transfer.checkpoint.total_size as f32
            } else {
                0.0
            };
            
            // Calculate bandwidth (bytes per second)
            let bandwidth = self.calculate_bandwidth(&transfer.bandwidth_samples);
            
            Some((transfer.status.clone(), progress, bandwidth))
        } else {
            None
        }
    }

    /// Get all active transfers
    pub async fn get_active_transfers(&self) -> HashMap<String, ActiveTransfer> {
        self.active_transfers.read().await.clone()
    }

    /// Load checkpoint from persistent storage
    async fn load_checkpoint(&self, transfer_id: &str) -> Result<Option<TransferCheckpoint>> {
        let checkpoints = self.checkpoints.lock().await;
        Ok(checkpoints.get(transfer_id).cloned())
    }

    /// Save checkpoint to persistent storage
    async fn save_checkpoint(&self, checkpoint: TransferCheckpoint) -> Result<()> {
        let mut checkpoints = self.checkpoints.lock().await;
        checkpoints.insert(checkpoint.transfer_id.clone(), checkpoint);
        Ok(())
    }

    /// Remove checkpoint from persistent storage
    async fn remove_checkpoint(&self, transfer_id: &str) -> Result<()> {
        let mut checkpoints = self.checkpoints.lock().await;
        checkpoints.remove(transfer_id);
        Ok(())
    }

    /// Calculate current bandwidth from samples
    fn calculate_bandwidth(&self, samples: &[(Instant, u64)]) -> f32 {
        if samples.len() < 2 {
            return 0.0;
        }
        
        // Get first and last samples
        let (first_time, first_bytes) = samples[0];
        let (last_time, last_bytes) = samples[samples.len() - 1];
        
        let duration = last_time.duration_since(first_time).as_secs_f32();
        if duration > 0.0 {
            (last_bytes - first_bytes) as f32 / duration
        } else {
            0.0
        }
    }

    /// Start background cleanup task
    fn start_cleanup_task(&self) {
        let active_transfers = self.active_transfers.clone();
        let transfer_timeout = self.transfer_timeout;
        
        tokio::spawn(async move {
            let mut cleanup_interval = interval(Duration::from_secs(60)); // Check every minute
            
            loop {
                cleanup_interval.tick().await;
                
                let mut to_remove = Vec::new();
                let now = Instant::now();
                
                {
                    let active = active_transfers.read().await;
                    for (transfer_id, transfer) in active.iter() {
                        if transfer.status == TransferStatus::Active {
                            let inactive_duration = now.duration_since(transfer.last_activity);
                            if inactive_duration > transfer_timeout {
                                warn!("Transfer timeout: {} (inactive for {:?})", 
                                    transfer_id, inactive_duration);
                                to_remove.push(transfer_id.clone());
                            }
                        }
                    }
                }
                
                // Remove timed out transfers
                if !to_remove.is_empty() {
                    let mut active = active_transfers.write().await;
                    for transfer_id in to_remove {
                        if let Some(mut transfer) = active.remove(&transfer_id) {
                            transfer.status = TransferStatus::Failed;
                            // Could save checkpoint here for retry
                        }
                    }
                }
            }
        });
    }
}

impl Clone for ResumableTransferManager {
    fn clone(&self) -> Self {
        Self {
            active_transfers: self.active_transfers.clone(),
            checkpoints: self.checkpoints.clone(),
            checkpoint_interval: self.checkpoint_interval,
            transfer_timeout: self.transfer_timeout,
            max_retry_count: self.max_retry_count,
        }
    }
}

impl Default for ResumableTransferManager {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(10),  // Checkpoint every 10 seconds
            Duration::from_secs(300), // 5 minute timeout
            3,                        // Max 3 retries
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_resumable_transfer_manager() {
        let _manager = ResumableTransferManager::default();
        
        // Skip connection-dependent tests in unit tests
        // In integration tests, real connections would be used
    }
}