//! Connection health monitoring and recovery for Day 2 implementation
//! 
//! This provides real-time health monitoring of QUIC connections with automatic
//! recovery and performance metrics tracking.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::connection::Connection;
use crate::pool::{ConnectionPool, PooledConnection};

/// Health status for a connection
#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Failing,
    Unhealthy,
}

/// Comprehensive connection metrics
#[derive(Debug, Clone)]
pub struct ConnectionMetrics {
    pub connection_id: String,
    pub peer_addr: String,
    pub health_status: HealthStatus,
    pub rtt_ms: u64,
    pub packets_sent: u64,
    pub packets_lost: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub active_streams: u32,
    pub last_activity: Instant,
    pub connection_age: Duration,
    pub reconnect_count: u32,
    pub error_count: u64,
}

/// Health monitoring configuration
#[derive(Debug, Clone)]
pub struct HealthConfig {
    pub check_interval: Duration,
    pub unhealthy_threshold: Duration,
    pub max_rtt_ms: u64,
    pub max_packet_loss_rate: f64,
    pub enable_auto_recovery: bool,
    pub recovery_timeout: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(5),
            unhealthy_threshold: Duration::from_secs(30),
            max_rtt_ms: 500, // 500ms max RTT
            max_packet_loss_rate: 0.05, // 5% max packet loss
            enable_auto_recovery: true,
            recovery_timeout: Duration::from_secs(10),
        }
    }
}

/// Connection health monitor
pub struct ConnectionHealthMonitor {
    pool: Arc<ConnectionPool>,
    config: HealthConfig,
    metrics: Arc<RwLock<HashMap<String, ConnectionMetrics>>>,
    monitoring_active: Arc<Mutex<bool>>,
    recovery_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl ConnectionHealthMonitor {
    /// Create new health monitor
    pub fn new(pool: Arc<ConnectionPool>, config: HealthConfig) -> Self {
        Self {
            pool,
            config,
            metrics: Arc::new(RwLock::new(HashMap::new())),
            monitoring_active: Arc::new(Mutex::new(false)),
            recovery_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start health monitoring
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut active = self.monitoring_active.lock().await;
        if *active {
            return Ok(());
        }
        *active = true;

        info!("Starting connection health monitoring (interval: {:?})", self.config.check_interval);
        
        self.start_monitoring_task().await;
        
        info!("Connection health monitoring started successfully");
        Ok(())
    }

    /// Stop health monitoring
    pub async fn stop(&self) {
        let mut active = self.monitoring_active.lock().await;
        *active = false;
        
        // Cancel all recovery tasks
        let mut recovery_tasks = self.recovery_tasks.lock().await;
        for (peer_id, handle) in recovery_tasks.drain() {
            handle.abort();
            debug!("Cancelled recovery task for peer: {}", peer_id);
        }
        
        info!("Connection health monitoring stopped");
    }

    /// Get health metrics for a specific connection
    pub async fn get_connection_metrics(&self, connection_id: &str) -> Option<ConnectionMetrics> {
        let metrics = self.metrics.read().await;
        metrics.get(connection_id).cloned()
    }

    /// Get all connection metrics
    pub async fn get_all_metrics(&self) -> HashMap<String, ConnectionMetrics> {
        self.metrics.read().await.clone()
    }

    /// Get health summary
    pub async fn get_health_summary(&self) -> HealthSummary {
        let metrics = self.metrics.read().await;
        
        let mut summary = HealthSummary {
            total_connections: metrics.len(),
            healthy_connections: 0,
            degraded_connections: 0,
            failing_connections: 0,
            unhealthy_connections: 0,
            average_rtt_ms: 0.0,
            total_bytes_transferred: 0,
            active_recoveries: 0,
        };

        let mut total_rtt = 0u64;
        let mut rtt_count = 0u64;

        for metric in metrics.values() {
            match metric.health_status {
                HealthStatus::Healthy => summary.healthy_connections += 1,
                HealthStatus::Degraded => summary.degraded_connections += 1,
                HealthStatus::Failing => summary.failing_connections += 1,
                HealthStatus::Unhealthy => summary.unhealthy_connections += 1,
            }

            total_rtt += metric.rtt_ms;
            rtt_count += 1;
            summary.total_bytes_transferred += metric.bytes_sent + metric.bytes_received;
        }

        if rtt_count > 0 {
            summary.average_rtt_ms = total_rtt as f64 / rtt_count as f64;
        }

        let recovery_tasks = self.recovery_tasks.lock().await;
        summary.active_recoveries = recovery_tasks.len();

        summary
    }

    /// Start the main monitoring task
    async fn start_monitoring_task(&self) {
        let pool = self.pool.clone();
        let metrics = self.metrics.clone();
        let config = self.config.clone();
        let monitoring_active = self.monitoring_active.clone();
        let recovery_tasks = self.recovery_tasks.clone();

        tokio::spawn(async move {
            let mut interval = interval(config.check_interval);

            loop {
                interval.tick().await;

                // Check if monitoring is still active
                if !*monitoring_active.lock().await {
                    break;
                }

                // Health check all connections
                if let Err(e) = Self::perform_health_check(
                    &pool,
                    &metrics,
                    &config,
                    &recovery_tasks,
                ).await {
                    error!("Error during health check: {}", e);
                }
            }

            debug!("Health monitoring task terminated");
        });
    }

    /// Perform health check on all connections
    async fn perform_health_check(
        pool: &Arc<ConnectionPool>,
        metrics: &Arc<RwLock<HashMap<String, ConnectionMetrics>>>,
        config: &HealthConfig,
        recovery_tasks: &Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        
        // Get current pool status (would need to add this method to ConnectionPool)
        let pool_stats = pool.get_pool_stats().await;
        debug!("Health check: {} total connections", pool_stats.total_connections);

        let mut updated_metrics = HashMap::new();
        let now = Instant::now();

        // Check each connection's health
        for (peer_addr, connection_count) in pool_stats.connections_per_peer {
            let connection_id = format!("{}:{}", peer_addr.ip(), peer_addr.port());
            
            // Create or update metrics
            let current_metrics = {
                let metrics_read = metrics.read().await;
                metrics_read.get(&connection_id).cloned()
            };

            let mut new_metrics = if let Some(mut existing) = current_metrics {
                // Update existing metrics
                existing.last_activity = now;
                existing.active_streams = connection_count as u32;
                existing
            } else {
                // Create new metrics
                ConnectionMetrics {
                    connection_id: connection_id.clone(),
                    peer_addr: peer_addr.to_string(),
                    health_status: HealthStatus::Healthy,
                    rtt_ms: 0,
                    packets_sent: 0,
                    packets_lost: 0,
                    bytes_sent: 0,
                    bytes_received: 0,
                    active_streams: connection_count as u32,
                    last_activity: now,
                    connection_age: Duration::from_secs(0),
                    reconnect_count: 0,
                    error_count: 0,
                }
            };

            // Assess health status
            new_metrics.health_status = Self::assess_health_status(&new_metrics, config);

            // Trigger recovery if needed
            if new_metrics.health_status == HealthStatus::Unhealthy && config.enable_auto_recovery {
                Self::trigger_recovery(
                    peer_addr,
                    pool.clone(),
                    recovery_tasks.clone(),
                    config.clone(),
                ).await;
            }

            updated_metrics.insert(connection_id, new_metrics);
        }

        // Update all metrics
        let mut metrics_write = metrics.write().await;
        *metrics_write = updated_metrics;

        Ok(())
    }

    /// Assess the health status of a connection
    fn assess_health_status(metrics: &ConnectionMetrics, config: &HealthConfig) -> HealthStatus {
        // Check if connection is inactive for too long
        if metrics.last_activity.elapsed() > config.unhealthy_threshold {
            return HealthStatus::Unhealthy;
        }

        // Check RTT
        if metrics.rtt_ms > config.max_rtt_ms {
            return HealthStatus::Degraded;
        }

        // Check packet loss rate
        if metrics.packets_sent > 0 {
            let loss_rate = metrics.packets_lost as f64 / metrics.packets_sent as f64;
            if loss_rate > config.max_packet_loss_rate {
                return HealthStatus::Failing;
            }
        }

        // Check error count
        if metrics.error_count > 10 {
            return HealthStatus::Degraded;
        }

        HealthStatus::Healthy
    }

    /// Trigger connection recovery
    async fn trigger_recovery(
        peer_addr: std::net::SocketAddr,
        pool: Arc<ConnectionPool>,
        recovery_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
        config: HealthConfig,
    ) {
        let peer_id = peer_addr.to_string();
        
        // Check if recovery already in progress
        {
            let tasks = recovery_tasks.lock().await;
            if tasks.contains_key(&peer_id) {
                return;
            }
        }

        warn!("Triggering recovery for unhealthy connection: {}", peer_id);

        let peer_id_clone = peer_id.clone();
        let recovery_task = tokio::spawn(async move {
            info!("Starting connection recovery for {}", peer_id_clone);
            
            // Remove unhealthy connections from pool
            pool.remove_peer_connections(peer_addr).await;
            
            // Wait for recovery timeout
            tokio::time::sleep(config.recovery_timeout).await;
            
            // Try to establish new connection
            match pool.get_connection(peer_addr).await {
                Ok(_) => {
                    info!("Connection recovery successful for {}", peer_id_clone);
                }
                Err(e) => {
                    error!("Connection recovery failed for {}: {}", peer_id_clone, e);
                }
            }
        });

        let mut tasks = recovery_tasks.lock().await;
        tasks.insert(peer_id, recovery_task);
    }
}

/// Health summary statistics
#[derive(Debug, Clone)]
pub struct HealthSummary {
    pub total_connections: usize,
    pub healthy_connections: usize,
    pub degraded_connections: usize,
    pub failing_connections: usize,
    pub unhealthy_connections: usize,
    pub average_rtt_ms: f64,
    pub total_bytes_transferred: u64,
    pub active_recoveries: usize,
}

/// Pool statistics (would need to add to ConnectionPool)
#[derive(Debug)]
pub struct PoolStats {
    pub total_connections: usize,
    pub connections_per_peer: HashMap<std::net::SocketAddr, usize>,
}

