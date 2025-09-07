//! Sync protocol state machine implementation
//!
//! Implements the synchronization protocol flow:
//! 1. Hello → Exchange device info
//! 2. FolderSummary → Share folder metadata  
//! 3. Want/Have → Negotiate required chunks
//! 4. ChunkData → Transfer content
//! 5. Ack → Confirm receipt

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

use landro_cas::ContentStore;
use landro_chunker::ContentHash;
use landro_index::manifest::{Manifest, ManifestEntry};
use landro_proto::{
    Ack, ChunkData, Error as ProtoError,
    FolderSummary, Hello, Manifest as ProtoManifest, Want,
};

use crate::diff::{DiffResult, IncrementalDiff};
use crate::errors::{Result, SyncError};
use crate::state::{AsyncSyncDatabase, PeerState, PeerSyncState};

/// Sync protocol version
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Maximum chunks to request in a single Want message
const MAX_WANT_CHUNKS: usize = 100;

/// Maximum concurrent chunk transfers
const MAX_CONCURRENT_TRANSFERS: usize = 10;

/// Sync session state machine
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    /// Initial state, waiting for handshake
    Initializing,
    /// Hello exchanged, negotiating folders
    Negotiating,
    /// Computing differences between manifests  
    ComputingDiff,
    /// Transferring chunks
    Transferring {
        total_chunks: usize,
        transferred_chunks: usize,
    },
    /// Verifying transferred data
    Verifying,
    /// Sync completed successfully
    Completed,
    /// Session failed with error
    Failed(String),
}

/// Represents a sync session with a peer
pub struct SyncSession {
    /// Session ID for logging
    session_id: String,
    /// Peer information
    peer_id: String,
    peer_name: String,
    /// Current session state
    state: Arc<RwLock<SessionState>>,
    /// Manifest for our folder
    our_manifest: Arc<RwLock<Option<Manifest>>>,
    /// Manifest from peer
    peer_manifest: Arc<RwLock<Option<ProtoManifest>>>,
    /// Diff result
    diff_result: Arc<RwLock<Option<DiffResult>>>,
    /// Chunks we need from peer
    wanted_chunks: Arc<RwLock<HashSet<String>>>,
    /// Chunks we have sent to peer
    sent_chunks: Arc<RwLock<HashSet<String>>>,
    /// Chunks received from peer
    received_chunks: Arc<RwLock<HashSet<String>>>,
    /// Content-addressed storage
    cas: Arc<ContentStore>,
    /// Incremental diff tracker
    diff_tracker: Arc<RwLock<IncrementalDiff>>,
    /// Database for persistence
    database: AsyncSyncDatabase,
    /// Session start time
    start_time: DateTime<Utc>,
    /// Transfer statistics
    stats: Arc<RwLock<TransferStats>>,
}

/// Transfer statistics for a session
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransferStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub chunks_sent: usize,
    pub chunks_received: usize,
    pub files_added: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
}

impl SyncSession {
    /// Create a new sync session
    pub fn new(
        peer_id: String,
        peer_name: String,
        cas: Arc<ContentStore>,
        database: AsyncSyncDatabase,
    ) -> Self {
        let session_id = format!("{}_{}", peer_id, Utc::now().timestamp());

        // Create chunk cache from CAS
        let chunk_cache = Arc::new(HashSet::new()); // TODO: Load from CAS

        Self {
            session_id: session_id.clone(),
            peer_id,
            peer_name,
            state: Arc::new(RwLock::new(SessionState::Initializing)),
            our_manifest: Arc::new(RwLock::new(None)),
            peer_manifest: Arc::new(RwLock::new(None)),
            diff_result: Arc::new(RwLock::new(None)),
            wanted_chunks: Arc::new(RwLock::new(HashSet::new())),
            sent_chunks: Arc::new(RwLock::new(HashSet::new())),
            received_chunks: Arc::new(RwLock::new(HashSet::new())),
            cas,
            diff_tracker: Arc::new(RwLock::new(IncrementalDiff::new(chunk_cache))),
            database,
            start_time: Utc::now(),
            stats: Arc::new(RwLock::new(TransferStats::default())),
        }
    }

    /// Get current session state
    pub async fn state(&self) -> SessionState {
        self.state.read().await.clone()
    }
    
    /// Get peer ID
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Process Hello message
    pub async fn handle_hello(&self, hello: Hello) -> Result<Hello> {
        let mut state = self.state.write().await;

        if *state != SessionState::Initializing {
            return Err(SyncError::Protocol(format!(
                "Unexpected Hello in state {:?}",
                *state
            )));
        }

        // Verify protocol version
        if !Self::is_version_compatible(&hello.version) {
            *state = SessionState::Failed("Incompatible protocol version".to_string());
            return Err(SyncError::Protocol(format!(
                "Protocol version {} not compatible with {}",
                hello.version, PROTOCOL_VERSION
            )));
        }

        info!(
            "[{}] Connected to peer {} ({})",
            self.session_id,
            self.peer_name,
            hex::encode(&hello.device_id[..8.min(hello.device_id.len())])
        );

        *state = SessionState::Negotiating;

        // Create our Hello response
        Ok(Hello {
            version: PROTOCOL_VERSION.to_string(),
            device_id: vec![0; 32], // TODO: Get from identity manager
            device_name: "landropic".to_string(),
            capabilities: vec![
                "sync.v1".to_string(),
                "incremental.v1".to_string(),
                "compression.zstd".to_string(),
            ],
            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        })
    }

    /// Process FolderSummary message
    pub async fn handle_folder_summary(&self, summary: FolderSummary) -> Result<FolderSummary> {
        let mut state = self.state.write().await;

        if *state != SessionState::Negotiating {
            return Err(SyncError::Protocol(format!(
                "Unexpected FolderSummary in state {:?}",
                *state
            )));
        }

        info!(
            "[{}] Peer folder: {} ({} files, {} bytes)",
            self.session_id, summary.folder_id, summary.file_count, summary.total_size
        );

        // Load our manifest
        let our_manifest = self.load_our_manifest(&summary.folder_id).await?;
        *self.our_manifest.write().await = Some(our_manifest.clone());

        *state = SessionState::ComputingDiff;

        // Return our folder summary
        Ok(FolderSummary {
            folder_id: summary.folder_id,
            folder_path: "/sync".to_string(), // TODO: Get from config
            manifest_hash: our_manifest
                .manifest_hash
                .as_ref()
                .map(|h| hex::decode(h).unwrap_or_default())
                .unwrap_or_default(),
            total_size: our_manifest.total_size(),
            file_count: our_manifest.file_count() as u64,
            last_modified: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        })
    }

    /// Process Manifest message
    pub async fn handle_manifest(&self, manifest: ProtoManifest) -> Result<Want> {
        let mut state = self.state.write().await;

        if *state != SessionState::ComputingDiff {
            return Err(SyncError::Protocol(format!(
                "Unexpected Manifest in state {:?}",
                *state
            )));
        }

        debug!(
            "[{}] Received manifest with {} files",
            self.session_id,
            manifest.files.len()
        );

        // Store peer manifest
        *self.peer_manifest.write().await = Some(manifest.clone());

        // Convert proto manifest to our format
        let peer_manifest = self.proto_to_manifest(manifest)?;
        let our_manifest = self
            .our_manifest
            .read()
            .await
            .as_ref()
            .ok_or_else(|| SyncError::Protocol("Our manifest not loaded".to_string()))?
            .clone();

        // Compute diff
        let mut diff_tracker = self.diff_tracker.write().await;
        let diff =
            diff_tracker.compute_incremental(&self.peer_id, &our_manifest, &peer_manifest)?;

        info!(
            "[{}] Diff computed: {} added, {} modified, {} deleted",
            self.session_id,
            diff.stats.files_added,
            diff.stats.files_modified,
            diff.stats.files_deleted
        );

        // Store diff result
        *self.diff_result.write().await = Some(diff.clone());

        // Update stats
        let mut stats = self.stats.write().await;
        stats.files_added = diff.stats.files_added;
        stats.files_modified = diff.stats.files_modified;
        stats.files_deleted = diff.stats.files_deleted;

        // Determine chunks we need
        let wanted: Vec<Vec<u8>> = diff
            .required_chunks
            .iter()
            .take(MAX_WANT_CHUNKS)
            .map(|h| hex::decode(h).unwrap_or_default())
            .collect();

        // Store wanted chunks
        *self.wanted_chunks.write().await = diff.required_chunks.clone();

        // Transition to transferring state
        *state = SessionState::Transferring {
            total_chunks: diff.required_chunks.len(),
            transferred_chunks: 0,
        };

        Ok(Want {
            folder_id: peer_manifest.folder_id,
            chunk_hashes: wanted,
            priority: 1,
        })
    }

    /// Process Want message (peer requesting chunks)
    pub async fn handle_want(&self, want: Want) -> Result<Vec<ChunkData>> {
        let state = self.state.read().await;

        if !matches!(*state, SessionState::Transferring { .. }) {
            return Err(SyncError::Protocol(format!(
                "Unexpected Want in state {:?}",
                *state
            )));
        }

        debug!(
            "[{}] Peer requesting {} chunks",
            self.session_id,
            want.chunk_hashes.len()
        );

        let mut chunks = Vec::new();
        let mut stats = self.stats.write().await;

        for hash_bytes in want.chunk_hashes.iter().take(MAX_CONCURRENT_TRANSFERS) {
            let hash_str = hex::encode(hash_bytes);

            // Skip if already sent
            if self.sent_chunks.read().await.contains(&hash_str) {
                continue;
            }

            // Try to load chunk from CAS
            match self.load_chunk(&hash_str).await {
                Ok(data) => {
                    trace!(
                        "[{}] Sending chunk {} ({} bytes)",
                        self.session_id,
                        hash_str,
                        data.len()
                    );

                    chunks.push(ChunkData {
                        hash: hash_bytes.clone(),
                        data: data.clone(),
                        compressed: false, // TODO: Implement compression
                        uncompressed_size: data.len() as u32,
                    });

                    self.sent_chunks.write().await.insert(hash_str);
                    stats.chunks_sent += 1;
                    stats.bytes_sent += data.len() as u64;
                }
                Err(e) => {
                    warn!(
                        "[{}] Failed to load chunk {}: {}",
                        self.session_id, hash_str, e
                    );
                }
            }
        }

        Ok(chunks)
    }

    /// Process Have message (peer advertising chunks)
    pub async fn handle_have(&self, have: Have) -> Result<()> {
        debug!(
            "[{}] Peer has {} chunks",
            self.session_id,
            have.chunk_hashes.len()
        );

        // Update our tracking of what peer has
        // This is useful for optimizing future transfers

        Ok(())
    }

    /// Process ChunkData message
    pub async fn handle_chunk_data(&self, chunk: ChunkData) -> Result<Ack> {
        let mut state = self.state.write().await;

        if !matches!(*state, SessionState::Transferring { .. }) {
            return Err(SyncError::Protocol(format!(
                "Unexpected ChunkData in state {:?}",
                *state
            )));
        }

        let hash_str = hex::encode(&chunk.hash);
        trace!(
            "[{}] Received chunk {} ({} bytes)",
            self.session_id,
            hash_str,
            chunk.data.len()
        );

        // Verify chunk hash
        let computed_hash = blake3::hash(&chunk.data);
        if computed_hash.as_bytes() != chunk.hash.as_slice() {
            return Err(SyncError::Protocol(format!(
                "Chunk hash mismatch for {}",
                hash_str
            )));
        }

        // Store chunk in CAS
        self.store_chunk(&hash_str, chunk.data.clone()).await?;

        // Update tracking
        self.received_chunks.write().await.insert(hash_str.clone());
        self.wanted_chunks.write().await.remove(&hash_str);

        // Update stats
        let mut stats = self.stats.write().await;
        stats.chunks_received += 1;
        stats.bytes_received += chunk.data.len() as u64;

        // Update state progress
        if let SessionState::Transferring {
            total_chunks,
            transferred_chunks,
        } = *state
        {
            let new_transferred = transferred_chunks + 1;

            if new_transferred >= total_chunks {
                info!("[{}] All chunks received, verifying...", self.session_id);
                *state = SessionState::Verifying;
            } else {
                *state = SessionState::Transferring {
                    total_chunks,
                    transferred_chunks: new_transferred,
                };

                if new_transferred % 10 == 0 {
                    let progress = (new_transferred as f64 / total_chunks as f64) * 100.0;
                    info!("[{}] Transfer progress: {:.1}%", self.session_id, progress);
                }
            }
        }

        Ok(Ack {
            request_id: hex::encode(&chunk.hash),
            success: true,
            message: "Chunk stored".to_string(),
        })
    }

    /// Process Ack message
    pub async fn handle_ack(&self, ack: Ack) -> Result<()> {
        if !ack.success {
            warn!(
                "[{}] Received negative ack: {}",
                self.session_id, ack.message
            );
        }

        // Check if we're done
        let state = self.state.read().await.clone();
        if state == SessionState::Verifying {
            // Verify all chunks received
            if self.verify_transfer().await? {
                *self.state.write().await = SessionState::Completed;
                info!("[{}] Sync completed successfully", self.session_id);

                // Update database
                self.update_peer_state().await?;
            } else {
                *self.state.write().await = SessionState::Failed("Verification failed".to_string());
                error!("[{}] Transfer verification failed", self.session_id);
            }
        }

        Ok(())
    }

    /// Handle protocol error
    pub async fn handle_error(&self, error: ProtoError) -> Result<()> {
        error!(
            "[{}] Received error from peer: {} - {}",
            self.session_id, error.message, error.details
        );

        *self.state.write().await = SessionState::Failed(error.message.clone());

        Ok(())
    }

    /// Load our manifest for a folder
    async fn load_our_manifest(&self, folder_id: &str) -> Result<Manifest> {
        // TODO: Load from index database
        Ok(Manifest::new(folder_id.to_string(), 1))
    }

    /// Convert proto manifest to our format
    fn proto_to_manifest(&self, proto: ProtoManifest) -> Result<Manifest> {
        let mut manifest = Manifest::new(proto.folder_id, 1);

        for file in proto.files {
            let entry = ManifestEntry {
                path: file.path,
                size: file.size,
                modified_at: file
                    .modified
                    .and_then(|t| DateTime::from_timestamp(t.seconds, t.nanos as u32))
                    .unwrap_or_else(Utc::now),
                content_hash: hex::encode(&file.content_hash),
                chunk_hashes: file.chunk_hashes.iter().map(|h| hex::encode(h)).collect(),
                mode: Some(file.mode),
            };
            manifest.add_file(entry);
        }

        manifest.finalize();
        Ok(manifest)
    }

    /// Load chunk from CAS
    async fn load_chunk(&self, hash: &str) -> Result<Vec<u8>> {
        let hash_bytes =
            hex::decode(hash).map_err(|e| SyncError::Protocol(format!("Invalid hash: {}", e)))?;

        let mut hash_array = [0u8; 32];
        hash_array.copy_from_slice(&hash_bytes[..32]);
        let content_hash = ContentHash::from_bytes(hash_array);

        let bytes = self
            .cas
            .read(&content_hash)
            .await
            .map_err(|e| SyncError::Storage(format!("Failed to load chunk: {}", e)))?;

        Ok(bytes.to_vec())
    }

    /// Store chunk in CAS
    async fn store_chunk(&self, hash: &str, data: Vec<u8>) -> Result<()> {
        // Verify hash matches data
        let computed_hash = blake3::hash(&data);
        let hash_bytes =
            hex::decode(hash).map_err(|e| SyncError::Protocol(format!("Invalid hash: {}", e)))?;

        if computed_hash.as_bytes() != hash_bytes.as_slice() {
            return Err(SyncError::Protocol(format!(
                "Hash mismatch: expected {}, got {}",
                hash,
                hex::encode(computed_hash.as_bytes())
            )));
        }

        self.cas
            .write(&data)
            .await
            .map_err(|e| SyncError::Storage(format!("Failed to store chunk: {}", e)))?;

        Ok(())
    }

    /// Verify all chunks were transferred correctly
    async fn verify_transfer(&self) -> Result<bool> {
        let wanted = self.wanted_chunks.read().await;
        let received = self.received_chunks.read().await;

        if !wanted.is_empty() {
            warn!(
                "[{}] Still missing {} chunks",
                self.session_id,
                wanted.len()
            );
            return Ok(false);
        }

        debug!(
            "[{}] Verification complete: {} chunks received",
            self.session_id,
            received.len()
        );

        Ok(true)
    }

    /// Update peer state in database
    async fn update_peer_state(&self) -> Result<()> {
        let stats = self.stats.read().await;
        let our_manifest = self.our_manifest.read().await;

        let peer_state = PeerSyncState {
            peer_id: self.peer_id.clone(),
            peer_name: self.peer_name.clone(),
            last_sync: Some(Utc::now()),
            last_manifest_hash: our_manifest.as_ref().and_then(|m| m.manifest_hash.clone()),
            bytes_sent: stats.bytes_sent,
            bytes_received: stats.bytes_received,
            files_sent: stats.chunks_sent as u64,
            files_received: stats.chunks_received as u64,
            current_state: PeerState::Completed {
                timestamp: Utc::now(),
            },
        };

        self.database.upsert_peer_state(&peer_state).await?;

        Ok(())
    }

    /// Check version compatibility
    fn is_version_compatible(version: &str) -> bool {
        // Simple major version check
        let our_major = PROTOCOL_VERSION.split('.').next().unwrap_or("1");
        let their_major = version.split('.').next().unwrap_or("0");
        our_major == their_major
    }

    /// Get transfer progress
    pub async fn get_progress(&self) -> f64 {
        if let SessionState::Transferring {
            total_chunks,
            transferred_chunks,
        } = &*self.state.read().await
        {
            if *total_chunks > 0 {
                return (*transferred_chunks as f64 / *total_chunks as f64) * 100.0;
            }
        }
        0.0
    }

    /// Get transfer statistics
    pub async fn get_stats(&self) -> TransferStats {
        self.stats.read().await.clone()
    }
}

/// Protocol message for Have (advertising available chunks)
#[derive(Debug, Clone)]
pub struct Have {
    pub folder_id: String,
    pub chunk_hashes: Vec<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_session() -> (SyncSession, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let cas = Arc::new(
            ContentStore::new(temp_dir.path().join("cas"))
                .await
                .unwrap(),
        );
        let db = AsyncSyncDatabase::open_in_memory().await.unwrap();

        let session = SyncSession::new("test-peer".to_string(), "Test Device".to_string(), cas, db);

        (session, temp_dir)
    }

    #[tokio::test]
    async fn test_session_state_transitions() {
        let (session, _temp) = create_test_session().await;

        assert_eq!(session.state().await, SessionState::Initializing);

        // Process Hello
        let hello = Hello {
            version: PROTOCOL_VERSION.to_string(),
            device_id: vec![1, 2, 3],
            device_name: "Peer".to_string(),
            capabilities: vec!["sync.v1".to_string()],
            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        };

        let _response = session.handle_hello(hello).await.unwrap();
        assert_eq!(session.state().await, SessionState::Negotiating);
    }

    #[tokio::test]
    async fn test_version_compatibility() {
        assert!(SyncSession::is_version_compatible("1.0.0"));
        assert!(SyncSession::is_version_compatible("1.5.2"));
        assert!(!SyncSession::is_version_compatible("2.0.0"));
        assert!(!SyncSession::is_version_compatible("0.9.0"));
    }

    #[tokio::test]
    async fn test_chunk_tracking() {
        let (session, _temp) = create_test_session().await;

        // Simulate receiving chunks
        session
            .received_chunks
            .write()
            .await
            .insert("chunk1".to_string());
        session
            .received_chunks
            .write()
            .await
            .insert("chunk2".to_string());

        let received = session.received_chunks.read().await;
        assert_eq!(received.len(), 2);
        assert!(received.contains("chunk1"));
        assert!(received.contains("chunk2"));
    }

    #[tokio::test]
    async fn test_stats_tracking() {
        let (session, _temp) = create_test_session().await;

        // Update stats
        {
            let mut stats = session.stats.write().await;
            stats.bytes_sent = 1024;
            stats.bytes_received = 2048;
            stats.chunks_sent = 5;
            stats.chunks_received = 10;
        }

        let stats = session.get_stats().await;
        assert_eq!(stats.bytes_sent, 1024);
        assert_eq!(stats.bytes_received, 2048);
        assert_eq!(stats.chunks_sent, 5);
        assert_eq!(stats.chunks_received, 10);
    }
}
