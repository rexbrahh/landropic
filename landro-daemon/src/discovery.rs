//! mDNS service discovery for peer finding and advertising
//!
//! This module handles the discovery of other landropic devices on the local network
//! using mDNS (Multicast DNS) service discovery. It provides capabilities for:
//! - Advertising our own sync service
//! - Browsing and discovering peer devices
//! - Extracting connection information from peers
//! - Handling network interface changes gracefully

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};

// Using mdns-sd for more reliable cross-platform mDNS
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

/// Service type for landropic sync
const SERVICE_TYPE: &str = "_landropic._tcp.local.";
const SERVICE_DOMAIN: &str = "local.";

/// Discovered peer information
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub device_id: String,
    pub device_name: String,
    pub address: SocketAddr,
    pub capabilities: Vec<String>,
    pub last_seen: Instant,
    pub version: String,
}

/// mDNS discovery service for finding and advertising peers
pub struct DiscoveryService {
    device_id: String,
    device_name: String,
    mdns: Arc<Mutex<ServiceDaemon>>,
    discovered_peers: Arc<RwLock<HashMap<String, PeerInfo>>>,
    running: Arc<RwLock<bool>>,
    service_port: u16,
    capabilities: Vec<String>,
    event_receiver: Arc<Mutex<Option<mdns_sd::Receiver<ServiceEvent>>>>,
    background_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl DiscoveryService {
    /// Create a new discovery service
    pub fn new(device_name: &str) -> Result<Self, String> {
        let mdns =
            ServiceDaemon::new().map_err(|e| format!("Failed to create mDNS daemon: {}", e))?;

        let device_id = generate_device_id();

        Ok(Self {
            device_id: device_id.clone(),
            device_name: device_name.to_string(),
            mdns: Arc::new(Mutex::new(mdns)),
            discovered_peers: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(false)),
            service_port: 0,
            capabilities: Vec::new(),
            event_receiver: Arc::new(Mutex::new(None)),
            background_task: Arc::new(Mutex::new(None)),
        })
    }

    /// Start advertising our sync service
    pub async fn start_advertising(
        &mut self,
        port: u16,
        capabilities: Vec<String>,
    ) -> Result<(), String> {
        if *self.running.read().await {
            return Err("Discovery service already running".into());
        }

        info!("Starting mDNS advertising on port {}", port);

        self.service_port = port;
        self.capabilities = capabilities.clone();

        // Create service info
        let service_name = format!("landropic-{}", &self.device_id[..8]);
        let host_name = format!("{}.local.", gethostname::gethostname().to_string_lossy());

        let mut properties = HashMap::new();
        properties.insert("device_id".to_string(), self.device_id.clone());
        properties.insert("device_name".to_string(), self.device_name.clone());
        properties.insert("version".to_string(), "0.1.0".to_string());
        properties.insert("capabilities".to_string(), capabilities.join(","));

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &service_name,
            &host_name,
            (), // Will be filled with actual IPs
            port,
            Some(properties),
        )
        .map_err(|e| format!("Failed to create service info: {}", e))?;

        // Register the service
        let mdns = self.mdns.lock().await;
        mdns.register(service_info.clone())
            .map_err(|e| format!("Failed to register mDNS service: {}", e))?;
        drop(mdns);

        // Start browsing for peers
        self.start_browsing().await?;

        *self.running.write().await = true;
        info!(
            "mDNS advertising started for device: {} ({})",
            self.device_name, self.device_id
        );

        Ok(())
    }

    /// Start browsing for peer services
    async fn start_browsing(&mut self) -> Result<(), String> {
        debug!("Starting mDNS browsing for peers");

        let mdns = self.mdns.lock().await;
        let receiver = mdns
            .browse(SERVICE_TYPE)
            .map_err(|e| format!("Failed to start browsing: {}", e))?;
        drop(mdns);

        *self.event_receiver.lock().await = Some(receiver);

        // Start background task to process events
        self.start_event_processor().await;

        Ok(())
    }

    /// Start background task to process mDNS events
    async fn start_event_processor(&self) {
        let discovered_peers = self.discovered_peers.clone();
        let event_receiver = self.event_receiver.clone();
        let device_id = self.device_id.clone();

        let task = tokio::spawn(async move {
            let mut receiver = match event_receiver.lock().await.take() {
                Some(r) => r,
                None => {
                    error!("No event receiver available");
                    return;
                }
            };

            while let Some(event) = receiver.recv_async().await.ok() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        // Skip our own service
                        let props = info.get_properties();
                        if let Some(discovered_id) = props.get_property_val_str("device_id") {
                            if discovered_id == device_id {
                                continue;
                            }
                        }

                        debug!("Discovered service: {}", info.get_fullname());

                        if let Some(peer_info) = parse_service_info(&info) {
                            let mut peers = discovered_peers.write().await;
                            let peer_id = peer_info.device_id.clone();

                            if !peers.contains_key(&peer_id) {
                                info!(
                                    "Discovered new peer: {} ({}) at {}",
                                    peer_info.device_name, peer_info.device_id, peer_info.address
                                );
                            }

                            peers.insert(peer_id, peer_info);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        debug!("Service removed: {}", fullname);
                        // Find and remove the peer
                        let mut peers = discovered_peers.write().await;
                        peers.retain(|_, peer| {
                            // In production, we'd match by fullname
                            // For now, rely on timeout-based cleanup
                            true
                        });
                    }
                    ServiceEvent::SearchStarted(_) => {
                        debug!("mDNS search started");
                    }
                    ServiceEvent::SearchStopped(_) => {
                        debug!("mDNS search stopped");
                    }
                    ServiceEvent::ServiceFound(_, _) => {
                        debug!("Service found, waiting for resolution");
                    }
                }
            }
        });

        *self.background_task.lock().await = Some(task);
    }

    /// Browse for peer services
    pub async fn browse_peers(&mut self) -> Result<Vec<PeerInfo>, String> {
        debug!("Browsing for peer services");

        // Clean up old peers (remove peers not seen in last 5 minutes)
        let now = Instant::now();
        let mut peers = self.discovered_peers.write().await;
        peers.retain(|_, peer| now.duration_since(peer.last_seen) < Duration::from_secs(300));

        // Return current discovered peers
        Ok(peers.values().cloned().collect())
    }

    /// Get a specific peer by device ID
    pub fn get_peer(&self, device_id: &str) -> Option<PeerInfo> {
        // This needs to be async but we're keeping the interface for compatibility
        // In production, we'd refactor to make this async
        let peers = futures::executor::block_on(self.discovered_peers.read());
        peers.get(device_id).cloned()
    }

    /// Get all discovered peers
    pub async fn get_all_peers(&self) -> Vec<PeerInfo> {
        self.discovered_peers
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// Update or add a peer
    pub async fn update_peer(&mut self, peer: PeerInfo) {
        self.discovered_peers
            .write()
            .await
            .insert(peer.device_id.clone(), peer);
    }

    /// Remove a peer
    pub async fn remove_peer(&mut self, device_id: &str) -> Option<PeerInfo> {
        self.discovered_peers.write().await.remove(device_id)
    }

    /// Stop the discovery service
    pub async fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !*self.running.read().await {
            return Ok(());
        }

        info!("Stopping mDNS discovery service");

        // Stop background task
        if let Some(task) = self.background_task.lock().await.take() {
            task.abort();
        }

        // Shutdown mDNS daemon
        let mdns = self.mdns.lock().await;
        mdns.shutdown()?;
        drop(mdns);

        self.discovered_peers.write().await.clear();
        *self.running.write().await = false;

        info!("mDNS discovery service stopped");
        Ok(())
    }

    /// Re-announce our service (useful after network changes)
    pub async fn re_announce(&mut self) -> Result<(), String> {
        if !*self.running.read().await {
            return Ok(());
        }

        info!("Re-announcing mDNS service");

        // In mdns-sd, the service is automatically re-announced
        // We can trigger a manual update if needed
        Ok(())
    }

    /// Check if the service is running
    pub fn is_running(&self) -> bool {
        futures::executor::block_on(self.running.read()).clone()
    }

    /// Get the number of discovered peers
    pub fn peer_count(&self) -> usize {
        futures::executor::block_on(self.discovered_peers.read()).len()
    }
}

/// Parse service info into PeerInfo
fn parse_service_info(info: &ServiceInfo) -> Option<PeerInfo> {
    let properties = info.get_properties();

    let device_id = properties.get_property_val_str("device_id")?.to_string();
    let device_name = properties.get_property_val_str("device_name")?.to_string();
    let version = properties
        .get_property_val_str("version")
        .unwrap_or("unknown")
        .to_string();

    let capabilities = properties
        .get_property_val_str("capabilities")
        .map(|c| c.split(',').map(|s| s.to_string()).collect())
        .unwrap_or_default();

    // Get the first valid address
    let addresses = info.get_addresses();
    if addresses.is_empty() {
        warn!("No addresses found for service {}", info.get_fullname());
        return None;
    }

    // Prefer IPv4 addresses for compatibility
    let ip = addresses
        .iter()
        .find(|addr| matches!(addr, IpAddr::V4(_)))
        .or_else(|| addresses.iter().next())
        .cloned()?;

    let address = SocketAddr::new(ip, info.get_port());

    Some(PeerInfo {
        device_id,
        device_name,
        address,
        capabilities,
        last_seen: Instant::now(),
        version,
    })
}

/// Generate a unique device ID for this session
fn generate_device_id() -> String {
    use std::time::SystemTime;

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Include hostname for better uniqueness
    let hostname = gethostname::gethostname()
        .to_string_lossy()
        .chars()
        .take(8)
        .collect::<String>();

    format!("{:x}_{}", timestamp, hostname)
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        if self.is_running() {
            // Best effort cleanup
            let mdns = self.mdns.clone();
            let task = self.background_task.clone();

            tokio::spawn(async move {
                if let Some(t) = task.lock().await.take() {
                    t.abort();
                }
                if let Ok(m) = mdns.lock().await.shutdown() {
                    debug!("mDNS daemon shutdown in drop");
                }
            });
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
    async fn test_device_id_generation() {
        let id1 = generate_device_id();
        sleep(Duration::from_millis(10)).await;
        let id2 = generate_device_id();

        // IDs should be unique
        assert_ne!(id1, id2);

        // IDs should have expected format
        assert!(id1.contains('_'));
        assert!(id2.contains('_'));
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
            version: "0.1.0".to_string(),
        };

        // Add peer
        discovery.update_peer(peer.clone()).await;
        assert_eq!(discovery.peer_count(), 1);

        // Get peer
        let retrieved = discovery.get_peer("test_peer_1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().device_name, "Test Peer");

        // Remove peer
        let removed = discovery.remove_peer("test_peer_1").await;
        assert!(removed.is_some());
        assert_eq!(discovery.peer_count(), 0);
    }

    #[tokio::test]
    async fn test_start_stop_advertising() {
        let mut discovery = DiscoveryService::new("test-device").unwrap();

        // Start advertising
        let result = discovery
            .start_advertising(9999, vec!["sync".to_string()])
            .await;
        assert!(result.is_ok());
        assert!(discovery.is_running());

        // Stop
        let result = discovery.stop().await;
        assert!(result.is_ok());
        assert!(!discovery.is_running());
    }
}
