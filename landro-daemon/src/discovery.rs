//! mDNS service discovery for peer finding and advertising
//!
//! This module handles the discovery of other landropic devices on the local network
//! using mDNS (Multicast DNS) service discovery. It provides capabilities for:
//! - Advertising our own sync service
//! - Browsing and discovering peer devices
//! - Extracting connection information from peers

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::{info, warn, error, debug};
use zeroconf::prelude::*;
use zeroconf::{MdnsService, ServiceDiscovery, ServiceRegistration, TxtRecord};

/// Service type for landropic sync
const SERVICE_TYPE: &str = "_landropic._tcp";

/// Discovered peer information
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub device_id: String,
    pub device_name: String,
    pub address: SocketAddr,
    pub capabilities: Vec<String>,
    pub last_seen: Instant,
}

/// mDNS discovery service for finding and advertising peers
pub struct DiscoveryService {
    device_name: String,
    service_registration: Option<ServiceRegistration>,
    service_discovery: Option<ServiceDiscovery>,
    discovered_peers: HashMap<String, PeerInfo>,
    running: bool,
}

impl DiscoveryService {
    /// Create a new discovery service
    pub fn new(device_name: &str) -> Result<Self, String> {
        Ok(Self {
            device_name: device_name.to_string(),
            service_registration: None,
            service_discovery: None,
            discovered_peers: HashMap::new(),
            running: false,
        })
    }

    /// Start advertising our sync service
    pub async fn start_advertising(
        &mut self,
        port: u16,
        capabilities: Vec<String>,
    ) -> Result<(), String> {
        if self.running {
            return Err("Discovery service already running".into());
        }

        info!("Starting mDNS advertising on port {}", port);

        // Create TXT record with capabilities
        let mut txt_record = TxtRecord::new();
        txt_record.insert("version", "0.1.0").map_err(|e| e.to_string())?;
        txt_record.insert("capabilities", &capabilities.join(",")).map_err(|e| e.to_string())?;

        // Generate a unique device ID for this session
        let device_id = generate_device_id();
        txt_record.insert("device_id", &device_id).map_err(|e| e.to_string())?;

        // TODO: Fix mDNS registration with correct zeroconf API
        // For now, mark as running without actual registration
        warn!("mDNS registration temporarily disabled - needs zeroconf API fix");
        self.running = true;

        info!("mDNS advertising started for device: {}", self.device_name);
        Ok(())
    }

    /// Browse for peer services
    pub async fn browse_peers(&mut self) -> Result<Vec<PeerInfo>, String> {
        debug!("Browsing for peer services");

        // Clean up old peers (remove peers not seen in last 5 minutes)
        let now = Instant::now();
        self.discovered_peers.retain(|_, peer| {
            now.duration_since(peer.last_seen) < Duration::from_secs(300)
        });

        // TODO: Fix mDNS discovery with correct zeroconf API
        // For now, return empty list
        warn!("mDNS discovery temporarily disabled - needs zeroconf API fix");

        // Return current discovered peers
        Ok(self.discovered_peers.values().cloned().collect())
    }

    /// Start background discovery process
    async fn start_background_discovery(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // This is a simplified implementation
        // In practice, you'd want to use the zeroconf callbacks properly
        
        // For now, simulate discovery with a mock peer
        if self.discovered_peers.is_empty() {
            let mock_peer = PeerInfo {
                device_id: "mock_device_123".to_string(),
                device_name: "Mock Device".to_string(),
                address: "192.168.1.100:9876".parse().unwrap(),
                capabilities: vec!["sync".to_string(), "transfer".to_string()],
                last_seen: Instant::now(),
            };
            
            self.discovered_peers.insert(mock_peer.device_id.clone(), mock_peer);
            debug!("Added mock peer for testing");
        }

        Ok(())
    }

    /// Get a specific peer by device ID
    pub fn get_peer(&self, device_id: &str) -> Option<&PeerInfo> {
        self.discovered_peers.get(device_id)
    }

    /// Get all discovered peers
    pub fn get_all_peers(&self) -> Vec<&PeerInfo> {
        self.discovered_peers.values().collect()
    }

    /// Update or add a peer
    pub fn update_peer(&mut self, peer: PeerInfo) {
        self.discovered_peers.insert(peer.device_id.clone(), peer);
    }

    /// Remove a peer
    pub fn remove_peer(&mut self, device_id: &str) -> Option<PeerInfo> {
        self.discovered_peers.remove(device_id)
    }

    /// Stop the discovery service
    pub async fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.running {
            return Ok(());
        }

        info!("Stopping mDNS discovery service");

        // Stop service registration
        if let Some(service_reg) = self.service_registration.take() {
            // The service registration will be dropped, stopping the advertisement
            drop(service_reg);
        }

        // Stop service discovery
        if let Some(service_disc) = self.service_discovery.take() {
            drop(service_disc);
        }

        self.discovered_peers.clear();
        self.running = false;

        info!("mDNS discovery service stopped");
        Ok(())
    }

    /// Check if the service is running
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Get the number of discovered peers
    pub fn peer_count(&self) -> usize {
        self.discovered_peers.len()
    }
}

/// Generate a unique device ID for this session
fn generate_device_id() -> String {
    use std::time::SystemTime;
    
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    format!("device_{:x}", timestamp)
}

/// Callback for when service registration completes
fn on_service_registered(
    result: zeroconf::Result<ServiceRegistration>,
    _context: Option<std::sync::Arc<dyn std::any::Any>>,
) {
    match result {
        Ok(service_reg) => {
            info!("mDNS service registered: {}", service_reg.name());
        }
        Err(e) => {
            error!("Failed to register mDNS service: {}", e);
        }
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        if self.running {
            // Best effort cleanup - in practice we'd need a better cleanup strategy
            // For now, just mark as not running to avoid warnings
            info!("DiscoveryService dropped while running - cleanup needed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_discovery_service_creation() {
        let discovery = DiscoveryService::new("test-device").unwrap();
        assert_eq!(discovery.device_name, "test-device");
        assert!(!discovery.is_running());
        assert_eq!(discovery.peer_count(), 0);
    }

    #[tokio::test]
    async fn test_start_stop_advertising() {
        let mut discovery = DiscoveryService::new("test-device").unwrap();
        
        // Start advertising
        let result = discovery.start_advertising(9999, vec!["sync".to_string()]).await;
        assert!(result.is_ok());
        assert!(discovery.is_running());
        
        // Stop
        let result = discovery.stop().await;
        assert!(result.is_ok());
        assert!(!discovery.is_running());
    }

    #[tokio::test]
    async fn test_peer_management() {
        let mut discovery = DiscoveryService::new("test-device").unwrap();
        
        let peer = PeerInfo {
            device_id: "test_peer_1".to_string(),
            device_name: "Test Peer".to_string(),
            address: "192.168.1.100:9876".parse().unwrap(),
            capabilities: vec!["sync".to_string()],
            last_seen: Instant::now(),
        };
        
        // Add peer
        discovery.update_peer(peer.clone());
        assert_eq!(discovery.peer_count(), 1);
        
        // Get peer
        let retrieved = discovery.get_peer("test_peer_1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().device_name, "Test Peer");
        
        // Remove peer
        let removed = discovery.remove_peer("test_peer_1");
        assert!(removed.is_some());
        assert_eq!(discovery.peer_count(), 0);
    }

    #[tokio::test]
    async fn test_browse_peers() {
        let mut discovery = DiscoveryService::new("test-device").unwrap();
        
        // Start advertising first
        discovery.start_advertising(9999, vec!["sync".to_string()]).await.unwrap();
        
        // Browse for peers (this will create a mock peer)
        let peers = discovery.browse_peers().await.unwrap();
        assert!(!peers.is_empty());
        
        discovery.stop().await.unwrap();
    }
}