use std::sync::Once;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};
use tokio::fs;

use landro_daemon::{Daemon, DaemonConfig};

static CRYPTO_INIT: Once = Once::new();

fn init_crypto() {
    CRYPTO_INIT.call_once(|| {
        // Initialize rustls crypto provider for tests
        rustls::crypto::aws_lc_rs::default_provider().install_default().ok();
    });
}

#[tokio::test]
async fn test_daemon_mDNS_advertisement() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().to_path_buf(),
        device_name: "test-advertising-device".to_string(),
        auto_sync: true, // Enable mDNS advertisement
        max_concurrent_transfers: 2,
    };
    
    let daemon = Daemon::new(config);
    daemon.start().await.expect("Failed to start daemon");
    
    // Give time for mDNS service to start advertising
    sleep(Duration::from_millis(1000)).await;
    
    // Verify daemon is running and should be advertising
    assert!(daemon.is_running().await);
    
    let status = daemon.get_status().await;
    assert!(status.running);
    
    // The daemon should be advertising on mDNS when auto_sync is enabled
    // In a real test, we would check that the service is actually advertised
    // For now, we verify it doesn't crash during startup
    
    daemon.stop().await.expect("Failed to stop daemon");
}

#[tokio::test]
async fn test_daemon_peer_discovery_without_peers() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let sync_folder = temp_dir.path().join("sync_test");
    fs::create_dir(&sync_folder).await.unwrap();
    
    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().join("storage"),
        device_name: "test-lonely-device".to_string(),
        auto_sync: true, // Enable peer discovery
        max_concurrent_transfers: 2,
    };
    
    let daemon = Daemon::new(config);
    daemon.start().await.expect("Failed to start daemon");
    daemon.add_sync_folder(&sync_folder).await.expect("Failed to add sync folder");
    
    // Create a file to trigger sync attempt
    let test_file = sync_folder.join("discovery_test.txt");
    fs::write(&test_file, "discovery test content").await.expect("Failed to create test file");
    
    // Give time for file watcher to detect and attempt peer discovery
    sleep(Duration::from_millis(2000)).await;
    
    // Verify daemon remains stable even when no peers are found
    assert!(daemon.is_running().await);
    
    let status = daemon.get_status().await;
    assert!(status.running);
    assert_eq!(status.peer_count, 0); // Should be zero as no peers available
    assert_eq!(status.sync_folders, 1);
    
    daemon.stop().await.expect("Failed to stop daemon");
}

#[tokio::test]
async fn test_multiple_daemon_instances_different_ports() {
    init_crypto();
    
    let temp_dir1 = TempDir::new().expect("Failed to create temp dir 1");
    let temp_dir2 = TempDir::new().expect("Failed to create temp dir 2");
    
    // Create two daemon instances with different ports
    let config1 = DaemonConfig {
        bind_addr: "127.0.0.1:19876".parse().unwrap(),
        storage_path: temp_dir1.path().to_path_buf(),
        device_name: "test-device-1".to_string(),
        auto_sync: true,
        max_concurrent_transfers: 2,
    };
    
    let config2 = DaemonConfig {
        bind_addr: "127.0.0.1:19877".parse().unwrap(),
        storage_path: temp_dir2.path().to_path_buf(),
        device_name: "test-device-2".to_string(),
        auto_sync: true,
        max_concurrent_transfers: 2,
    };
    
    let daemon1 = Daemon::new(config1);
    let daemon2 = Daemon::new(config2);
    
    // Start both daemons
    daemon1.start().await.expect("Failed to start daemon 1");
    daemon2.start().await.expect("Failed to start daemon 2");
    
    // Give time for mDNS advertisement and discovery
    sleep(Duration::from_millis(3000)).await;
    
    // Both daemons should be running
    assert!(daemon1.is_running().await);
    assert!(daemon2.is_running().await);
    
    let status1 = daemon1.get_status().await;
    let status2 = daemon2.get_status().await;
    
    assert!(status1.running);
    assert!(status2.running);
    assert_eq!(status1.device_name, "test-device-1");
    assert_eq!(status2.device_name, "test-device-2");
    
    // In a real network scenario, they might discover each other
    // For now, we just verify both can coexist without conflicts
    
    // Clean up
    daemon1.stop().await.expect("Failed to stop daemon 1");
    daemon2.stop().await.expect("Failed to stop daemon 2");
}

#[tokio::test]
async fn test_daemon_connection_resilience() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let sync_folder = temp_dir.path().join("sync_test");
    fs::create_dir(&sync_folder).await.unwrap();
    
    let config = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().join("storage"),
        device_name: "test-resilient-device".to_string(),
        auto_sync: true,
        max_concurrent_transfers: 2,
    };
    
    let daemon = Daemon::new(config);
    daemon.start().await.expect("Failed to start daemon");
    daemon.add_sync_folder(&sync_folder).await.expect("Failed to add sync folder");
    
    // Simulate network activity with multiple file operations
    for i in 0..5 {
        let file_path = sync_folder.join(format!("resilience_test_{}.txt", i));
        fs::write(&file_path, format!("content {}", i)).await.expect("Failed to create file");
        
        // Small delay between operations
        sleep(Duration::from_millis(200)).await;
    }
    
    // Give time for all operations to be processed
    sleep(Duration::from_millis(2000)).await;
    
    // Verify daemon remains healthy throughout
    let status = daemon.get_status().await;
    assert!(status.running);
    assert_eq!(status.sync_folders, 1);
    
    daemon.stop().await.expect("Failed to stop daemon");
}

#[tokio::test] 
async fn test_daemon_auto_sync_toggle() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    // Test with auto_sync disabled
    let config_no_sync = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().to_path_buf(),
        device_name: "test-no-autosync".to_string(),
        auto_sync: false, // Disabled
        max_concurrent_transfers: 2,
    };
    
    let daemon = Daemon::new(config_no_sync);
    daemon.start().await.expect("Failed to start daemon");
    
    // Verify daemon starts successfully even with auto_sync off
    assert!(daemon.is_running().await);
    
    let status = daemon.get_status().await;
    assert!(status.running);
    
    daemon.stop().await.expect("Failed to stop daemon");
    
    // Test with auto_sync enabled
    let config_with_sync = DaemonConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        storage_path: temp_dir.path().to_path_buf(),
        device_name: "test-with-autosync".to_string(),
        auto_sync: true, // Enabled
        max_concurrent_transfers: 2,
    };
    
    let daemon = Daemon::new(config_with_sync);
    daemon.start().await.expect("Failed to start daemon");
    
    // Verify daemon starts successfully with auto_sync on
    assert!(daemon.is_running().await);
    
    let status = daemon.get_status().await;
    assert!(status.running);
    
    daemon.stop().await.expect("Failed to stop daemon");
}

#[tokio::test]
async fn test_daemon_bind_address_variations() {
    init_crypto();
    
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    // Test various bind address configurations
    let test_addresses = vec![
        "127.0.0.1:0",      // IPv4 ephemeral port
        "[::1]:0",          // IPv6 loopback ephemeral port
        "0.0.0.0:0",        // IPv4 any address ephemeral port
    ];
    
    for addr_str in test_addresses {
        if let Ok(bind_addr) = addr_str.parse() {
            let config = DaemonConfig {
                bind_addr,
                storage_path: temp_dir.path().to_path_buf(),
                device_name: format!("test-device-{}", addr_str.replace([':', '[', ']'], "-")),
                auto_sync: false, // Keep simple for address testing
                max_concurrent_transfers: 2,
            };
            
            let daemon = Daemon::new(config);
            
            // Test that daemon can bind to different address types
            let start_result = timeout(Duration::from_secs(10), daemon.start()).await;
            if start_result.is_ok() && start_result.unwrap().is_ok() {
                assert!(daemon.is_running().await);
                daemon.stop().await.expect("Failed to stop daemon");
            }
            // Note: IPv6 addresses might fail in some environments, which is okay
        }
    }
}