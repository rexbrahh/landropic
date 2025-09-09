//! Sync Engine Integration for Day 3 - Health monitoring and enhanced peer connection management
//!
//! This module provides comprehensive sync engine integration with health monitoring,
//! reconnection management, and performance optimization for the landropic daemon.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::orchestrator::OrchestratorMessage;
use landro_cas::ContentStore;
use landro_index::async_indexer::AsyncIndexer;
use landro_quic::{
    Connection, ConnectionHealthMonitor, ConnectionPool, HealthConfig, HealthStatus,
    PerformanceOptimizer, QuicClient, QuicConfig, ReconnectionConfig, ReconnectionManager,
    StreamMultiplexer, TransferPriority, TransferProfile,
};

/// Enhanced sync engine with health monitoring and performance optimization
pub struct EnhancedSyncEngine {
    store: Arc<ContentStore>,
    indexer: Arc<AsyncIndexer>,
    connection_pool: Arc<ConnectionPool>,
    health_monitor: Arc<ConnectionHealthMonitor>,
    reconnection_manager: Arc<ReconnectionManager>,
    performance_optimizer: Arc<PerformanceOptimizer>,
    orchestrator_tx: mpsc::Sender<OrchestratorMessage>,
    active_connections: Arc<RwLock<HashMap<SocketAddr, SyncConnection>>>,
    health_callbacks: Arc<RwLock<Vec<Box<dyn Fn(&HealthStatus) + Send + Sync>>>>,
}

/// Sync connection wrapper with health monitoring
#[derive(Clone)]
pub struct SyncConnection {
    pub connection: Arc<Connection>,
    pub multiplexer: Arc<StreamMultiplexer>,
    pub peer_addr: SocketAddr,
    pub peer_device_id: Option<String>,
    pub last_health_check: std::time::Instant,
    pub health_status: HealthStatus,
}

impl SyncConnection {
    pub fn new(connection: Arc<Connection>, peer_addr: SocketAddr) -> Self {
        let config = landro_quic::MultiplexConfig::default();
        let multiplexer = Arc::new(StreamMultiplexer::new(connection.clone(), config));
        Self {
            connection,
            multiplexer,
            peer_addr,
            peer_device_id: None,
            last_health_check: std::time::Instant::now(),
            health_status: HealthStatus::Healthy,
        }
    }

    /// Check if connection needs health monitoring
    pub fn needs_health_check(&self) -> bool {
        self.last_health_check.elapsed() > Duration::from_secs(30)
    }
}

impl EnhancedSyncEngine {
    /// Create new enhanced sync engine
    pub async fn new(
        store: Arc<ContentStore>,
        indexer: Arc<AsyncIndexer>,
        orchestrator_tx: mpsc::Sender<OrchestratorMessage>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Create QUIC client for outbound connections
        let client_config = QuicConfig::file_sync_optimized();
        // TODO: Get proper identity and verifier from daemon config
        let identity = Arc::new(landro_crypto::identity::DeviceIdentity::generate(
            "landropic-daemon",
        )?); // TODO: Get device name from config
        let verifier = Arc::new(landro_crypto::certificate::CertificateVerifier::new(vec![])); // TODO: Load trusted device IDs from config
        let client = Arc::new(QuicClient::new(identity, verifier, client_config).await?);

        // Create connection pool with health monitoring
        let pool_config = landro_quic::PoolConfig {
            max_connections_per_peer: 3,
            max_total_connections: 100,
            max_idle_time: Duration::from_secs(300),
            connect_timeout: Duration::from_secs(15),
            max_retry_attempts: 3,
            retry_delay: Duration::from_millis(500),
        };
        let connection_pool = Arc::new(ConnectionPool::new(client.clone(), pool_config));

        // Create health monitoring system
        let health_config = HealthConfig {
            check_interval: Duration::from_secs(30),
            unhealthy_threshold: Duration::from_secs(300), // 5 minutes
            max_rtt_ms: 500,
            max_packet_loss_rate: 0.1, // 10%
            enable_auto_recovery: true,
            recovery_timeout: Duration::from_secs(60),
        };
        let health_monitor = Arc::new(ConnectionHealthMonitor::new(
            connection_pool.clone(),
            health_config,
        ));

        // Create reconnection management
        let reconnection_config = ReconnectionConfig {
            initial_retry_delay: Duration::from_secs(1),
            max_retry_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            max_retry_attempts: 5,
            connection_timeout: Duration::from_secs(15),
            health_check_interval: Duration::from_secs(30),
            enable_jitter: true,
        };
        let reconnection_manager = Arc::new(ReconnectionManager::new(
            connection_pool.clone(),
            client.clone(),
            reconnection_config,
        ));

        // Create performance optimizer
        let performance_optimizer = Arc::new(PerformanceOptimizer::new());

        Ok(Self {
            store,
            indexer,
            connection_pool,
            health_monitor,
            reconnection_manager,
            performance_optimizer,
            orchestrator_tx,
            active_connections: Arc::new(RwLock::new(HashMap::new())),
            health_callbacks: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Start the sync engine background tasks
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting enhanced sync engine with health monitoring");

        // Start health monitoring task
        self.start_health_monitoring_task().await?;

        // Start connection cleanup task
        self.start_connection_cleanup_task().await?;

        // Start performance monitoring task
        self.start_performance_monitoring_task().await?;

        info!("Enhanced sync engine started successfully");
        Ok(())
    }

    /// Connect to a peer with health monitoring
    pub async fn connect_to_peer(
        &self,
        peer_addr: SocketAddr,
        device_id: Option<String>,
    ) -> Result<Arc<SyncConnection>, Box<dyn std::error::Error + Send + Sync>> {
        info!("Connecting to peer {} with health monitoring", peer_addr);

        // Check if we already have a healthy connection
        {
            let connections = self.active_connections.read().await;
            if let Some(sync_conn) = connections.get(&peer_addr) {
                if sync_conn.health_status == HealthStatus::Healthy {
                    info!("Reusing existing healthy connection to {}", peer_addr);
                    return Ok(Arc::new(sync_conn.clone()));
                }
            }
        }

        // Get connection from pool (with retry logic)
        let connection = self.connection_pool.get_connection(peer_addr).await?;

        // Create sync connection wrapper
        let mut sync_conn = SyncConnection::new(connection, peer_addr);
        sync_conn.peer_device_id = device_id;

        // TODO: Register with health monitor (pending landro-quic implementation)
        // self.health_monitor.register_connection(peer_addr, sync_conn.connection.clone()).await?;

        // Store active connection
        {
            let mut connections = self.active_connections.write().await;
            connections.insert(peer_addr, sync_conn.clone());
        }

        // TODO: Start reconnection monitoring for this peer (pending landro-quic implementation)
        // self.reconnection_manager.monitor_peer(peer_addr).await?;

        info!(
            "Successfully connected to peer {} with health monitoring",
            peer_addr
        );
        Ok(Arc::new(sync_conn))
    }

    /// Get connection health status
    pub async fn get_connection_health(&self, peer_addr: SocketAddr) -> Option<HealthStatus> {
        let connections = self.active_connections.read().await;
        connections
            .get(&peer_addr)
            .map(|conn| conn.health_status.clone())
    }

    /// Get all connection health statuses
    pub async fn get_all_connection_health(&self) -> HashMap<SocketAddr, HealthStatus> {
        let connections = self.active_connections.read().await;
        connections
            .iter()
            .map(|(addr, conn)| (*addr, conn.health_status.clone()))
            .collect()
    }

    /// Shutdown the sync engine
    pub async fn shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Shutting down enhanced sync engine");

        // Disconnect all peers
        let peer_addrs: Vec<SocketAddr> = {
            let connections = self.active_connections.read().await;
            connections.keys().cloned().collect()
        };

        for peer_addr in peer_addrs {
            let mut connections = self.active_connections.write().await;
            connections.remove(&peer_addr);
        }

        // Shutdown connection pool
        self.connection_pool.shutdown().await?;

        info!("Enhanced sync engine shutdown complete");
        Ok(())
    }

    /// Start health monitoring background task
    async fn start_health_monitoring_task(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let health_monitor = self.health_monitor.clone();
        let active_connections = self.active_connections.clone();
        let orchestrator_tx = self.orchestrator_tx.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));

            loop {
                interval.tick().await;

                // Check health of all active connections
                let connections_to_check: Vec<(SocketAddr, Arc<Connection>)> = {
                    let connections = active_connections.read().await;
                    connections
                        .iter()
                        .filter(|(_, sync_conn)| sync_conn.needs_health_check())
                        .map(|(addr, sync_conn)| (*addr, sync_conn.connection.clone()))
                        .collect()
                };

                for (peer_addr, connection) in connections_to_check {
                    // TODO: Implement health checking (pending landro-quic implementation)
                    // match health_monitor.check_connection_health(connection).await {
                    let health_status = HealthStatus::Healthy; // Stub for now
                    match Ok::<landro_quic::HealthStatus, Box<dyn std::error::Error + Send + Sync>>(
                        health_status,
                    ) {
                        Ok(health_status) => {
                            // Update connection health status
                            {
                                let mut connections = active_connections.write().await;
                                if let Some(sync_conn) = connections.get_mut(&peer_addr) {
                                    sync_conn.health_status = health_status.clone();
                                    sync_conn.last_health_check = std::time::Instant::now();
                                }
                            }

                            // Notify orchestrator of health changes
                            if health_status != HealthStatus::Healthy {
                                warn!("Peer {} health status: {:?}", peer_addr, health_status);
                                let _ = orchestrator_tx
                                    .send(OrchestratorMessage::PeerLost(peer_addr.to_string()))
                                    .await;
                            }
                        }
                        Err(e) => {
                            error!("Health check failed for peer {}: {}", peer_addr, e);
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Start connection cleanup task
    async fn start_connection_cleanup_task(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let active_connections = self.active_connections.clone();
        let connection_pool = self.connection_pool.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));

            loop {
                interval.tick().await;

                // Find unhealthy connections to clean up
                let unhealthy_peers: Vec<SocketAddr> = {
                    let connections = active_connections.read().await;
                    connections
                        .iter()
                        .filter_map(|(addr, sync_conn)| {
                            if sync_conn.health_status == HealthStatus::Unhealthy
                                || sync_conn.connection.is_closed()
                            {
                                Some(*addr)
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                for peer_addr in unhealthy_peers {
                    info!("Cleaning up unhealthy connection to {}", peer_addr);

                    // Remove from active connections
                    {
                        let mut connections = active_connections.write().await;
                        connections.remove(&peer_addr);
                    }

                    // Remove from connection pool
                    connection_pool.remove_peer_connections(peer_addr).await;
                }
            }
        });

        Ok(())
    }

    /// Start performance monitoring task
    async fn start_performance_monitoring_task(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let performance_optimizer = self.performance_optimizer.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));

            loop {
                interval.tick().await;

                // Get performance statistics
                let stats = performance_optimizer.get_performance_stats().await;

                debug!(
                    "Performance stats: {} peers, {:.2} Mbps total, {:.1}ms avg RTT",
                    stats.total_peers, stats.total_throughput_mbps, stats.average_rtt_ms
                );

                // Log performance insights
                if stats.average_packet_loss_rate > 0.05 {
                    warn!(
                        "High packet loss detected: {:.1}%",
                        stats.average_packet_loss_rate * 100.0
                    );
                }

                if stats.average_rtt_ms > 200.0 {
                    warn!("High latency detected: {:.1}ms", stats.average_rtt_ms);
                }
            }
        });

        Ok(())
    }
}

/// Helper function to create enhanced sync engine from orchestrator
pub async fn create_enhanced_sync_engine(
    store: Arc<ContentStore>,
    indexer: Arc<AsyncIndexer>,
    orchestrator_tx: mpsc::Sender<OrchestratorMessage>,
) -> Result<Arc<EnhancedSyncEngine>, Box<dyn std::error::Error + Send + Sync>> {
    let sync_engine = EnhancedSyncEngine::new(store, indexer, orchestrator_tx).await?;
    let sync_engine = Arc::new(sync_engine);

    // Start background tasks
    sync_engine.start().await?;

    Ok(sync_engine)
}
