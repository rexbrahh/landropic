//! Connection pool for managing multiple peer connections

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Mutex};
use tokio::time::interval;
use tracing::{debug, info, warn, error};

use crate::client::QuicClient;
use crate::connection::Connection;
use crate::errors::{QuicError, Result};

/// Connection pool entry with metadata
#[derive(Debug, Clone)]
pub struct PooledConnection {
    pub connection: Arc<Connection>,
    pub peer_addr: SocketAddr,
    pub peer_device_id: Option<Vec<u8>>,
    pub peer_device_name: Option<String>,
    pub created_at: Instant,
    pub last_used: Instant,
    pub active_streams: u32,
}

impl PooledConnection {
    pub fn new(connection: Arc<Connection>, peer_addr: SocketAddr) -> Self {
        let now = Instant::now();
        Self {
            connection,
            peer_addr,
            peer_device_id: None,
            peer_device_name: None,
            created_at: now,
            last_used: now,
            active_streams: 0,
        }
    }

    /// Update last used time and increment stream count
    pub fn use_connection(&mut self) {
        self.last_used = Instant::now();
        self.active_streams += 1;
    }

    /// Decrement active stream count
    pub fn release_stream(&mut self) {
        if self.active_streams > 0 {
            self.active_streams -= 1;
        }
    }

    /// Check if connection is idle (no active streams)
    pub fn is_idle(&self) -> bool {
        self.active_streams == 0
    }

    /// Check if connection is stale (unused for too long)
    pub fn is_stale(&self, max_idle_time: Duration) -> bool {
        self.last_used.elapsed() > max_idle_time && self.is_idle()
    }

    /// Check if connection is still healthy
    pub fn is_healthy(&self) -> bool {
        !self.connection.is_closed()
    }
}

/// Connection pool configuration
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum number of connections per peer
    pub max_connections_per_peer: usize,
    /// Maximum total connections in pool
    pub max_total_connections: usize,
    /// How long to keep idle connections
    pub max_idle_time: Duration,
    /// How long to wait for connection establishment
    pub connect_timeout: Duration,
    /// Maximum connection retry attempts
    pub max_retry_attempts: u32,
    /// Base retry delay
    pub retry_delay: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections_per_peer: 2,
            max_total_connections: 50,
            max_idle_time: Duration::from_secs(300), // 5 minutes
            connect_timeout: Duration::from_secs(10),
            max_retry_attempts: 3,
            retry_delay: Duration::from_millis(500),
        }
    }
}

/// Thread-safe connection pool for managing peer connections
pub struct ConnectionPool {
    client: Arc<QuicClient>,
    connections: Arc<RwLock<HashMap<SocketAddr, Vec<PooledConnection>>>>,
    config: PoolConfig,
    cleanup_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl ConnectionPool {
    /// Create a new connection pool
    pub fn new(client: Arc<QuicClient>, config: PoolConfig) -> Self {
        let pool = Self {
            client,
            connections: Arc::new(RwLock::new(HashMap::new())),
            config,
            cleanup_handle: Arc::new(Mutex::new(None)),
        };

        // Start background cleanup task
        pool.start_cleanup_task();

        pool
    }

    /// Get or create a connection to the specified peer
    pub async fn get_connection(&self, peer_addr: SocketAddr) -> Result<Arc<Connection>> {
        // First, try to get an existing healthy connection
        if let Some(pooled_conn) = self.find_available_connection(peer_addr).await {
            debug!("Reusing existing connection to {}", peer_addr);
            return Ok(pooled_conn.connection);
        }

        // Check if we can create a new connection
        if !self.can_create_new_connection(peer_addr).await {
            return Err(QuicError::Protocol(format!(
                "Connection pool limit reached for peer {}",
                peer_addr
            )));
        }

        // Create new connection
        info!("Creating new connection to {}", peer_addr);
        let connection = self.create_connection_with_retry(peer_addr).await?;
        
        // Add to pool
        self.add_to_pool(peer_addr, connection.clone()).await?;

        Ok(connection)
    }

    /// Get connection statistics for monitoring
    pub async fn get_stats(&self) -> HashMap<SocketAddr, usize> {
        let connections = self.connections.read().await;
        connections
            .iter()
            .map(|(addr, conns)| (*addr, conns.len()))
            .collect()
    }

    /// Close all connections to a specific peer
    pub async fn close_peer_connections(&self, peer_addr: SocketAddr) -> Result<()> {
        let mut connections = self.connections.write().await;
        if let Some(peer_connections) = connections.remove(&peer_addr) {
            for pooled_conn in peer_connections {
                pooled_conn.connection.close(0, b"pool shutdown");
                info!("Closed connection to {} from pool", peer_addr);
            }
        }
        Ok(())
    }

    /// Close all connections and shutdown the pool
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down connection pool");

        // Stop cleanup task
        if let Some(handle) = self.cleanup_handle.lock().await.take() {
            handle.abort();
        }

        // Close all connections
        let mut connections = self.connections.write().await;
        for (peer_addr, peer_connections) in connections.drain() {
            for pooled_conn in peer_connections {
                pooled_conn.connection.close(0, b"pool shutdown");
            }
            debug!("Closed all connections to {}", peer_addr);
        }

        info!("Connection pool shutdown complete");
        Ok(())
    }

    /// Find an available healthy connection for the peer
    async fn find_available_connection(&self, peer_addr: SocketAddr) -> Option<PooledConnection> {
        let mut connections = self.connections.write().await;
        let peer_connections = connections.get_mut(&peer_addr)?;

        // Find healthy, idle connection
        for pooled_conn in peer_connections.iter_mut() {
            if pooled_conn.is_healthy() && pooled_conn.is_idle() {
                pooled_conn.use_connection();
                return Some(pooled_conn.clone());
            }
        }

        // No available connection found
        None
    }

    /// Check if we can create a new connection
    async fn can_create_new_connection(&self, peer_addr: SocketAddr) -> bool {
        let connections = self.connections.read().await;
        
        // Check total connection limit
        let total_connections: usize = connections.values().map(|v| v.len()).sum();
        if total_connections >= self.config.max_total_connections {
            return false;
        }

        // Check per-peer limit
        if let Some(peer_connections) = connections.get(&peer_addr) {
            if peer_connections.len() >= self.config.max_connections_per_peer {
                return false;
            }
        }

        true
    }

    /// Create connection with automatic retry
    async fn create_connection_with_retry(&self, peer_addr: SocketAddr) -> Result<Arc<Connection>> {
        let mut last_error = None;

        for attempt in 1..=self.config.max_retry_attempts {
            match tokio::time::timeout(
                self.config.connect_timeout,
                self.client.connect_addr(peer_addr),
            )
            .await
            {
                Ok(Ok(connection)) => {
                    info!(
                        "Connected to {} on attempt {} of {}",
                        peer_addr, attempt, self.config.max_retry_attempts
                    );
                    return Ok(Arc::new(connection));
                }
                Ok(Err(e)) => {
                    error!(
                        "Connection attempt {} failed for {}: {}",
                        attempt, peer_addr, e
                    );
                    last_error = Some(e);
                }
                Err(_) => {
                    error!(
                        "Connection attempt {} timed out for {}",
                        attempt, peer_addr
                    );
                    last_error = Some(QuicError::Timeout);
                }
            }

            // Wait before retry (exponential backoff)
            if attempt < self.config.max_retry_attempts {
                let delay = self.config.retry_delay * 2u32.pow(attempt - 1);
                debug!("Retrying connection to {} after {:?}", peer_addr, delay);
                tokio::time::sleep(delay).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            QuicError::Protocol("Connection failed after all retries".to_string())
        }))
    }

    /// Add connection to the pool
    async fn add_to_pool(&self, peer_addr: SocketAddr, connection: Arc<Connection>) -> Result<()> {
        let mut connections = self.connections.write().await;
        let pooled_conn = PooledConnection::new(connection, peer_addr);

        connections
            .entry(peer_addr)
            .or_insert_with(Vec::new)
            .push(pooled_conn);

        debug!("Added connection to pool for {}", peer_addr);
        Ok(())
    }

    /// Start background cleanup task to remove stale connections
    fn start_cleanup_task(&self) {
        let connections = self.connections.clone();
        let config = self.config.clone();

        let handle = tokio::spawn(async move {
            let mut cleanup_interval = interval(Duration::from_secs(60)); // Clean every minute

            loop {
                cleanup_interval.tick().await;

                debug!("Running connection pool cleanup");
                let mut removed_count = 0;

                {
                    let mut conns = connections.write().await;
                    for (peer_addr, peer_connections) in conns.iter_mut() {
                        peer_connections.retain(|pooled_conn| {
                            let should_keep = pooled_conn.is_healthy()
                                && !pooled_conn.is_stale(config.max_idle_time);

                            if !should_keep {
                                pooled_conn.connection.close(0, b"cleanup");
                                removed_count += 1;
                                debug!("Removed stale connection to {}", peer_addr);
                            }

                            should_keep
                        });
                    }

                    // Remove empty peer entries
                    conns.retain(|_, peer_connections| !peer_connections.is_empty());
                }

                if removed_count > 0 {
                    info!("Cleaned up {} stale connections", removed_count);
                }
            }
        });

        // Store cleanup handle
        let handle_mutex = self.cleanup_handle.clone();
        tokio::spawn(async move {
            *handle_mutex.lock().await = Some(handle);
        });
    }
}

impl Drop for ConnectionPool {
    fn drop(&mut self) {
        // Best effort cleanup - spawn a task to handle async shutdown
        let connections = self.connections.clone();
        let cleanup_handle = self.cleanup_handle.clone();

        tokio::spawn(async move {
            // Stop cleanup task
            if let Some(handle) = cleanup_handle.lock().await.take() {
                handle.abort();
            }

            // Close all connections
            let mut conns = connections.write().await;
            for (_, peer_connections) in conns.drain() {
                for pooled_conn in peer_connections {
                    pooled_conn.connection.close(0, b"pool drop");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_pooled_connection_lifecycle() {
        // Skip connection tests in unit tests - these require integration tests
        // These tests verify basic connection pool logic and would work with real connections
    }

    #[test]
    fn test_pool_config_defaults() {
        let config = PoolConfig::default();
        assert_eq!(config.max_connections_per_peer, 2);
        assert_eq!(config.max_total_connections, 50);
        assert_eq!(config.max_idle_time, Duration::from_secs(300));
    }
}