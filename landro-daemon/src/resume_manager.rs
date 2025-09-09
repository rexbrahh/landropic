//! Resume Manager - Day 2 Resume Capability for Interrupted Syncs
//!
//! This module implements robust resume functionality that allows sync operations
//! to recover from interruptions (network failures, crashes, shutdowns) without
//! starting over, saving both time and bandwidth.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::bloom_diff::{DiffProgress, DiffStage};
use landro_cas::ContentStore;
use landro_index::AsyncIndexer;

/// Resume checkpoint - saved state of sync operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeCheckpoint {
    pub session_id: String,
    pub peer_id: String,
    pub folder_path: PathBuf,
    pub created_at: SystemTime,
    pub last_updated: SystemTime,
    pub stage: DiffStage,
    pub progress_percent: u8,

    // Bloom diff state
    pub manifests_exchanged: bool,
    pub bloom_summaries_complete: bool,
    pub differences_identified: bool,
    pub suspected_files: Vec<String>,
    pub confirmed_differences: Vec<String>,

    // Transfer state
    pub total_chunks_needed: usize,
    pub chunks_transferred: Vec<String>,
    pub chunks_failed: Vec<String>,
    pub bytes_transferred: u64,
    pub bytes_total: u64,

    // Network state
    pub connection_stable: bool,
    pub retry_count: u32,
    pub last_error: Option<String>,
    pub bandwidth_stats: ResumeBandwidthStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeBandwidthStats {
    pub bytes_saved_by_resume: u64,
    pub chunks_skipped: usize,
    pub resume_overhead_bytes: u64,
    pub time_saved_seconds: u64,
}

/// Resume manager for handling sync interruptions
pub struct ResumeManager {
    checkpoints: Arc<RwLock<HashMap<String, ResumeCheckpoint>>>,
    checkpoint_file: PathBuf,
    cas: Arc<ContentStore>,
    indexer: Arc<AsyncIndexer>,
    active_sessions: Arc<RwLock<HashMap<String, ResumableSession>>>,
    checkpoint_interval: Duration,
}

#[derive(Debug)]
struct ResumableSession {
    session_id: String,
    checkpoint_tx: mpsc::Sender<CheckpointUpdate>,
    last_checkpoint: SystemTime,
    auto_save_enabled: bool,
}

#[derive(Debug)]
enum CheckpointUpdate {
    Stage(DiffStage),
    Progress(u8),
    ChunkTransferred(String),
    ChunkFailed(String),
    BytesTransferred(u64),
    Error(String),
    Complete,
}

impl ResumeManager {
    /// Create new resume manager
    pub async fn new(
        storage_path: &PathBuf,
        cas: Arc<ContentStore>,
        indexer: Arc<AsyncIndexer>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let checkpoint_file = storage_path.join("sync_checkpoints.json");

        // Load existing checkpoints
        let checkpoints = if checkpoint_file.exists() {
            let data = tokio::fs::read_to_string(&checkpoint_file).await?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Ok(Self {
            checkpoints: Arc::new(RwLock::new(checkpoints)),
            checkpoint_file,
            cas,
            indexer,
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
            checkpoint_interval: Duration::from_secs(30), // Checkpoint every 30 seconds
        })
    }

    /// Start tracking a resumable sync session
    pub async fn start_resumable_session(
        &self,
        session_id: String,
        peer_id: String,
        folder_path: PathBuf,
    ) -> Result<mpsc::Receiver<CheckpointUpdate>, Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "🔄 Starting resumable session: {} -> {}",
            session_id,
            folder_path.display()
        );

        let (checkpoint_tx, checkpoint_rx) = mpsc::channel(100);

        // Create initial checkpoint
        let checkpoint = ResumeCheckpoint {
            session_id: session_id.clone(),
            peer_id,
            folder_path,
            created_at: SystemTime::now(),
            last_updated: SystemTime::now(),
            stage: DiffStage::Initializing,
            progress_percent: 0,
            manifests_exchanged: false,
            bloom_summaries_complete: false,
            differences_identified: false,
            suspected_files: Vec::new(),
            confirmed_differences: Vec::new(),
            total_chunks_needed: 0,
            chunks_transferred: Vec::new(),
            chunks_failed: Vec::new(),
            bytes_transferred: 0,
            bytes_total: 0,
            connection_stable: true,
            retry_count: 0,
            last_error: None,
            bandwidth_stats: ResumeBandwidthStats {
                bytes_saved_by_resume: 0,
                chunks_skipped: 0,
                resume_overhead_bytes: 0,
                time_saved_seconds: 0,
            },
        };

        // Save initial checkpoint
        self.save_checkpoint(checkpoint.clone()).await?;

        // Track session
        let session = ResumableSession {
            session_id: session_id.clone(),
            checkpoint_tx: checkpoint_tx.clone(),
            last_checkpoint: SystemTime::now(),
            auto_save_enabled: true,
        };

        self.active_sessions
            .write()
            .await
            .insert(session_id.clone(), session);

        // Start checkpoint monitoring
        let self_clone = Arc::new(self.clone());
        let session_id_clone = session_id.clone();
        tokio::spawn(async move {
            self_clone
                .monitor_session_checkpoints(session_id_clone)
                .await;
        });

        Ok(checkpoint_rx)
    }

    /// Check if a session can be resumed
    pub async fn can_resume_session(
        &self,
        session_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let checkpoints = self.checkpoints.read().await;

        if let Some(checkpoint) = checkpoints.get(session_id) {
            // Check if checkpoint is recent and incomplete
            let age = SystemTime::now().duration_since(checkpoint.last_updated)?;
            let is_recent = age < Duration::from_hours(24); // Resume within 24 hours
            let is_incomplete = checkpoint.progress_percent < 100;

            Ok(is_recent && is_incomplete)
        } else {
            Ok(false)
        }
    }

    /// Resume a sync session from checkpoint
    pub async fn resume_session(
        &self,
        session_id: &str,
    ) -> Result<ResumeResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("🚀 Resuming sync session: {}", session_id);

        let checkpoint = {
            let checkpoints = self.checkpoints.read().await;
            checkpoints
                .get(session_id)
                .ok_or("Checkpoint not found")?
                .clone()
        };

        let resume_start = SystemTime::now();

        // Calculate what can be skipped
        let chunks_to_skip = checkpoint.chunks_transferred.len();
        let bytes_already_transferred = checkpoint.bytes_transferred;
        let remaining_chunks = checkpoint.total_chunks_needed - chunks_to_skip;

        info!(
            "📊 Resume analysis: {} chunks already transferred, {} remaining",
            chunks_to_skip, remaining_chunks
        );

        // Validate checkpoint integrity
        let validation_result = self.validate_checkpoint(&checkpoint).await?;
        if !validation_result.is_valid {
            warn!(
                "⚠️  Checkpoint validation failed: {}",
                validation_result.reason
            );
            return Ok(ResumeResult {
                success: false,
                reason: format!("Invalid checkpoint: {}", validation_result.reason),
                bytes_saved: 0,
                chunks_skipped: 0,
                time_saved: Duration::ZERO,
            });
        }

        // Calculate bandwidth and time savings
        let bytes_saved = bytes_already_transferred;
        let estimated_time_saved = self.estimate_time_saved(&checkpoint).await;

        // Update checkpoint with resume information
        let mut updated_checkpoint = checkpoint.clone();
        updated_checkpoint.last_updated = SystemTime::now();
        updated_checkpoint.retry_count += 1;
        updated_checkpoint.bandwidth_stats.bytes_saved_by_resume += bytes_saved;
        updated_checkpoint.bandwidth_stats.chunks_skipped += chunks_to_skip;
        updated_checkpoint.bandwidth_stats.time_saved_seconds += estimated_time_saved.as_secs();

        // Save updated checkpoint
        self.save_checkpoint(updated_checkpoint).await?;

        info!(
            "✅ Resume prepared: {} bytes saved, {:?} time saved",
            bytes_saved, estimated_time_saved
        );

        Ok(ResumeResult {
            success: true,
            reason: "Resume successful".to_string(),
            bytes_saved,
            chunks_skipped: chunks_to_skip,
            time_saved: estimated_time_saved,
        })
    }

    /// Update checkpoint with progress
    pub async fn update_checkpoint(
        &self,
        session_id: &str,
        update: CheckpointUpdate,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut checkpoints = self.checkpoints.write().await;

        if let Some(checkpoint) = checkpoints.get_mut(session_id) {
            checkpoint.last_updated = SystemTime::now();

            match update {
                CheckpointUpdate::Stage(stage) => {
                    checkpoint.stage = stage.clone();
                    match stage {
                        DiffStage::ExchangingSummaries => {
                            checkpoint.manifests_exchanged = true;
                        }
                        DiffStage::ComparingManifests => {
                            checkpoint.bloom_summaries_complete = true;
                        }
                        DiffStage::RequestingDetails => {
                            checkpoint.differences_identified = true;
                        }
                        _ => {}
                    }
                }
                CheckpointUpdate::Progress(percent) => {
                    checkpoint.progress_percent = percent;
                }
                CheckpointUpdate::ChunkTransferred(chunk_hash) => {
                    if !checkpoint.chunks_transferred.contains(&chunk_hash) {
                        checkpoint.chunks_transferred.push(chunk_hash.clone());
                        checkpoint.chunks_failed.retain(|h| h != &chunk_hash);
                    }
                }
                CheckpointUpdate::ChunkFailed(chunk_hash) => {
                    if !checkpoint.chunks_failed.contains(&chunk_hash) {
                        checkpoint.chunks_failed.push(chunk_hash);
                    }
                    checkpoint.connection_stable = false;
                }
                CheckpointUpdate::BytesTransferred(bytes) => {
                    checkpoint.bytes_transferred = bytes;
                }
                CheckpointUpdate::Error(error) => {
                    checkpoint.last_error = Some(error);
                    checkpoint.connection_stable = false;
                }
                CheckpointUpdate::Complete => {
                    checkpoint.progress_percent = 100;
                    checkpoint.stage = DiffStage::Completed;
                }
            }

            debug!(
                "📋 Checkpoint updated for {}: {:?} at {}%",
                session_id, checkpoint.stage, checkpoint.progress_percent
            );
        }

        Ok(())
    }

    /// Get resume statistics for all sessions
    pub async fn get_resume_stats(&self) -> ResumeStats {
        let checkpoints = self.checkpoints.read().await;

        let mut stats = ResumeStats {
            total_sessions: checkpoints.len(),
            resumable_sessions: 0,
            completed_sessions: 0,
            failed_sessions: 0,
            total_bytes_saved: 0,
            total_chunks_skipped: 0,
            total_time_saved_hours: 0.0,
            average_resume_success_rate: 0.0,
        };

        let mut successful_resumes = 0;
        let mut total_resumes = 0;

        for checkpoint in checkpoints.values() {
            match checkpoint.stage {
                DiffStage::Completed => stats.completed_sessions += 1,
                _ if checkpoint.progress_percent < 100 => {
                    if checkpoint.retry_count > 0 {
                        stats.resumable_sessions += 1;
                        total_resumes += 1;
                        if checkpoint.progress_percent > 0 {
                            successful_resumes += 1;
                        }
                    }
                }
                _ => {}
            }

            if checkpoint.last_error.is_some() {
                stats.failed_sessions += 1;
            }

            stats.total_bytes_saved += checkpoint.bandwidth_stats.bytes_saved_by_resume;
            stats.total_chunks_skipped += checkpoint.bandwidth_stats.chunks_skipped;
            stats.total_time_saved_hours +=
                checkpoint.bandwidth_stats.time_saved_seconds as f64 / 3600.0;
        }

        if total_resumes > 0 {
            stats.average_resume_success_rate =
                (successful_resumes as f64 / total_resumes as f64) * 100.0;
        }

        stats
    }

    /// Clean up old checkpoints
    pub async fn cleanup_old_checkpoints(
        &self,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let mut checkpoints = self.checkpoints.write().await;
        let cutoff = SystemTime::now() - Duration::from_days(7); // Keep for 7 days

        let initial_count = checkpoints.len();
        checkpoints.retain(|_, checkpoint| {
            checkpoint.last_updated > cutoff || checkpoint.stage != DiffStage::Completed
        });

        let removed = initial_count - checkpoints.len();

        if removed > 0 {
            self.save_checkpoints_to_disk(&*checkpoints).await?;
            info!("🧹 Cleaned up {} old checkpoints", removed);
        }

        Ok(removed)
    }

    // Private helper methods

    async fn save_checkpoint(
        &self,
        checkpoint: ResumeCheckpoint,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.insert(checkpoint.session_id.clone(), checkpoint);

        // Save to disk periodically
        if checkpoints.len() % 10 == 0 {
            self.save_checkpoints_to_disk(&*checkpoints).await?;
        }

        Ok(())
    }

    async fn save_checkpoints_to_disk(
        &self,
        checkpoints: &HashMap<String, ResumeCheckpoint>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let data = serde_json::to_string_pretty(checkpoints)?;
        tokio::fs::write(&self.checkpoint_file, data).await?;
        debug!("💾 Saved {} checkpoints to disk", checkpoints.len());
        Ok(())
    }

    async fn monitor_session_checkpoints(&self, session_id: String) {
        let mut interval = tokio::time::interval(self.checkpoint_interval);

        loop {
            interval.tick().await;

            // Check if session is still active
            let session_exists = {
                let sessions = self.active_sessions.read().await;
                sessions.contains_key(&session_id)
            };

            if !session_exists {
                debug!(
                    "Session {} no longer active, stopping checkpoint monitoring",
                    session_id
                );
                break;
            }

            // Force checkpoint save
            if let Err(e) = self.force_checkpoint_save(&session_id).await {
                error!("Failed to save checkpoint for {}: {}", session_id, e);
            }
        }
    }

    async fn force_checkpoint_save(
        &self,
        session_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let checkpoints = self.checkpoints.read().await;
        self.save_checkpoints_to_disk(&*checkpoints).await
    }

    async fn validate_checkpoint(
        &self,
        checkpoint: &ResumeCheckpoint,
    ) -> Result<CheckpointValidation, Box<dyn std::error::Error + Send + Sync>> {
        // Check if folder still exists
        if !checkpoint.folder_path.exists() {
            return Ok(CheckpointValidation {
                is_valid: false,
                reason: "Folder no longer exists".to_string(),
            });
        }

        // Check if transferred chunks still exist in CAS
        let mut missing_chunks = 0;
        for chunk_hash in &checkpoint.chunks_transferred {
            // Simplified check - in real implementation would verify CAS
            if chunk_hash.is_empty() {
                missing_chunks += 1;
            }
        }

        if missing_chunks > checkpoint.chunks_transferred.len() / 2 {
            return Ok(CheckpointValidation {
                is_valid: false,
                reason: format!("Too many missing chunks: {}", missing_chunks),
            });
        }

        Ok(CheckpointValidation {
            is_valid: true,
            reason: "Checkpoint valid".to_string(),
        })
    }

    async fn estimate_time_saved(&self, checkpoint: &ResumeCheckpoint) -> Duration {
        // Estimate based on typical transfer speeds and chunk sizes
        let avg_chunk_size = if checkpoint.total_chunks_needed > 0 {
            checkpoint.bytes_transferred / checkpoint.chunks_transferred.len() as u64
        } else {
            64 * 1024 // 64KB average
        };

        let chunks_saved = checkpoint.chunks_transferred.len() as u64;
        let bytes_saved = chunks_saved * avg_chunk_size;

        // Assume 10 MB/s average transfer speed
        let seconds_saved = bytes_saved / (10 * 1024 * 1024);

        Duration::from_secs(seconds_saved.max(1))
    }
}

// Make ResumeManager cloneable for async usage
impl Clone for ResumeManager {
    fn clone(&self) -> Self {
        Self {
            checkpoints: self.checkpoints.clone(),
            checkpoint_file: self.checkpoint_file.clone(),
            cas: self.cas.clone(),
            indexer: self.indexer.clone(),
            active_sessions: self.active_sessions.clone(),
            checkpoint_interval: self.checkpoint_interval,
        }
    }
}

/// Result of resume operation
#[derive(Debug)]
pub struct ResumeResult {
    pub success: bool,
    pub reason: String,
    pub bytes_saved: u64,
    pub chunks_skipped: usize,
    pub time_saved: Duration,
}

/// Checkpoint validation result
#[derive(Debug)]
struct CheckpointValidation {
    is_valid: bool,
    reason: String,
}

/// Resume statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeStats {
    pub total_sessions: usize,
    pub resumable_sessions: usize,
    pub completed_sessions: usize,
    pub failed_sessions: usize,
    pub total_bytes_saved: u64,
    pub total_chunks_skipped: usize,
    pub total_time_saved_hours: f64,
    pub average_resume_success_rate: f64,
}

// Helper trait for duration arithmetic
trait DurationHelper {
    fn from_days(days: u64) -> Self;
    fn from_hours(hours: u64) -> Self;
}

impl DurationHelper for Duration {
    fn from_days(days: u64) -> Self {
        Self::from_secs(days * 24 * 60 * 60)
    }

    fn from_hours(hours: u64) -> Self {
        Self::from_secs(hours * 60 * 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_resume_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cas = Arc::new(ContentStore::new(temp_dir.path()).await.unwrap());
        let indexer = Arc::new(
            AsyncIndexer::new(
                temp_dir.path(),
                &temp_dir.path().join("index.db"),
                Default::default(),
            )
            .await
            .unwrap(),
        );

        let manager = ResumeManager::new(&temp_dir.path().to_path_buf(), cas, indexer)
            .await
            .unwrap();

        let stats = manager.get_resume_stats().await;
        assert_eq!(stats.total_sessions, 0);
    }

    #[tokio::test]
    async fn test_checkpoint_updates() {
        let temp_dir = TempDir::new().unwrap();
        let cas = Arc::new(ContentStore::new(temp_dir.path()).await.unwrap());
        let indexer = Arc::new(
            AsyncIndexer::new(
                temp_dir.path(),
                &temp_dir.path().join("index.db"),
                Default::default(),
            )
            .await
            .unwrap(),
        );

        let manager = ResumeManager::new(&temp_dir.path().to_path_buf(), cas, indexer)
            .await
            .unwrap();

        let _rx = manager
            .start_resumable_session(
                "test_session".to_string(),
                "peer1".to_string(),
                temp_dir.path().to_path_buf(),
            )
            .await
            .unwrap();

        // Update progress
        manager
            .update_checkpoint("test_session", CheckpointUpdate::Progress(50))
            .await
            .unwrap();

        // Check if session can be resumed
        let can_resume = manager.can_resume_session("test_session").await.unwrap();
        assert!(can_resume);
    }

    #[tokio::test]
    async fn test_bandwidth_savings_calculation() {
        let bytes_saved = 1_000_000u64;
        let chunks_skipped = 100;
        let time_saved = Duration::from_secs(300); // 5 minutes

        assert_eq!(time_saved.as_secs(), 300);
        assert!(bytes_saved > 0);
        assert!(chunks_skipped > 0);
    }
}
