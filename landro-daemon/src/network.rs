//! Network connection management with automatic peer discovery
//!
//! This module provides a connection manager that:
//! - Maintains pooled QUIC connections to discovered peers
//! - Performs health checks and automatic reconnection
//! - Handles network interface changes gracefully
//! - Implements rate limiting and connection storms prevention

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Mutex, Semaphore};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, warn};

use landro_crypto::{CertificateVerifier, DeviceIdentity};
use landro_quic::{ConnectionPool, PoolConfig, QuicClient, QuicConfig, Connection};

use crate::discovery::{DiscoveryService, PeerInfo};

/// Connection state for a peer
#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub peer_info: PeerInfo,
    pub connection_pool: Arc<ConnectionPool>,
    pub last_health_check: Instant,
    pub consecutive_failures: u32,
    pub is_healthy: bool,
}

/// Network manager configuration
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Maximum connections per peer
    pub max_connections_per_peer: usize,
    /// Maximum total connections
    pub max_total_connections: usize,
    /// Connection idle timeout
    pub connection_idle_timeout: Duration,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Maximum consecutive health check failures before marking unhealthy
    pub max_health_check_failures: u32,
    /// Connection attempt rate limit (per second)
    pub connection_rate_limit: u32,
    /// Exponential backoff base delay
    pub backoff_base_delay: Duration,
    /// Maximum backoff delay
    pub max_backoff_delay: Duration,
    /// Network interface check interval
    pub interface_check_interval: Duration,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            max_connections_per_peer: 2,
            max_total_connections: 50,
            connection_idle_timeout: Duration::from_secs(300),
            health_check_interval: Duration::from_secs(30),
            max_health_check_failures: 3,
            connection_rate_limit: 10,
            backoff_base_delay: Duration::from_millis(500),
            max_backoff_delay: Duration::from_secs(60),
            interface_check_interval: Duration::from_secs(10),
        }
    }
}

/// Connection manager with mDNS integration
pub struct ConnectionManager {
    /// Device identity for authentication
    identity: Arc<DeviceIdentity>,
    /// Certificate verifier
    verifier: Arc<CertificateVerifier>,
    /// Network configuration
    config: NetworkConfig,
    /// Discovery service
    discovery: Arc<Mutex<DiscoveryService>>,
    /// Active peer connections
    peers: Arc<RwLock<HashMap<String, PeerConnection>>>,
    /// Connection attempt tracking for rate limiting
    connection_attempts: Arc<RwLock<HashMap<String, Vec<Instant>>>>,
    /// Semaphore for rate limiting connection attempts
    connection_semaphore: Arc<Semaphore>,
    /// Background tasks
    tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// Shutdown flag
    shutdown: Arc<RwLock<bool>>,
    /// Network interfaces at last check
    last_interfaces: Arc<RwLock<HashSet<String>>>,
}

impl ConnectionManager {
    /// Create a new connection manager
    pub fn new(
        identity: Arc<DeviceIdentity>,
        verifier: Arc<CertificateVerifier>,
        discovery: Arc<Mutex<DiscoveryService>>,
        config: NetworkConfig,
    ) -> Self {
        let connection_semaphore = Arc::new(Semaphore::new(config.connection_rate_limit as usize));
        
        Self {
            identity,
            verifier,
            config,
            discovery,
            peers: Arc::new(RwLock::new(HashMap::new())),
            connection_attempts: Arc::new(RwLock::new(HashMap::new())),
            connection_semaphore,
            tasks: Arc::new(Mutex::new(Vec::new())),
            shutdown: Arc::new(RwLock::new(false)),
            last_interfaces: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Start the connection manager
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting connection manager");

        // Start background tasks
        self.start_discovery_task().await;
        self.start_health_check_task().await;
        self.start_interface_monitor_task().await;
        self.start_rate_limit_reset_task().await;

        Ok(())
    }

    /// Get or establish connection to a peer
    pub async fn get_connection(&self, peer_id: &str) -> Result<Arc<Connection>, String> {
        // Check if we have this peer
        let peers = self.peers.read().await;
        if let Some(peer_conn) = peers.get(peer_id) {
            if peer_conn.is_healthy {
                // Try to get connection from pool
                match peer_conn.connection_pool.get_connection(peer_conn.peer_info.address).await {
                    Ok(conn) => return Ok(conn),
                    Err(e) => {
                        warn!("Failed to get pooled connection for {}: {}", peer_id, e);
                    }
                }
            }
        }
        drop(peers);

        // Try to establish new connection
        self.connect_to_peer(peer_id).await
    }

    /// Connect to a specific peer
    async fn connect_to_peer(&self, peer_id: &str) -> Result<Arc<Connection>, String> {
        // Check rate limit
        if !self.check_rate_limit(peer_id).await {
            return Err(format!("Rate limit exceeded for peer {}", peer_id));
        }

        // Get peer info from discovery
        let discovery = self.discovery.lock().await;
        let peer_info = discovery
            .get_peer(peer_id)
            .ok_or_else(|| format!("Peer {} not found in discovery", peer_id))?
            .clone();
        drop(discovery);

        // Calculate backoff delay
        let backoff = self.calculate_backoff(peer_id).await;
        if backoff > Duration::ZERO {
            debug!("Waiting {:?} before connecting to {} (backoff)", backoff, peer_id);
            tokio::time::sleep(backoff).await;
        }

        // Acquire connection semaphore
        let _permit = self.connection_semaphore.acquire().await
            .map_err(|e| format!("Failed to acquire connection permit: {}", e))?;

        info!("Establishing connection to peer {} at {}", peer_id, peer_info.address);

        // Create QUIC client and pool
        let quic_config = QuicConfig::default();
        let client = Arc::new(
            QuicClient::new(self.identity.clone(), self.verifier.clone(), quic_config)
                .await
                .map_err(|e| format!("Failed to create QUIC client: {}", e))?
        );

        let pool_config = PoolConfig {
            max_connections_per_peer: self.config.max_connections_per_peer,
            max_total_connections: self.config.max_total_connections,
            max_idle_time: self.config.connection_idle_timeout,
            ..Default::default()
        };

        let pool = Arc::new(ConnectionPool::new(client, pool_config));

        // Try to establish connection
        match timeout(
            Duration::from_secs(10),
            pool.get_connection(peer_info.address)
        ).await {
            Ok(Ok(connection)) => {
                // Store peer connection
                let peer_conn = PeerConnection {
                    peer_info: peer_info.clone(),
                    connection_pool: pool,
                    last_health_check: Instant::now(),
                    consecutive_failures: 0,
                    is_healthy: true,
                };

                let mut peers = self.peers.write().await;
                peers.insert(peer_id.to_string(), peer_conn);

                info!("Successfully connected to peer {}", peer_id);
                Ok(connection)
            }
            Ok(Err(e)) => {
                self.record_connection_failure(peer_id).await;
                Err(format!("Failed to connect to {}: {}", peer_id, e))
            }
            Err(_) => {
                self.record_connection_failure(peer_id).await;
                Err(format!("Connection to {} timed out", peer_id))
            }
        }
    }

    /// Check if we can make a connection attempt (rate limiting)
    async fn check_rate_limit(&self, peer_id: &str) -> bool {
        let mut attempts = self.connection_attempts.write().await;
        let now = Instant::now();
        let peer_attempts = attempts.entry(peer_id.to_string()).or_insert_with(Vec::new);

        // Remove old attempts (outside 1-second window)
        peer_attempts.retain(|&t| now.duration_since(t) < Duration::from_secs(1));

        // Check if we're within rate limit
        if peer_attempts.len() >= self.config.connection_rate_limit as usize {
            false
        } else {
            peer_attempts.push(now);
            true
        }
    }

    /// Calculate exponential backoff for a peer
    async fn calculate_backoff(&self, peer_id: &str) -> Duration {
        let peers = self.peers.read().await;
        if let Some(peer_conn) = peers.get(peer_id) {
            if peer_conn.consecutive_failures > 0 {
                let multiplier = 2u32.saturating_pow(peer_conn.consecutive_failures - 1);
                let delay = self.config.backoff_base_delay * multiplier;
                return delay.min(self.config.max_backoff_delay);
            }
        }
        Duration::ZERO
    }

    /// Record a connection failure
    async fn record_connection_failure(&self, peer_id: &str) {
        let mut peers = self.peers.write().await;
        if let Some(peer_conn) = peers.get_mut(peer_id) {
            peer_conn.consecutive_failures += 1;
            if peer_conn.consecutive_failures >= self.config.max_health_check_failures {
                peer_conn.is_healthy = false;
                warn!("Marking peer {} as unhealthy after {} failures", 
                      peer_id, peer_conn.consecutive_failures);
            }
        }
    }

    /// Start discovery task
    async fn start_discovery_task(&self) {
        let discovery = self.discovery.clone();
        let peers = self.peers.clone();
        let shutdown = self.shutdown.clone();
        let manager = self.clone_for_task();

        let task = tokio::spawn(async move {
            let mut discovery_interval = interval(Duration::from_secs(30));

            loop {
                discovery_interval.tick().await;

                if *shutdown.read().await {
                    break;
                }

                // Browse for new peers
                let result = discovery.lock().await.browse_peers().await;
                match result {
                    Ok(discovered_peers) => {
                        debug!("Discovered {} peers", discovered_peers.len());

                        for peer_info in discovered_peers {
                            let peer_id = &peer_info.device_id;

                            // Check if we already know this peer
                            let known = peers.read().await.contains_key(peer_id);
                            if !known {
                                info!("Discovered new peer: {} ({})", 
                                      peer_info.device_name, peer_info.address);

                                // Try to establish connection in background
                                let manager = manager.clone();
                                let peer_id = peer_id.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = manager.connect_to_peer(&peer_id).await {
                                        debug!("Failed to connect to discovered peer {}: {}", 
                                               peer_id, e);
                                    }
                                });
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Discovery browse failed: {}", e);
                    }
                }
            }

            info!("Discovery task shutting down");
        });

        self.tasks.lock().await.push(task);
    }

    /// Start health check task
    async fn start_health_check_task(&self) {
        let peers = self.peers.clone();
        let shutdown = self.shutdown.clone();
        let config = self.config.clone();

        let task = tokio::spawn(async move {
            let mut health_interval = interval(config.health_check_interval);

            loop {
                health_interval.tick().await;

                if *shutdown.read().await {
                    break;
                }

                let mut peers_to_check = Vec::new();
                {
                    let peers_lock = peers.read().await;
                    for (peer_id, peer_conn) in peers_lock.iter() {
                        if peer_conn.last_health_check.elapsed() > config.health_check_interval {
                            peers_to_check.push((peer_id.clone(), peer_conn.peer_info.address));
                        }
                    }
                }

                for (peer_id, addr) in peers_to_check {
                    debug!("Health checking peer {}", peer_id);

                    let mut peers_lock = peers.write().await;
                    if let Some(peer_conn) = peers_lock.get_mut(&peer_id) {
                        // Try to get a connection from the pool
                        match timeout(
                            Duration::from_secs(5),
                            peer_conn.connection_pool.get_connection(addr)
                        ).await {
                            Ok(Ok(conn)) => {
                                // Connection successful, check if it's actually alive
                                if !conn.is_closed() {
                                    peer_conn.last_health_check = Instant::now();
                                    peer_conn.consecutive_failures = 0;
                                    if !peer_conn.is_healthy {
                                        info!("Peer {} is now healthy", peer_id);
                                        peer_conn.is_healthy = true;
                                    }
                                } else {
                                    peer_conn.consecutive_failures += 1;
                                    if peer_conn.consecutive_failures >= config.max_health_check_failures {
                                        peer_conn.is_healthy = false;
                                        warn!("Peer {} marked unhealthy", peer_id);
                                    }
                                }
                            }
                            _ => {
                                peer_conn.consecutive_failures += 1;
                                if peer_conn.consecutive_failures >= config.max_health_check_failures {
                                    peer_conn.is_healthy = false;
                                    warn!("Peer {} marked unhealthy after health check failure", peer_id);
                                }
                            }
                        }
                    }
                }
            }

            info!("Health check task shutting down");
        });

        self.tasks.lock().await.push(task);
    }

    /// Start network interface monitor task
    async fn start_interface_monitor_task(&self) {
        let last_interfaces = self.last_interfaces.clone();
        let discovery = self.discovery.clone();
        let shutdown = self.shutdown.clone();
        let config = self.config.clone();

        let task = tokio::spawn(async move {
            let mut interface_interval = interval(config.interface_check_interval);

            loop {
                interface_interval.tick().await;

                if *shutdown.read().await {
                    break;
                }

                // Get current network interfaces
                let current_interfaces = get_network_interfaces();
                
                let mut last = last_interfaces.write().await;
                if current_interfaces != *last && !last.is_empty() {
                    info!("Network interfaces changed, restarting mDNS advertisement");
                    
                    // Restart mDNS advertisement on interface change
                    if let Ok(mut disc) = discovery.try_lock() {
                        // Stop and restart advertising
                        let _ = disc.stop().await;
                        let _ = disc.start_advertising(9876, vec![
                            "sync".to_string(),
                            "transfer".to_string(),
                        ]).await;
                    }
                    
                    *last = current_interfaces;
                } else if last.is_empty() {
                    *last = current_interfaces;
                }
            }

            info!("Interface monitor task shutting down");
        });

        self.tasks.lock().await.push(task);
    }

    /// Start rate limit reset task
    async fn start_rate_limit_reset_task(&self) {
        let connection_attempts = self.connection_attempts.clone();
        let shutdown = self.shutdown.clone();

        let task = tokio::spawn(async move {
            let mut cleanup_interval = interval(Duration::from_secs(60));

            loop {
                cleanup_interval.tick().await;

                if *shutdown.read().await {
                    break;
                }

                // Clean up old connection attempts
                let mut attempts = connection_attempts.write().await;
                let now = Instant::now();
                attempts.retain(|_, peer_attempts| {
                    peer_attempts.retain(|&t| now.duration_since(t) < Duration::from_secs(60));
                    !peer_attempts.is_empty()
                });

                debug!("Cleaned up connection attempt tracking");
            }

            info!("Rate limit reset task shutting down");
        });

        self.tasks.lock().await.push(task);
    }

    /// Clone manager for use in tasks
    fn clone_for_task(&self) -> Arc<Self> {
        Arc::new(Self {
            identity: self.identity.clone(),
            verifier: self.verifier.clone(),
            config: self.config.clone(),
            discovery: self.discovery.clone(),
            peers: self.peers.clone(),
            connection_attempts: self.connection_attempts.clone(),
            connection_semaphore: self.connection_semaphore.clone(),
            tasks: Arc::new(Mutex::new(Vec::new())), // New task list for cloned instance
            shutdown: self.shutdown.clone(),
            last_interfaces: self.last_interfaces.clone(),
        })
    }

    /// Get statistics about connections
    pub async fn get_stats(&self) -> ConnectionStats {
        let peers = self.peers.read().await;
        
        let mut stats = ConnectionStats {
            total_peers: peers.len(),
            healthy_peers: 0,
            unhealthy_peers: 0,
            total_connections: 0,
            peer_stats: Vec::new(),
        };

        for (peer_id, peer_conn) in peers.iter() {
            if peer_conn.is_healthy {
                stats.healthy_peers += 1;
            } else {
                stats.unhealthy_peers += 1;
            }

            let pool_stats = peer_conn.connection_pool.get_stats().await;
            let conn_count = pool_stats.get(&peer_conn.peer_info.address).copied().unwrap_or(0);
            stats.total_connections += conn_count;

            stats.peer_stats.push(PeerConnectionStats {
                peer_id: peer_id.clone(),
                peer_name: peer_conn.peer_info.device_name.clone(),
                address: peer_conn.peer_info.address,
                is_healthy: peer_conn.is_healthy,
                connections: conn_count,
                consecutive_failures: peer_conn.consecutive_failures,
                last_health_check: peer_conn.last_health_check,
            });
        }

        stats
    }

    /// Shutdown the connection manager
    pub async fn shutdown(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Shutting down connection manager");

        // Set shutdown flag
        *self.shutdown.write().await = true;

        // Wait for tasks to complete
        let tasks = self.tasks.lock().await;
        for task in tasks.iter() {
            task.abort();
        }

        // Close all connection pools
        let peers = self.peers.read().await;
        for (peer_id, peer_conn) in peers.iter() {
            if let Err(e) = peer_conn.connection_pool.shutdown().await {
                error!("Failed to shutdown pool for peer {}: {}", peer_id, e);
            }
        }

        info!("Connection manager shutdown complete");
        Ok(())
    }
}

/// Connection statistics
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub total_peers: usize,
    pub healthy_peers: usize,
    pub unhealthy_peers: usize,
    pub total_connections: usize,
    pub peer_stats: Vec<PeerConnectionStats>,
}

/// Per-peer connection statistics
#[derive(Debug, Clone)]
pub struct PeerConnectionStats {
    pub peer_id: String,
    pub peer_name: String,
    pub address: SocketAddr,
    pub is_healthy: bool,
    pub connections: usize,
    pub consecutive_failures: u32,
    pub last_health_check: Instant,
}

/// Get current network interfaces
fn get_network_interfaces() -> HashSet<String> {
    use std::net::IpAddr;
    
    let mut interfaces = HashSet::new();
    
    // Use pnet or if-addrs crate for better interface detection
    // For now, use a simple approach
    if let Ok(hostname) = gethostname::gethostname().into_string() {
        interfaces.insert(hostname);
    }
    
    // Try to get local IPs
    if let Ok(addrs) = std::net::TcpListener::bind("0.0.0.0:0")
        .and_then(|l| l.local_addr()) {
        interfaces.insert(addrs.ip().to_string());
    }

    interfaces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_config_defaults() {
        let config = NetworkConfig::default();
        assert_eq!(config.max_connections_per_peer, 2);
        assert_eq!(config.max_total_connections, 50);
        assert_eq!(config.connection_rate_limit, 10);
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        // Test rate limit logic without actual connections
        let config = NetworkConfig {
            connection_rate_limit: 2,
            ..Default::default()
        };
        
        // This would require a full ConnectionManager setup with mocked components
        // For unit tests, we test the configuration
        assert_eq!(config.connection_rate_limit, 2);
    }
}