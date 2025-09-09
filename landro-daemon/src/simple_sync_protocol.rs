//! Simplified sync protocol for alpha release
//!
//! This implements a basic file transfer protocol for one-way sync:
//! 1. FileTransferRequest - Request to send a file
//! 2. FileTransferResponse - Accept/reject the file transfer  
//! 3. FileData - Actual file content
//! 4. TransferComplete - Acknowledgment after successful transfer

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info};

/// Simple sync message types for alpha release
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SimpleSyncMessage {
    /// Request to transfer a file
    FileTransferRequest {
        file_path: String,
        file_size: u64,
        checksum: String, // Blake3 hash
    },
    /// Response to file transfer request
    FileTransferResponse {
        accepted: bool,
        reason: Option<String>,
    },
    /// File content data
    FileData { data: Vec<u8> },
    /// Transfer complete acknowledgment
    TransferComplete {
        success: bool,
        checksum_verified: bool,
        message: String,
    },
}

impl SimpleSyncMessage {
    /// Serialize message to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let json = serde_json::to_string(self)?;
        Ok(json.into_bytes())
    }

    /// Deserialize message from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let json = String::from_utf8(bytes.to_vec())?;
        let message = serde_json::from_str(&json)?;
        Ok(message)
    }

    /// Send message over QUIC stream
    pub async fn send<W: AsyncWriteExt + Unpin>(
        &self,
        writer: &mut W,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let bytes = self.to_bytes()?;
        let len = bytes.len() as u32;

        // Write message length first
        writer.write_all(&len.to_be_bytes()).await?;
        // Write message data
        writer.write_all(&bytes).await?;

        debug!("Sent SimpleSyncMessage: {:?}", self);
        Ok(())
    }

    /// Receive message from QUIC stream
    pub async fn receive<R: AsyncReadExt + Unpin>(
        reader: &mut R,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Read message length
        let mut len_buf = [0u8; 4];
        reader.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        // Validate message size (max 100MB for file data)
        if len > 100 * 1024 * 1024 {
            return Err("Message too large".into());
        }

        // Read message data
        let mut msg_buf = vec![0u8; len];
        reader.read_exact(&mut msg_buf).await?;

        let message = Self::from_bytes(&msg_buf)?;
        debug!("Received SimpleSyncMessage: {:?}", message);
        Ok(message)
    }
}

/// Simple file transfer handler
pub struct SimpleFileTransfer {
    source_path: PathBuf,
    dest_path: PathBuf,
}

impl SimpleFileTransfer {
    pub fn new(source_path: PathBuf, dest_path: PathBuf) -> Self {
        Self {
            source_path,
            dest_path,
        }
    }

    /// Send a file to peer (sender side)
    pub async fn send_file<W: AsyncWriteExt + Unpin, R: AsyncReadExt + Unpin>(
        &self,
        writer: &mut W,
        reader: &mut R,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting file transfer: {}", self.source_path.display());

        // Read file and calculate checksum
        let file_data = tokio::fs::read(&self.source_path).await?;
        let checksum = blake3::hash(&file_data);
        let checksum_hex = hex::encode(checksum.as_bytes());

        // Send transfer request
        let request = SimpleSyncMessage::FileTransferRequest {
            file_path: self.source_path.display().to_string(),
            file_size: file_data.len() as u64,
            checksum: checksum_hex.clone(),
        };
        request.send(writer).await?;

        // Wait for response
        let response = SimpleSyncMessage::receive(reader).await?;
        match response {
            SimpleSyncMessage::FileTransferResponse { accepted: true, .. } => {
                info!("File transfer accepted, sending data");
            }
            SimpleSyncMessage::FileTransferResponse {
                accepted: false,
                reason,
            } => {
                let reason = reason.unwrap_or_else(|| "Unknown reason".to_string());
                return Err(format!("File transfer rejected: {}", reason).into());
            }
            _ => {
                return Err("Unexpected response to file transfer request".into());
            }
        }

        // Send file data
        let data_message = SimpleSyncMessage::FileData { data: file_data };
        data_message.send(writer).await?;

        // Wait for completion acknowledgment
        let completion = SimpleSyncMessage::receive(reader).await?;
        match completion {
            SimpleSyncMessage::TransferComplete {
                success: true,
                checksum_verified: true,
                ..
            } => {
                info!("File transfer completed successfully");
                Ok(())
            }
            SimpleSyncMessage::TransferComplete {
                success,
                checksum_verified,
                message,
            } => {
                error!(
                    "File transfer failed: success={}, checksum_verified={}, message={}",
                    success, checksum_verified, message
                );
                Err(format!("Transfer failed: {}", message).into())
            }
            _ => Err("Unexpected response to file transfer".into()),
        }
    }

    /// Receive a file from peer (receiver side)
    pub async fn receive_file<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        R: AsyncReadExt + Unpin,
        W: AsyncWriteExt + Unpin,
    {
        info!("Waiting for file transfer request");

        // Wait for transfer request
        let request = SimpleSyncMessage::receive(reader).await?;
        let (file_path, file_size, expected_checksum) = match request {
            SimpleSyncMessage::FileTransferRequest {
                file_path,
                file_size,
                checksum,
            } => {
                info!(
                    "Received file transfer request: {} ({} bytes)",
                    file_path, file_size
                );
                (file_path, file_size, checksum)
            }
            _ => {
                let error_response = SimpleSyncMessage::FileTransferResponse {
                    accepted: false,
                    reason: Some("Expected file transfer request".to_string()),
                };
                error_response.send(writer).await?;
                return Err("Expected file transfer request".into());
            }
        };

        // Accept the transfer (for alpha version, always accept)
        let accept_response = SimpleSyncMessage::FileTransferResponse {
            accepted: true,
            reason: None,
        };
        accept_response.send(writer).await?;

        // Receive file data
        let file_data = match SimpleSyncMessage::receive(reader).await? {
            SimpleSyncMessage::FileData { data } => {
                info!("Received file data: {} bytes", data.len());
                data
            }
            _ => {
                let error_response = SimpleSyncMessage::TransferComplete {
                    success: false,
                    checksum_verified: false,
                    message: "Expected file data".to_string(),
                };
                error_response.send(writer).await?;
                return Err("Expected file data".into());
            }
        };

        // Verify checksum
        let actual_checksum = blake3::hash(&file_data);
        let actual_checksum_hex = hex::encode(actual_checksum.as_bytes());
        let checksum_valid = actual_checksum_hex == expected_checksum;

        if !checksum_valid {
            error!(
                "Checksum mismatch: expected {}, got {}",
                expected_checksum, actual_checksum_hex
            );
            let error_response = SimpleSyncMessage::TransferComplete {
                success: false,
                checksum_verified: false,
                message: "Checksum verification failed".to_string(),
            };
            error_response.send(writer).await?;
            return Err("Checksum verification failed".into());
        }

        // Create destination directory if needed
        if let Some(parent) = self.dest_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Write file to destination
        tokio::fs::write(&self.dest_path, &file_data).await?;

        info!("File written successfully to: {}", self.dest_path.display());

        // Send completion acknowledgment
        let completion_response = SimpleSyncMessage::TransferComplete {
            success: true,
            checksum_verified: true,
            message: "File transfer completed successfully".to_string(),
        };
        completion_response.send(writer).await?;

        info!("File transfer completed: {}", self.dest_path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::Cursor;

    #[tokio::test]
    async fn test_message_serialization() {
        let msg = SimpleSyncMessage::FileTransferRequest {
            file_path: "test.txt".to_string(),
            file_size: 1024,
            checksum: "abcd1234".to_string(),
        };

        let bytes = msg.to_bytes().unwrap();
        let decoded = SimpleSyncMessage::from_bytes(&bytes).unwrap();

        match decoded {
            SimpleSyncMessage::FileTransferRequest {
                file_path,
                file_size,
                checksum,
            } => {
                assert_eq!(file_path, "test.txt");
                assert_eq!(file_size, 1024);
                assert_eq!(checksum, "abcd1234");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[tokio::test]
    async fn test_message_send_receive() {
        let msg = SimpleSyncMessage::FileTransferResponse {
            accepted: true,
            reason: None,
        };

        let mut buffer = Vec::new();
        msg.send(&mut buffer).await.unwrap();

        let mut cursor = Cursor::new(buffer);
        let received = SimpleSyncMessage::receive(&mut cursor).await.unwrap();

        match received {
            SimpleSyncMessage::FileTransferResponse { accepted, reason } => {
                assert!(accepted);
                assert!(reason.is_none());
            }
            _ => panic!("Wrong message type"),
        }
    }
}
