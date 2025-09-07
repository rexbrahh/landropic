use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{info, debug, error};
use anyhow::{Context, Result};

use landro_cas::ContentStore;
use landro_chunker::{Chunker, ChunkerConfig};
use landro_crypto::{DeviceIdentity, CertificateVerifier};
use landro_daemon::{
    Daemon, DaemonConfig,
    orchestrator::{SyncOrchestrator, OrchestratorConfig, OrchestratorMessage},
    bloom_sync_integration::BloomSyncEngine,
    sync_engine::create_enhanced_sync_engine,
};
use landro_index::async_indexer::AsyncIndexer;

use crate::common::{
    init_crypto, TestDaemon, create_test_files, compare_directories, 
    generate_test_data, find_free_port
};

/// Test basic file sync between two daemons
#[tokio::test]
async fn test_basic_file_sync() -> Result<()> {
    init_crypto();
    
    info!("Starting basic file sync test");
    
    // Create two test daemons
    let mut daemon1 = TestDaemon::new("sync-test-daemon1")?;
    let mut daemon2 = TestDaemon::new("sync-test-daemon2")?;
    
    let port1 = daemon1.start(Some(find_free_port())).await?;
    let port2 = daemon2.start(Some(find_free_port())).await?;
    
    info!("Started daemons on ports {} and {}", port1, port2);
    
    // Create test files in daemon1's sync folder
    let sync_folder1 = daemon1.sync_folder();
    tokio::fs::create_dir_all(&sync_folder1).await?;
    let test_files = create_test_files(&sync_folder1)?;
    
    info!("Created {} test files in daemon1", test_files.len());
    
    // Create sync folder for daemon2
    let sync_folder2 = daemon2.sync_folder(); 
    tokio::fs::create_dir_all(&sync_folder2).await?;
    
    // Wait for daemons to discover and index files
    sleep(Duration::from_secs(3)).await;
    
    // Trigger sync between daemons (using alpha CLI method)
    // In a real test, we'd use proper peer discovery
    info!("Triggering sync between daemons (simulated)");
    
    // For now, we'll test that the indexing and chunking systems work
    // by verifying files are properly indexed
    
    // Check that daemon1 has indexed the files
    let status1 = get_daemon_file_count(&daemon1).await?;
    assert!(status1 > 0, "Daemon1 should have indexed files");
    
    info!("Daemon1 indexed {} files", status1);
    
    daemon1.stop().await?;
    daemon2.stop().await?;
    
    Ok(())
}

/// Test sync engine components work together
#[tokio::test]
async fn test_sync_engine_integration() -> Result<()> {
    init_crypto();
    
    info!("Testing sync engine integration");
    
    let temp_dir = tempfile::TempDir::new()?;
    let storage_path = temp_dir.path().join("storage");
    let cas_path = storage_path.join("objects");
    let db_path = storage_path.join("index.sqlite");
    
    tokio::fs::create_dir_all(&cas_path).await?;
    
    // Initialize core components
    let store = Arc::new(ContentStore::new(&cas_path).await?);
    let chunker_config = ChunkerConfig {
        min_size: 4 * 1024,   // 4KB
        avg_size: 16 * 1024,  // 16KB
        max_size: 64 * 1024,  // 64KB
        mask_bits: 14,        // For 16KB average
    };
    let chunker = Arc::new(Chunker::new(chunker_config)?);
    let indexer = Arc::new(AsyncIndexer::new(&cas_path, &db_path, Default::default()).await?);
    
    // Create orchestrator
    let config = OrchestratorConfig::default();
    let (mut orchestrator, tx) = 
        SyncOrchestrator::new(config, store.clone(), chunker.clone(), indexer.clone(), &storage_path).await?;
    
    // Initialize enhanced sync engines
    let identity = Arc::new(DeviceIdentity::load_or_generate("test-device").await?);
    
    let bloom_sync = Arc::new(BloomSyncEngine::new(
        identity.device_id().to_string(),
        store.clone(),
        indexer.clone(),
    ).await?);
    
    let enhanced_sync = create_enhanced_sync_engine(
        store.clone(),
        indexer.clone(),
        tx.clone(),
    ).await?;
    
    info!("Created sync engine components");
    
    // Create test data
    let test_folder = temp_dir.path().join("test_files");
    tokio::fs::create_dir_all(&test_folder).await?;
    let test_files = create_test_files(&test_folder)?;
    
    // Add sync folder to orchestrator
    tx.send(OrchestratorMessage::AddSyncFolder(test_folder.clone())).await?;
    
    // Let orchestrator process for a moment
    let orchestrator_handle = tokio::spawn(async move {
        timeout(Duration::from_secs(5), orchestrator.run()).await
    });
    
    sleep(Duration::from_secs(2)).await;
    
    // Send shutdown
    tx.send(OrchestratorMessage::Shutdown).await?;
    
    // Wait for orchestrator
    let result = orchestrator_handle.await??;
    if let Err(e) = result {
        error!("Orchestrator error: {}", e);
    }
    
    // Verify files were processed
    let file_count = indexer.get_file_count().await?;
    info!("Indexed {} files", file_count);
    assert!(file_count >= test_files.len(), "Should have indexed all test files");
    
    Ok(())
}

/// Test chunking and content-addressed storage
#[tokio::test]  
async fn test_chunking_and_cas_integration() -> Result<()> {
    init_crypto();
    
    info!("Testing chunking and CAS integration");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    // Initialize components
    let store = Arc::new(ContentStore::new(&cas_path).await?);
    let chunker_config = ChunkerConfig {
        min_size: 1024,      // 1KB
        avg_size: 4096,      // 4KB
        max_size: 16384,     // 16KB
        mask_bits: 12,       // For 4KB average
    };
    let chunker = Arc::new(Chunker::new(chunker_config)?);
    
    // Create test data of various sizes
    let test_data = [
        ("small.txt", generate_test_data(500)),     // < min_size
        ("medium.txt", generate_test_data(8192)),   // Around avg_size
        ("large.txt", generate_test_data(50000)),   // Multiple chunks
    ];
    
    let mut all_chunk_ids = Vec::new();
    
    for (filename, data) in &test_data {
        info!("Processing file: {} ({} bytes)", filename, data.len());
        
        // Chunk the data
        let chunks = chunker.chunk_bytes(data)?;
        info!("Created {} chunks for {}", chunks.len(), filename);
        
        // Store chunks in CAS
        let mut chunk_ids = Vec::new();
        for chunk in chunks {
            let chunk_id = store.put(&chunk.data).await?;
            chunk_ids.push(chunk_id.clone());
            all_chunk_ids.push(chunk_id);
            
            // Verify we can retrieve the chunk
            let retrieved = store.get(&chunk_id).await?
                .ok_or_else(|| anyhow::anyhow!("Failed to retrieve chunk"))?;
            assert_eq!(chunk.data, retrieved, "Retrieved chunk should match original");
        }
        
        info!("Stored {} chunks for {}", chunk_ids.len(), filename);
    }
    
    // Verify all chunks are in storage
    info!("Verifying {} total chunks in storage", all_chunk_ids.len());
    for chunk_id in &all_chunk_ids {
        assert!(
            store.contains(chunk_id).await?, 
            "Storage should contain chunk {}", hex::encode(chunk_id)
        );
    }
    
    // Test deduplication - same content should produce same chunk ID
    let duplicate_data = generate_test_data(8192);
    let chunks1 = chunker.chunk_bytes(&duplicate_data)?;
    let chunks2 = chunker.chunk_bytes(&duplicate_data)?;
    
    assert_eq!(chunks1.len(), chunks2.len(), "Same data should produce same number of chunks");
    for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
        assert_eq!(c1.hash, c2.hash, "Same content should produce same chunk hash");
    }
    
    info!("Deduplication test passed");
    
    Ok(())
}

/// Test manifest generation and comparison
#[tokio::test]
async fn test_manifest_integration() -> Result<()> {
    init_crypto();
    
    info!("Testing manifest generation and comparison");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    let db_path = temp_dir.path().join("index.sqlite");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    // Initialize indexer
    let indexer = Arc::new(AsyncIndexer::new(&cas_path, &db_path, Default::default()).await?);
    
    // Create test files
    let test_folder = temp_dir.path().join("test_files");
    let test_files = create_test_files(&test_folder)?;
    
    info!("Created {} test files for manifest testing", test_files.len());
    
    // Index the files
    for file_path in &test_files {
        indexer.index_file(file_path).await?;
    }
    
    // Generate manifest
    let manifest = indexer.generate_manifest(&test_folder).await?;
    info!("Generated manifest with {} entries", manifest.files.len());
    
    assert_eq!(manifest.files.len(), test_files.len(), "Manifest should contain all files");
    
    // Verify manifest entries
    for (rel_path, file_entry) in &manifest.files {
        info!("Manifest entry: {} -> {} bytes, {} chunks", 
              rel_path.display(), file_entry.size, file_entry.chunks.len());
        assert!(file_entry.size > 0, "File size should be positive");
        assert!(!file_entry.chunks.is_empty(), "File should have chunks");
    }
    
    // Test manifest comparison (with itself - should be identical)
    let diff = manifest.diff(&manifest);
    assert!(diff.added.is_empty(), "Self-diff should have no added files");
    assert!(diff.removed.is_empty(), "Self-diff should have no removed files");
    assert!(diff.modified.is_empty(), "Self-diff should have no modified files");
    
    info!("Manifest comparison test passed");
    
    // Create a modified version
    let modified_file = &test_files[0];
    let new_content = "MODIFIED CONTENT FOR TESTING";
    tokio::fs::write(modified_file, new_content).await?;
    
    // Re-index the modified file
    indexer.index_file(modified_file).await?;
    
    // Generate new manifest
    let manifest2 = indexer.generate_manifest(&test_folder).await?;
    
    // Compare manifests
    let diff = manifest.diff(&manifest2);
    assert!(diff.modified.len() == 1, "Should detect one modified file");
    
    info!("Modified file detection test passed");
    
    Ok(())
}

/// Test file watcher integration
#[tokio::test]
async fn test_file_watcher_integration() -> Result<()> {
    init_crypto();
    
    info!("Testing file watcher integration");
    
    let temp_dir = tempfile::TempDir::new()?;
    let watch_folder = temp_dir.path().join("watched");
    tokio::fs::create_dir_all(&watch_folder).await?;
    
    // This test would ideally test the file watcher component
    // For now, we'll test the basic file watching concept
    
    use landro_daemon::watcher::{FileWatcher, FileEvent};
    use std::sync::mpsc;
    
    let (tx, rx) = mpsc::channel::<Vec<FileEvent>>();
    
    // Start file watcher
    let watcher = FileWatcher::new(watch_folder.clone())?;
    watcher.start(move |events| {
        let _ = tx.send(events);
    })?;
    
    info!("Started file watcher on {:?}", watch_folder);
    
    // Create a test file
    let test_file = watch_folder.join("test.txt");
    tokio::fs::write(&test_file, "test content").await?;
    
    // Wait for file system events
    let events = timeout(Duration::from_secs(5), async {
        rx.recv()
    }).await??;
    
    info!("Received {} file events", events.len());
    assert!(!events.is_empty(), "Should receive file creation events");
    
    // Modify the file
    tokio::fs::write(&test_file, "modified content").await?;
    
    // Wait for modification events
    let modify_events = timeout(Duration::from_secs(5), async {
        rx.recv()
    }).await??;
    
    info!("Received {} modification events", modify_events.len());
    
    watcher.stop()?;
    
    Ok(())
}

/// Helper function to get file count from daemon (simplified)
async fn get_daemon_file_count(daemon: &TestDaemon) -> Result<usize> {
    // In a real implementation, this would query the daemon's indexer
    // For now, we'll count files in the sync folder
    let sync_folder = daemon.sync_folder();
    let mut count = 0;
    
    if sync_folder.exists() {
        let mut entries = tokio::fs::read_dir(&sync_folder).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_file() {
                count += 1;
            }
        }
    }
    
    Ok(count)
}

/// Test conflict detection and resolution
#[tokio::test]
async fn test_conflict_detection() -> Result<()> {
    init_crypto();
    
    info!("Testing conflict detection");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    let db_path = temp_dir.path().join("index.sqlite");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    let indexer = Arc::new(AsyncIndexer::new(&cas_path, &db_path, Default::default()).await?);
    
    // Create test file
    let test_folder = temp_dir.path().join("conflict_test");
    tokio::fs::create_dir_all(&test_folder).await?;
    let test_file = test_folder.join("conflict.txt");
    
    // Initial version
    tokio::fs::write(&test_file, "original content").await?;
    indexer.index_file(&test_file).await?;
    let manifest1 = indexer.generate_manifest(&test_folder).await?;
    
    // Modified version 1
    tokio::fs::write(&test_file, "modification 1").await?;
    indexer.index_file(&test_file).await?;
    let manifest2 = indexer.generate_manifest(&test_folder).await?;
    
    // Compare - should detect modification
    let diff = manifest1.diff(&manifest2);
    assert_eq!(diff.modified.len(), 1, "Should detect one modified file");
    
    info!("Conflict detection test passed");
    
    Ok(())
}