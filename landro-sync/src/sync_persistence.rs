//! Sync state persistence for resume capability
//!
//! Provides persistent storage of sync state to enable resume after interruptions

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::conflict_detection::{Conflict, ConflictType};
use crate::errors::{Result, SyncError};
use landro_index::manifest::Manifest;

/// Status of a sync session
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncSessionStatus {
    /// Session is actively syncing
    Active,
    /// Session completed successfully
    Completed,
    /// Session was interrupted and can be resumed
    Interrupted,
    /// Session failed and cannot be resumed
    Failed,
}

/// Information about a sync session for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSyncSession {
    pub session_id: String,
    pub peer_id: String,
    pub folder_path: String,
    pub status: SyncSessionStatus,
    pub started_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,

    /// Chunks that have been successfully transferred
    pub completed_chunks: std::collections::HashSet<String>,
    /// Chunks that still need to be transferred
    pub pending_chunks: std::collections::HashSet<String>,
    /// Chunks that failed transfer (for retry)
    pub failed_chunks: std::collections::HashSet<String>,

    /// Detected conflicts during sync
    pub conflicts: Vec<Conflict>,

    /// Progress statistics
    pub total_chunks: usize,
    pub total_bytes: u64,
    pub transferred_bytes: u64,

    /// Resume metadata
    pub resume_token: Option<String>,
    pub checkpoint_data: Option<serde_json::Value>,
}

impl PersistedSyncSession {
    /// Create new persisted session
    pub fn new(session_id: String, peer_id: String, folder_path: String) -> Self {
        let now = Utc::now();
        Self {
            session_id,
            peer_id,
            folder_path,
            status: SyncSessionStatus::Active,
            started_at: now,
            last_activity: now,
            completed_at: None,
            completed_chunks: std::collections::HashSet::new(),
            pending_chunks: std::collections::HashSet::new(),
            failed_chunks: std::collections::HashSet::new(),
            conflicts: Vec::new(),
            total_chunks: 0,
            total_bytes: 0,
            transferred_bytes: 0,
            resume_token: None,
            checkpoint_data: None,
        }
    }

    /// Calculate sync progress as percentage
    pub fn progress_percentage(&self) -> f64 {
        if self.total_chunks == 0 {
            return 0.0;
        }
        (self.completed_chunks.len() as f64 / self.total_chunks as f64) * 100.0
    }

    /// Check if session can be resumed
    pub fn can_resume(&self) -> bool {
        matches!(self.status, SyncSessionStatus::Interrupted) && !self.pending_chunks.is_empty()
    }

    /// Mark chunk as completed
    pub fn mark_chunk_completed(&mut self, chunk_hash: String) {
        self.pending_chunks.remove(&chunk_hash);
        self.failed_chunks.remove(&chunk_hash);
        self.completed_chunks.insert(chunk_hash);
        self.update_activity();
    }

    /// Mark chunk as failed
    pub fn mark_chunk_failed(&mut self, chunk_hash: String) {
        self.pending_chunks.remove(&chunk_hash);
        self.failed_chunks.insert(chunk_hash);
        self.update_activity();
    }

    /// Add conflict to session
    pub fn add_conflict(&mut self, conflict: Conflict) {
        self.conflicts.push(conflict);
        self.update_activity();
    }

    /// Update last activity timestamp
    pub fn update_activity(&mut self) {
        self.last_activity = Utc::now();
    }

    /// Mark session as completed
    pub fn complete(&mut self) {
        self.status = SyncSessionStatus::Completed;
        self.completed_at = Some(Utc::now());
        self.update_activity();
    }

    /// Mark session as interrupted (for resume)
    pub fn interrupt(&mut self) {
        self.status = SyncSessionStatus::Interrupted;
        self.update_activity();
    }

    /// Mark session as failed
    pub fn fail(&mut self) {
        self.status = SyncSessionStatus::Failed;
        self.update_activity();
    }
}

/// Configuration for sync persistence
#[derive(Debug, Clone)]
pub struct SyncPersistenceConfig {
    /// Directory to store sync state files
    pub state_dir: PathBuf,
    /// How often to save state during active sync
    pub save_interval_chunks: usize,
    /// How long to keep completed sessions
    pub completed_session_retention: chrono::Duration,
    /// Maximum number of sessions to keep in memory
    pub max_in_memory_sessions: usize,
}

impl Default for SyncPersistenceConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from(".landropic/sync_state"),
            save_interval_chunks: 50, // Save every 50 chunks
            completed_session_retention: chrono::Duration::days(7),
            max_in_memory_sessions: 100,
        }
    }
}

/// Manages persistent storage of sync state
pub struct SyncPersistenceManager {
    config: SyncPersistenceConfig,
    sessions: RwLock<HashMap<String, PersistedSyncSession>>,
    chunk_counter: RwLock<HashMap<String, usize>>, // For save interval tracking
}

impl SyncPersistenceManager {
    /// Create new persistence manager
    pub async fn new(config: SyncPersistenceConfig) -> Result<Self> {
        // Create state directory if it doesn't exist
        if !config.state_dir.exists() {
            tokio::fs::create_dir_all(&config.state_dir)
                .await
                .map_err(|e| {
                    SyncError::Storage(format!("Failed to create state directory: {}", e))
                })?;
        }

        let manager = Self {
            config,
            sessions: RwLock::new(HashMap::new()),
            chunk_counter: RwLock::new(HashMap::new()),
        };

        // Load existing sessions from disk
        manager.load_sessions().await?;

        Ok(manager)
    }

    /// Start tracking a new sync session
    pub async fn start_session(&self, session: PersistedSyncSession) -> Result<()> {
        let session_id = session.session_id.clone();

        info!("Starting sync session tracking: {}", session_id);

        // Add to in-memory tracking
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), session);
        }

        // Initialize chunk counter
        {
            let mut counter = self.chunk_counter.write().await;
            counter.insert(session_id.clone(), 0);
        }

        // Save to disk immediately
        self.save_session(&session_id).await?;

        Ok(())
    }

    /// Update session with chunk completion
    pub async fn mark_chunk_completed(
        &self,
        session_id: &str,
        chunk_hash: String,
        chunk_size: u64,
    ) -> Result<()> {
        let should_save = {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                session.mark_chunk_completed(chunk_hash);
                session.transferred_bytes += chunk_size;

                // Check if we should save to disk
                let mut counter = self.chunk_counter.write().await;
                let count = counter.entry(session_id.to_string()).or_insert(0);
                *count += 1;
                *count >= self.config.save_interval_chunks
            } else {
                false
            }
        };

        if should_save {
            // Reset counter and save
            {
                let mut counter = self.chunk_counter.write().await;
                counter.insert(session_id.to_string(), 0);
            }
            self.save_session(session_id).await?;
        }

        Ok(())
    }

    /// Mark chunk as failed
    pub async fn mark_chunk_failed(&self, session_id: &str, chunk_hash: String) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.mark_chunk_failed(chunk_hash);
            // Save immediately on failures for better recovery
            drop(sessions);
            self.save_session(session_id).await?;
        }
        Ok(())
    }

    /// Add conflict to session
    pub async fn add_conflict(&self, session_id: &str, conflict: Conflict) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.add_conflict(conflict);
        }
        Ok(())
    }

    /// Complete a sync session
    pub async fn complete_session(&self, session_id: &str) -> Result<()> {
        info!("Completing sync session: {}", session_id);

        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.complete();
            drop(sessions);
            self.save_session(session_id).await?;
        }

        // Clean up chunk counter
        {
            let mut counter = self.chunk_counter.write().await;
            counter.remove(session_id);
        }

        Ok(())
    }

    /// Mark session as interrupted (for resume)
    pub async fn interrupt_session(&self, session_id: &str) -> Result<()> {
        warn!("Interrupting sync session: {}", session_id);

        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.interrupt();
            drop(sessions);
            self.save_session(session_id).await?;
        }

        Ok(())
    }

    /// Get resumable sessions
    pub async fn get_resumable_sessions(&self) -> Vec<PersistedSyncSession> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .filter(|s| s.can_resume())
            .cloned()
            .collect()
    }

    /// Get session by ID
    pub async fn get_session(&self, session_id: &str) -> Option<PersistedSyncSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    /// Save session to disk
    async fn save_session(&self, session_id: &str) -> Result<()> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };

        if let Some(session) = session {
            let file_path = self.config.state_dir.join(format!("{}.json", session_id));
            let json_data = serde_json::to_string_pretty(&session)
                .map_err(|e| SyncError::Storage(format!("Failed to serialize session: {}", e)))?;

            tokio::fs::write(&file_path, json_data)
                .await
                .map_err(|e| SyncError::Storage(format!("Failed to save session: {}", e)))?;

            debug!("Saved sync session to: {:?}", file_path);
        }

        Ok(())
    }

    /// Load all sessions from disk
    async fn load_sessions(&self) -> Result<()> {
        if !self.config.state_dir.exists() {
            return Ok(());
        }

        let mut dir = tokio::fs::read_dir(&self.config.state_dir)
            .await
            .map_err(|e| SyncError::Storage(format!("Failed to read state directory: {}", e)))?;

        let mut loaded_count = 0;
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| SyncError::Storage(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(session) = self.load_session(&path).await {
                    let mut sessions = self.sessions.write().await;
                    sessions.insert(session.session_id.clone(), session);
                    loaded_count += 1;
                }
            }
        }

        info!("Loaded {} sync sessions from disk", loaded_count);
        Ok(())
    }

    /// Load single session from file
    async fn load_session(&self, file_path: &Path) -> Result<PersistedSyncSession> {
        let json_data = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| SyncError::Storage(format!("Failed to read session file: {}", e)))?;

        let session: PersistedSyncSession = serde_json::from_str(&json_data)
            .map_err(|e| SyncError::Storage(format!("Failed to parse session: {}", e)))?;

        Ok(session)
    }

    /// Clean up old completed sessions
    pub async fn cleanup_old_sessions(&self) -> Result<()> {
        let cutoff = Utc::now() - self.config.completed_session_retention;
        let mut sessions_to_remove = Vec::new();

        {
            let sessions = self.sessions.read().await;
            for (id, session) in sessions.iter() {
                if session.status == SyncSessionStatus::Completed {
                    if let Some(completed_at) = session.completed_at {
                        if completed_at < cutoff {
                            sessions_to_remove.push(id.clone());
                        }
                    }
                }
            }
        }

        for session_id in sessions_to_remove {
            self.remove_session(&session_id).await?;
        }

        Ok(())
    }

    /// Remove a session from memory and disk
    async fn remove_session(&self, session_id: &str) -> Result<()> {
        // Remove from memory
        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(session_id);
        }

        // Remove file
        let file_path = self.config.state_dir.join(format!("{}.json", session_id));
        if file_path.exists() {
            tokio::fs::remove_file(&file_path)
                .await
                .map_err(|e| SyncError::Storage(format!("Failed to remove session file: {}", e)))?;
        }

        debug!("Removed session: {}", session_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_session_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let config = SyncPersistenceConfig {
            state_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let manager = SyncPersistenceManager::new(config).await.unwrap();

        // Create and start session
        let session = PersistedSyncSession::new(
            "test-session".to_string(),
            "peer-123".to_string(),
            "/test/path".to_string(),
        );

        manager.start_session(session).await.unwrap();

        // Verify it was saved
        let loaded_session = manager.get_session("test-session").await;
        assert!(loaded_session.is_some());

        let loaded = loaded_session.unwrap();
        assert_eq!(loaded.session_id, "test-session");
        assert_eq!(loaded.peer_id, "peer-123");
        assert_eq!(loaded.status, SyncSessionStatus::Active);
    }

    #[tokio::test]
    async fn test_chunk_tracking() {
        let temp_dir = TempDir::new().unwrap();
        let config = SyncPersistenceConfig {
            state_dir: temp_dir.path().to_path_buf(),
            save_interval_chunks: 2, // Save every 2 chunks for testing
            ..Default::default()
        };

        let manager = SyncPersistenceManager::new(config).await.unwrap();

        // Start session
        let mut session = PersistedSyncSession::new(
            "chunk-test".to_string(),
            "peer-456".to_string(),
            "/test".to_string(),
        );
        session.total_chunks = 3;
        session.pending_chunks.insert("chunk1".to_string());
        session.pending_chunks.insert("chunk2".to_string());
        session.pending_chunks.insert("chunk3".to_string());

        manager.start_session(session).await.unwrap();

        // Mark chunks as completed
        manager
            .mark_chunk_completed("chunk-test", "chunk1".to_string(), 1024)
            .await
            .unwrap();
        manager
            .mark_chunk_completed("chunk-test", "chunk2".to_string(), 2048)
            .await
            .unwrap();

        // Check progress
        let session = manager.get_session("chunk-test").await.unwrap();
        assert_eq!(session.completed_chunks.len(), 2);
        assert_eq!(session.pending_chunks.len(), 1);
        assert_eq!(session.transferred_bytes, 3072);
        assert!((session.progress_percentage() - 66.67).abs() < 0.1);
    }

    #[tokio::test]
    async fn test_resume_capability() {
        let temp_dir = TempDir::new().unwrap();
        let config = SyncPersistenceConfig {
            state_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let manager = SyncPersistenceManager::new(config).await.unwrap();

        // Create interrupted session
        let mut session = PersistedSyncSession::new(
            "resume-test".to_string(),
            "peer-789".to_string(),
            "/resume".to_string(),
        );
        session.pending_chunks.insert("pending1".to_string());
        session.pending_chunks.insert("pending2".to_string());

        manager.start_session(session).await.unwrap();

        // Interrupt the session
        manager.interrupt_session("resume-test").await.unwrap();

        // Check it can be resumed
        let resumable = manager.get_resumable_sessions().await;
        assert_eq!(resumable.len(), 1);
        assert_eq!(resumable[0].session_id, "resume-test");
        assert!(resumable[0].can_resume());
    }
}
