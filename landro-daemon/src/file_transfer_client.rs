//! Simple file transfer client for testing the alpha sync functionality

use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info};
use quinn::{Connection, Endpoint};

use crate::simple_sync_protocol::{SimpleSyncMessage, SimpleFileTransfer};
use landro_crypto::{DeviceIdentity, CertificateVerifier};
use landro_quic::{QuicClient, QuicConfig};

/// Simple client for transferring files to a daemon
pub struct FileTransferClient {
    client: Arc<QuicClient>,
}

impl FileTransferClient {
    /// Create a new file transfer client
    pub async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let identity = Arc::new(DeviceIdentity::generate("file-transfer-client")?);
        let verifier = Arc::new(CertificateVerifier::for_pairing()); // Accept any certificate for testing
        let config = QuicConfig::file_sync_optimized();
        
        let client = Arc::new(QuicClient::new(identity, verifier, config).await?);
        
        Ok(Self { client })
    }
    
    /// Transfer a file to the specified daemon
    pub async fn transfer_file(
        &self,
        target_addr: std::net::SocketAddr,
        file_path: PathBuf,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Connecting to daemon at {}", target_addr);
        
        // Connect to the daemon
        let connection = self.client.connect(target_addr).await?;
        info!("Connected to daemon");
        
        // Open bidirectional stream for sync protocol
        let (mut send_stream, mut recv_stream) = connection.open_bi().await?;
        
        // Create SimpleFileTransfer and use it
        let transfer = SimpleFileTransfer::new(file_path.clone(), PathBuf::new()); // Dest path not used for sender
        transfer.send_file(&mut send_stream, &mut recv_stream).await
    }
}

/// Command-line interface for file transfers
pub struct FileTransferCli;

impl FileTransferCli {
    /// Parse command line arguments and execute file transfer
    pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let args: Vec<String> = std::env::args().collect();
        
        if args.len() < 3 {
            eprintln!("Usage: {} <daemon_host:port> <file_path>", args[0]);
            eprintln!("Example: {} 127.0.0.1:9876 /path/to/file.txt", args[0]);
            std::process::exit(1);
        }
        
        let daemon_addr = args[1].parse::<std::net::SocketAddr>()
            .map_err(|_| "Invalid daemon address. Use format: host:port")?;
        let file_path = PathBuf::from(&args[2]);
        
        if !file_path.exists() {
            return Err(format!("File does not exist: {}", file_path.display()).into());
        }
        
        if !file_path.is_file() {
            return Err(format!("Path is not a file: {}", file_path.display()).into());
        }
        
        info!("Starting file transfer client");
        info!("Target daemon: {}", daemon_addr);
        info!("File to transfer: {}", file_path.display());
        
        let client = FileTransferClient::new().await?;
        let start_time = std::time::Instant::now();
        
        client.transfer_file(daemon_addr, file_path).await?;
        
        let duration = start_time.elapsed();
        info!("File transfer completed in {:.2} seconds", duration.as_secs_f64());
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_client_creation() {
        let result = FileTransferClient::new().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_file_transfer_preparation() {
        // Create a temporary file for testing
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Hello, World!").await.unwrap();
        let temp_path = temp_file.path().to_path_buf();
        
        // Verify file exists and can be read
        let file_data = tokio::fs::read(&temp_path).await.unwrap();
        assert_eq!(file_data, b"Hello, World!");
        
        let checksum = blake3::hash(&file_data);
        let checksum_hex = hex::encode(checksum.as_bytes());
        assert!(!checksum_hex.is_empty());
    }
}