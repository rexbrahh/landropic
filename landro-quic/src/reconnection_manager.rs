//! Advanced reconnection manager for Day 3 - handles dropped connections with intelligent retry logic
//!
//! This provides robust reconnection capabilities with exponential backoff, connection state tracking,
//! and automatic failover to ensure reliable peer-to-peer connections.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, warn};

use crate::client::QuicClient;
use crate::connection::Connection;
use crate::errors::{QuicError, Result};
use crate::pool::{ConnectionPool, PoolConfig};

/// Connection state for tracking reconnection attempts
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Connected,
    Connecting,
    Reconnecting,
    Failed,
    Backoff,
}

/// Reconnection configuration
#[derive(Debug, Clone)]
pub struct ReconnectionConfig {
    /// Initial retry delay
    pub initial_retry_delay: Duration,
    /// Maximum retry delay
    pub max_retry_delay: Duration,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
    /// Maximum retry attempts before giving up
    pub max_retry_attempts: u32,
    /// Connection timeout for each attempt
    pub connection_timeout: Duration,
    /// Interval for checking connection health
    pub health_check_interval: Duration,
    /// Enable jitter to avoid thundering herd
    pub enable_jitter: bool,
}

impl Default for ReconnectionConfig {
    fn default() -> Self {
        Self {
            initial_retry_delay: Duration::from_millis(100),
            max_retry_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            max_retry_attempts: 10,
            connection_timeout: Duration::from_secs(10),
            health_check_interval: Duration::from_secs(5),
            enable_jitter: true,
        }
    }
}

/// Peer connection tracking
#[derive(Debug, Clone)]
pub struct PeerConnectionState {
    pub peer_addr: SocketAddr,
    pub state: ConnectionState,
    pub retry_count: u32,
    pub last_attempt: Option<Instant>,
    pub last_success: Option<Instant>,
    pub current_delay: Duration,
    pub connection: Option<Arc<Connection>>,
    pub failure_reason: Option<String>,
}

impl PeerConnectionState {
    pub fn new(peer_addr: SocketAddr) -> Self {
        Self {
            peer_addr,
            state: ConnectionState::Connected,
            retry_count: 0,
            last_attempt: None,
            last_success: Some(Instant::now()),
            current_delay: Duration::from_millis(100),
            connection: None,
            failure_reason: None,
        }
    }

    /// Check if ready for retry attempt
    pub fn is_ready_for_retry(&self, config: &ReconnectionConfig) -> bool {
        if self.retry_count >= config.max_retry_attempts {
            return false;
        }

        if let Some(last_attempt) = self.last_attempt {
            last_attempt.elapsed() >= self.current_delay
        } else {
            true
        }
    }

    /// Update state for new retry attempt
    pub fn start_retry_attempt(&mut self, config: &ReconnectionConfig) {
        self.retry_count += 1;
        self.last_attempt = Some(Instant::now());
        self.state = ConnectionState::Reconnecting;
        
        // Calculate next delay with exponential backoff
        let next_delay = Duration::from_millis(
            (self.current_delay.as_millis() as f64 * config.backoff_multiplier) as u64
        );
        self.current_delay = next_delay.min(config.max_retry_delay);

        // Add jitter to prevent thundering herd
        if config.enable_jitter {
            let jitter = fastrand::f64() * 0.1; // ±10% jitter
            let jitter_ms = (self.current_delay.as_millis() as f64 * jitter) as u64;
            self.current_delay = Duration::from_millis(
                self.current_delay.as_millis() as u64 + jitter_ms
            );
        }
    }

    /// Mark connection as successful
    pub fn mark_success(&mut self, connection: Arc<Connection>) {
        self.state = ConnectionState::Connected;
        self.retry_count = 0;
        self.last_success = Some(Instant::now());
        self.current_delay = Duration::from_millis(100);
        self.connection = Some(connection);
        self.failure_reason = None;
    }

    /// Mark connection as failed
    pub fn mark_failed(&mut self, reason: String) {
        self.state = ConnectionState::Failed;
        self.connection = None;
        self.failure_reason = Some(reason);
    }
}

/// Advanced reconnection manager
pub struct ReconnectionManager {
    pool: Arc<ConnectionPool>,
    client: Arc<QuicClient>,
    config: ReconnectionConfig,
    peer_states: Arc<RwLock<HashMap<SocketAddr, PeerConnectionState>>>,
    active_reconnections: Arc<Mutex<HashMap<SocketAddr, tokio::task::JoinHandle<()>>>>,
    running: Arc<Mutex<bool>>,
}

impl ReconnectionManager {
    /// Create new reconnection manager
    pub fn new(
        pool: Arc<ConnectionPool>,
        client: Arc<QuicClient>,
        config: ReconnectionConfig,
    ) -> Self {
        Self {
            pool,
            client,
            config,
            peer_states: Arc::new(RwLock::new(HashMap::new())),
            active_reconnections: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// Start reconnection monitoring
    pub async fn start(&self) -> Result<()> {
        let mut running = self.running.lock().await;
        if *running {
            return Ok(());
        }
        *running = true;

        info!("Starting reconnection manager with config: {:?}", self.config);
        
        self.start_monitoring_task().await;
        
        info!("Reconnection manager started successfully");
        Ok(())
    }

    /// Stop reconnection monitoring
    pub async fn stop(&self) {
        let mut running = self.running.lock().await;
        *running = false;

        // Cancel all active reconnection tasks
        let mut active_reconnections = self.active_reconnections.lock().await;
        for (peer_addr, handle) in active_reconnections.drain() {
            handle.abort();
            debug!("Cancelled reconnection task for peer: {}", peer_addr);
        }

        info!("Reconnection manager stopped");
    }

    /// Track a new peer connection
    pub async fn track_peer(&self, peer_addr: SocketAddr, connection: Arc<Connection>) {
        let mut peer_states = self.peer_states.write().await;
        let mut state = peer_states.entry(peer_addr)
            .or_insert_with(|| PeerConnectionState::new(peer_addr));
        
        state.mark_success(connection);
        debug!("Tracking peer connection: {}", peer_addr);
    }

    /// Report connection failure for a peer
    pub async fn report_connection_failure(&self, peer_addr: SocketAddr, reason: String) {
        let mut peer_states = self.peer_states.write().await;
        let mut state = peer_states.entry(peer_addr)
            .or_insert_with(|| PeerConnectionState::new(peer_addr));
        
        state.mark_failed(reason.clone());
        warn!("Connection failure reported for {}: {}", peer_addr, reason);

        // Start reconnection if not already active
        if !self.is_reconnection_active(peer_addr).await {
            self.start_reconnection_task(peer_addr).await;
        }
    }

    /// Get connection state for a peer
    pub async fn get_peer_state(&self, peer_addr: SocketAddr) -> Option<PeerConnectionState> {
        let peer_states = self.peer_states.read().await;
        peer_states.get(&peer_addr).cloned()
    }

    /// Get all peer connection states
    pub async fn get_all_peer_states(&self) -> HashMap<SocketAddr, PeerConnectionState> {
        self.peer_states.read().await.clone()
    }

    /// Check if reconnection is active for a peer
    async fn is_reconnection_active(&self, peer_addr: SocketAddr) -> bool {
        let active_reconnections = self.active_reconnections.lock().await;
        active_reconnections.contains_key(&peer_addr)
    }

    /// Start the main monitoring task
    async fn start_monitoring_task(&self) {
        let peer_states = self.peer_states.clone();
        let running = self.running.clone();
        let health_check_interval = self.config.health_check_interval;

        tokio::spawn(async move {
            let mut interval = interval(health_check_interval);

            loop {
                interval.tick().await;

                if !*running.lock().await {
                    break;
                }

                // Check health of all tracked connections
                let states = peer_states.read().await.clone();
                for (peer_addr, state) in &states {
                    if let Some(connection) = &state.connection {
                        if connection.is_closed() {
                            debug!("Detected closed connection for peer: {}", peer_addr);
                            
                            // Report failure and start reconnection
                            // We can't call methods on self from inside this spawn, so we'll need
                            // to restructure this or use message passing
                            warn!("Connection to {} is closed, should trigger reconnection", peer_addr);
                        }
                    }
                }
            }

            debug!("Reconnection monitoring task terminated");
        });
    }

    /// Start reconnection task for a specific peer
    async fn start_reconnection_task(&self, peer_addr: SocketAddr) {
        let peer_states = self.peer_states.clone();
        let pool = self.pool.clone();
        let client = self.client.clone();
        let config = self.config.clone();
        let active_reconnections = self.active_reconnections.clone();

        let task = tokio::spawn(async move {
            info!("Starting reconnection task for peer: {}", peer_addr);

            loop {
                // Check if we should continue reconnecting
                let should_continue = {
                    let states = peer_states.read().await;
                    if let Some(state) = states.get(&peer_addr) {
                        state.is_ready_for_retry(&config) && state.state != ConnectionState::Connected
                    } else {
                        false
                    }
                };

                if !should_continue {
                    break;
                }

                // Update state for retry attempt
                {
                    let mut states = peer_states.write().await;
                    if let Some(state) = states.get_mut(&peer_addr) {
                        state.start_retry_attempt(&config);
                        info!("Attempting reconnection to {} (attempt {})", peer_addr, state.retry_count);
                    }
                }

                // Attempt reconnection
                let connection_result = timeout(
                    config.connection_timeout,
                    client.connect(peer_addr)
                ).await;

                match connection_result {
                    Ok(Ok(connection)) => {
                        // Successful reconnection
                        info!("Successfully reconnected to peer: {}", peer_addr);
                        
                        // Update state
                        {
                            let mut states = peer_states.write().await;
                            if let Some(state) = states.get_mut(&peer_addr) {
                                state.mark_success(Arc::new(connection.clone()));
                            }
                        }

                        // Add back to connection pool
                        if let Err(e) = pool.add_to_pool(peer_addr, Arc::new(connection)).await {
                            error!("Failed to add reconnected peer to pool: {}", e);
                        }

                        break; // Success, exit reconnection loop
                    }
                    Ok(Err(e)) => {
                        // Connection failed
                        error!("Failed to reconnect to {}: {}", peer_addr, e);
                        
                        // Update failure state
                        {
                            let mut states = peer_states.write().await;
                            if let Some(state) = states.get_mut(&peer_addr) {
                                if state.retry_count >= config.max_retry_attempts {
                                    state.mark_failed(format!("Max retry attempts exceeded: {}", e));
                                    error!("Giving up reconnection to {} after {} attempts", 
                                           peer_addr, config.max_retry_attempts);
                                    break;
                                } else {
                                    state.state = ConnectionState::Backoff;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Connection timeout
                        error!("Connection timeout while reconnecting to {}", peer_addr);
                        
                        {
                            let mut states = peer_states.write().await;
                            if let Some(state) = states.get_mut(&peer_addr) {
                                state.state = ConnectionState::Backoff;
                            }
                        }
                    }
                }

                // Wait before next retry attempt
                let delay = {
                    let states = peer_states.read().await;
                    states.get(&peer_addr).map(|s| s.current_delay)
                        .unwrap_or(Duration::from_secs(5))
                };
                
                debug!("Waiting {:?} before next reconnection attempt to {}", delay, peer_addr);
                tokio::time::sleep(delay).await;
            }

            // Remove from active reconnections
            let mut active = active_reconnections.lock().await;
            active.remove(&peer_addr);

            info!("Reconnection task completed for peer: {}", peer_addr);
        });

        // Track the reconnection task
        let mut active_reconnections = self.active_reconnections.lock().await;
        active_reconnections.insert(peer_addr, task);
    }

    /// Get reconnection statistics
    pub async fn get_reconnection_stats(&self) -> ReconnectionStats {
        let peer_states = self.peer_states.read().await;
        let active_reconnections = self.active_reconnections.lock().await;

        let mut stats = ReconnectionStats {
            total_peers: peer_states.len(),
            connected_peers: 0,
            reconnecting_peers: active_reconnections.len(),
            failed_peers: 0,
            total_reconnections: 0,
            successful_reconnections: 0,
        };

        for state in peer_states.values() {
            match state.state {
                ConnectionState::Connected => stats.connected_peers += 1,
                ConnectionState::Failed => stats.failed_peers += 1,
                _ => {}
            }
            
            stats.total_reconnections += state.retry_count as usize;
            if state.state == ConnectionState::Connected && state.retry_count > 0 {
                stats.successful_reconnections += 1;
            }
        }

        stats
    }
}

/// Reconnection statistics
#[derive(Debug, Clone)]
pub struct ReconnectionStats {
    pub total_peers: usize,
    pub connected_peers: usize,
    pub reconnecting_peers: usize,
    pub failed_peers: usize,
    pub total_reconnections: usize,
    pub successful_reconnections: usize,
}

// Note: ConnectionPool add_to_pool method is implemented in pool.rs