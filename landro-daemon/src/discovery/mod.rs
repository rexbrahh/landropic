use landro_crypto::DeviceIdentity;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{broadcast, RwLock};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

const SERVICE_TYPE: &str = "_landropic._tcp.local.";
const RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_RETRIES: usize = 3;

/// Errors that can occur during service discovery
#[derive(Error, Debug)]
pub enum DiscoveryError {
    #[error("mDNS daemon failed to start: {0}")]
    DaemonStart(String),
    
    #[error("Service registration failed: {0}")]
    ServiceRegistration(String),
    
    #[error("Service browsing failed: {0}")]
    ServiceBrowsing(String),
    
    #[error("Network timeout: {0}")]
    Timeout(String),
    
    #[error("Invalid service data: {0}")]
    InvalidServiceData(String),
    
    #[error("Service already registered")]
    AlreadyRegistered,
    
    #[error("Service not found: {0}")]
    ServiceNotFound(String),
}

type Result<T> = std::result::Result<T, DiscoveryError>;

/// Event notifications for peer discovery
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    PeerDiscovered(PeerInfo),
    PeerRemoved(String), // device_id
    PeerUpdated(PeerInfo),
    ServiceRegistered,
    ServiceUnregistered,
    NetworkError(String),
}

/// mDNS service discovery manager
pub struct Discovery {
    mdns: Option<ServiceDaemon>,
    registered_service: Option<String>,
    peers: Arc<RwLock<HashMap<String, PeerInfo>>>,
    event_sender: broadcast::Sender<DiscoveryEvent>,
    device_identity: Arc<DeviceIdentity>,
    service_name: String,
}

impl Discovery {
    /// Create new discovery service with device identity
    pub fn new(device_identity: Arc<DeviceIdentity>) -> Self {
        let (event_sender, _) = broadcast::channel(256);
        let service_name = format!("landropic-{}", device_identity.device_name());
        
        Self {
            mdns: None,
            registered_service: None,
            peers: Arc::new(RwLock::new(HashMap::new())),
            event_sender,
            device_identity,
            service_name,
        }
    }
    
    /// Subscribe to discovery events
    pub fn subscribe(&self) -> broadcast::Receiver<DiscoveryEvent> {
        self.event_sender.subscribe()
    }
    
    /// Initialize the mDNS daemon
    pub async fn initialize(&mut self) -> Result<()> {
        if self.mdns.is_some() {
            return Ok(());
        }
        
        let mdns = ServiceDaemon::new()
            .map_err(|e| DiscoveryError::DaemonStart(e.to_string()))?;
            
        self.mdns = Some(mdns);
        info!("mDNS daemon initialized");
        Ok(())
    }
    
    /// Start advertising our service with retry logic
    pub async fn start_advertising(&mut self, port: u16, capabilities: Vec<String>) -> Result<()> {
        if self.registered_service.is_some() {
            return Err(DiscoveryError::AlreadyRegistered);
        }
        
        self.initialize().await?;
        
        let mut attempts = 0;
        loop {
            match self.register_service(port, &capabilities).await {
                Ok(()) => {
                    info!("mDNS service registration successful on port {}", port);
                    let _ = self.event_sender.send(DiscoveryEvent::ServiceRegistered);
                    return Ok(());
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= MAX_RETRIES {
                        error!("Failed to register service after {} attempts: {}", MAX_RETRIES, e);
                        return Err(e);
                    }
                    warn!("Service registration attempt {} failed: {}. Retrying in {:?}", 
                          attempts, e, RETRY_DELAY);
                    sleep(RETRY_DELAY).await;
                }
            }
        }
    }
    
    /// Internal service registration
    async fn register_service(&mut self, port: u16, capabilities: &[String]) -> Result<()> {
        let mdns = self.mdns.as_ref()
            .ok_or_else(|| DiscoveryError::DaemonStart("mDNS daemon not initialized".to_string()))?;
        
        let device_id = self.device_identity.device_id();
        let hostname = format!("{}.local.", gethostname::gethostname().to_string_lossy());
        
        // Build TXT records with device capabilities
        let mut properties = HashMap::new();
        properties.insert("device_id".to_string(), device_id.to_hex());
        properties.insert("device_name".to_string(), self.device_identity.device_name().to_string());
        properties.insert("hostname".to_string(), hostname.clone());
        properties.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
        properties.insert("protocol_version".to_string(), "1".to_string());
        
        // Add capabilities as comma-separated values
        if !capabilities.is_empty() {
            properties.insert("capabilities".to_string(), capabilities.join(","));
        }
        
        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &self.service_name,
            &hostname,
            "", // Let the system choose IP addresses
            port,
            properties,
        )
        .map_err(|e| DiscoveryError::ServiceRegistration(e.to_string()))?;
        
        // Register service (synchronous operation)
        mdns.register(service_info)
            .map_err(|e| DiscoveryError::ServiceRegistration(e.to_string()))?;
        
        self.registered_service = Some(self.service_name.clone());
        Ok(())
    }
    
    /// Start browsing for peer services
    pub async fn start_browsing(&mut self) -> Result<()> {
        self.initialize().await?;
        
        let mdns = self.mdns.as_ref()
            .ok_or_else(|| DiscoveryError::DaemonStart("mDNS daemon not initialized".to_string()))?;
        
        let receiver = mdns.browse(SERVICE_TYPE)
            .map_err(|e| DiscoveryError::ServiceBrowsing(e.to_string()))?;
        
        let peers = Arc::clone(&self.peers);
        let event_sender = self.event_sender.clone();
        let device_id = self.device_identity.device_id();
        
        // Spawn task to handle service events
        tokio::spawn(async move {
            Self::handle_service_events(receiver, peers, event_sender, device_id).await;
        });
        
        info!("Started browsing for landropic peers");
        Ok(())
    }
    
    /// Handle incoming service events
    async fn handle_service_events(
        receiver: mdns_sd::Receiver<ServiceEvent>,
        peers: Arc<RwLock<HashMap<String, PeerInfo>>>,
        event_sender: broadcast::Sender<DiscoveryEvent>,
        our_device_id: landro_crypto::DeviceId,
    ) {
        while let Ok(event) = receiver.recv_async().await {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    match Self::parse_service_info(&info, &our_device_id) {
                        Ok(Some(peer_info)) => {
                            debug!("Discovered peer: {} at {}:{}", 
                                   peer_info.device_name, peer_info.address, peer_info.port);
                            
                            let mut peers_guard = peers.write().await;
                            let is_new = !peers_guard.contains_key(&peer_info.device_id);
                            peers_guard.insert(peer_info.device_id.clone(), peer_info.clone());
                            drop(peers_guard);
                            
                            let event = if is_new {
                                DiscoveryEvent::PeerDiscovered(peer_info)
                            } else {
                                DiscoveryEvent::PeerUpdated(peer_info)
                            };
                            
                            let _ = event_sender.send(event);
                        }
                        Ok(None) => {
                            // This was our own service - ignore
                        }
                        Err(e) => {
                            warn!("Failed to parse service info: {}", e);
                            let _ = event_sender.send(DiscoveryEvent::NetworkError(e.to_string()));
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(typ, name) => {
                    debug!("Service removed: {} ({})", name, typ);
                    
                    // Find and remove the peer by service name
                    let mut peers_guard = peers.write().await;
                    let removed_device_id = peers_guard
                        .iter()
                        .find(|(_, peer)| peer.service_name == name)
                        .map(|(device_id, _)| device_id.clone());
                    
                    if let Some(device_id) = removed_device_id {
                        peers_guard.remove(&device_id);
                        drop(peers_guard);
                        let _ = event_sender.send(DiscoveryEvent::PeerRemoved(device_id));
                    }
                }
                _ => {
                    // Handle other service events if needed
                    debug!("Other service event: {:?}", event);
                }
            }
        }
    }
    
    /// Parse service info into peer information
    fn parse_service_info(
        service: &ServiceInfo,
        our_device_id: &landro_crypto::DeviceId,
    ) -> Result<Option<PeerInfo>> {
        let properties = service.get_properties();
        
        // Extract device ID
        let device_id_str = properties.get("device_id")
            .ok_or_else(|| DiscoveryError::InvalidServiceData("Missing device_id".to_string()))?;
        
        // The TXT property format is "key=value", so we need to extract just the value
        let device_id_hex = device_id_str.to_string();
        let device_id_hex = if device_id_hex.starts_with("device_id=") {
            &device_id_hex[10..] // Skip "device_id="
        } else {
            &device_id_hex
        };
            
        let device_id = landro_crypto::DeviceId::from_str(device_id_hex)
            .map_err(|e| DiscoveryError::InvalidServiceData(format!("Invalid device_id: {}", e)))?;
        
        // Skip our own service
        if device_id == *our_device_id {
            return Ok(None);
        }
        
        // Extract device name
        let device_name = properties.get("device_name")
            .map(|p| {
                let name_str = p.to_string();
                if name_str.starts_with("device_name=") {
                    name_str[12..].to_string() // Skip "device_name="
                } else {
                    name_str
                }
            })
            .unwrap_or_else(|| service.get_hostname().to_string());
        
        // Get first available address
        let addresses: Vec<IpAddr> = service.get_addresses().iter().copied().collect();
        let address = addresses.first()
            .ok_or_else(|| DiscoveryError::InvalidServiceData("No addresses available".to_string()))?;
        
        let capabilities = properties.get("capabilities")
            .map(|s| {
                let caps_str = s.to_string();
                let caps_str = if caps_str.starts_with("capabilities=") {
                    &caps_str[13..] // Skip "capabilities="
                } else {
                    &caps_str
                };
                caps_str.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()
            })
            .unwrap_or_default();
            
        let protocol_version = properties.get("protocol_version")
            .and_then(|s| {
                let version_str = s.to_string();
                let version_str = if version_str.starts_with("protocol_version=") {
                    &version_str[17..] // Skip "protocol_version="
                } else {
                    &version_str
                };
                version_str.parse().ok()
            })
            .unwrap_or(1);
        
        Ok(Some(PeerInfo {
            device_id: device_id.to_hex(),
            device_name: device_name.clone(),
            service_name: service.get_fullname().to_string(),
            address: address.to_string(),
            port: service.get_port(),
            capabilities,
            protocol_version,
            last_seen: std::time::SystemTime::now(),
        }))
    }
    
    /// Get currently discovered peers
    pub async fn get_peers(&self) -> Vec<PeerInfo> {
        self.peers.read().await.values().cloned().collect()
    }
    
    /// Get a specific peer by device ID
    pub async fn get_peer(&self, device_id: &str) -> Option<PeerInfo> {
        self.peers.read().await.get(device_id).cloned()
    }
    
    /// Stop advertising our service
    pub async fn stop_advertising(&mut self) -> Result<()> {
        if let Some(service_name) = self.registered_service.take() {
            if let Some(mdns) = &self.mdns {
                mdns.unregister(&service_name)
                    .map_err(|e| DiscoveryError::ServiceRegistration(e.to_string()))?;
                info!("Unregistered mDNS service: {}", service_name);
                let _ = self.event_sender.send(DiscoveryEvent::ServiceUnregistered);
            }
        }
        Ok(())
    }
    
    /// Shutdown the discovery service
    pub async fn shutdown(&mut self) -> Result<()> {
        // Stop advertising
        if let Err(e) = self.stop_advertising().await {
            warn!("Error stopping advertising during shutdown: {}", e);
        }
        
        // Shutdown mDNS daemon
        if let Some(mdns) = self.mdns.take() {
            mdns.shutdown().map_err(|e| DiscoveryError::DaemonStart(e.to_string()))?;
            info!("mDNS daemon shut down");
        }
        
        // Clear peers
        self.peers.write().await.clear();
        
        Ok(())
    }
    
    /// Force refresh of peer discovery
    pub async fn refresh_peers(&mut self) -> Result<()> {
        // Clear existing peers
        self.peers.write().await.clear();
        
        // Restart browsing
        self.start_browsing().await?;
        
        info!("Refreshed peer discovery");
        Ok(())
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if self.mdns.is_some() {
            warn!("Discovery service dropped without proper shutdown");
        }
    }
}

#[cfg(test)]
mod tests;

/// Discovered peer information
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub device_id: String,
    pub device_name: String,
    pub service_name: String,
    pub address: String,
    pub port: u16,
    pub capabilities: Vec<String>,
    pub protocol_version: u32,
    pub last_seen: std::time::SystemTime,
}

impl PeerInfo {
    /// Get socket address for connecting to this peer
    pub fn socket_addr(&self) -> std::result::Result<SocketAddr, std::net::AddrParseError> {
        format!("{}:{}", self.address, self.port).parse()
    }
    
    /// Check if peer supports a specific capability
    pub fn supports_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
    
    /// Check if peer is recently seen (within the last 30 seconds)
    pub fn is_recently_active(&self) -> bool {
        self.last_seen.elapsed().unwrap_or(Duration::from_secs(u64::MAX)) < Duration::from_secs(30)
    }
}
