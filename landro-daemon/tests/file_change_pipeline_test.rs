use std::sync::Once;
use std::time::Duration;
use tempfile::TempDir;
use tokio::fs;
use tokio::time::{sleep, timeout};

use landro_daemon::{Daemon, DaemonConfig};

static CRYPTO_INIT: Once = Once::new();

fn init_crypto() {
    CRYPTO_INIT.call_once(|| {
        // Initialize rustls crypto provider for tests
        rustls::crypto::aws_lc_rs::default_provider().install_default().ok();
    });
}

#[tokio::test]
async fn test_file_change_pipeline_indexing() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    // Create a test sync folder
    let sync_folder = temp_dir.path().join("sync_test");
    fs::create_dir(&sync_folder).await.unwrap();
    
    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().join("storage"),
        device_name: "test-device".to_string(),
        auto_sync: false, // Disable auto-sync to avoid peer discovery issues in test
        max_concurrent_transfers: 2,
    };
    
    let daemon = Daemon::new(config);
    daemon.start().await.expect("Failed to start daemon");
    
    // Add sync folder to daemon
    daemon.add_sync_folder(&sync_folder).await.expect("Failed to add sync folder");
    
    // Create a test file
    let test_file = sync_folder.join("test.txt");
    fs::write(&test_file, "initial content").await.expect("Failed to create test file");
    
    // Give file watcher time to detect the file
    sleep(Duration::from_millis(500)).await;
    
    // Modify the file
    fs::write(&test_file, "modified content").await.expect("Failed to modify test file");
    
    // Give file watcher time to detect the change
    sleep(Duration::from_millis(500)).await;
    
    // Verify the daemon is still running after file changes
    assert!(daemon.is_running().await);
    
    // Create another file
    let test_file2 = sync_folder.join("test2.txt");
    fs::write(&test_file2, "second file content").await.expect("Failed to create second test file");
    
    // Give file watcher time to detect the new file
    sleep(Duration::from_millis(500)).await;
    
    // Delete a file
    fs::remove_file(&test_file).await.expect("Failed to delete test file");
    
    // Give file watcher time to detect the deletion
    sleep(Duration::from_millis(500)).await;
    
    // Rename a file
    let renamed_file = sync_folder.join("test2_renamed.txt");
    fs::rename(&test_file2, &renamed_file).await.expect("Failed to rename file");
    
    // Give file watcher time to detect the rename
    sleep(Duration::from_millis(500)).await;
    
    // Verify daemon is still healthy after all file operations
    let status = daemon.get_status().await;
    assert!(status.running);
    assert_eq!(status.sync_folders, 1);
    
    // Clean up
    daemon.stop().await.expect("Failed to stop daemon");
}

#[tokio::test] 
async fn test_file_change_pipeline_with_auto_sync() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let sync_folder = temp_dir.path().join("sync_test");
    fs::create_dir(&sync_folder).await.unwrap();
    
    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().join("storage"),
        device_name: "test-device-autosync".to_string(),
        auto_sync: true, // Enable auto-sync to test peer discovery integration
        max_concurrent_transfers: 2,
    };
    
    let daemon = Daemon::new(config);
    daemon.start().await.expect("Failed to start daemon");
    daemon.add_sync_folder(&sync_folder).await.expect("Failed to add sync folder");
    
    // Create a file that should trigger auto-sync
    let test_file = sync_folder.join("autosync_test.txt");
    fs::write(&test_file, "auto sync content").await.expect("Failed to create test file");
    
    // Give time for file watcher and potential sync operations
    sleep(Duration::from_millis(1000)).await;
    
    // Verify daemon remains stable even when auto-sync can't find peers
    assert!(daemon.is_running().await);
    
    // Create multiple files to test batched operations
    for i in 0..5 {
        let file_path = sync_folder.join(format!("batch_test_{}.txt", i));
        fs::write(&file_path, format!("batch content {}", i)).await.expect("Failed to create batch file");
        
        // Small delay between file creations to avoid overwhelming the system
        sleep(Duration::from_millis(50)).await;
    }
    
    // Give time for all file operations to be processed
    sleep(Duration::from_millis(1000)).await;
    
    // Verify daemon stability
    let status = daemon.get_status().await;
    assert!(status.running);
    assert_eq!(status.sync_folders, 1);
    
    daemon.stop().await.expect("Failed to stop daemon");
}

#[tokio::test]
async fn test_file_change_pipeline_concurrent_operations() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let sync_folder = temp_dir.path().join("sync_test");
    fs::create_dir(&sync_folder).await.unwrap();
    
    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().join("storage"),
        device_name: "test-concurrent".to_string(),
        auto_sync: false,
        max_concurrent_transfers: 4,
    };
    
    let daemon = Daemon::new(config);
    daemon.start().await.expect("Failed to start daemon");
    daemon.add_sync_folder(&sync_folder).await.expect("Failed to add sync folder");
    
    // Spawn concurrent file operations
    let mut handles = Vec::new();
    
    for thread_id in 0..3 {
        let folder_clone = sync_folder.clone();
        let handle = tokio::spawn(async move {
            for i in 0..10 {
                let file_path = folder_clone.join(format!("concurrent_{}_{}.txt", thread_id, i));
                let content = format!("Thread {} file {} content", thread_id, i);
                
                // Create file
                fs::write(&file_path, &content).await.expect("Failed to create file");
                
                // Small delay
                sleep(Duration::from_millis(10)).await;
                
                // Modify file
                let modified_content = format!("{} - modified", content);
                fs::write(&file_path, modified_content).await.expect("Failed to modify file");
                
                sleep(Duration::from_millis(10)).await;
            }
        });
        handles.push(handle);
    }
    
    // Wait for all concurrent operations to complete
    for handle in handles {
        timeout(Duration::from_secs(10), handle).await
            .expect("Concurrent operation timed out")
            .expect("Concurrent operation failed");
    }
    
    // Give file watcher time to process all changes
    sleep(Duration::from_millis(2000)).await;
    
    // Verify daemon remains stable after concurrent operations
    let status = daemon.get_status().await;
    assert!(status.running);
    assert_eq!(status.sync_folders, 1);
    
    daemon.stop().await.expect("Failed to stop daemon");
}

#[tokio::test]
async fn test_file_change_pipeline_large_files() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let sync_folder = temp_dir.path().join("sync_test");
    fs::create_dir(&sync_folder).await.unwrap();
    
    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().join("storage"),
        device_name: "test-large-files".to_string(),
        auto_sync: false,
        max_concurrent_transfers: 2,
    };
    
    let daemon = Daemon::new(config);
    daemon.start().await.expect("Failed to start daemon");
    daemon.add_sync_folder(&sync_folder).await.expect("Failed to add sync folder");
    
    // Create a larger test file (1MB)
    let large_content = "x".repeat(1024 * 1024);
    let large_file = sync_folder.join("large_test.txt");
    fs::write(&large_file, &large_content).await.expect("Failed to create large file");
    
    // Give time for indexing the large file
    sleep(Duration::from_millis(2000)).await;
    
    // Modify the large file
    let modified_large_content = format!("modified: {}", large_content);
    fs::write(&large_file, modified_large_content).await.expect("Failed to modify large file");
    
    // Give time for re-indexing
    sleep(Duration::from_millis(2000)).await;
    
    // Verify daemon handles large files without issues
    let status = daemon.get_status().await;
    assert!(status.running);
    
    daemon.stop().await.expect("Failed to stop daemon");
}