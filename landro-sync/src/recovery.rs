//! Crash safety and recovery mechanisms for sync operations

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::errors::Result;
use crate::state::AsyncSyncDatabase;

/// Recovery manager for handling crash safety and resumption
pub struct RecoveryManager {
    database: AsyncSyncDatabase,
    recovery_log_path: PathBuf,
    active_operations: Arc<RwLock<HashMap<String, Operation>>>,
}

/// Types of operations that can be recovered
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationType {
    /// File transfer operation
    Transfer {
        peer_id: String,
        file_path: String,
        chunk_hash: String,
        total_size: u64,
        bytes_transferred: u64,
    },
    /// Manifest synchronization
    ManifestSync {
        peer_id: String,
        folder_id: String,
        local_manifest_hash: String,
        remote_manifest_hash: String,
    },
    /// Index update operation
    IndexUpdate {
        folder_path: String,
        files_processed: usize,
        total_files: usize,
    },
}

/// Recoverable operation record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub operation_type: OperationType,
    pub started_at: DateTime<Utc>,
    pub last_checkpoint: DateTime<Utc>,
    pub status: OperationStatus,
    pub retry_count: u32,
    pub error_message: Option<String>,
}

/// Status of an operation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OperationStatus {
    /// Operation is currently running
    InProgress,
    /// Operation completed successfully
    Completed,
    /// Operation failed and needs retry
    Failed,
    /// Operation was interrupted (e.g., by crash)
    Interrupted,
    /// Operation is being recovered
    Recovering,
}

/// Recovery statistics
#[derive(Debug, Clone)]
pub struct RecoveryStats {
    pub operations_recovered: usize,
    pub operations_completed: usize,
    pub operations_failed: usize,
    pub operations_abandoned: usize,
    pub total_recovery_time: std::time::Duration,
}

impl RecoveryManager {
    /// Create a new recovery manager
    pub async fn new(database: AsyncSyncDatabase, storage_path: impl AsRef<Path>) -> Result<Self> {
        let storage_path = storage_path.as_ref();
        let recovery_log_path = storage_path.join("recovery.log");

        // Ensure recovery directory exists
        if let Some(parent) = recovery_log_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        Ok(Self {
            database,
            recovery_log_path,
            active_operations: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Start recovery process on system startup
    pub async fn recover_on_startup(&self) -> Result<RecoveryStats> {
        info!("Starting crash recovery process");
        let start_time = std::time::Instant::now();

        let mut stats = RecoveryStats {
            operations_recovered: 0,
            operations_completed: 0,
            operations_failed: 0,
            operations_abandoned: 0,
            total_recovery_time: std::time::Duration::default(),
        };

        // Load recovery log
        let operations = self.load_recovery_log().await?;

        if operations.is_empty() {
            info!("No operations to recover");
            return Ok(stats);
        }

        info!("Found {} operations to recover", operations.len());

        // Process each operation
        for mut operation in operations {
            stats.operations_recovered += 1;

            // Check if operation is too old to recover
            let age = Utc::now().signed_duration_since(operation.started_at);
            if age.num_hours() > 24 {
                warn!(
                    "Abandoning old operation: {} (age: {} hours)",
                    operation.id,
                    age.num_hours()
                );
                stats.operations_abandoned += 1;
                continue;
            }

            // Mark as recovering
            operation.status = OperationStatus::Recovering;
            self.save_operation(&operation).await?;

            // Attempt recovery based on operation type
            match self.recover_operation(&mut operation).await {
                Ok(true) => {
                    info!("Successfully recovered operation: {}", operation.id);
                    operation.status = OperationStatus::Completed;
                    stats.operations_completed += 1;
                }
                Ok(false) => {
                    warn!("Operation recovery incomplete: {}", operation.id);
                    operation.status = OperationStatus::InProgress;
                }
                Err(e) => {
                    error!("Failed to recover operation {}: {}", operation.id, e);
                    operation.retry_count += 1;
                    operation.error_message = Some(e.to_string());

                    if operation.retry_count >= 3 {
                        operation.status = OperationStatus::Failed;
                        stats.operations_failed += 1;
                    } else {
                        operation.status = OperationStatus::InProgress;
                    }
                }
            }

            self.save_operation(&operation).await?;
        }

        stats.total_recovery_time = start_time.elapsed();

        info!(
            "Recovery completed: {} recovered, {} completed, {} failed, {} abandoned in {:?}",
            stats.operations_recovered,
            stats.operations_completed,
            stats.operations_failed,
            stats.operations_abandoned,
            stats.total_recovery_time
        );

        Ok(stats)
    }

    /// Start tracking a new operation
    pub async fn start_operation(&self, operation: Operation) -> Result<()> {
        debug!("Starting operation: {}", operation.id);

        // Add to active operations
        {
            let mut active = self.active_operations.write().await;
            active.insert(operation.id.clone(), operation.clone());
        }

        // Write to recovery log
        self.save_operation(&operation).await?;

        Ok(())
    }

    /// Update operation progress (checkpoint)
    pub async fn checkpoint_operation(
        &self,
        operation_id: &str,
        progress_update: impl FnOnce(&mut Operation),
    ) -> Result<()> {
        let mut active = self.active_operations.write().await;

        if let Some(operation) = active.get_mut(operation_id) {
            // Apply progress update
            progress_update(operation);
            operation.last_checkpoint = Utc::now();

            // Save checkpoint to log
            self.save_operation(operation).await?;

            debug!("Checkpointed operation: {}", operation_id);
        } else {
            warn!(
                "Attempted to checkpoint unknown operation: {}",
                operation_id
            );
        }

        Ok(())
    }

    /// Complete an operation successfully
    pub async fn complete_operation(&self, operation_id: &str) -> Result<()> {
        debug!("Completing operation: {}", operation_id);

        let mut active = self.active_operations.write().await;

        if let Some(mut operation) = active.remove(operation_id) {
            operation.status = OperationStatus::Completed;
            operation.last_checkpoint = Utc::now();

            // Save final state
            self.save_operation(&operation).await?;

            // Remove from recovery log immediately
            // In a production system, you might want to keep completed operations
            // for a while for debugging purposes, but for now we'll clean up immediately
            if let Err(e) = self.remove_operation(operation_id).await {
                warn!("Failed to remove completed operation from log: {}", e);
            }
        }

        Ok(())
    }

    /// Mark operation as failed
    pub async fn fail_operation(&self, operation_id: &str, error: &str) -> Result<()> {
        debug!("Failing operation: {} - {}", operation_id, error);

        let mut active = self.active_operations.write().await;

        if let Some(mut operation) = active.get_mut(operation_id) {
            operation.status = OperationStatus::Failed;
            operation.error_message = Some(error.to_string());
            operation.retry_count += 1;
            operation.last_checkpoint = Utc::now();

            // Save failed state
            self.save_operation(&operation).await?;

            // Remove from active if max retries reached
            if operation.retry_count >= 3 {
                active.remove(operation_id);
            }
        }

        Ok(())
    }

    /// Get list of active operations
    pub async fn get_active_operations(&self) -> HashMap<String, Operation> {
        self.active_operations.read().await.clone()
    }

    /// Recover a specific operation based on its type
    async fn recover_operation(&self, operation: &mut Operation) -> Result<bool> {
        match &operation.operation_type {
            OperationType::Transfer {
                peer_id,
                file_path,
                chunk_hash,
                total_size,
                bytes_transferred,
            } => {
                self.recover_transfer_operation(
                    peer_id,
                    file_path,
                    chunk_hash,
                    *total_size,
                    *bytes_transferred,
                )
                .await
            }
            OperationType::ManifestSync {
                peer_id,
                folder_id,
                local_manifest_hash,
                remote_manifest_hash,
            } => {
                self.recover_manifest_sync_operation(
                    peer_id,
                    folder_id,
                    local_manifest_hash,
                    remote_manifest_hash,
                )
                .await
            }
            OperationType::IndexUpdate {
                folder_path,
                files_processed,
                total_files,
            } => {
                self.recover_index_update_operation(folder_path, *files_processed, *total_files)
                    .await
            }
        }
    }

    /// Recover a file transfer operation
    async fn recover_transfer_operation(
        &self,
        peer_id: &str,
        file_path: &str,
        chunk_hash: &str,
        total_size: u64,
        bytes_transferred: u64,
    ) -> Result<bool> {
        debug!(
            "Recovering transfer: peer={}, file={}, chunk={}, progress={}/{}",
            peer_id, file_path, chunk_hash, bytes_transferred, total_size
        );

        // Check if transfer was already completed
        if bytes_transferred >= total_size {
            info!("Transfer already completed during recovery: {}", chunk_hash);
            return Ok(true);
        }

        // Add back to pending transfers in database
        self.database
            .add_pending_transfer(
                peer_id, "default", // TODO: Get actual folder ID
                file_path, chunk_hash, 0, // High priority for resumed transfers
            )
            .await?;

        // Transfer will be resumed by the scheduler
        Ok(false)
    }

    /// Recover a manifest sync operation
    async fn recover_manifest_sync_operation(
        &self,
        peer_id: &str,
        folder_id: &str,
        local_manifest_hash: &str,
        remote_manifest_hash: &str,
    ) -> Result<bool> {
        debug!(
            "Recovering manifest sync: peer={}, folder={}, local={}, remote={}",
            peer_id, folder_id, local_manifest_hash, remote_manifest_hash
        );

        // Check if sync is still needed
        if local_manifest_hash == remote_manifest_hash {
            info!("Manifest sync no longer needed: {}", folder_id);
            return Ok(true);
        }

        // Manifest sync will be retrigger by the orchestrator
        info!("Manifest sync will be retriggered: {}", folder_id);
        Ok(false)
    }

    /// Recover an index update operation
    async fn recover_index_update_operation(
        &self,
        folder_path: &str,
        files_processed: usize,
        total_files: usize,
    ) -> Result<bool> {
        debug!(
            "Recovering index update: folder={}, progress={}/{}",
            folder_path, files_processed, total_files
        );

        // Check if indexing was completed
        if files_processed >= total_files {
            info!("Index update already completed: {}", folder_path);
            return Ok(true);
        }

        // Index update will be retriggered by file watcher
        info!("Index update will be retriggered: {}", folder_path);
        Ok(false)
    }

    /// Save operation to recovery log
    async fn save_operation(&self, operation: &Operation) -> Result<()> {
        let log_entry = format!("{}\n", serde_json::to_string(operation)?);

        // Append to recovery log (atomic write)
        let temp_path = format!("{}.tmp", self.recovery_log_path.display());

        // Read existing log
        let mut existing_content = String::new();
        if self.recovery_log_path.exists() {
            existing_content = fs::read_to_string(&self.recovery_log_path)
                .await
                .unwrap_or_default();
        }

        // Update or append operation
        let mut lines: Vec<String> = existing_content.lines().map(|s| s.to_string()).collect();
        let mut found = false;

        for line in &mut lines {
            if let Ok(existing_op) = serde_json::from_str::<Operation>(line) {
                if existing_op.id == operation.id {
                    *line = serde_json::to_string(operation)?;
                    found = true;
                    break;
                }
            }
        }

        if !found {
            lines.push(serde_json::to_string(operation)?);
        }

        let new_content = lines.join("\n") + "\n";

        // Atomic write
        fs::write(&temp_path, new_content).await?;
        fs::rename(&temp_path, &self.recovery_log_path).await?;

        Ok(())
    }

    /// Load operations from recovery log
    async fn load_recovery_log(&self) -> Result<Vec<Operation>> {
        if !self.recovery_log_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.recovery_log_path).await?;
        let mut operations = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<Operation>(line) {
                Ok(operation) => {
                    // Only recover incomplete operations
                    if matches!(
                        operation.status,
                        OperationStatus::InProgress | OperationStatus::Interrupted
                    ) {
                        operations.push(operation);
                    }
                }
                Err(e) => {
                    warn!("Failed to parse recovery log line: {} - {}", e, line);
                }
            }
        }

        Ok(operations)
    }

    /// Remove operation from recovery log
    async fn remove_operation(&self, operation_id: &str) -> Result<()> {
        if !self.recovery_log_path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&self.recovery_log_path).await?;
        let mut lines = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<Operation>(line) {
                Ok(operation) => {
                    if operation.id != operation_id {
                        lines.push(line.to_string());
                    }
                }
                Err(_) => {
                    lines.push(line.to_string());
                }
            }
        }

        let new_content = lines.join("\n") + "\n";
        fs::write(&self.recovery_log_path, new_content).await?;

        Ok(())
    }
}

impl Clone for RecoveryManager {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            recovery_log_path: self.recovery_log_path.clone(),
            active_operations: self.active_operations.clone(),
        }
    }
}

/// Helper function to generate operation IDs
pub fn generate_operation_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let random: u32 = rand::random();
    format!("op_{}_{:08x}", timestamp, random)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_recovery_manager() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = AsyncSyncDatabase::open_in_memory().await.unwrap();

        let recovery_manager = RecoveryManager::new(db, temp_dir.path()).await.unwrap();

        // Create test operation
        let operation = Operation {
            id: generate_operation_id(),
            operation_type: OperationType::Transfer {
                peer_id: "test-peer".to_string(),
                file_path: "test.txt".to_string(),
                chunk_hash: "abcd1234".to_string(),
                total_size: 1024,
                bytes_transferred: 512,
            },
            started_at: Utc::now(),
            last_checkpoint: Utc::now(),
            status: OperationStatus::InProgress,
            retry_count: 0,
            error_message: None,
        };

        // Start operation
        recovery_manager
            .start_operation(operation.clone())
            .await
            .unwrap();

        // Check it's active
        let active = recovery_manager.get_active_operations().await;
        assert!(active.contains_key(&operation.id));

        // Complete operation
        recovery_manager
            .complete_operation(&operation.id)
            .await
            .unwrap();

        // Check it's no longer active
        let active = recovery_manager.get_active_operations().await;
        assert!(!active.contains_key(&operation.id));
    }
}
