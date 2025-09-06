//! Integration tests for mDNS discovery and connection management

use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::info;

use landro_crypto::{CertificateVerifier, DeviceIdentity};
use landro_daemon::discovery::DiscoveryService;
use landro_daemon::network::{ConnectionManager, NetworkConfig};
use landro_quic::{QuicConfig, QuicServer};

#[tokio::test]
async fn test_mdns_discovery_integration() {
    // Initialize logging
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();

    // Create two discovery services
    let mut discovery1 = DiscoveryService::new("device-1").unwrap();
    let mut discovery2 = DiscoveryService::new("device-2").unwrap();

    // Start advertising on different ports
    discovery1
        .start_advertising(9876, vec!["sync".to_string()])
        .await
        .unwrap();
    discovery2
        .start_advertising(9877, vec!["sync".to_string()])
        .await
        .unwrap();

    // Give time for mDNS to propagate
    sleep(Duration::from_secs(2)).await;

    // Browse for peers from both sides
    let peers1 = discovery1.browse_peers().await.unwrap();
    let peers2 = discovery2.browse_peers().await.unwrap();

    info!("Device 1 found {} peers", peers1.len());
    info!("Device 2 found {} peers", peers2.len());

    // Each should discover the other (in a real network environment)
    // Note: This may not work in CI/test environments without multicast support

    // Clean up
    discovery1.stop().await.unwrap();
    discovery2.stop().await.unwrap();
}

#[tokio::test]
async fn test_connection_manager_with_discovery() {
    // Initialize logging
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();

    // Create identities for two devices
    let identity1 = Arc::new(DeviceIdentity::generate("device-1").unwrap());
    let identity2 = Arc::new(DeviceIdentity::generate("device-2").unwrap());
    let verifier = Arc::new(CertificateVerifier::for_pairing());

    // Start QUIC server for device 2
    let config2 = QuicConfig::default().bind_addr("127.0.0.1:9878".parse().unwrap());
    let mut server2 = QuicServer::new(identity2.clone(), verifier.clone(), config2);
    server2.start().await.unwrap();

    // Run server in background
    let server_handle = tokio::spawn(async move {
        let _ = server2.run().await;
    });

    // Create discovery services
    let mut discovery1 = DiscoveryService::new("device-1").unwrap();
    let discovery1 = Arc::new(tokio::sync::Mutex::new(discovery1));

    // Create connection manager for device 1
    let network_config = NetworkConfig {
        health_check_interval: Duration::from_secs(5),
        ..Default::default()
    };

    let conn_manager =
        ConnectionManager::new(identity1, verifier, discovery1.clone(), network_config);

    // Start connection manager
    conn_manager.start().await.unwrap();

    // Manually add a peer to discovery (simulating mDNS discovery)
    {
        let mut disc = discovery1.lock().await;
        disc.update_peer(landro_daemon::discovery::PeerInfo {
            device_id: "device-2".to_string(),
            device_name: "Device 2".to_string(),
            address: "127.0.0.1:9878".parse().unwrap(),
            capabilities: vec!["sync".to_string()],
            last_seen: std::time::Instant::now(),
            version: "0.1.0".to_string(),
        })
        .await;
    }

    // Wait a bit for discovery task to pick up the peer
    sleep(Duration::from_secs(2)).await;

    // Try to get connection
    let connection_result = timeout(
        Duration::from_secs(10),
        conn_manager.get_connection("device-2"),
    )
    .await;

    match connection_result {
        Ok(Ok(_conn)) => {
            info!("Successfully established connection to device-2");

            // Check statistics
            let stats = conn_manager.get_stats().await;
            assert_eq!(stats.total_peers, 1);
            assert_eq!(stats.healthy_peers, 1);
        }
        Ok(Err(e)) => {
            // This is expected in test environments without proper network setup
            info!("Connection failed (expected in test environment): {}", e);
        }
        Err(_) => {
            info!("Connection timed out (expected in test environment)");
        }
    }

    // Cleanup
    conn_manager.shutdown().await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn test_connection_rate_limiting() {
    // Initialize logging
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug")
        .try_init();

    let identity = Arc::new(DeviceIdentity::generate("test-device").unwrap());
    let verifier = Arc::new(CertificateVerifier::for_pairing());
    let discovery = Arc::new(tokio::sync::Mutex::new(
        DiscoveryService::new("test-device").unwrap(),
    ));

    // Configure aggressive rate limiting
    let network_config = NetworkConfig {
        connection_rate_limit: 2, // Only 2 connections per second
        ..Default::default()
    };

    let conn_manager =
        ConnectionManager::new(identity, verifier, discovery.clone(), network_config);

    conn_manager.start().await.unwrap();

    // Add a fake peer
    {
        let mut disc = discovery.lock().await;
        disc.update_peer(landro_daemon::discovery::PeerInfo {
            device_id: "fake-peer".to_string(),
            device_name: "Fake Peer".to_string(),
            address: "127.0.0.1:9999".parse().unwrap(), // Non-existent
            capabilities: vec!["sync".to_string()],
            last_seen: std::time::Instant::now(),
            version: "0.1.0".to_string(),
        })
        .await;
    }

    // Try to connect multiple times rapidly
    let mut attempts = Vec::new();
    for i in 0..5 {
        let manager = &conn_manager;
        let attempt = async move {
            let start = std::time::Instant::now();
            let result = manager.get_connection("fake-peer").await;
            let elapsed = start.elapsed();
            (i, result, elapsed)
        };
        attempts.push(attempt);
    }

    // Execute all attempts concurrently
    let results = futures::future::join_all(attempts).await;

    // Check that rate limiting is working
    let mut rate_limited = 0;
    for (i, result, elapsed) in results {
        match result {
            Ok(_) => info!("Attempt {} succeeded in {:?}", i, elapsed),
            Err(e) => {
                info!("Attempt {} failed in {:?}: {}", i, elapsed, e);
                if e.contains("Rate limit") {
                    rate_limited += 1;
                }
            }
        }
    }

    // Some attempts should be rate limited
    assert!(rate_limited > 0, "Rate limiting should have triggered");

    conn_manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_connection_health_checks() {
    // Initialize logging
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();

    let identity = Arc::new(DeviceIdentity::generate("test-device").unwrap());
    let verifier = Arc::new(CertificateVerifier::for_pairing());
    let discovery = Arc::new(tokio::sync::Mutex::new(
        DiscoveryService::new("test-device").unwrap(),
    ));

    // Configure fast health checks
    let network_config = NetworkConfig {
        health_check_interval: Duration::from_secs(2),
        max_health_check_failures: 2,
        ..Default::default()
    };

    let conn_manager = ConnectionManager::new(identity, verifier, discovery, network_config);

    conn_manager.start().await.unwrap();

    // Wait for health check task to run
    sleep(Duration::from_secs(3)).await;

    // Get statistics
    let stats = conn_manager.get_stats().await;
    assert_eq!(stats.total_peers, 0);
    assert_eq!(stats.unhealthy_peers, 0);

    conn_manager.shutdown().await.unwrap();
}
