//! Integration tests for sync functionality

use landro_index::{FileIndexer, FolderWatcher, IndexerConfig, WatcherConfig};
use landro_proto::{SyncProtocolHandler, SyncState};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::fs;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};

// Import prost_types for timestamp creation
extern crate prost_types;

#[tokio::test]
async fn test_folder_indexing_and_manifest_creation() {
    // Create temporary directories for testing
    let temp_dir = tempdir().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let db_path = temp_dir.path().join("index.db");
    let test_folder = temp_dir.path().join("test_data");

    // Create test folder structure
    fs::create_dir_all(&test_folder).await.unwrap();
    fs::write(test_folder.join("file1.txt"), b"Hello, World!")
        .await
        .unwrap();
    fs::write(test_folder.join("file2.txt"), b"Test content")
        .await
        .unwrap();

    let subdir = test_folder.join("subdir");
    fs::create_dir_all(&subdir).await.unwrap();
    fs::write(subdir.join("file3.txt"), b"Nested file")
        .await
        .unwrap();

    // Create indexer and index the folder
    let config = IndexerConfig::default();
    let mut indexer = FileIndexer::new(&cas_dir, &db_path, config).await.unwrap();

    let manifest = indexer.index_folder(&test_folder).await.unwrap();

    // Verify manifest
    assert_eq!(manifest.file_count(), 3);
    assert!(manifest.manifest_hash.is_some());
    assert!(manifest.total_size() > 0);

    // Check that files are in the manifest
    let file_paths: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
    assert!(file_paths.iter().any(|p| p.contains("file1.txt")));
    assert!(file_paths.iter().any(|p| p.contains("file2.txt")));
    assert!(file_paths.iter().any(|p| p.contains("file3.txt")));
}

#[tokio::test]
async fn test_manifest_comparison() {
    let temp_dir = tempdir().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let db_path = temp_dir.path().join("index.db");
    let test_folder = temp_dir.path().join("test_data");

    // Create initial files
    fs::create_dir_all(&test_folder).await.unwrap();
    fs::write(test_folder.join("file1.txt"), b"Initial content")
        .await
        .unwrap();
    fs::write(test_folder.join("file2.txt"), b"Another file")
        .await
        .unwrap();

    let config = IndexerConfig::default();
    let mut indexer = FileIndexer::new(&cas_dir, &db_path, config).await.unwrap();

    // Create first manifest
    let manifest1 = indexer.index_folder(&test_folder).await.unwrap();

    // Modify files
    fs::write(test_folder.join("file1.txt"), b"Modified content")
        .await
        .unwrap();
    fs::write(test_folder.join("file3.txt"), b"New file")
        .await
        .unwrap();
    fs::remove_file(test_folder.join("file2.txt"))
        .await
        .unwrap();

    // Create second manifest
    let manifest2 = indexer.index_folder(&test_folder).await.unwrap();

    // Compare manifests
    // diff() compares "self" (manifest1) to "other" (manifest2)
    // manifest1 is the old state (file1, file2)
    // manifest2 is the new state (file1 modified, file3 added, file2 removed)
    let diff = manifest1.diff(&manifest2);

    assert!(diff.has_changes());
    // file1.txt was modified (exists in both, different content)
    assert!(diff.modified.iter().any(|f| f.path.contains("file1.txt")));
    // file3.txt was added (exists in manifest2, not in manifest1)
    assert!(diff.added.iter().any(|f| f.path.contains("file3.txt")));
    // file2.txt was deleted (exists in manifest1, not in manifest2)
    assert!(diff.deleted.iter().any(|f| f.path.contains("file2.txt")));
}

#[tokio::test]
async fn test_sync_protocol_handshake() {
    // Create two sync handlers (simulating two peers)
    let handler1 = SyncProtocolHandler::new(vec![1, 2, 3, 4], "device-1".to_string());

    let handler2 = SyncProtocolHandler::new(vec![5, 6, 7, 8], "device-2".to_string());

    // Device 1 creates hello message
    let hello1 = handler1.create_hello();

    // Device 2 receives and responds
    let hello2 = handler2.handle_hello(hello1).await.unwrap();
    assert_eq!(handler2.state().await, SyncState::Connected);

    // Device 1 receives response
    handler1.handle_hello(hello2).await.unwrap();
    assert_eq!(handler1.state().await, SyncState::Connected);
}

#[tokio::test]
async fn test_sync_protocol_folder_sync() {
    use landro_proto::{FileEntry as ProtoFileEntry, FolderSummary, Manifest as ProtoManifest};

    let handler = SyncProtocolHandler::new(vec![1, 2, 3, 4], "test-device".to_string());

    // Establish connection first
    let peer_hello = handler.create_hello();
    handler.handle_hello(peer_hello).await.unwrap();

    // Send folder summary
    let summary = FolderSummary {
        folder_id: "test-folder".to_string(),
        folder_path: "/test/path".to_string(),
        manifest_hash: vec![1, 2, 3],
        total_size: 1024,
        file_count: 5,
        last_modified: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
    };

    handler.handle_folder_summary(summary).await.unwrap();
    assert_eq!(handler.state().await, SyncState::Syncing);

    // Send manifest
    let manifest = ProtoManifest {
        folder_id: "test-folder".to_string(),
        files: vec![ProtoFileEntry {
            path: "file1.txt".to_string(),
            size: 100,
            mode: 0o644,
            modified: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
            chunk_hashes: vec![vec![1, 2], vec![3, 4]],
            content_hash: vec![5, 6],
        }],
        generated_at: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        manifest_hash: vec![7, 8, 9],
    };

    let want = handler.handle_manifest(manifest).await.unwrap();
    assert_eq!(want.chunk_hashes.len(), 2); // Should request both chunks

    // Check progress tracking
    let progress = handler.get_progress().await;
    assert!(progress.contains_key("test-folder"));
    assert_eq!(progress["test-folder"].total_chunks, 2);
}

#[tokio::test]
async fn test_file_watcher_detection() {
    let temp_dir = tempdir().unwrap();
    let test_folder = temp_dir.path().join("watched");
    fs::create_dir_all(&test_folder).await.unwrap();

    // Create initial file
    let test_file = test_folder.join("test.txt");
    fs::write(&test_file, b"Initial content").await.unwrap();

    // Create watcher
    let config = WatcherConfig {
        debounce_delay: Duration::from_millis(100),
        ..Default::default()
    };
    let mut watcher = FolderWatcher::new(config).await.unwrap();

    // Start watching
    watcher
        .watch_folder(&test_folder, "test-folder-id".to_string())
        .await
        .unwrap();

    // Verify folder is being watched
    let watched = watcher.watched_folders().await;
    assert_eq!(watched.len(), 1);
    assert_eq!(watched[0].1, "test-folder-id");

    // Create a new file (would trigger events in real scenario)
    fs::write(test_folder.join("new_file.txt"), b"New content")
        .await
        .unwrap();

    // Give watcher time to detect (in real scenario)
    sleep(Duration::from_millis(200)).await;

    // Stop watching
    watcher.unwatch_folder(&test_folder).await.unwrap();
    assert!(watcher.watched_folders().await.is_empty());
}

#[tokio::test]
async fn test_end_to_end_sync_scenario() {
    // This test simulates a complete sync scenario between two devices

    // Setup for device 1
    let temp_dir1 = tempdir().unwrap();
    let cas_dir1 = temp_dir1.path().join("cas");
    let db_path1 = temp_dir1.path().join("index.db");
    let folder1 = temp_dir1.path().join("sync_folder");

    fs::create_dir_all(&folder1).await.unwrap();
    fs::write(folder1.join("doc1.txt"), b"Document 1")
        .await
        .unwrap();
    fs::write(folder1.join("doc2.txt"), b"Document 2")
        .await
        .unwrap();

    // Setup for device 2
    let temp_dir2 = tempdir().unwrap();
    let cas_dir2 = temp_dir2.path().join("cas");
    let db_path2 = temp_dir2.path().join("index.db");
    let folder2 = temp_dir2.path().join("sync_folder");

    fs::create_dir_all(&folder2).await.unwrap();

    // Create indexers
    let config = IndexerConfig::default();
    let mut indexer1 = FileIndexer::new(&cas_dir1, &db_path1, config.clone())
        .await
        .unwrap();
    let mut indexer2 = FileIndexer::new(&cas_dir2, &db_path2, config)
        .await
        .unwrap();

    // Index folders
    let manifest1 = indexer1.index_folder(&folder1).await.unwrap();
    let manifest2 = indexer2.index_folder(&folder2).await.unwrap();

    // Device 2 should detect it needs files from device 1
    assert_eq!(manifest1.file_count(), 2);
    assert_eq!(manifest2.file_count(), 0);

    // Simulate sync protocol
    let handler1 = SyncProtocolHandler::new(vec![1, 2, 3], "device-1".to_string());
    let handler2 = SyncProtocolHandler::new(vec![4, 5, 6], "device-2".to_string());

    // Exchange hellos
    let hello1 = handler1.create_hello();
    let hello2 = handler2.handle_hello(hello1).await.unwrap();
    handler1.handle_hello(hello2).await.unwrap();

    assert_eq!(handler1.state().await, SyncState::Connected);
    assert_eq!(handler2.state().await, SyncState::Connected);

    // Create file watcher for continuous sync
    let watcher_config = WatcherConfig::default();
    let mut watcher1 = FolderWatcher::new(watcher_config.clone()).await.unwrap();
    let mut watcher2 = FolderWatcher::new(watcher_config).await.unwrap();

    // Start watching both folders
    watcher1
        .watch_folder(&folder1, "folder1".to_string())
        .await
        .unwrap();
    watcher2
        .watch_folder(&folder2, "folder2".to_string())
        .await
        .unwrap();

    // Verify both folders are being watched
    assert_eq!(watcher1.watched_folders().await.len(), 1);
    assert_eq!(watcher2.watched_folders().await.len(), 1);
}

#[tokio::test]
async fn test_concurrent_file_operations() {
    // Test that the system handles concurrent file operations correctly
    let temp_dir = tempdir().unwrap();
    let cas_dir = temp_dir.path().join("cas");
    let db_path = temp_dir.path().join("index.db");
    let test_folder = temp_dir.path().join("concurrent_test");

    fs::create_dir_all(&test_folder).await.unwrap();

    let config = IndexerConfig::default();
    let indexer = Arc::new(RwLock::new(
        FileIndexer::new(&cas_dir, &db_path, config).await.unwrap(),
    ));

    // Create multiple files concurrently
    let mut handles = vec![];
    for i in 0..10 {
        let folder = test_folder.clone();
        let handle = tokio::spawn(async move {
            let file_path = folder.join(format!("file_{}.txt", i));
            fs::write(file_path, format!("Content {}", i))
                .await
                .unwrap();
        });
        handles.push(handle);
    }

    // Wait for all files to be created
    for handle in handles {
        handle.await.unwrap();
    }

    // Index the folder
    let manifest = indexer
        .write()
        .await
        .index_folder(&test_folder)
        .await
        .unwrap();

    // Verify all files were indexed
    assert_eq!(manifest.file_count(), 10);

    // Verify each file is in the manifest
    for i in 0..10 {
        let file_name = format!("file_{}.txt", i);
        assert!(manifest.files.iter().any(|f| f.path.contains(&file_name)));
    }
}
