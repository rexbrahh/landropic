//! CLI Progress API - Real-time sync progress for Day 2
//!
//! This module provides comprehensive real-time progress reporting specifically
//! designed for CLI consumption, integrating Bloom diff metrics, transfer stats,
//! and resume capabilities.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, info};

use crate::bloom_diff::{DiffProgress, DiffStage};
use crate::bloom_sync_integration::{BandwidthSavings, BloomSyncStats, EnhancedSyncStats};
use crate::resume_manager::{ResumeResult, ResumeStats};

/// Real-time progress update for CLI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliProgressUpdate {
    pub session_id: String,
    pub peer_id: String,
    pub folder_path: String,
    pub timestamp: SystemTime,

    // Overall progress
    pub stage: ProgressStage,
    pub progress_percent: u8,
    pub eta_seconds: Option<u64>,

    // Transfer stats
    pub transfer_stats: CliTransferStats,

    // Bloom filter efficiency
    pub bloom_stats: CliBloomStats,

    // Resume information
    pub resume_info: Option<CliResumeInfo>,

    // Network status
    pub network_status: NetworkStatus,

    // Performance metrics
    pub performance: PerformanceMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProgressStage {
    Initializing,
    ConnectingToPeer,
    ExchangingManifests,
    ComputingDiff,
    TransferringFiles,
    Verifying,
    Completed,
    Failed(String),
    Resuming,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliTransferStats {
    pub files_total: u64,
    pub files_completed: u64,
    pub files_failed: u64,
    pub bytes_total: u64,
    pub bytes_transferred: u64,
    pub chunks_total: u64,
    pub chunks_completed: u64,
    pub current_speed_mbps: f64,
    pub average_speed_mbps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliBloomStats {
    pub bandwidth_saved_percent: f64,
    pub bytes_saved: u64,
    pub false_positive_rate: f64,
    pub filter_efficiency_percent: f64,
    pub compression_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliResumeInfo {
    pub is_resumed_session: bool,
    pub resume_count: u32,
    pub chunks_skipped: u64,
    pub bytes_saved_by_resume: u64,
    pub time_saved_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkStatus {
    Stable,
    Unstable,
    Reconnecting,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub cpu_usage_percent: f64,
    pub memory_usage_mb: f64,
    pub disk_io_mbps: f64,
    pub network_utilization_percent: f64,
}

/// CLI Progress API manager
pub struct CliProgressApi {
    active_sessions: Arc<RwLock<HashMap<String, SessionProgress>>>,
    progress_tx: broadcast::Sender<CliProgressUpdate>,
    update_interval: Duration,
}

#[derive(Debug)]
struct SessionProgress {
    session_id: String,
    peer_id: String,
    folder_path: String,
    started_at: SystemTime,
    last_update: SystemTime,
    current_progress: CliProgressUpdate,
    progress_history: Vec<CliProgressUpdate>,
    update_tx: mpsc::Sender<ProgressCommand>,
}

#[derive(Debug)]
pub enum ProgressCommand {
    UpdateStage(ProgressStage),
    UpdateProgress(u8),
    UpdateTransferStats(CliTransferStats),
    UpdateBloomStats(CliBloomStats),
    UpdateResumeInfo(CliResumeInfo),
    UpdateNetworkStatus(NetworkStatus),
    Complete,
    Failed(String),
}

impl CliProgressApi {
    /// Create new CLI progress API
    pub fn new(buffer_size: usize) -> Self {
        let (progress_tx, _) = broadcast::channel(buffer_size);

        Self {
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
            progress_tx,
            update_interval: Duration::from_millis(500), // Update every 500ms
        }
    }

    /// Start tracking progress for a session
    pub async fn start_session_tracking(
        &self,
        session_id: String,
        peer_id: String,
        folder_path: String,
    ) -> mpsc::Receiver<ProgressCommand> {
        info!("📊 Starting CLI progress tracking for {}", session_id);

        let (update_tx, update_rx) = mpsc::channel(100);

        let initial_progress = CliProgressUpdate {
            session_id: session_id.clone(),
            peer_id: peer_id.clone(),
            folder_path: folder_path.clone(),
            timestamp: SystemTime::now(),
            stage: ProgressStage::Initializing,
            progress_percent: 0,
            eta_seconds: None,
            transfer_stats: CliTransferStats::default(),
            bloom_stats: CliBloomStats::default(),
            resume_info: None,
            network_status: NetworkStatus::Stable,
            performance: PerformanceMetrics::default(),
        };

        let session = SessionProgress {
            session_id: session_id.clone(),
            peer_id,
            folder_path,
            started_at: SystemTime::now(),
            last_update: SystemTime::now(),
            current_progress: initial_progress.clone(),
            progress_history: vec![initial_progress.clone()],
            update_tx,
        };

        self.active_sessions
            .write()
            .await
            .insert(session_id.clone(), session);

        // Broadcast initial progress
        let _ = self.progress_tx.send(initial_progress);

        // Start progress monitoring
        let self_clone = Arc::new(self.clone());
        tokio::spawn(async move {
            self_clone.monitor_session_progress(session_id).await;
        });

        update_rx
    }

    /// Subscribe to progress updates
    pub fn subscribe_to_progress(&self) -> broadcast::Receiver<CliProgressUpdate> {
        self.progress_tx.subscribe()
    }

    /// Get current progress for a session
    pub async fn get_session_progress(&self, session_id: &str) -> Option<CliProgressUpdate> {
        let sessions = self.active_sessions.read().await;
        sessions.get(session_id).map(|s| s.current_progress.clone())
    }

    /// Get progress for all active sessions
    pub async fn get_all_progress(&self) -> Vec<CliProgressUpdate> {
        let sessions = self.active_sessions.read().await;
        sessions
            .values()
            .map(|s| s.current_progress.clone())
            .collect()
    }

    /// Update session with enhanced sync stats
    pub async fn update_from_enhanced_stats(
        &self,
        session_id: &str,
        enhanced_stats: &EnhancedSyncStats,
        stage: ProgressStage,
        progress_percent: u8,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut sessions = self.active_sessions.write().await;

        if let Some(session) = sessions.get_mut(session_id) {
            // Update transfer stats
            session.current_progress.transfer_stats = CliTransferStats {
                files_total: (enhanced_stats.traditional_stats.files_added
                    + enhanced_stats.traditional_stats.files_modified)
                    as u64,
                files_completed: (enhanced_stats.traditional_stats.files_added
                    + enhanced_stats.traditional_stats.files_modified)
                    as u64,
                files_failed: 0, // TODO: Get from error stats
                bytes_total: enhanced_stats.traditional_stats.bytes_sent
                    + enhanced_stats.traditional_stats.bytes_received,
                bytes_transferred: enhanced_stats.traditional_stats.bytes_sent
                    + enhanced_stats.traditional_stats.bytes_received,
                chunks_total: 0,     // TODO: Calculate from files
                chunks_completed: 0, // TODO: Calculate
                current_speed_mbps: self.calculate_current_speed_mbps(session).await,
                average_speed_mbps: self.calculate_average_speed_mbps(session).await,
            };

            // Update Bloom stats
            session.current_progress.bloom_stats = CliBloomStats {
                bandwidth_saved_percent: enhanced_stats.bandwidth_savings.total_savings_percent,
                bytes_saved: enhanced_stats.bandwidth_savings.bytes_saved_by_bloom,
                false_positive_rate: enhanced_stats.bloom_stats.false_positives as f64,
                filter_efficiency_percent: enhanced_stats.bloom_stats.filter_efficiency,
                compression_ratio: enhanced_stats.bloom_stats.compression_ratio,
            };

            // Update general progress
            session.current_progress.stage = stage.clone();
            session.current_progress.progress_percent = progress_percent;
            session.current_progress.timestamp = SystemTime::now();
            session.current_progress.eta_seconds = self.calculate_eta(session);
            session.last_update = SystemTime::now();

            // Add to history
            session
                .progress_history
                .push(session.current_progress.clone());

            // Keep only last 100 updates in history
            if session.progress_history.len() > 100 {
                session.progress_history.remove(0);
            }

            // Broadcast update
            let _ = self.progress_tx.send(session.current_progress.clone());

            debug!(
                "📈 Progress updated for {}: {}% at {:?}",
                session_id, progress_percent, stage
            );
        }

        Ok(())
    }

    /// Update session with resume information
    pub async fn update_with_resume_info(
        &self,
        session_id: &str,
        resume_result: &ResumeResult,
        resume_stats: &ResumeStats,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut sessions = self.active_sessions.write().await;

        if let Some(session) = sessions.get_mut(session_id) {
            session.current_progress.resume_info = Some(CliResumeInfo {
                is_resumed_session: resume_result.success,
                resume_count: 1, // TODO: Get actual count
                chunks_skipped: resume_result.chunks_skipped as u64,
                bytes_saved_by_resume: resume_result.bytes_saved,
                time_saved_seconds: resume_result.time_saved.as_secs(),
            });

            if resume_result.success {
                session.current_progress.stage = ProgressStage::Resuming;
                info!(
                    "🔄 Resume info updated: {} bytes saved, {} chunks skipped",
                    resume_result.bytes_saved, resume_result.chunks_skipped
                );
            }

            // Broadcast update
            let _ = self.progress_tx.send(session.current_progress.clone());
        }

        Ok(())
    }

    /// Update network status
    pub async fn update_network_status(
        &self,
        session_id: &str,
        status: NetworkStatus,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut sessions = self.active_sessions.write().await;

        if let Some(session) = sessions.get_mut(session_id) {
            session.current_progress.network_status = status;
            session.current_progress.timestamp = SystemTime::now();

            // Broadcast update
            let _ = self.progress_tx.send(session.current_progress.clone());
        }

        Ok(())
    }

    /// Complete session tracking
    pub async fn complete_session(&self, session_id: &str) {
        let mut sessions = self.active_sessions.write().await;

        if let Some(session) = sessions.get_mut(session_id) {
            session.current_progress.stage = ProgressStage::Completed;
            session.current_progress.progress_percent = 100;
            session.current_progress.timestamp = SystemTime::now();
            session.current_progress.eta_seconds = Some(0);

            // Final broadcast
            let _ = self.progress_tx.send(session.current_progress.clone());

            info!("✅ Session {} completed", session_id);
        }
    }

    /// Mark session as failed
    pub async fn fail_session(&self, session_id: &str, reason: String) {
        let mut sessions = self.active_sessions.write().await;

        if let Some(session) = sessions.get_mut(session_id) {
            session.current_progress.stage = ProgressStage::Failed(reason.clone());
            session.current_progress.timestamp = SystemTime::now();

            // Final broadcast
            let _ = self.progress_tx.send(session.current_progress.clone());

            info!("❌ Session {} failed: {}", session_id, reason);
        }
    }

    /// Get CLI-formatted progress summary
    pub async fn get_cli_summary(&self, session_id: &str) -> Option<String> {
        let sessions = self.active_sessions.read().await;

        if let Some(session) = sessions.get(session_id) {
            let progress = &session.current_progress;

            let stage_emoji = match progress.stage {
                ProgressStage::Initializing => "🔄",
                ProgressStage::ConnectingToPeer => "🔌",
                ProgressStage::ExchangingManifests => "📋",
                ProgressStage::ComputingDiff => "🧮",
                ProgressStage::TransferringFiles => "📁",
                ProgressStage::Verifying => "✅",
                ProgressStage::Completed => "🎉",
                ProgressStage::Failed(_) => "❌",
                ProgressStage::Resuming => "🔄",
            };

            let eta_text = progress
                .eta_seconds
                .map(|s| format!("ETA: {}s", s))
                .unwrap_or_else(|| "ETA: --".to_string());

            let speed_text = if progress.transfer_stats.current_speed_mbps > 0.0 {
                format!("{:.1} MB/s", progress.transfer_stats.current_speed_mbps)
            } else {
                "--".to_string()
            };

            let bloom_savings = format!("{:.1}%", progress.bloom_stats.bandwidth_saved_percent);

            Some(format!(
                "{} {} {}% | {} | {} | Bloom: {} saved | {}",
                stage_emoji,
                progress.peer_id,
                progress.progress_percent,
                speed_text,
                eta_text,
                bloom_savings,
                format!("{:?}", progress.stage)
            ))
        } else {
            None
        }
    }

    // Private helper methods

    async fn monitor_session_progress(&self, session_id: String) {
        let mut interval = tokio::time::interval(self.update_interval);

        loop {
            interval.tick().await;

            let session_exists = {
                let sessions = self.active_sessions.read().await;
                sessions.contains_key(&session_id)
            };

            if !session_exists {
                break;
            }

            // Update performance metrics
            self.update_performance_metrics(&session_id).await;

            // Check if session is complete or failed
            let should_stop = {
                let sessions = self.active_sessions.read().await;
                if let Some(session) = sessions.get(&session_id) {
                    matches!(
                        session.current_progress.stage,
                        ProgressStage::Completed | ProgressStage::Failed(_)
                    )
                } else {
                    true
                }
            };

            if should_stop {
                break;
            }
        }

        debug!("Stopped monitoring progress for {}", session_id);
    }

    async fn update_performance_metrics(&self, session_id: &str) {
        // TODO: Implement actual performance monitoring
        // For now, use placeholder values

        let mut sessions = self.active_sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.current_progress.performance = PerformanceMetrics {
                cpu_usage_percent: 15.0,           // Placeholder
                memory_usage_mb: 128.0,            // Placeholder
                disk_io_mbps: 50.0,                // Placeholder
                network_utilization_percent: 75.0, // Placeholder
            };
        }
    }

    async fn calculate_current_speed_mbps(&self, session: &SessionProgress) -> f64 {
        // Calculate based on recent transfer progress
        if session.progress_history.len() >= 2 {
            let recent = &session.progress_history[session.progress_history.len() - 1];
            let previous = &session.progress_history[session.progress_history.len() - 2];

            let bytes_delta = recent
                .transfer_stats
                .bytes_transferred
                .saturating_sub(previous.transfer_stats.bytes_transferred);
            let time_delta = recent
                .timestamp
                .duration_since(previous.timestamp)
                .unwrap_or(Duration::from_secs(1))
                .as_secs_f64();

            if time_delta > 0.0 {
                (bytes_delta as f64 / time_delta) / (1024.0 * 1024.0) // Convert to MB/s
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    async fn calculate_average_speed_mbps(&self, session: &SessionProgress) -> f64 {
        let elapsed = session
            .started_at
            .elapsed()
            .unwrap_or(Duration::from_secs(1));
        let bytes = session.current_progress.transfer_stats.bytes_transferred;

        (bytes as f64 / elapsed.as_secs_f64()) / (1024.0 * 1024.0)
    }

    fn calculate_eta(&self, session: &SessionProgress) -> Option<u64> {
        let progress = session.current_progress.progress_percent;
        if progress == 0 {
            return None;
        }

        let elapsed = session.started_at.elapsed().unwrap_or(Duration::ZERO);
        let remaining_percent = 100 - progress;

        if remaining_percent == 0 {
            Some(0)
        } else {
            let estimated_total_time = (elapsed.as_secs() * 100) / progress as u64;
            Some(estimated_total_time.saturating_sub(elapsed.as_secs()))
        }
    }
}

// Make CliProgressApi cloneable
impl Clone for CliProgressApi {
    fn clone(&self) -> Self {
        Self {
            active_sessions: self.active_sessions.clone(),
            progress_tx: self.progress_tx.clone(),
            update_interval: self.update_interval,
        }
    }
}

// Default implementations
impl Default for CliTransferStats {
    fn default() -> Self {
        Self {
            files_total: 0,
            files_completed: 0,
            files_failed: 0,
            bytes_total: 0,
            bytes_transferred: 0,
            chunks_total: 0,
            chunks_completed: 0,
            current_speed_mbps: 0.0,
            average_speed_mbps: 0.0,
        }
    }
}

impl Default for CliBloomStats {
    fn default() -> Self {
        Self {
            bandwidth_saved_percent: 0.0,
            bytes_saved: 0,
            false_positive_rate: 0.0,
            filter_efficiency_percent: 0.0,
            compression_ratio: 0.0,
        }
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self {
            cpu_usage_percent: 0.0,
            memory_usage_mb: 0.0,
            disk_io_mbps: 0.0,
            network_utilization_percent: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cli_progress_api_creation() {
        let api = CliProgressApi::new(100);
        let progress = api.get_all_progress().await;
        assert!(progress.is_empty());
    }

    #[tokio::test]
    async fn test_progress_subscription() {
        let api = CliProgressApi::new(10);
        let mut rx = api.subscribe_to_progress();

        let _cmd_rx = api
            .start_session_tracking(
                "test_session".to_string(),
                "peer1".to_string(),
                "/test/path".to_string(),
            )
            .await;

        // Should receive initial progress update
        let update = rx.recv().await.unwrap();
        assert_eq!(update.session_id, "test_session");
        assert_eq!(update.progress_percent, 0);
    }

    #[tokio::test]
    async fn test_cli_summary_formatting() {
        let api = CliProgressApi::new(10);

        let _cmd_rx = api
            .start_session_tracking(
                "test_session".to_string(),
                "peer1".to_string(),
                "/test/path".to_string(),
            )
            .await;

        let summary = api.get_cli_summary("test_session").await;
        assert!(summary.is_some());
        assert!(summary.unwrap().contains("peer1"));
    }
}
