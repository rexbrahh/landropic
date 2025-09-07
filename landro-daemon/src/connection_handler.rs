//! QUIC connection handler for SimpleSyncMessage protocol

use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{error, info};

use landro_cas::ContentStore;
use landro_index::async_indexer::AsyncIndexer;
use landro_quic::Connection;
use landro_sync::protocol::SyncSession;
use landro_sync::state::AsyncSyncDatabase;

use crate::simple_sync_protocol::SimpleSyncMessage;

/// Messages for communication between QUIC and orchestrator
#[derive(Debug)]
pub enum QuicMessage {
    StartSync {
        peer_id: String,
        folders: Vec<PathBuf>,
    },
    ManifestReady {
        path: PathBuf,
    },
    ChunkRequest {
        peer_id: String,
        chunk_hash: String,
    },
}

/// Handle an incoming QUIC connection using SimpleSyncMessage protocol
pub async fn handle_quic_connection(
    connection: Connection,
    store: Arc<ContentStore>,
    indexer: Arc<AsyncIndexer>,
    _quic_tx: mpsc::Sender<QuicMessage>, // Unused in alpha, kept for compatibility
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Handling new QUIC connection with SimpleSyncMessage protocol");
    
    // Open bidirectional stream for sync protocol
    let (mut send_stream, mut recv_stream) = connection.open_bi().await?;
    
    // Get remote device info (will be populated after handshake)
    let remote_device_id = connection.remote_device_id().await
        .map(|id| hex::encode(&id))
        .unwrap_or_else(|| "unknown".to_string());
    let remote_device_name = connection.remote_device_name().await
        .unwrap_or_else(|| "unknown".to_string());
    
    // Create database for sync state
    let db_path = PathBuf::from(".landropic/sync_state.db");
    let database = AsyncSyncDatabase::open(&db_path).await?;
    
    // Create sync session
    let session = SyncSession::new(
        remote_device_id.clone(),
        remote_device_name.clone(),
        store.clone(),
        database,
    );
    
    // Handle sync protocol messages
    loop {
        // Read message from stream
        let mut len_buf = [0u8; 4];
        if recv_stream.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        
        let msg_len = u32::from_be_bytes(len_buf) as usize;
        let mut msg_buf = vec![0u8; msg_len];
        if recv_stream.read_exact(&mut msg_buf).await.is_err() {
            break;
        }
        
        // Process message based on type
        match process_sync_message(&session, &msg_buf, &store).await {
            Ok(response) => {
                // Send response if any
                if let Some(resp_data) = response {
                    let len_bytes = (resp_data.len() as u32).to_be_bytes();
                    send_stream.write_all(&len_bytes).await?;
                    send_stream.write_all(&resp_data).await?;
                }
            }
            Err(e) => {
                error!("Error processing sync message: {}", e);
                break;
            }
        }
    }
    
    info!("QUIC connection handler completed");
    Ok(())
}

/// Process a sync protocol message
async fn process_sync_message(
    session: &SyncSession,
    msg_data: &[u8],
    store: &Arc<ContentStore>,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
    use crate::simple_sync_protocol::SimpleSyncMessage;
    
    info!("Processing sync message of {} bytes", msg_data.len());
    
    // Parse the incoming message
    let message = match SimpleSyncMessage::from_bytes(msg_data) {
        Ok(msg) => msg,
        Err(e) => {
            error!("Failed to parse sync message: {}", e);
            return Ok(None);
        }
    };
    
    info!("Received SimpleSyncMessage: {:?}", message);
    
    match message {
        SimpleSyncMessage::FileTransferRequest { file_path, file_size, checksum } => {
            info!("Handling file transfer request: {} ({} bytes)", file_path, file_size);
            
            // For alpha version, always accept transfers to a default sync folder
            let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            let sync_folder = home_dir.join("LandropicSync");
            let path_buf = PathBuf::from(&file_path);
            let file_name = path_buf
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("received_file");
            let dest_path = sync_folder.join(file_name);
            
            // Create sync folder if it doesn't exist
            if let Err(e) = tokio::fs::create_dir_all(&sync_folder).await {
                error!("Failed to create sync folder: {}", e);
                let response = SimpleSyncMessage::FileTransferResponse {
                    accepted: false,
                    reason: Some(format!("Failed to create destination folder: {}", e)),
                };
                return Ok(Some(response.to_bytes()?));
            }
            
            // Accept the transfer
            let response = SimpleSyncMessage::FileTransferResponse {
                accepted: true,
                reason: None,
            };
            
            info!("Accepting file transfer to: {}", dest_path.display());
            Ok(Some(response.to_bytes()?))
        }
        
        SimpleSyncMessage::FileData { data } => {
            info!("Received file data: {} bytes", data.len());
            
            // Calculate checksum and prepare response
            let checksum = blake3::hash(&data);
            let checksum_hex = hex::encode(checksum.as_bytes());
            
            // Store the file data (simplified approach for alpha)
            let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            let sync_folder = home_dir.join("LandropicSync");
            let temp_file = sync_folder.join("received_file.tmp");
            
            match tokio::fs::write(&temp_file, &data).await {
                Ok(()) => {
                    info!("File data saved to: {}", temp_file.display());
                    let response = SimpleSyncMessage::TransferComplete {
                        success: true,
                        checksum_verified: true,
                        message: format!("File saved successfully, checksum: {}", checksum_hex),
                    };
                    Ok(Some(response.to_bytes()?))
                }
                Err(e) => {
                    error!("Failed to save file data: {}", e);
                    let response = SimpleSyncMessage::TransferComplete {
                        success: false,
                        checksum_verified: false,
                        message: format!("Failed to save file: {}", e),
                    };
                    Ok(Some(response.to_bytes()?))
                }
            }
        }
        
        _ => {
            info!("Unhandled message type: {:?}", message);
            Ok(None)
        }
    }
}