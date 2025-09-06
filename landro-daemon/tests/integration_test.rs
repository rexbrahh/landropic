use std::sync::Once;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

use landro_daemon::{Daemon, DaemonConfig};

static CRYPTO_INIT: Once = Once::new();

fn init_crypto() {
    CRYPTO_INIT.call_once(|| {
        // Initialize rustls crypto provider for tests
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
    });
}

#[tokio::test]
async fn test_daemon_startup_and_shutdown() {
    init_crypto();

    // Create a temporary directory for this test
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create daemon configuration
    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(), // Use ephemeral port
        storage_path: temp_dir.path().to_path_buf(),
        device_name: "test-device".to_string(),
        auto_sync: false, // Disable auto-sync for testing
        max_concurrent_transfers: 2,
    };

    // Create daemon instance
    let daemon = Daemon::new(config);

    // Test initial state
    assert!(!daemon.is_running().await);

    // Test daemon startup
    let start_result = timeout(Duration::from_secs(10), daemon.start()).await;
    assert!(start_result.is_ok(), "Daemon startup timed out");
    assert!(start_result.unwrap().is_ok(), "Daemon failed to start");

    // Test daemon is now running
    assert!(daemon.is_running().await);

    // Test daemon status
    let status = daemon.get_status().await;
    assert!(status.running);
    assert_eq!(status.device_name, "test-device");
    assert_eq!(status.peer_count, 0); // No peers initially
    assert_eq!(status.sync_folders, 0); // No folders initially

    // Test daemon shutdown
    let stop_result = timeout(Duration::from_secs(5), daemon.stop()).await;
    assert!(stop_result.is_ok(), "Daemon shutdown timed out");
    assert!(stop_result.unwrap().is_ok(), "Daemon failed to stop");

    // Test daemon is no longer running
    assert!(!daemon.is_running().await);
}

#[tokio::test]
async fn test_daemon_sync_folder_management() {
    init_crypto();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create a test folder to sync
    let sync_folder = temp_dir.path().join("sync_test");
    tokio::fs::create_dir(&sync_folder).await.unwrap();

    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().join("storage"),
        device_name: "test-device".to_string(),
        auto_sync: false,
        max_concurrent_transfers: 2,
    };

    let daemon = Daemon::new(config);

    // Start daemon
    daemon.start().await.expect("Failed to start daemon");

    // Test adding sync folder
    let add_result = daemon.add_sync_folder(&sync_folder).await;
    assert!(
        add_result.is_ok(),
        "Failed to add sync folder: {:?}",
        add_result.err()
    );

    // Test listing sync folders
    let folders = daemon.list_sync_folders().await;
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0], sync_folder);

    // Test daemon status shows sync folder
    let status = daemon.get_status().await;
    assert_eq!(status.sync_folders, 1);

    // Test removing sync folder
    let remove_result = daemon.remove_sync_folder(&sync_folder).await;
    assert!(remove_result.is_ok(), "Failed to remove sync folder");

    // Test folder is removed
    let folders = daemon.list_sync_folders().await;
    assert_eq!(folders.len(), 0);

    // Clean up
    daemon.stop().await.expect("Failed to stop daemon");
}

#[tokio::test]
async fn test_daemon_double_start_prevention() {
    init_crypto();

    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().to_path_buf(),
        device_name: "test-device".to_string(),
        auto_sync: false,
        max_concurrent_transfers: 2,
    };

    let daemon = Daemon::new(config);

    // Start daemon first time - should succeed
    let first_start = daemon.start().await;
    assert!(first_start.is_ok());
    assert!(daemon.is_running().await);

    // Try to start again - should fail
    let second_start = daemon.start().await;
    assert!(second_start.is_err());
    assert_eq!(
        second_start.unwrap_err().to_string(),
        "Daemon already running"
    );

    // Clean up
    daemon.stop().await.expect("Failed to stop daemon");
}
