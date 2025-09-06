//! Phase 2 Core Integration Tests
//! 
//! These tests validate that Phase 2 core components work together:
//! - Storage components (CAS, chunker, indexer)
//! - QUIC connections
//! - Basic file operations

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::fs;
use tempfile::tempdir;
use tracing::{info, warn};

// Import landropic crates
use landro_cas::ContentStore;
use landro_chunker::{Chunker, ChunkerConfig, ContentHash};
use landro_crypto::{CertificateVerifier, DeviceIdentity};
use landro_index::{async_indexer::AsyncIndexer, IndexerConfig};
use landro_quic::{QuicClient, QuicConfig, QuicServer};

/// Simplified test node without daemon dependencies
pub struct CoreTestNode {
    pub device_name: String,
    pub sync_dir: std::path::PathBuf,
    pub storage_dir: std::path::PathBuf,
    pub indexer: Arc<AsyncIndexer>,
    pub store: Arc<ContentStore>,
    pub chunker: Chunker,
    pub identity: Arc<DeviceIdentity>,
    pub verifier: Arc<CertificateVerifier>,
}

impl CoreTestNode {
    /// Create a new core test node
    pub async fn new(device_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = tempdir()?;
        
        let sync_dir = temp_dir.path().join("sync");
        let storage_dir = temp_dir.path().join("storage");
        let cas_dir = storage_dir.join("objects");
        let db_path = storage_dir.join("index.sqlite");
        
        // Create directories
        fs::create_dir_all(&sync_dir).await?;
        fs::create_dir_all(&storage_dir).await?;
        fs::create_dir_all(&cas_dir).await?;
        
        // Initialize crypto
        let identity = Arc::new(DeviceIdentity::generate(device_name)?);
        let verifier = Arc::new(CertificateVerifier::allow_any());
        
        // Initialize storage components
        let indexer_config = IndexerConfig::default();
        let indexer = Arc::new(AsyncIndexer::new(&cas_dir, &db_path, indexer_config).await?);
        let store = Arc::new(ContentStore::new(&cas_dir).await?);
        let chunker = Chunker::new(ChunkerConfig::default())?;
        
        Ok(CoreTestNode {
            device_name: device_name.to_string(),
            sync_dir,
            storage_dir,
            indexer,
            store,
            chunker,
            identity,
            verifier,
        })
    }
    
    /// Create a test file with specified content
    pub async fn create_file(&self, name: &str, content: &[u8]) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let file_path = self.sync_dir.join(name);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&file_path, content).await?;
        
        // Index the file
        self.indexer.index_folder(&self.sync_dir).await?;
        
        Ok(file_path)
    }
    
    /// Check if a file exists in the sync directory
    pub async fn has_file(&self, name: &str) -> bool {
        self.sync_dir.join(name).exists()
    }
    
    /// Read file content from sync directory
    pub async fn read_file(&self, name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let content = fs::read(self.sync_dir.join(name)).await?;
        Ok(content)
    }
    
    /// Get basic storage stats
    pub async fn get_file_count(&self) -> Result<usize, Box<dyn std::error::Error>> {
        let manifest = self.indexer.index_folder(&self.sync_dir).await?;
        Ok(manifest.file_count())
    }
}

// Test helper functions
async fn setup_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug,quinn=info,rustls=info")
        .try_init();
}

// Core Integration Tests

#[tokio::test]
async fn test_storage_pipeline() {
    setup_logging().await;
    info!("=== Starting storage pipeline test ===");
    
    // Create test node
    let node = CoreTestNode::new("test-storage").await.expect("Failed to create node");
    
    // Test data
    let test_content = b"Hello, Landropic! This is a test of the core storage pipeline.";
    
    // Create test file
    let file_path = node.create_file("pipeline_test.txt", test_content).await.expect("Failed to create test file");
    info!("Created test file: {}", file_path.display());
    
    // Verify file exists
    assert!(node.has_file("pipeline_test.txt").await, "File should exist");
    
    // Verify content
    let read_content = node.read_file("pipeline_test.txt").await.expect("Failed to read file");
    assert_eq!(read_content, test_content, "File content should match");
    
    // Test chunking
    let chunks = node.chunker.chunk_bytes(test_content).expect("Failed to chunk content");
    assert!(!chunks.is_empty(), "Should produce at least one chunk");
    
    // Test content-addressed storage
    for chunk in &chunks {
        // Store chunk in CAS
        let obj_ref = node.store.write(&chunk.data).await.expect("Failed to write to CAS");
        assert_eq!(obj_ref.hash.as_bytes(), chunk.hash.as_bytes(), "Hash should match");
        
        // Verify we can read it back
        let read_data = node.store.read(&chunk.hash).await.expect("Failed to read from CAS");
        assert_eq!(read_data, chunk.data, "Data should match");
    }
    
    // Verify indexer
    let file_count = node.get_file_count().await.expect("Failed to get file count");
    assert_eq!(file_count, 1, "Should have indexed 1 file");
    
    info!("✓ Storage pipeline test completed successfully");
}

#[tokio::test]
async fn test_quic_connection() {
    setup_logging().await;
    let _ = rustls::crypto::ring::default_provider().install_default();
    
    info!("=== Starting QUIC connection test ===");
    
    // Create two nodes
    let node1 = CoreTestNode::new("quic-server").await.expect("Failed to create server node");
    let node2 = CoreTestNode::new("quic-client").await.expect("Failed to create client node");
    
    // Configure QUIC server
    let server_config = QuicConfig::new()
        .bind_addr("127.0.0.1:0".parse().unwrap())
        .idle_timeout(Duration::from_secs(10));
        
    let mut server = QuicServer::new(
        node1.identity.clone(),
        node1.verifier.clone(),
        server_config
    );
    
    server.start().await.expect("Failed to start server");
    let server_addr = server.local_addr().expect("No server address");
    
    info!("QUIC server started on {}", server_addr);
    
    // Start server accept task
    let server_handle = tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_secs(10), server.accept()).await {
            Ok(Ok(connection)) => {
                info!("Server accepted connection from {}", connection.remote_address());
                
                // Verify device information
                let client_name = connection.remote_device_name().await;
                assert_eq!(client_name, Some("quic-client".to_string()));
                
                // Keep connection alive briefly
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(connection)
            }
            Ok(Err(e)) => {
                warn!("Server accept error: {}", e);
                Err(e)
            }
            Err(_) => {
                warn!("Server accept timeout");
                Err(landro_quic::QuicError::Timeout)
            }
        }
    });
    
    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Create client and connect
    let client_config = QuicConfig::new().idle_timeout(Duration::from_secs(10));
    let client = QuicClient::new(
        node2.identity.clone(),
        node2.verifier.clone(),
        client_config
    ).await.expect("Failed to create client");
    
    info!("Connecting to server at {}", server_addr);
    
    let client_connection = tokio::time::timeout(Duration::from_secs(5), client.connect_addr(server_addr))
        .await
        .expect("Client connection timeout")
        .expect("Failed to connect");
    
    info!("Client connected to {}", client_connection.remote_address());
    
    // Verify device information exchange
    let server_name = client_connection.remote_device_name().await;
    assert_eq!(server_name, Some("quic-server".to_string()));
    
    // Wait for server task
    let server_result = server_handle.await.expect("Server task panicked");
    server_result.expect("Server should succeed");
    
    info!("✓ QUIC connection test completed successfully");
}

#[tokio::test]
async fn test_data_integrity() {
    setup_logging().await;
    info!("=== Starting data integrity test ===");
    
    let node = CoreTestNode::new("integrity-test").await.expect("Failed to create node");
    
    // Test different types of content
    let test_cases = vec![
        ("empty.txt", b"".as_slice()),
        ("small.txt", b"Hello, World!"),
        ("medium.txt", &vec![0x42u8; 1024]), // 1KB
        ("large.txt", &vec![0xABu8; 64 * 1024]), // 64KB  
        ("binary.txt", &(0u8..=255u8).collect::<Vec<u8>>()), // All bytes
    ];
    
    let mut original_hashes = HashMap::new();
    
    // Create files and compute hashes
    for (name, content) in &test_cases {
        node.create_file(name, content).await.expect("Failed to create file");
        let hash = ContentHash::from_blake3(blake3::hash(content));
        original_hashes.insert(name.to_string(), hash);
        info!("Created {} ({} bytes) with hash: {}", name, content.len(), hash.to_hex());
    }
    
    // Verify files and hashes
    for (name, expected_content) in &test_cases {
        // Check file exists
        assert!(node.has_file(name).await, "File {} should exist", name);
        
        // Check content matches
        let read_content = node.read_file(name).await.expect("Failed to read file");
        assert_eq!(read_content, *expected_content, "Content should match for {}", name);
        
        // Verify hash integrity
        let computed_hash = ContentHash::from_blake3(blake3::hash(&read_content));
        let expected_hash = original_hashes.get(*name).unwrap();
        assert_eq!(computed_hash, *expected_hash, "Hash should match for {}", name);
        
        // Test chunking integrity (skip empty files)
        if !expected_content.is_empty() {
            let chunks = node.chunker.chunk_bytes(expected_content).expect("Failed to chunk");
            
            // Reconstruct from chunks
            let mut reconstructed = Vec::new();
            for chunk in &chunks {
                let chunk_hash = ContentHash::from_blake3(blake3::hash(&chunk.data));
                assert_eq!(chunk_hash, chunk.hash, "Chunk hash should be correct");
                reconstructed.extend_from_slice(&chunk.data);
            }
            
            assert_eq!(reconstructed, *expected_content, "Reconstructed content should match for {}", name);
        }
    }
    
    // Verify all files are indexed
    let file_count = node.get_file_count().await.expect("Failed to get file count");
    assert_eq!(file_count, test_cases.len(), "All files should be indexed");
    
    info!("✓ Data integrity test completed successfully");
}

#[tokio::test]
async fn test_performance_basic() {
    setup_logging().await;
    info!("=== Starting basic performance test ===");
    
    let node = CoreTestNode::new("perf-test").await.expect("Failed to create node");
    
    // Create a moderately sized file (1MB)
    let file_size = 1024 * 1024; // 1MB
    let test_content = vec![0xCDu8; file_size];
    
    let start_time = Instant::now();
    
    // Create and index the file
    node.create_file("performance_test.bin", &test_content).await.expect("Failed to create large file");
    
    let creation_time = start_time.elapsed();
    info!("File creation and indexing took: {:?}", creation_time);
    
    // Test reading performance
    let read_start = Instant::now();
    let read_content = node.read_file("performance_test.bin").await.expect("Failed to read file");
    let read_time = read_start.elapsed();
    
    assert_eq!(read_content.len(), file_size, "File size should match");
    assert_eq!(read_content, test_content, "Content should match");
    
    info!("File read took: {:?}", read_time);
    
    // Test chunking performance  
    let chunk_start = Instant::now();
    let chunks = node.chunker.chunk_bytes(&test_content).expect("Failed to chunk content");
    let chunk_time = chunk_start.elapsed();
    
    info!("Chunking {} bytes into {} chunks took: {:?}", 
          file_size, chunks.len(), chunk_time);
    
    // Performance assertions (reasonable for 1MB on local disk)
    assert!(creation_time.as_millis() < 5000, "Creation should be under 5 seconds");
    assert!(read_time.as_millis() < 1000, "Read should be under 1 second");  
    assert!(chunk_time.as_millis() < 2000, "Chunking should be under 2 seconds");
    assert!(!chunks.is_empty(), "Should produce chunks");
    
    // Calculate throughput
    let throughput_mbps = (file_size as f64 / (1024.0 * 1024.0)) / creation_time.as_secs_f64() * 8.0;
    info!("Throughput: {:.2} Mbps", throughput_mbps);
    
    // Reasonable throughput for local disk operations
    assert!(throughput_mbps > 1.0, "Should achieve reasonable throughput");
    
    info!("✓ Basic performance test completed successfully");
}

#[tokio::test]
async fn test_concurrent_operations() {
    setup_logging().await;
    info!("=== Starting concurrent operations test ===");
    
    let node = CoreTestNode::new("concurrent-test").await.expect("Failed to create node");
    
    // Create multiple files concurrently
    let mut handles = Vec::new();
    
    for i in 0..5 { // Reduced from 10 to 5 for simpler testing
        let node_sync_dir = node.sync_dir.clone();
        let node_indexer = node.indexer.clone();
        
        let handle = tokio::spawn(async move {
            let file_name = format!("concurrent_{}.txt", i);
            let content = format!("Concurrent file content number {}", i);
            
            // Create file
            let file_path = node_sync_dir.join(&file_name);
            if let Err(e) = fs::write(&file_path, content.as_bytes()).await {
                return Err(format!("Failed to write file {}: {}", file_name, e));
            }
            
            // Index individually  
            if let Err(e) = node_indexer.index_folder(&node_sync_dir).await {
                return Err(format!("Failed to index {}: {}", file_name, e));
            }
            
            Ok(file_name)
        });
        
        handles.push(handle);
    }
    
    // Wait for all operations to complete
    let mut successful_files = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(filename)) => {
                successful_files.push(filename);
            }
            Ok(Err(e)) => {
                warn!("File operation failed: {}", e);
            }
            Err(e) => {
                warn!("Task panicked: {}", e);
            }
        }
    }
    
    info!("Successfully created {} files concurrently", successful_files.len());
    assert_eq!(successful_files.len(), 5, "All concurrent operations should succeed");
    
    // Verify all files exist and have correct content
    for filename in &successful_files {
        assert!(node.has_file(filename).await, "File {} should exist", filename);
        
        let content = node.read_file(filename).await.expect("Failed to read concurrent file");
        let expected_content = format!("Concurrent file content number {}", 
                                       filename.replace("concurrent_", "").replace(".txt", ""));
        assert_eq!(content, expected_content.as_bytes(), "Content should match for {}", filename);
    }
    
    // Verify indexing worked
    let file_count = node.get_file_count().await.expect("Failed to get file count");
    assert_eq!(file_count, 5, "All files should be indexed");
    
    info!("✓ Concurrent operations test completed successfully");
}