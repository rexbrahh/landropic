use super::*;
use landro_crypto::DeviceIdentity;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn test_discovery_initialization() {
    let identity = Arc::new(DeviceIdentity::generate("test-device").unwrap());
    let mut discovery = Discovery::new(identity);
    
    // Should initialize successfully
    assert!(discovery.initialize().await.is_ok());
    
    // Should be idempotent
    assert!(discovery.initialize().await.is_ok());
    
    // Clean shutdown
    assert!(discovery.shutdown().await.is_ok());
}

#[tokio::test]
async fn test_service_registration_and_shutdown() {
    let identity = Arc::new(DeviceIdentity::generate("test-device").unwrap());
    let mut discovery = Discovery::new(identity);
    
    // Start advertising
    let result = discovery.start_advertising(12345, vec!["sync".to_string()]).await;
    assert!(result.is_ok(), "Failed to start advertising: {:?}", result);
    
    // Should not allow double registration
    let result = discovery.start_advertising(12346, vec![]).await;
    assert!(matches!(result, Err(DiscoveryError::AlreadyRegistered)));
    
    // Stop advertising
    assert!(discovery.stop_advertising().await.is_ok());
    
    // Should allow re-registration after stopping
    let result = discovery.start_advertising(12347, vec![]).await;
    assert!(result.is_ok());
    
    // Clean shutdown
    assert!(discovery.shutdown().await.is_ok());
}

#[tokio::test]
async fn test_browsing_initialization() {
    let identity = Arc::new(DeviceIdentity::generate("test-device").unwrap());
    let mut discovery = Discovery::new(identity);
    
    // Should start browsing without error
    assert!(discovery.start_browsing().await.is_ok());
    
    // Should be able to start multiple times
    assert!(discovery.start_browsing().await.is_ok());
    
    // Clean shutdown
    assert!(discovery.shutdown().await.is_ok());
}

#[tokio::test]
async fn test_event_subscription() {
    let identity = Arc::new(DeviceIdentity::generate("test-device").unwrap());
    let discovery = Discovery::new(identity);
    
    // Should be able to subscribe to events
    let mut receiver1 = discovery.subscribe();
    let mut receiver2 = discovery.subscribe();
    
    // Send a test event
    let _ = discovery.event_sender.send(DiscoveryEvent::ServiceRegistered);
    
    // Both receivers should get the event
    let timeout_duration = Duration::from_millis(100);
    
    let event1 = timeout(timeout_duration, receiver1.recv()).await;
    assert!(event1.is_ok());
    assert!(matches!(event1.unwrap().unwrap(), DiscoveryEvent::ServiceRegistered));
    
    let event2 = timeout(timeout_duration, receiver2.recv()).await;
    assert!(event2.is_ok());
    assert!(matches!(event2.unwrap().unwrap(), DiscoveryEvent::ServiceRegistered));
}

#[tokio::test]
async fn test_peer_info_functionality() {
    let peer = PeerInfo {
        device_id: "test-device-id".to_string(),
        device_name: "Test Device".to_string(),
        service_name: "landropic-test._landropic._tcp.local.".to_string(),
        address: "192.168.1.100".to_string(),
        port: 12345,
        capabilities: vec!["sync".to_string(), "backup".to_string()],
        protocol_version: 1,
        last_seen: std::time::SystemTime::now(),
    };
    
    // Test socket address parsing
    let socket_addr = peer.socket_addr().unwrap();
    assert_eq!(socket_addr.port(), 12345);
    
    // Test capability checking
    assert!(peer.supports_capability("sync"));
    assert!(peer.supports_capability("backup"));
    assert!(!peer.supports_capability("unknown"));
    
    // Test activity checking
    assert!(peer.is_recently_active());
    
    // Test old peer
    let old_peer = PeerInfo {
        last_seen: std::time::SystemTime::now() - Duration::from_secs(60),
        ..peer
    };
    assert!(!old_peer.is_recently_active());
}

#[tokio::test]
async fn test_peer_storage_and_retrieval() {
    let identity = Arc::new(DeviceIdentity::generate("test-device").unwrap());
    let discovery = Discovery::new(identity);
    
    // Initially no peers
    assert!(discovery.get_peers().await.is_empty());
    assert!(discovery.get_peer("unknown").await.is_none());
    
    // Add a mock peer
    let peer = PeerInfo {
        device_id: "test-peer".to_string(),
        device_name: "Test Peer".to_string(),
        service_name: "landropic-test._landropic._tcp.local.".to_string(),
        address: "192.168.1.101".to_string(),
        port: 12345,
        capabilities: vec!["sync".to_string()],
        protocol_version: 1,
        last_seen: std::time::SystemTime::now(),
    };
    
    discovery.peers.write().await.insert(peer.device_id.clone(), peer.clone());
    
    // Should be able to retrieve peer
    let retrieved = discovery.get_peer("test-peer").await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().device_name, "Test Peer");
    
    // Should be in peer list
    let peers = discovery.get_peers().await;
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].device_id, "test-peer");
}

#[test]
fn test_service_info_parsing_with_our_device() {
    let identity = DeviceIdentity::generate("test-device").unwrap();
    let our_device_id = identity.device_id();
    
    // Create a mock ServiceInfo representing our own service
    let mut properties = HashMap::new();
    properties.insert("device_id".to_string(), our_device_id.to_hex());
    properties.insert("device_name".to_string(), "Our Device".to_string());
    
    let service_info = ServiceInfo::new(
        "_landropic._tcp.local.",
        "landropic-test",
        "test-host.local.",
        "192.168.1.100",
        12345,
        properties,
    ).unwrap();
    
    // Should return None for our own service
    let result = Discovery::parse_service_info(&service_info, &our_device_id).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_service_info_parsing_with_peer_device() {
    let our_identity = DeviceIdentity::generate("our-device").unwrap();
    let peer_identity = DeviceIdentity::generate("peer-device").unwrap();
    let our_device_id = our_identity.device_id();
    let peer_device_id = peer_identity.device_id();
    
    // Create a mock ServiceInfo representing a peer service
    let mut properties = HashMap::new();
    properties.insert("device_id".to_string(), peer_device_id.to_hex());
    properties.insert("device_name".to_string(), "Peer Device".to_string());
    properties.insert("capabilities".to_string(), "sync,backup".to_string());
    properties.insert("protocol_version".to_string(), "2".to_string());
    
    let service_info = ServiceInfo::new(
        "_landropic._tcp.local.",
        "landropic-peer",
        "peer-host.local.",
        "192.168.1.101",
        12346,
        properties,
    ).unwrap();
    
    // Should parse successfully
    let result = Discovery::parse_service_info(&service_info, &our_device_id).unwrap();
    assert!(result.is_some());
    
    let peer_info = result.unwrap();
    assert_eq!(peer_info.device_id, peer_device_id.to_hex());
    assert_eq!(peer_info.device_name, "Peer Device");
    assert_eq!(peer_info.port, 12346);
    assert_eq!(peer_info.capabilities, vec!["sync", "backup"]);
    assert_eq!(peer_info.protocol_version, 2);
}

#[test]
fn test_error_types() {
    // Test error display
    let error = DiscoveryError::DaemonStart("test error".to_string());
    assert!(error.to_string().contains("mDNS daemon failed to start"));
    
    let error = DiscoveryError::ServiceRegistration("registration failed".to_string());
    assert!(error.to_string().contains("Service registration failed"));
    
    let error = DiscoveryError::AlreadyRegistered;
    assert!(error.to_string().contains("Service already registered"));
    
    let error = DiscoveryError::InvalidServiceData("bad data".to_string());
    assert!(error.to_string().contains("Invalid service data"));
}

#[tokio::test]
async fn test_discovery_service_name() {
    let identity = Arc::new(DeviceIdentity::generate("My Test Device").unwrap());
    let discovery = Discovery::new(identity);
    
    assert_eq!(discovery.service_name, "landropic-My Test Device");
}

#[tokio::test]
async fn test_refresh_peers() {
    let identity = Arc::new(DeviceIdentity::generate("test-device").unwrap());
    let mut discovery = Discovery::new(identity);
    
    // Add some mock peers
    let peer = PeerInfo {
        device_id: "old-peer".to_string(),
        device_name: "Old Peer".to_string(),
        service_name: "landropic-old._landropic._tcp.local.".to_string(),
        address: "192.168.1.100".to_string(),
        port: 12345,
        capabilities: vec![],
        protocol_version: 1,
        last_seen: std::time::SystemTime::now(),
    };
    
    discovery.peers.write().await.insert(peer.device_id.clone(), peer);
    assert_eq!(discovery.get_peers().await.len(), 1);
    
    // Refresh should clear peers and restart browsing
    assert!(discovery.refresh_peers().await.is_ok());
    
    // Peers should be cleared (though browsing may discover new ones)
    // We can't easily test the browsing part without a real mDNS environment
    
    // Clean shutdown
    assert!(discovery.shutdown().await.is_ok());
}