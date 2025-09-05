//! Main sync orchestration logic

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::conflict::{ConflictResolver, ConflictResolution};
use crate::errors::{Result, SyncError};
use crate::progress::SyncProgress;
use crate::scheduler::{TransferScheduler, TransferPriority, TransferRequest};
use crate::state::{AsyncSyncDatabase, PeerSyncState, PeerState, SyncState};

/// Configuration for sync orchestrator
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// Maximum concurrent transfers per peer
    pub max_concurrent_transfers: usize,
    /// Default conflict resolution strategy
    pub default_conflict_resolution: ConflictResolution,
    /// Database path for sync state
    pub database_path: String,
    /// Enable auto-sync on changes
    pub auto_sync: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            max_concurrent_transfers: 4,
            default_conflict_resolution: ConflictResolution::TakeTheirs,
            database_path: ".landropic/sync.db".to_string(),
            auto_sync: true,
        }
    }
}

/// Main sync orchestrator
pub struct SyncOrchestrator {
    config: SyncConfig,
    database: AsyncSyncDatabase,
    state: Arc<RwLock<SyncState>>,
    peer_states: Arc<RwLock<HashMap<String, PeerSyncState>>>,
    scheduler: Arc<RwLock<TransferScheduler>>,
    conflict_resolver: ConflictResolver,
    progress: Arc<RwLock<HashMap<String, SyncProgress>>>,
}

impl SyncOrchestrator {
    /// Create new orchestrator
    pub async fn new(config: SyncConfig) -> Result<Self> {
        let database = AsyncSyncDatabase::open(&config.database_path).await?;
        let peer_states = database.get_all_peer_states().await?;
        
        let scheduler = TransferScheduler::new(config.max_concurrent_transfers);
        let conflict_resolver = ConflictResolver::new(config.default_conflict_resolution.clone());

        Ok(Self {
            config,
            database,
            state: Arc::new(RwLock::new(SyncState::Idle)),
            peer_states: Arc::new(RwLock::new(peer_states)),
            scheduler: Arc::new(RwLock::new(scheduler)),
            conflict_resolver,
            progress: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Start sync with a peer
    pub async fn start_sync(&self, peer_id: String, peer_name: String) -> Result<()> {
        info!("Starting sync with peer: {} ({})", peer_name, peer_id);

        // Check if already syncing with this peer
        let peer_states = self.peer_states.read().await;
        if let Some(peer_state) = peer_states.get(&peer_id) {
            if matches!(peer_state.current_state, PeerState::Negotiating | PeerState::Transferring { .. }) {
                return Err(SyncError::AlreadySyncing(peer_id));
            }
        }
        drop(peer_states);

        // Update peer state
        let mut peer_state = PeerSyncState {
            peer_id: peer_id.clone(),
            peer_name,
            last_sync: None,
            last_manifest_hash: None,
            bytes_sent: 0,
            bytes_received: 0,
            files_sent: 0,
            files_received: 0,
            current_state: PeerState::Negotiating,
        };

        // Save to database and memory
        self.database.upsert_peer_state(&peer_state).await?;
        
        {
            let mut peer_states = self.peer_states.write().await;
            peer_states.insert(peer_id.clone(), peer_state.clone());
        }

        // Update global sync state
        {
            let mut state = self.state.write().await;
            let peer_count = self.peer_states.read().await
                .values()
                .filter(|p| matches!(p.current_state, PeerState::Negotiating | PeerState::Transferring { .. }))
                .count();
            
            *state = SyncState::Syncing {
                peer_count,
                start_time: chrono::Utc::now(),
            };
        }

        // Initialize progress tracking
        {
            let mut progress = self.progress.write().await;
            progress.insert(peer_id.clone(), SyncProgress::new(peer_id.clone()));
        }

        debug!("Sync started with peer: {}", peer_id);
        Ok(())
    }

    /// Handle manifest exchange
    pub async fn handle_manifest_exchange(
        &self,
        peer_id: &str,
        _our_manifest: &landro_index::Manifest,
        _their_manifest: &landro_index::Manifest,
    ) -> Result<()> {
        info!("Exchanging manifests with peer: {}", peer_id);

        // TODO: Implement actual manifest comparison and transfer scheduling
        // 1. Compare manifests to find differences
        // 2. Schedule transfers for missing/updated files
        // 3. Handle conflicts

        // Update peer state to transferring
        {
            let mut peer_states = self.peer_states.write().await;
            if let Some(peer_state) = peer_states.get_mut(peer_id) {
                peer_state.current_state = PeerState::Transferring { progress: 0.0 };
            }
        }

        Ok(())
    }

    /// Schedule a transfer
    pub async fn schedule_transfer(
        &self,
        peer_id: String,
        file_path: String,
        chunk_hash: String,
        size: u64,
        priority: TransferPriority,
    ) -> Result<()> {
        let request = TransferRequest {
            peer_id: peer_id.clone(),
            chunk_hash: chunk_hash.clone(),
            file_path: file_path.clone(),
            size,
        };

        let mut scheduler = self.scheduler.write().await;
        scheduler.schedule(request, priority);

        // Also add to database for persistence
        self.database.add_pending_transfer(
            &peer_id,
            "default", // TODO: Get actual folder ID
            &file_path,
            &chunk_hash,
            priority as i32,
        ).await?;

        debug!("Scheduled transfer: {} ({})", file_path, chunk_hash);
        Ok(())
    }

    /// Get next transfer to process
    pub async fn next_transfer(&self) -> Option<TransferRequest> {
        let mut scheduler = self.scheduler.write().await;
        scheduler.next_transfer()
    }

    /// Complete a transfer
    pub async fn complete_transfer(&self, peer_id: &str, chunk_hash: &str, bytes: u64) -> Result<()> {
        // Update scheduler
        {
            let mut scheduler = self.scheduler.write().await;
            scheduler.complete_transfer();
        }

        // Update peer state statistics
        {
            let mut peer_states = self.peer_states.write().await;
            if let Some(peer_state) = peer_states.get_mut(peer_id) {
                peer_state.bytes_received += bytes;
                peer_state.files_received += 1;
            }
        }

        // Update progress
        {
            let mut progress = self.progress.write().await;
            if let Some(sync_progress) = progress.get_mut(peer_id) {
                sync_progress.transferred_bytes += bytes;
                sync_progress.completed_files += 1;
            }
        }

        debug!("Completed transfer: {} ({} bytes)", chunk_hash, bytes);
        Ok(())
    }

    /// Complete sync with a peer
    pub async fn complete_sync(&self, peer_id: &str) -> Result<()> {
        info!("Completing sync with peer: {}", peer_id);

        // Update peer state
        {
            let mut peer_states = self.peer_states.write().await;
            if let Some(peer_state) = peer_states.get_mut(peer_id) {
                peer_state.current_state = PeerState::Completed {
                    timestamp: chrono::Utc::now(),
                };
                peer_state.last_sync = Some(chrono::Utc::now());
                
                // Save to database
                self.database.upsert_peer_state(peer_state).await?;
            }
        }

        // Check if all syncs are complete
        {
            let peer_states = self.peer_states.read().await;
            let active_count = peer_states
                .values()
                .filter(|p| matches!(p.current_state, PeerState::Negotiating | PeerState::Transferring { .. }))
                .count();
            
            if active_count == 0 {
                let mut state = self.state.write().await;
                *state = SyncState::Idle;
            }
        }

        Ok(())
    }

    /// Get current sync state
    pub async fn get_state(&self) -> SyncState {
        self.state.read().await.clone()
    }

    /// Get sync progress for a peer
    pub async fn get_progress(&self, peer_id: &str) -> Option<SyncProgress> {
        self.progress.read().await.get(peer_id).cloned()
    }

    /// Get all peer states
    pub async fn get_peer_states(&self) -> HashMap<String, PeerSyncState> {
        self.peer_states.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let config = SyncConfig {
            database_path: ":memory:".to_string(),
            ..Default::default()
        };

        let orchestrator = SyncOrchestrator::new(config).await.unwrap();
        assert!(matches!(orchestrator.get_state().await, SyncState::Idle));
    }

    #[tokio::test]
    async fn test_start_sync() {
        let config = SyncConfig {
            database_path: ":memory:".to_string(),
            ..Default::default()
        };

        let orchestrator = SyncOrchestrator::new(config).await.unwrap();
        orchestrator.start_sync("peer-1".to_string(), "Test Peer".to_string()).await.unwrap();

        let state = orchestrator.get_state().await;
        assert!(matches!(state, SyncState::Syncing { peer_count: 1, .. }));

        let peer_states = orchestrator.get_peer_states().await;
        assert!(peer_states.contains_key("peer-1"));
    }
}