//! Integration test for the actor-based orchestrator
//!
//! Tests the complete pipeline: file watching → chunking → storage → sync

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

use landro_cas::ContentStore;
use landro_chunker::Chunker;
use landro_daemon::orchestrator::{
    OrchestratorConfig, OrchestratorMessage, OrchestratorStatus, SyncOrchestrator,
};
use landro_daemon::watcher::{FileEvent, FileEventKind};
use landro_index::async_indexer::AsyncIndexer;

#[tokio::test]
async fn test_orchestrator_file_pipeline() {
    // Create temporary directory for testing
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path().to_path_buf();

    // Initialize components
    let cas_path = storage_path.join("cas");
    let db_path = storage_path.join("index.sqlite");

    let store = Arc::new(ContentStore::open(&cas_path).await.unwrap());
    let chunker = Arc::new(Chunker::new(64 * 1024)); // 64KB chunks
    let indexer = Arc::new(
        AsyncIndexer::new(&cas_path, &db_path, Default::default())
            .await
            .unwrap(),
    );

    // Create orchestrator with fast debouncing for testing
    let mut config = OrchestratorConfig::default();
    config.debounce_duration = Duration::from_millis(100);
    config.auto_sync = false; // Disable auto-sync for this test

    let (orchestrator, sender) = SyncOrchestrator::new(
        config,
        store.clone(),
        chunker.clone(),
        indexer.clone(),
        &storage_path,
    )
    .await
    .unwrap();

    // Spawn orchestrator in background
    let orchestrator_handle = tokio::spawn(async move { orchestrator.run().await });

    // Test 1: Add a sync folder
    let test_folder = storage_path.join("test_sync");
    tokio::fs::create_dir(&test_folder).await.unwrap();

    sender
        .send(OrchestratorMessage::AddSyncFolder(test_folder.clone()))
        .await
        .unwrap();

    // Give time to process
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify folder was added
    let (status_tx, mut status_rx) = mpsc::channel(1);
    sender
        .send(OrchestratorMessage::GetStatus(status_tx))
        .await
        .unwrap();
    let status = status_rx.recv().await.unwrap();
    assert_eq!(status.synced_folders.len(), 1);
    assert!(status.synced_folders.contains(&test_folder));

    // Test 2: Create a file and verify it gets processed
    let test_file = test_folder.join("test.txt");
    let test_content = b"Hello, Landropic!";
    tokio::fs::write(&test_file, test_content).await.unwrap();

    // Send file change event
    let event = FileEvent {
        path: test_file.clone(),
        kind: FileEventKind::Created,
    };
    sender
        .send(OrchestratorMessage::FileChanged(event))
        .await
        .unwrap();

    // Wait for debouncing and processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify file was chunked and stored
    let chunks = chunker.chunk(test_content).unwrap();
    assert!(!chunks.is_empty());

    // Verify chunks are in CAS
    for chunk in &chunks {
        let hash = blake3::hash(&chunk.data);
        let stored = store.get(&landro_cas::Hash(hash.into())).await.unwrap();
        assert!(stored.is_some());
        assert_eq!(&stored.unwrap()[..], &chunk.data);
    }

    // Test 3: Batch file changes
    let mut batch_events = Vec::new();
    for i in 0..5 {
        let file_path = test_folder.join(format!("batch_{}.txt", i));
        tokio::fs::write(&file_path, format!("Content {}", i))
            .await
            .unwrap();

        batch_events.push(FileEvent {
            path: file_path,
            kind: FileEventKind::Created,
        });
    }

    sender
        .send(OrchestratorMessage::FileChangesBatch(batch_events))
        .await
        .unwrap();

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify all files were processed
    for i in 0..5 {
        let file_path = test_folder.join(format!("batch_{}.txt", i));
        let content = format!("Content {}", i);
        let chunks = chunker.chunk(content.as_bytes()).unwrap();

        for chunk in &chunks {
            let hash = blake3::hash(&chunk.data);
            let stored = store.get(&landro_cas::Hash(hash.into())).await.unwrap();
            assert!(stored.is_some());
        }
    }

    // Test 4: File modification
    let new_content = b"Modified content!";
    tokio::fs::write(&test_file, new_content).await.unwrap();

    let event = FileEvent {
        path: test_file.clone(),
        kind: FileEventKind::Modified,
    };
    sender
        .send(OrchestratorMessage::FileChanged(event))
        .await
        .unwrap();

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify new content was chunked and stored
    let new_chunks = chunker.chunk(new_content).unwrap();
    for chunk in &new_chunks {
        let hash = blake3::hash(&chunk.data);
        let stored = store.get(&landro_cas::Hash(hash.into())).await.unwrap();
        assert!(stored.is_some());
    }

    // Test 5: File deletion
    tokio::fs::remove_file(&test_file).await.unwrap();

    let event = FileEvent {
        path: test_file.clone(),
        kind: FileEventKind::Deleted,
    };
    sender
        .send(OrchestratorMessage::FileChanged(event))
        .await
        .unwrap();

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Test 6: Remove sync folder
    sender
        .send(OrchestratorMessage::RemoveSyncFolder(test_folder.clone()))
        .await
        .unwrap();

    // Give time to process
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify folder was removed
    let (status_tx, mut status_rx) = mpsc::channel(1);
    sender
        .send(OrchestratorMessage::GetStatus(status_tx))
        .await
        .unwrap();
    let status = status_rx.recv().await.unwrap();
    assert_eq!(status.synced_folders.len(), 0);

    // Shutdown orchestrator
    sender.send(OrchestratorMessage::Shutdown).await.unwrap();

    // Wait for graceful shutdown
    tokio::time::timeout(Duration::from_secs(5), orchestrator_handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn test_orchestrator_peer_discovery() {
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path().to_path_buf();

    // Initialize components
    let cas_path = storage_path.join("cas");
    let db_path = storage_path.join("index.sqlite");

    let store = Arc::new(ContentStore::open(&cas_path).await.unwrap());
    let chunker = Arc::new(Chunker::new(64 * 1024));
    let indexer = Arc::new(
        AsyncIndexer::new(&cas_path, &db_path, Default::default())
            .await
            .unwrap(),
    );

    let config = OrchestratorConfig::default();
    let (orchestrator, sender) =
        SyncOrchestrator::new(config, store, chunker, indexer, &storage_path)
            .await
            .unwrap();

    // Spawn orchestrator
    let orchestrator_handle = tokio::spawn(async move { orchestrator.run().await });

    // Simulate peer discovery
    let peer = landro_daemon::discovery::PeerInfo {
        device_id: "test-peer-001".to_string(),
        device_name: "Test Device".to_string(),
        addresses: vec!["192.168.1.100".to_string()],
        port: 9876,
        txt_records: vec!["sync=true".to_string()],
    };

    sender
        .send(OrchestratorMessage::PeerDiscovered(peer))
        .await
        .unwrap();

    // Give time to process
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check status shows peer
    let (status_tx, mut status_rx) = mpsc::channel(1);
    sender
        .send(OrchestratorMessage::GetStatus(status_tx))
        .await
        .unwrap();
    let status = status_rx.recv().await.unwrap();
    assert_eq!(status.active_peers, 1);

    // Simulate peer loss
    sender
        .send(OrchestratorMessage::PeerLost("test-peer-001".to_string()))
        .await
        .unwrap();

    // Give time to process
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify peer was removed
    let (status_tx, mut status_rx) = mpsc::channel(1);
    sender
        .send(OrchestratorMessage::GetStatus(status_tx))
        .await
        .unwrap();
    let status = status_rx.recv().await.unwrap();
    assert_eq!(status.active_peers, 0);

    // Shutdown
    sender.send(OrchestratorMessage::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), orchestrator_handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn test_orchestrator_concurrent_operations() {
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path().to_path_buf();

    // Initialize components
    let cas_path = storage_path.join("cas");
    let db_path = storage_path.join("index.sqlite");

    let store = Arc::new(ContentStore::open(&cas_path).await.unwrap());
    let chunker = Arc::new(Chunker::new(64 * 1024));
    let indexer = Arc::new(
        AsyncIndexer::new(&cas_path, &db_path, Default::default())
            .await
            .unwrap(),
    );

    let mut config = OrchestratorConfig::default();
    config.max_concurrent_chunks = 4;
    config.debounce_duration = Duration::from_millis(50);

    let (orchestrator, sender) = SyncOrchestrator::new(
        config,
        store.clone(),
        chunker.clone(),
        indexer,
        &storage_path,
    )
    .await
    .unwrap();

    // Spawn orchestrator
    let orchestrator_handle = tokio::spawn(async move { orchestrator.run().await });

    // Create test folder
    let test_folder = storage_path.join("concurrent_test");
    tokio::fs::create_dir(&test_folder).await.unwrap();
    sender
        .send(OrchestratorMessage::AddSyncFolder(test_folder.clone()))
        .await
        .unwrap();

    // Send many file changes concurrently
    let mut tasks = Vec::new();
    for i in 0..20 {
        let sender = sender.clone();
        let test_folder = test_folder.clone();

        let task = tokio::spawn(async move {
            let file_path = test_folder.join(format!("concurrent_{}.txt", i));
            tokio::fs::write(&file_path, format!("Concurrent content {}", i))
                .await
                .unwrap();

            let event = FileEvent {
                path: file_path,
                kind: FileEventKind::Created,
            };
            sender
                .send(OrchestratorMessage::FileChanged(event))
                .await
                .unwrap();
        });

        tasks.push(task);
    }

    // Wait for all tasks to complete
    for task in tasks {
        task.await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify all files were processed
    for i in 0..20 {
        let content = format!("Concurrent content {}", i);
        let chunks = chunker.chunk(content.as_bytes()).unwrap();

        for chunk in &chunks {
            let hash = blake3::hash(&chunk.data);
            let stored = store.get(&landro_cas::Hash(hash.into())).await.unwrap();
            assert!(stored.is_some(), "Chunk for file {} not found in CAS", i);
        }
    }

    // Shutdown
    sender.send(OrchestratorMessage::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), orchestrator_handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
