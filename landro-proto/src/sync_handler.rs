//! Sync protocol message handlers and state machine

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, trace, warn};

use crate::generated::{
    error::ErrorType, Ack, ChunkData, Error as ProtoError, FileEntry, FolderSummary, Hello,
    Manifest, Want,
};
use crate::{VersionNegotiator, PROTOCOL_VERSION};

/// Sync session state
#[derive(Debug, Clone, PartialEq)]
pub enum SyncState {
    /// Initial state, waiting for Hello message
    WaitingForHello,
    /// Hello exchanged, ready for sync operations
    Connected,
    /// Actively syncing folders
    Syncing,
    /// Sync completed successfully
    Completed,
    /// Session terminated due to error
    Failed(String),
}

/// Represents a peer in the sync protocol
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub device_id: Vec<u8>,
    pub device_name: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

/// Tracks sync progress for a folder
#[derive(Debug, Clone)]
pub struct FolderSyncProgress {
    pub folder_id: String,
    pub total_chunks: usize,
    pub received_chunks: usize,
    pub sent_chunks: usize,
    pub pending_chunks: HashSet<Vec<u8>>,
}

impl FolderSyncProgress {
    pub fn new(folder_id: String) -> Self {
        Self {
            folder_id,
            total_chunks: 0,
            received_chunks: 0,
            sent_chunks: 0,
            pending_chunks: HashSet::new(),
        }
    }

    pub fn progress_percentage(&self) -> f32 {
        if self.total_chunks == 0 {
            return 0.0;
        }
        (self.received_chunks as f32 / self.total_chunks as f32) * 100.0
    }

    pub fn is_complete(&self) -> bool {
        self.total_chunks > 0 && self.received_chunks == self.total_chunks
    }
}

/// Handles sync protocol messages and maintains session state
pub struct SyncProtocolHandler {
    state: Arc<RwLock<SyncState>>,
    peer_info: Arc<RwLock<Option<PeerInfo>>>,
    folder_progress: Arc<RwLock<HashMap<String, FolderSyncProgress>>>,
    our_device_id: Vec<u8>,
    our_device_name: String,
}

impl SyncProtocolHandler {
    /// Create a new sync protocol handler
    pub fn new(device_id: Vec<u8>, device_name: String) -> Self {
        Self {
            state: Arc::new(RwLock::new(SyncState::WaitingForHello)),
            peer_info: Arc::new(RwLock::new(None)),
            folder_progress: Arc::new(RwLock::new(HashMap::new())),
            our_device_id: device_id,
            our_device_name: device_name,
        }
    }

    /// Get current sync state
    pub async fn state(&self) -> SyncState {
        self.state.read().await.clone()
    }

    /// Create our Hello message
    pub fn create_hello(&self) -> Hello {
        Hello {
            version: PROTOCOL_VERSION.to_string(),
            device_id: self.our_device_id.clone(),
            device_name: self.our_device_name.clone(),
            capabilities: vec![
                "sync.v1".to_string(),
                "delta.v1".to_string(),
                "compression.zstd".to_string(),
            ],
            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        }
    }

    /// Handle incoming Hello message
    pub async fn handle_hello(&self, hello: Hello) -> Result<Hello, ProtoError> {
        let mut state = self.state.write().await;
        
        // Check if we're in the right state
        if *state != SyncState::WaitingForHello {
            return Err(ProtoError {
                error_type: ErrorType::InvalidRequest as i32,
                message: "Unexpected Hello message".to_string(),
                details: format!("Current state: {:?}", *state),
            });
        }

        // Check protocol version compatibility
        if !VersionNegotiator::is_compatible(&hello.version) {
            *state = SyncState::Failed("Version incompatible".to_string());
            return Err(ProtoError {
                error_type: ErrorType::ProtocolVersionMismatch as i32,
                message: "Protocol version mismatch".to_string(),
                details: VersionNegotiator::compatibility_error(&hello.version),
            });
        }

        // Store peer information
        let peer_info = PeerInfo {
            device_id: hello.device_id.clone(),
            device_name: hello.device_name.clone(),
            version: hello.version.clone(),
            capabilities: hello.capabilities.clone(),
        };

        info!(
            "Connected to peer: {} ({})",
            peer_info.device_name,
            hex::encode(&peer_info.device_id[..8]) // Show first 8 bytes of device ID
        );

        *self.peer_info.write().await = Some(peer_info);
        *state = SyncState::Connected;

        // Return our Hello message
        Ok(self.create_hello())
    }

    /// Handle folder summary message
    pub async fn handle_folder_summary(
        &self,
        summary: FolderSummary,
    ) -> Result<(), ProtoError> {
        let state = self.state.read().await;
        
        if *state != SyncState::Connected && *state != SyncState::Syncing {
            return Err(ProtoError {
                error_type: ErrorType::InvalidRequest as i32,
                message: "Not ready for folder sync".to_string(),
                details: format!("Current state: {:?}", *state),
            });
        }

        info!(
            "Received folder summary: {} ({} files, {} bytes)",
            summary.folder_id, summary.file_count, summary.total_size
        );

        // Initialize progress tracking for this folder
        let mut progress = self.folder_progress.write().await;
        progress.insert(summary.folder_id.clone(), FolderSyncProgress::new(summary.folder_id));

        // Update state to syncing if not already
        if *state == SyncState::Connected {
            drop(state);
            *self.state.write().await = SyncState::Syncing;
        }

        Ok(())
    }

    /// Handle manifest message
    pub async fn handle_manifest(&self, manifest: Manifest) -> Result<Want, ProtoError> {
        let state = self.state.read().await;
        
        if *state != SyncState::Syncing {
            return Err(ProtoError {
                error_type: ErrorType::InvalidRequest as i32,
                message: "Not in syncing state".to_string(),
                details: format!("Current state: {:?}", *state),
            });
        }

        debug!(
            "Received manifest for folder: {} ({} files)",
            manifest.folder_id,
            manifest.files.len()
        );

        // Collect all unique chunk hashes from the manifest
        let mut all_chunks = HashSet::new();
        for file in &manifest.files {
            for chunk_hash in &file.chunk_hashes {
                all_chunks.insert(chunk_hash.clone());
            }
        }

        // Update progress tracking
        let mut progress = self.folder_progress.write().await;
        if let Some(folder_progress) = progress.get_mut(&manifest.folder_id) {
            folder_progress.total_chunks = all_chunks.len();
            folder_progress.pending_chunks = all_chunks.clone();
        }

        // For now, request all chunks (in a real implementation, we'd check what we already have)
        let want = Want {
            folder_id: manifest.folder_id,
            chunk_hashes: all_chunks.into_iter().collect(),
            priority: 1,
        };

        Ok(want)
    }

    /// Handle Want message (peer requesting chunks from us)
    pub async fn handle_want(&self, want: Want) -> Result<Vec<ChunkData>, ProtoError> {
        let state = self.state.read().await;
        
        if *state != SyncState::Syncing {
            return Err(ProtoError {
                error_type: ErrorType::InvalidRequest as i32,
                message: "Not in syncing state".to_string(),
                details: format!("Current state: {:?}", *state),
            });
        }

        info!(
            "Peer requesting {} chunks for folder {}",
            want.chunk_hashes.len(),
            want.folder_id
        );

        // Update sent chunks counter
        let mut progress = self.folder_progress.write().await;
        if let Some(folder_progress) = progress.get_mut(&want.folder_id) {
            folder_progress.sent_chunks += want.chunk_hashes.len();
        }

        // In a real implementation, we'd fetch these chunks from storage
        // For now, return empty vec (would be populated with actual chunk data)
        Ok(Vec::new())
    }

    /// Handle chunk data received
    pub async fn handle_chunk_data(&self, chunk: ChunkData) -> Result<Ack, ProtoError> {
        let state = self.state.read().await;
        
        if *state != SyncState::Syncing {
            return Err(ProtoError {
                error_type: ErrorType::InvalidRequest as i32,
                message: "Not in syncing state".to_string(),
                details: format!("Current state: {:?}", *state),
            });
        }

        trace!("Received chunk: {} ({} bytes)", hex::encode(&chunk.hash), chunk.data.len());

        // Update progress
        let mut progress = self.folder_progress.write().await;
        for folder_progress in progress.values_mut() {
            if folder_progress.pending_chunks.remove(&chunk.hash) {
                folder_progress.received_chunks += 1;
                
                // Check if this folder is complete
                if folder_progress.is_complete() {
                    info!(
                        "Folder {} sync complete ({} chunks)",
                        folder_progress.folder_id, folder_progress.total_chunks
                    );
                }
                break;
            }
        }

        // Check if all folders are complete
        let all_complete = progress.values().all(|p| p.is_complete());
        if all_complete && !progress.is_empty() {
            drop(state);
            drop(progress);
            *self.state.write().await = SyncState::Completed;
            info!("All folders synced successfully");
        }

        Ok(Ack {
            request_id: hex::encode(&chunk.hash),
            success: true,
            message: "Chunk received".to_string(),
        })
    }

    /// Handle error message
    pub async fn handle_error(&self, error: ProtoError) {
        warn!(
            "Received error from peer: {} - {}",
            error.message, error.details
        );
        
        *self.state.write().await = SyncState::Failed(error.message.clone());
    }

    /// Get sync progress for all folders
    pub async fn get_progress(&self) -> HashMap<String, FolderSyncProgress> {
        self.folder_progress.read().await.clone()
    }

    /// Check if sync is complete
    pub async fn is_complete(&self) -> bool {
        *self.state.read().await == SyncState::Completed
    }

    /// Check if sync has failed
    pub async fn is_failed(&self) -> bool {
        matches!(*self.state.read().await, SyncState::Failed(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hello_exchange() {
        let handler = SyncProtocolHandler::new(vec![1, 2, 3], "test-device".to_string());
        
        // Create a peer hello
        let peer_hello = Hello {
            version: PROTOCOL_VERSION.to_string(),
            device_id: vec![4, 5, 6],
            device_name: "peer-device".to_string(),
            capabilities: vec!["sync.v1".to_string()],
            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        };

        // Handle peer hello
        let our_hello = handler.handle_hello(peer_hello).await.unwrap();
        
        assert_eq!(our_hello.device_name, "test-device");
        assert_eq!(handler.state().await, SyncState::Connected);
        
        // Check peer info was stored
        let peer_info = handler.peer_info.read().await;
        assert!(peer_info.is_some());
        assert_eq!(peer_info.as_ref().unwrap().device_name, "peer-device");
    }

    #[tokio::test]
    async fn test_version_mismatch() {
        let handler = SyncProtocolHandler::new(vec![1, 2, 3], "test-device".to_string());
        
        // Create a peer hello with incompatible version
        let peer_hello = Hello {
            version: "2.0.0".to_string(), // Different major version
            device_id: vec![4, 5, 6],
            device_name: "peer-device".to_string(),
            capabilities: vec!["sync.v1".to_string()],
            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        };

        // Handle peer hello should fail
        let result = handler.handle_hello(peer_hello).await;
        assert!(result.is_err());
        
        let error = result.unwrap_err();
        assert_eq!(error.error_type, ErrorType::ProtocolVersionMismatch as i32);
        assert!(matches!(handler.state().await, SyncState::Failed(_)));
    }

    #[tokio::test]
    async fn test_sync_progress_tracking() {
        let handler = SyncProtocolHandler::new(vec![1, 2, 3], "test-device".to_string());
        
        // First establish connection
        *handler.state.write().await = SyncState::Connected;
        
        // Handle folder summary
        let summary = FolderSummary {
            folder_id: "test-folder".to_string(),
            folder_path: "/test".to_string(),
            manifest_hash: vec![1, 2, 3],
            total_size: 1000,
            file_count: 10,
            last_modified: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        };
        
        handler.handle_folder_summary(summary).await.unwrap();
        assert_eq!(handler.state().await, SyncState::Syncing);
        
        // Check progress was initialized
        let progress = handler.get_progress().await;
        assert!(progress.contains_key("test-folder"));
    }

    #[tokio::test]
    async fn test_chunk_progress() {
        let mut progress = FolderSyncProgress::new("test".to_string());
        progress.total_chunks = 100;
        progress.received_chunks = 25;
        
        assert_eq!(progress.progress_percentage(), 25.0);
        assert!(!progress.is_complete());
        
        progress.received_chunks = 100;
        assert_eq!(progress.progress_percentage(), 100.0);
        assert!(progress.is_complete());
    }
}