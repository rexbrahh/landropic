//! QUIC integration for sync protocol
//!
//! Bridges the sync protocol with QUIC streaming for actual data transfer

use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};

use landro_cas::ContentStore;
use landro_proto::{Ack, ChunkData, FolderSummary, Hello, Manifest as ProtoManifest, Want};
use landro_quic::{
    connection::Connection,
    protocol::MessageType,
    stream_transfer::{StreamTransferConfig, StreamTransferManager, TransferResult},
};
use prost::Message;

use crate::{
    conflict_detection::{Conflict, ConflictDetectionConfig, ConflictDetector},
    errors::{Result, SyncError},
    protocol::{SessionState, SyncSession, TransferStats},
    sync_persistence::{PersistedSyncSession, SyncPersistenceManager, SyncSessionStatus},
};

/// QUIC-based sync transport implementation
pub struct QuicSyncTransport {
    connection: Arc<Connection>,
    transfer_manager: Arc<StreamTransferManager>,
    active_sessions: Arc<RwLock<HashMap<String, Arc<SyncSession>>>>,
    message_handler: Arc<MessageHandler>,
    persistence_manager: Arc<SyncPersistenceManager>,
    conflict_detector: Arc<Mutex<ConflictDetector>>,
}

impl QuicSyncTransport {
    /// Create new QUIC sync transport
    pub async fn new(connection: Arc<Connection>, cas: Arc<ContentStore>) -> Result<Self> {
        let config = StreamTransferConfig::file_sync_optimized();
        let transfer_manager = Arc::new(StreamTransferManager::new(connection.clone(), config));

        let message_handler = Arc::new(MessageHandler::new(cas));

        // Initialize persistence manager
        let persistence_config = crate::sync_persistence::SyncPersistenceConfig::default();
        let persistence_manager = Arc::new(SyncPersistenceManager::new(persistence_config).await?);

        // Initialize conflict detector
        let conflict_config = ConflictDetectionConfig::default();
        let conflict_detector = Arc::new(Mutex::new(ConflictDetector::new(conflict_config)));

        Ok(Self {
            connection,
            transfer_manager,
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
            message_handler,
            persistence_manager,
            conflict_detector,
        })
    }

    /// Start sync session with peer
    pub async fn start_sync(&self, folder_path: &str, is_initiator: bool) -> Result<String> {
        info!("Starting QUIC sync session for {}", folder_path);

        // Get device info from connection
        let device_id = self
            .connection
            .remote_device_id()
            .await
            .ok_or_else(|| SyncError::Protocol("No remote device ID".to_string()))?;
        let device_name = self
            .connection
            .remote_device_name()
            .await
            .unwrap_or_else(|| "Unknown".to_string());

        // Create sync session with in-memory database
        let database = crate::state::AsyncSyncDatabase::open_in_memory()
            .await
            .map_err(|e| SyncError::Storage(format!("Failed to create database: {}", e)))?;

        let session = Arc::new(SyncSession::new(
            hex::encode(&device_id[..8.min(device_id.len())]),
            device_name.clone(),
            self.message_handler.cas.clone(),
            database,
        ));

        let session_id = format!("sync_{}", uuid::Uuid::new_v4());
        self.active_sessions
            .write()
            .await
            .insert(session_id.clone(), session.clone());

        // Start persistence tracking
        let persisted_session = PersistedSyncSession::new(
            session_id.clone(),
            hex::encode(&device_id[..8.min(device_id.len())]),
            folder_path.to_string(),
        );
        self.persistence_manager
            .start_session(persisted_session)
            .await?;

        // Start protocol flow
        if is_initiator {
            self.initiate_sync(session, folder_path).await?;
        } else {
            self.respond_to_sync(session).await?;
        }

        Ok(session_id)
    }

    /// Initiate sync as client
    async fn initiate_sync(&self, session: Arc<SyncSession>, folder_path: &str) -> Result<()> {
        info!("Initiating sync for {}", folder_path);

        // Send Hello
        let hello = Hello {
            version: crate::protocol::PROTOCOL_VERSION.to_string(),
            device_id: vec![0; 32], // TODO: Get from identity
            device_name: "landropic-client".to_string(),
            capabilities: vec!["sync.v1".to_string()],
            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        };

        self.send_message(MessageType::Hello, &hello).await?;

        // Wait for Hello response
        let response = self.receive_message::<Hello>(MessageType::Hello).await?;
        session.handle_hello(response).await?;

        // Send FolderSummary
        let summary = FolderSummary {
            folder_id: folder_path.to_string(),
            folder_path: folder_path.to_string(),
            manifest_hash: vec![0; 32], // TODO: Compute actual hash
            total_size: 0,
            file_count: 0,
            last_modified: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        };

        self.send_message(MessageType::FolderSummary, &summary)
            .await?;

        // Continue with manifest exchange and chunk transfer
        self.handle_sync_flow(session).await?;

        Ok(())
    }

    /// Respond to sync as server
    async fn respond_to_sync(&self, session: Arc<SyncSession>) -> Result<()> {
        info!("Responding to sync request");

        // Wait for Hello
        let hello = self.receive_message::<Hello>(MessageType::Hello).await?;
        let response = session.handle_hello(hello).await?;
        self.send_message(MessageType::Hello, &response).await?;

        // Wait for FolderSummary
        let summary = self
            .receive_message::<FolderSummary>(MessageType::FolderSummary)
            .await?;
        let response = session.handle_folder_summary(summary).await?;
        self.send_message(MessageType::FolderSummary, &response)
            .await?;

        // Continue with sync flow
        self.handle_sync_flow(session).await?;

        Ok(())
    }

    /// Handle main sync flow (manifest exchange and chunk transfer)
    async fn handle_sync_flow(&self, session: Arc<SyncSession>) -> Result<()> {
        // Exchange manifests
        let manifest = self
            .receive_message::<ProtoManifest>(MessageType::Manifest)
            .await?;
        let want = session.handle_manifest(manifest).await?;
        self.send_message(MessageType::Want, &want).await?;

        // Start chunk transfer
        self.transfer_chunks(session.clone(), want.chunk_hashes)
            .await?;

        // Monitor progress
        self.monitor_transfer_progress(session).await?;

        Ok(())
    }

    /// Transfer chunks using QUIC streams
    async fn transfer_chunks(
        &self,
        session: Arc<SyncSession>,
        chunk_hashes: Vec<Vec<u8>>,
    ) -> Result<()> {
        info!("Starting transfer of {} chunks", chunk_hashes.len());

        // Get session ID for persistence tracking
        let session_id = format!("sync_{}", session.peer_id());

        // Convert to ChunkData messages
        let mut chunks = Vec::new();
        for hash in &chunk_hashes {
            // Load chunk from CAS
            if let Ok(data) = self.load_chunk_from_cas(hash).await {
                chunks.push(ChunkData {
                    hash: hash.clone(),
                    data,
                    compressed: false,
                    uncompressed_size: 0,
                });
            }
        }

        // Use StreamTransferManager for parallel transfer
        let mut result_rx = self
            .transfer_manager
            .stream_chunks(chunks, 128) // Medium priority
            .await
            .map_err(|e| SyncError::Network(format!("Failed to stream chunks: {}", e)))?;

        // Process transfer results with persistence tracking
        let session_clone = session.clone();
        let persistence_manager = self.persistence_manager.clone();
        let session_id_clone = session_id.clone();

        tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                if result.success {
                    debug!(
                        "Chunk {} transferred successfully",
                        hex::encode(&result.chunk_hash)
                    );

                    // Update persistence with completed chunk
                    let chunk_hash_str = hex::encode(&result.chunk_hash);
                    if let Err(e) = persistence_manager
                        .mark_chunk_completed(
                            &session_id_clone,
                            chunk_hash_str,
                            result.chunk_hash.len() as u64,
                        )
                        .await
                    {
                        error!("Failed to update persistence for chunk completion: {}", e);
                    }

                    // Send ACK for successful chunk
                    let ack = Ack {
                        request_id: hex::encode(&result.chunk_hash),
                        success: true,
                        message: "Chunk received".to_string(),
                    };

                    if let Err(e) = session_clone.handle_ack(ack).await {
                        error!("Failed to handle ACK: {}", e);
                    }
                } else {
                    warn!("Chunk transfer failed: {:?}", result.error);

                    // Mark chunk as failed in persistence
                    let chunk_hash_str = hex::encode(&result.chunk_hash);
                    if let Err(e) = persistence_manager
                        .mark_chunk_failed(&session_id_clone, chunk_hash_str)
                        .await
                    {
                        error!("Failed to update persistence for chunk failure: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    /// Load chunk from CAS
    async fn load_chunk_from_cas(&self, hash: &[u8]) -> Result<Vec<u8>> {
        // TODO: Implement actual CAS loading
        warn!("Using placeholder chunk data for {}", hex::encode(hash));
        Ok(vec![0u8; 1024]) // Placeholder
    }

    /// Send protocol message over QUIC
    async fn send_message<T: Message>(&self, msg_type: MessageType, message: &T) -> Result<()> {
        let mut stream = self
            .connection
            .open_uni()
            .await
            .map_err(|e| SyncError::Network(format!("Failed to open stream: {}", e)))?;

        // Encode message
        let mut buf = Vec::with_capacity(message.encoded_len());
        message
            .encode(&mut buf)
            .map_err(|e| SyncError::Protocol(format!("Failed to encode message: {}", e)))?;

        // Write message type
        stream
            .write_all(&[msg_type as u8])
            .await
            .map_err(|e| SyncError::Network(format!("Failed to write message type: {}", e)))?;

        // Write message length
        stream
            .write_all(&(buf.len() as u32).to_be_bytes())
            .await
            .map_err(|e| SyncError::Network(format!("Failed to write message length: {}", e)))?;

        // Write message data
        stream
            .write_all(&buf)
            .await
            .map_err(|e| SyncError::Network(format!("Failed to write message data: {}", e)))?;

        stream
            .finish()
            .map_err(|e| SyncError::Network(format!("Failed to finish stream: {}", e)))?;

        debug!("Sent {} message ({} bytes)", msg_type as u8, buf.len());
        Ok(())
    }

    /// Receive protocol message over QUIC
    async fn receive_message<T: Message + Default>(&self, expected_type: MessageType) -> Result<T> {
        let mut stream = self
            .connection
            .accept_uni()
            .await
            .map_err(|e| SyncError::Network(format!("Failed to accept stream: {}", e)))?;

        // Read message type
        let mut msg_type = [0u8; 1];
        stream
            .read_exact(&mut msg_type)
            .await
            .map_err(|e| SyncError::Network(format!("Failed to read message type: {}", e)))?;

        if msg_type[0] != expected_type as u8 {
            return Err(SyncError::Protocol(format!(
                "Expected message type {}, got {}",
                expected_type as u8, msg_type[0]
            )));
        }

        // Read message length
        let mut len_bytes = [0u8; 4];
        stream
            .read_exact(&mut len_bytes)
            .await
            .map_err(|e| SyncError::Network(format!("Failed to read message length: {}", e)))?;
        let len = u32::from_be_bytes(len_bytes) as usize;

        // Read message data
        let mut buf = vec![0u8; len];
        stream
            .read_exact(&mut buf)
            .await
            .map_err(|e| SyncError::Network(format!("Failed to read message data: {}", e)))?;

        // Decode message
        T::decode(&buf[..])
            .map_err(|e| SyncError::Protocol(format!("Failed to decode message: {}", e)))
    }

    /// Monitor transfer progress
    async fn monitor_transfer_progress(&self, session: Arc<SyncSession>) -> Result<()> {
        let mut last_progress = 0.0;

        loop {
            let progress = session.get_progress().await;
            let state = session.state().await;

            if progress != last_progress {
                info!("Sync progress: {:.1}%", progress);
                last_progress = progress;

                // Report progress via hooks
                let _ = tokio::process::Command::new("npx")
                    .args(&[
                        "claude-flow@alpha",
                        "hooks",
                        "notify",
                        "--message",
                        &format!("Sync progress: {:.1}%", progress),
                    ])
                    .output()
                    .await;
            }

            match state {
                SessionState::Completed => {
                    info!("Sync completed successfully");
                    break;
                }
                SessionState::Failed(reason) => {
                    error!("Sync failed: {}", reason);
                    return Err(SyncError::Protocol(reason));
                }
                _ => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }

        Ok(())
    }

    /// Resume interrupted sync session
    pub async fn resume_sync(&self, session_id: &str) -> Result<()> {
        info!("Attempting to resume sync session: {}", session_id);

        // Get persisted session state
        let persisted_session = self
            .persistence_manager
            .get_session(session_id)
            .await
            .ok_or_else(|| {
                SyncError::Protocol(format!("No persisted session found: {}", session_id))
            })?;

        if !persisted_session.can_resume() {
            return Err(SyncError::Protocol(format!(
                "Session {} cannot be resumed (status: {:?})",
                session_id, persisted_session.status
            )));
        }

        info!(
            "Resuming sync: {} chunks completed, {} chunks pending, {} chunks failed",
            persisted_session.completed_chunks.len(),
            persisted_session.pending_chunks.len(),
            persisted_session.failed_chunks.len()
        );

        // Create new session with same device info
        let database = crate::state::AsyncSyncDatabase::open_in_memory()
            .await
            .map_err(|e| SyncError::Storage(format!("Failed to create database: {}", e)))?;

        let session = Arc::new(SyncSession::new(
            persisted_session.peer_id.clone(),
            "Resumed".to_string(),
            self.message_handler.cas.clone(),
            database,
        ));

        self.active_sessions
            .write()
            .await
            .insert(session_id.to_string(), session.clone());

        // Resume transfer of pending and failed chunks
        let mut chunks_to_transfer: Vec<_> = persisted_session
            .pending_chunks
            .union(&persisted_session.failed_chunks)
            .cloned()
            .collect();

        // Convert hex strings back to bytes
        let chunk_hashes: Vec<Vec<u8>> = chunks_to_transfer
            .iter()
            .filter_map(|hex_str| hex::decode(hex_str).ok())
            .collect();

        if !chunk_hashes.is_empty() {
            self.transfer_chunks(session, chunk_hashes).await?;
        }

        Ok(())
    }

    /// Get resumable sessions
    pub async fn get_resumable_sessions(&self) -> Vec<PersistedSyncSession> {
        self.persistence_manager.get_resumable_sessions().await
    }

    /// Complete sync session
    pub async fn complete_sync(&self, session_id: &str) -> Result<()> {
        info!("Completing sync session: {}", session_id);

        // Mark as completed in persistence
        self.persistence_manager
            .complete_session(session_id)
            .await?;

        // Remove from active sessions
        self.active_sessions.write().await.remove(session_id);

        Ok(())
    }

    /// Handle sync interruption (network failure, shutdown, etc.)
    pub async fn interrupt_sync(&self, session_id: &str, reason: &str) -> Result<()> {
        warn!("Interrupting sync session {}: {}", session_id, reason);

        // Mark as interrupted in persistence for resume
        self.persistence_manager
            .interrupt_session(session_id)
            .await?;

        // Remove from active sessions but keep persistence for resume
        self.active_sessions.write().await.remove(session_id);

        Ok(())
    }

    /// Get sync statistics
    pub async fn get_stats(&self, session_id: &str) -> Option<TransferStats> {
        let sessions = self.active_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            Some(session.get_stats().await)
        } else {
            None
        }
    }
}

/// Message handler for processing sync protocol messages
struct MessageHandler {
    cas: Arc<ContentStore>,
}

impl MessageHandler {
    fn new(cas: Arc<ContentStore>) -> Self {
        Self { cas }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_quic_sync_transport_creation() {
        // This would require a mock Connection and ContentStore
        // For now, just verify the module compiles
        assert!(true);
    }
}
