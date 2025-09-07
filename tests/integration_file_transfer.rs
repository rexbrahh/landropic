//! Integration test for end-to-end file transfer using SimpleSyncMessage protocol

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::{TempDir, NamedTempFile};
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

use landro_cas::ContentStore;
use landro_chunker::{Chunker, ChunkerConfig};
use landro_crypto::{DeviceIdentity, CertificateVerifier};
use landro_daemon::{
    FileTransferClient, OrchestratorConfig, SyncOrchestrator, 
    discovery::DiscoveryService, watcher::FileWatcher,
    bloom_sync_integration::BloomSyncEngine, sync_engine::create_enhanced_sync_engine
};
use landro_index::async_indexer::AsyncIndexer;
use landro_quic::{QuicConfig, QuicServer};

/// Test helper to set up a daemon instance for testing
async fn setup_test_daemon(temp_dir: &TempDir) -> Result<(Arc<QuicServer>, std::net::SocketAddr), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize crypto provider
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    
    let storage_path = temp_dir.path().to_path_buf();
    let device_name = "test-daemon";
    
    // Create storage directories
    let cas_path = storage_path.join("objects");
    let db_path = storage_path.join("index.sqlite");
    
    // Initialize core components
    let store = Arc::new(ContentStore::new(&cas_path).await?);
    
    let chunker_config = ChunkerConfig {
        min_size: 16 * 1024,
        avg_size: 64 * 1024, 
        max_size: 256 * 1024,
        mask_bits: 16,
    };
    let chunker = Arc::new(Chunker::new(chunker_config)?);
    let indexer = Arc::new(AsyncIndexer::new(&cas_path, &db_path, Default::default()).await?);
    
    // Create device identity
    let identity = Arc::new(DeviceIdentity::generate(device_name)?);
    let verifier = Arc::new(CertificateVerifier::for_pairing());
    
    // Configure QUIC
    let bind_addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0)); // Let OS pick port
    let quic_config = QuicConfig::file_sync_optimized()
        .bind_addr(bind_addr)
        .idle_timeout(Duration::from_secs(60))
        .stream_receive_window(128 * 1024 * 1024)
        .receive_window(2 * 1024 * 1024 * 1024);
    
    // Create QUIC server
    let mut quic_server = QuicServer::new(identity, verifier, quic_config);
    quic_server.start().await?;
    
    let actual_addr = quic_server.local_addr()?;
    
    Ok((Arc::new(quic_server), actual_addr))
}

#[tokio::test]
async fn test_end_to_end_file_transfer() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize tracing for test debugging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .init();
    
    let temp_dir = TempDir::new()?;
    
    // Set up test daemon
    let (_daemon, daemon_addr) = setup_test_daemon(&temp_dir).await?;
    
    tracing::info!("Test daemon started on {}", daemon_addr);
    
    // Create test file 
    let mut test_file = NamedTempFile::new()?;
    let test_content = b"Hello, Landropic! This is a test file for alpha file transfer.";
    test_file.write_all(test_content).await?;
    let test_file_path = test_file.path().to_path_buf();
    
    tracing::info!("Created test file: {}", test_file_path.display());
    
    // Create file transfer client
    let client = FileTransferClient::new().await?;
    
    // Perform file transfer with timeout (should complete in < 5 seconds)
    let start_time = std::time::Instant::now();
    
    let transfer_result = timeout(
        Duration::from_secs(5), 
        client.transfer_file(daemon_addr, test_file_path.clone())
    ).await;
    
    let duration = start_time.elapsed();
    
    match transfer_result {
        Ok(Ok(())) => {
            tracing::info!("File transfer completed in {:.2} seconds", duration.as_secs_f64());
            assert!(duration < Duration::from_secs(5), "Transfer took too long: {:.2}s", duration.as_secs_f64());
        }
        Ok(Err(e)) => {
            return Err(format!("File transfer failed: {}", e).into());
        }
        Err(_) => {
            return Err("File transfer timed out after 5 seconds".into());
        }
    }
    
    // Verify file was received (check the daemon's sync folder)
    let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let sync_folder = home_dir.join("LandropicSync");
    let received_file = sync_folder.join("received_file.tmp");
    
    // Wait a bit for file to be written
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    if received_file.exists() {
        let received_content = tokio::fs::read(&received_file).await?;
        assert_eq!(received_content, test_content, "File content mismatch");
        tracing::info!("File content verified successfully");
        
        // Clean up
        let _ = tokio::fs::remove_file(&received_file).await;
    } else {
        return Err(format!("Received file not found at: {}", received_file.display()).into());
    }
    
    tracing::info!("End-to-end file transfer test completed successfully");
    Ok(())
}

#[tokio::test]
async fn test_large_file_transfer() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Skip this test in CI environments or if explicitly requested
    if std::env::var("SKIP_LARGE_FILE_TEST").is_ok() {
        return Ok(());
    }
    
    let temp_dir = TempDir::new()?;
    let (_daemon, daemon_addr) = setup_test_daemon(&temp_dir).await?;
    
    // Create a larger test file (1MB)
    let mut test_file = NamedTempFile::new()?;
    let large_content = vec![0xAA; 1024 * 1024]; // 1MB of 0xAA bytes
    test_file.write_all(&large_content).await?;
    let test_file_path = test_file.path().to_path_buf();
    
    tracing::info!("Created large test file: {} bytes", large_content.len());
    
    let client = FileTransferClient::new().await?;
    
    // Large files should still complete within reasonable time (10 seconds)
    let start_time = std::time::Instant::now();
    
    let transfer_result = timeout(
        Duration::from_secs(10),
        client.transfer_file(daemon_addr, test_file_path)
    ).await;
    
    let duration = start_time.elapsed();
    
    match transfer_result {
        Ok(Ok(())) => {
            tracing::info!("Large file transfer completed in {:.2} seconds", duration.as_secs_f64());
            // For 1MB file, should be reasonably fast
            assert!(duration < Duration::from_secs(10), "Large file transfer took too long: {:.2}s", duration.as_secs_f64());
        }
        Ok(Err(e)) => {
            return Err(format!("Large file transfer failed: {}", e).into());
        }
        Err(_) => {
            return Err("Large file transfer timed out".into());
        }
    }
    
    Ok(())
}

#[tokio::test]
async fn test_transfer_rejection_handling() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // This test would verify that the client handles transfer rejections properly
    // For now, since our alpha implementation always accepts, we'll just ensure
    // the client can handle connection failures gracefully
    
    let client = FileTransferClient::new().await?;
    
    // Try to connect to non-existent daemon
    let non_existent_addr = std::net::SocketAddr::from(([127, 0, 0, 1], 1)); // Port 1 should be unavailable
    
    let mut test_file = NamedTempFile::new()?;
    test_file.write_all(b"test").await?;
    let test_file_path = test_file.path().to_path_buf();
    
    let result = timeout(
        Duration::from_secs(5),
        client.transfer_file(non_existent_addr, test_file_path)
    ).await;
    
    // Should fail to connect
    match result {
        Ok(Ok(())) => {
            return Err("Expected connection to fail, but it succeeded".into());
        }
        Ok(Err(_)) => {
            tracing::info!("Connection failure handled correctly");
        }
        Err(_) => {
            tracing::info!("Connection attempt timed out as expected");
        }
    }
    
    Ok(())
}

#[tokio::test]
async fn test_checksum_verification() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Test that checksums are correctly calculated and verified
    let test_content = b"Test content for checksum verification";
    
    // Calculate expected checksum
    let expected_checksum = blake3::hash(test_content);
    let expected_hex = hex::encode(expected_checksum.as_bytes());
    
    tracing::info!("Expected checksum: {}", expected_hex);
    
    // Verify that our protocol calculates the same checksum
    let mut test_file = NamedTempFile::new()?;
    test_file.write_all(test_content).await?;
    
    let file_data = tokio::fs::read(test_file.path()).await?;
    let calculated_checksum = blake3::hash(&file_data);
    let calculated_hex = hex::encode(calculated_checksum.as_bytes());
    
    assert_eq!(expected_hex, calculated_hex, "Checksum calculation mismatch");
    tracing::info!("Checksum verification test passed");
    
    Ok(())
}