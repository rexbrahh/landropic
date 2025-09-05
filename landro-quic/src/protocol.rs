//! Stream protocol implementation for manifest exchange and chunk transfer

use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn, error};
use prost::Message;
use quinn::{RecvStream, SendStream};

use landro_proto::{
    Manifest, FolderSummary, Want, ChunkData, Ack, Error as ProtoError,
    validation::{self, limits, Validator}
};

use crate::connection::Connection;
use crate::errors::{QuicError, Result};

/// Stream message types for protocol multiplexing
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageType {
    Hello = 0,
    FolderSummary = 1,
    Manifest = 2,
    Want = 3,
    ChunkData = 4,
    Ack = 5,
    Error = 6,
}

impl MessageType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(MessageType::Hello),
            1 => Some(MessageType::FolderSummary),
            2 => Some(MessageType::Manifest),
            3 => Some(MessageType::Want),
            4 => Some(MessageType::ChunkData),
            5 => Some(MessageType::Ack),
            6 => Some(MessageType::Error),
            _ => None,
        }
    }
}

/// Protocol handler for managing sync streams and message exchange
pub struct StreamProtocol {
    connection: Arc<Connection>,
}

impl StreamProtocol {
    /// Create a new stream protocol handler
    pub fn new(connection: Arc<Connection>) -> Self {
        Self { connection }
    }

    /// Send a folder summary to peer
    pub async fn send_folder_summary(&self, summary: FolderSummary) -> Result<()> {
        debug!("Sending folder summary for {}", summary.folder_id);
        
        // Validate the folder summary
        validation::validate_folder_summary(&summary)
            .map_err(|e| QuicError::Protocol(format!("Invalid folder summary: {}", e)))?;

        let mut stream = self.connection.open_uni().await?;
        self.send_message(&mut stream, MessageType::FolderSummary, &summary).await?;
        stream.finish().map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        info!("Sent folder summary for {}", summary.folder_id);
        Ok(())
    }

    /// Receive and process a folder summary
    pub async fn receive_folder_summary(&self) -> Result<FolderSummary> {
        let mut stream = self.connection.accept_uni().await?;
        let summary: FolderSummary = self.receive_message(&mut stream, MessageType::FolderSummary).await?;

        // Validate received summary
        validation::validate_folder_summary(&summary)
            .map_err(|e| QuicError::Protocol(format!("Invalid received folder summary: {}", e)))?;

        info!("Received folder summary for {}", summary.folder_id);
        Ok(summary)
    }

    /// Send a complete manifest to peer
    pub async fn send_manifest(&self, manifest: Manifest) -> Result<()> {
        debug!("Sending manifest for folder {}", manifest.folder_id);

        // Validate the manifest
        validation::validate_manifest(&manifest)
            .map_err(|e| QuicError::Protocol(format!("Invalid manifest: {}", e)))?;

        let mut stream = self.connection.open_uni().await?;
        self.send_message(&mut stream, MessageType::Manifest, &manifest).await?;
        stream.finish().map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        info!("Sent manifest for folder {} with {} files", 
            manifest.folder_id, manifest.files.len());
        Ok(())
    }

    /// Receive and process a manifest
    pub async fn receive_manifest(&self) -> Result<Manifest> {
        let mut stream = self.connection.accept_uni().await?;
        let manifest: Manifest = self.receive_message(&mut stream, MessageType::Manifest).await?;

        // Validate received manifest
        validation::validate_manifest(&manifest)
            .map_err(|e| QuicError::Protocol(format!("Invalid received manifest: {}", e)))?;

        info!("Received manifest for folder {} with {} files", 
            manifest.folder_id, manifest.files.len());
        Ok(manifest)
    }

    /// Send a Want request for missing chunks
    pub async fn send_want(&self, want: Want) -> Result<()> {
        debug!("Sending want request for folder {} with {} chunks", 
            want.folder_id, want.chunk_hashes.len());

        // Validate the want request
        validation::validate_want(&want)
            .map_err(|e| QuicError::Protocol(format!("Invalid want request: {}", e)))?;

        let mut stream = self.connection.open_uni().await?;
        self.send_message(&mut stream, MessageType::Want, &want).await?;
        stream.finish().map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        info!("Sent want request for {} chunks", want.chunk_hashes.len());
        Ok(())
    }

    /// Receive and process a Want request
    pub async fn receive_want(&self) -> Result<Want> {
        let mut stream = self.connection.accept_uni().await?;
        let want: Want = self.receive_message(&mut stream, MessageType::Want).await?;

        // Validate received want request
        validation::validate_want(&want)
            .map_err(|e| QuicError::Protocol(format!("Invalid received want request: {}", e)))?;

        info!("Received want request for {} chunks", want.chunk_hashes.len());
        Ok(want)
    }

    /// Send chunk data to fulfill a Want request
    pub async fn send_chunk_data(&self, chunk: ChunkData) -> Result<()> {
        debug!("Sending chunk data: {} bytes", chunk.data.len());

        // Validate chunk data
        validation::validate_chunk_data(&chunk)
            .map_err(|e| QuicError::Protocol(format!("Invalid chunk data: {}", e)))?;

        let mut stream = self.connection.open_uni().await?;
        self.send_message(&mut stream, MessageType::ChunkData, &chunk).await?;
        stream.finish().map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        debug!("Sent chunk data: {} bytes", chunk.data.len());
        Ok(())
    }

    /// Receive chunk data
    pub async fn receive_chunk_data(&self) -> Result<ChunkData> {
        let mut stream = self.connection.accept_uni().await?;
        let chunk: ChunkData = self.receive_message(&mut stream, MessageType::ChunkData).await?;

        // Validate received chunk data
        validation::validate_chunk_data(&chunk)
            .map_err(|e| QuicError::Protocol(format!("Invalid received chunk data: {}", e)))?;

        debug!("Received chunk data: {} bytes", chunk.data.len());
        Ok(chunk)
    }

    /// Send an acknowledgment
    pub async fn send_ack(&self, ack: Ack) -> Result<()> {
        debug!("Sending ack for request: {}", ack.request_id);

        let mut stream = self.connection.open_uni().await?;
        self.send_message(&mut stream, MessageType::Ack, &ack).await?;
        stream.finish().map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        debug!("Sent ack: success={}", ack.success);
        Ok(())
    }

    /// Receive an acknowledgment
    pub async fn receive_ack(&self) -> Result<Ack> {
        let mut stream = self.connection.accept_uni().await?;
        let ack: Ack = self.receive_message(&mut stream, MessageType::Ack).await?;

        debug!("Received ack for request: {}", ack.request_id);
        Ok(ack)
    }

    /// Send an error message
    pub async fn send_error(&self, error: ProtoError) -> Result<()> {
        warn!("Sending error: {:?} - {}", error.error_type, error.message);

        let mut stream = self.connection.open_uni().await?;
        self.send_message(&mut stream, MessageType::Error, &error).await?;
        stream.finish().map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        warn!("Sent error message");
        Ok(())
    }

    /// Receive an error message
    pub async fn receive_error(&self) -> Result<ProtoError> {
        let mut stream = self.connection.accept_uni().await?;
        let error: ProtoError = self.receive_message(&mut stream, MessageType::Error).await?;

        warn!("Received error: {:?} - {}", error.error_type, error.message);
        Ok(error)
    }

    /// Multiplex receive - get next message of any type
    pub async fn receive_any_message(&self) -> Result<ProtocolMessage> {
        let mut stream = self.connection.accept_uni().await?;
        
        // Read message type
        let mut type_byte = [0u8; 1];
        stream.read_exact(&mut type_byte).await
            .map_err(|e| QuicError::Stream(format!("Failed to read message type: {}", e)))?;
        
        let message_type = MessageType::from_u8(type_byte[0])
            .ok_or_else(|| QuicError::Protocol(format!("Unknown message type: {}", type_byte[0])))?;

        // Read and parse message based on type
        let message = match message_type {
            MessageType::Hello => {
                return Err(QuicError::Protocol("Hello messages should be handled separately".to_string()));
            }
            MessageType::FolderSummary => {
                let summary = self.receive_typed_message::<FolderSummary>(&mut stream).await?;
                ProtocolMessage::FolderSummary(summary)
            }
            MessageType::Manifest => {
                let manifest = self.receive_typed_message::<Manifest>(&mut stream).await?;
                ProtocolMessage::Manifest(manifest)
            }
            MessageType::Want => {
                let want = self.receive_typed_message::<Want>(&mut stream).await?;
                ProtocolMessage::Want(want)
            }
            MessageType::ChunkData => {
                let chunk = self.receive_typed_message::<ChunkData>(&mut stream).await?;
                ProtocolMessage::ChunkData(chunk)
            }
            MessageType::Ack => {
                let ack = self.receive_typed_message::<Ack>(&mut stream).await?;
                ProtocolMessage::Ack(ack)
            }
            MessageType::Error => {
                let error = self.receive_typed_message::<ProtoError>(&mut stream).await?;
                ProtocolMessage::Error(error)
            }
        };

        Ok(message)
    }

    /// Generic message sending helper
    async fn send_message<T: Message>(
        &self,
        stream: &mut SendStream,
        msg_type: MessageType,
        message: &T,
    ) -> Result<()> {
        // Encode message
        let mut buf = Vec::new();
        message.encode(&mut buf)
            .map_err(|e| QuicError::Protocol(format!("Failed to encode message: {}", e)))?;

        // Validate message size
        if buf.len() > limits::MAX_MESSAGE_SIZE {
            return Err(QuicError::Protocol(format!(
                "Message too large: {} bytes (max: {})",
                buf.len(),
                limits::MAX_MESSAGE_SIZE
            )));
        }

        // Write message type
        stream.write_all(&[msg_type as u8]).await
            .map_err(|e| QuicError::Stream(format!("Failed to write message type: {}", e)))?;

        // Write message length
        let len = buf.len() as u32;
        stream.write_all(&len.to_be_bytes()).await
            .map_err(|e| QuicError::Stream(format!("Failed to write message length: {}", e)))?;

        // Write message data
        stream.write_all(&buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to write message data: {}", e)))?;

        Ok(())
    }

    /// Generic message receiving helper with expected type
    async fn receive_message<T: Message + Default>(
        &self,
        stream: &mut RecvStream,
        expected_type: MessageType,
    ) -> Result<T> {
        // Read message type
        let mut type_byte = [0u8; 1];
        stream.read_exact(&mut type_byte).await
            .map_err(|e| QuicError::Stream(format!("Failed to read message type: {}", e)))?;

        let message_type = MessageType::from_u8(type_byte[0])
            .ok_or_else(|| QuicError::Protocol(format!("Unknown message type: {}", type_byte[0])))?;

        if message_type != expected_type {
            return Err(QuicError::Protocol(format!(
                "Expected message type {:?}, got {:?}",
                expected_type, message_type
            )));
        }

        self.receive_typed_message(stream).await
    }

    /// Receive and parse a typed message (assumes type byte already read)
    async fn receive_typed_message<T: Message + Default>(&self, stream: &mut RecvStream) -> Result<T> {
        // Read message length
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await
            .map_err(|e| QuicError::Stream(format!("Failed to read message length: {}", e)))?;
        let len = u32::from_be_bytes(len_bytes) as usize;

        // Validate message size
        Validator::validate_message_size(len)
            .map_err(|e| QuicError::Protocol(format!("Invalid message size: {}", e)))?;

        // Read message data
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to read message data: {}", e)))?;

        // Parse message
        T::decode(&buf[..])
            .map_err(|e| QuicError::Protocol(format!("Failed to decode message: {}", e)))
    }
}

/// Protocol message enum for multiplexed message handling
#[derive(Debug, Clone)]
pub enum ProtocolMessage {
    FolderSummary(FolderSummary),
    Manifest(Manifest),
    Want(Want),
    ChunkData(ChunkData),
    Ack(Ack),
    Error(ProtoError),
}

/// Batch transfer manager for efficient chunk transfer
pub struct BatchTransferManager {
    protocol: StreamProtocol,
    chunk_cache: HashMap<Vec<u8>, ChunkData>,
}

impl BatchTransferManager {
    pub fn new(connection: Arc<Connection>) -> Self {
        Self {
            protocol: StreamProtocol::new(connection),
            chunk_cache: HashMap::new(),
        }
    }

    /// Transfer multiple chunks efficiently in batches
    pub async fn transfer_chunks(&mut self, wanted_chunks: Vec<Vec<u8>>) -> Result<Vec<ChunkData>> {
        let mut received_chunks = Vec::new();

        // Send want request
        let want = Want {
            folder_id: "default".to_string(), // This should be parameterized
            chunk_hashes: wanted_chunks.clone(),
            priority: 1,
        };

        self.protocol.send_want(want).await?;

        // Receive chunks
        for _i in 0..wanted_chunks.len() {
            match self.protocol.receive_chunk_data().await {
                Ok(chunk) => {
                    // Verify chunk hash
                    let hash = blake3::hash(&chunk.data).as_bytes().to_vec();
                    if wanted_chunks.contains(&hash) {
                        self.chunk_cache.insert(hash, chunk.clone());
                        received_chunks.push(chunk);
                    } else {
                        warn!("Received unexpected chunk with hash: {}", hex::encode(&hash));
                    }
                }
                Err(e) => {
                    error!("Failed to receive chunk: {}", e);
                    break;
                }
            }
        }

        info!("Received {} of {} requested chunks", received_chunks.len(), wanted_chunks.len());
        Ok(received_chunks)
    }

    /// Get cached chunk if available
    pub fn get_cached_chunk(&self, hash: &[u8]) -> Option<&ChunkData> {
        self.chunk_cache.get(hash)
    }

    /// Clear chunk cache to free memory
    pub fn clear_cache(&mut self) {
        self.chunk_cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_conversion() {
        assert_eq!(MessageType::from_u8(0), Some(MessageType::Hello));
        assert_eq!(MessageType::from_u8(1), Some(MessageType::FolderSummary));
        assert_eq!(MessageType::from_u8(6), Some(MessageType::Error));
        assert_eq!(MessageType::from_u8(99), None);
    }

    #[test]
    fn test_message_type_values() {
        assert_eq!(MessageType::Hello as u8, 0);
        assert_eq!(MessageType::FolderSummary as u8, 1);
        assert_eq!(MessageType::Manifest as u8, 2);
        assert_eq!(MessageType::Want as u8, 3);
        assert_eq!(MessageType::ChunkData as u8, 4);
        assert_eq!(MessageType::Ack as u8, 5);
        assert_eq!(MessageType::Error as u8, 6);
    }
}