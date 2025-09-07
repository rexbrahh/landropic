//! Performance optimizer for Day 3 - adaptive performance tuning for file transfers
//!
//! This module provides intelligent performance optimization based on network conditions,
//! file characteristics, and real-time performance metrics to maximize transfer efficiency.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::connection::Connection;
use crate::config::QuicConfig;
use crate::pool::{ConnectionPool, PoolConfig};
use crate::stream_transfer::StreamTransferConfig;

/// Performance metrics for optimization decisions
#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub peer_id: String,
    pub throughput_mbps: f64,
    pub rtt_ms: u64,
    pub packet_loss_rate: f64,
    pub cpu_utilization: f64,
    pub memory_usage_mb: u64,
    pub active_transfers: u32,
    pub last_updated: Instant,
}

/// Network condition assessment
#[derive(Debug, Clone, PartialEq)]
pub enum NetworkCondition {
    Excellent, // Low latency, high bandwidth, no loss
    Good,      // Normal conditions
    Poor,      // High latency or packet loss
    Congested, // Network congestion detected
}

/// File transfer characteristics
#[derive(Debug, Clone)]
pub struct TransferProfile {
    pub file_size: u64,
    pub file_type: FileType,
    pub priority: TransferPriority,
    pub estimated_duration: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FileType {
    SmallFile,    // < 1MB
    MediumFile,   // 1MB - 100MB
    LargeFile,    // 100MB - 1GB
    HugeFile,     // > 1GB
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TransferPriority {
    Low,
    Medium,
    High,
    Critical,
}

/// Optimized configuration recommendations
#[derive(Debug, Clone)]
pub struct OptimizedConfig {
    pub stream_config: StreamTransferConfig,
    pub pool_config: PoolConfig,
    pub quic_config: QuicConfig,
    pub optimization_reason: String,
}

/// Performance optimizer for adaptive tuning
pub struct PerformanceOptimizer {
    metrics: Arc<RwLock<HashMap<String, PerformanceMetrics>>>,
    baseline_configs: Arc<RwLock<HashMap<FileType, OptimizedConfig>>>,
    active_optimizations: Arc<Mutex<HashMap<String, OptimizedConfig>>>,
    monitoring_interval: Duration,
}

impl PerformanceOptimizer {
    /// Create new performance optimizer
    pub fn new() -> Self {
        let mut baseline_configs = HashMap::new();
        
        // Create baseline configurations for different file types
        baseline_configs.insert(FileType::SmallFile, Self::create_small_file_config());
        baseline_configs.insert(FileType::MediumFile, Self::create_medium_file_config());
        baseline_configs.insert(FileType::LargeFile, Self::create_large_file_config());
        baseline_configs.insert(FileType::HugeFile, Self::create_huge_file_config());

        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
            baseline_configs: Arc::new(RwLock::new(baseline_configs)),
            active_optimizations: Arc::new(Mutex::new(HashMap::new())),
            monitoring_interval: Duration::from_secs(10),
        }
    }

    /// Update performance metrics for a peer
    pub async fn update_metrics(&self, peer_id: String, metrics: PerformanceMetrics) {
        let mut metrics_map = self.metrics.write().await;
        metrics_map.insert(peer_id, metrics);
    }

    /// Get optimized configuration for a transfer
    pub async fn get_optimized_config(
        &self,
        peer_id: &str,
        transfer_profile: &TransferProfile,
    ) -> OptimizedConfig {
        // Get current performance metrics
        let current_metrics = {
            let metrics_map = self.metrics.read().await;
            metrics_map.get(peer_id).cloned()
        };

        // Assess network conditions
        let network_condition = if let Some(metrics) = &current_metrics {
            self.assess_network_condition(metrics)
        } else {
            NetworkCondition::Good // Default assumption
        };

        // Get baseline configuration for file type
        let baseline_config = {
            let baselines = self.baseline_configs.read().await;
            baselines.get(&transfer_profile.file_type).cloned()
                .unwrap_or_else(|| Self::create_medium_file_config())
        };

        // Apply adaptive optimizations
        self.apply_adaptive_optimizations(
            baseline_config,
            transfer_profile,
            &network_condition,
            current_metrics.as_ref(),
        ).await
    }

    /// Apply adaptive optimizations based on current conditions
    async fn apply_adaptive_optimizations(
        &self,
        mut config: OptimizedConfig,
        transfer_profile: &TransferProfile,
        network_condition: &NetworkCondition,
        metrics: Option<&PerformanceMetrics>,
    ) -> OptimizedConfig {
        let mut optimizations = Vec::new();

        // Network condition optimizations
        match network_condition {
            NetworkCondition::Excellent => {
                // Maximize performance
                config.stream_config.max_concurrent_streams = 128;
                config.stream_config.stream_buffer_size = 16 * 1024 * 1024; // 16MB
                config.pool_config.max_connections_per_peer = 8;
                optimizations.push("excellent network conditions");
            }
            NetworkCondition::Good => {
                // Standard optimized settings (already in baseline)
                optimizations.push("good network conditions");
            }
            NetworkCondition::Poor => {
                // Reduce parallelism, increase reliability
                config.stream_config.max_concurrent_streams = 16;
                config.stream_config.stream_buffer_size = 2 * 1024 * 1024; // 2MB
                config.stream_config.max_retransmit_attempts = 5;
                config.pool_config.max_connections_per_peer = 2;
                optimizations.push("poor network conditions");
            }
            NetworkCondition::Congested => {
                // Minimal parallelism, focus on completion
                config.stream_config.max_concurrent_streams = 4;
                config.stream_config.stream_buffer_size = 512 * 1024; // 512KB
                config.stream_config.max_retransmit_attempts = 7;
                config.pool_config.max_connections_per_peer = 1;
                optimizations.push("congested network");
            }
        }

        // Priority-based optimizations
        match transfer_profile.priority {
            TransferPriority::Critical => {
                config.stream_config.max_concurrent_streams = 
                    config.stream_config.max_concurrent_streams * 2;
                config.pool_config.max_connections_per_peer = 
                    config.pool_config.max_connections_per_peer.max(4);
                optimizations.push("critical priority");
            }
            TransferPriority::Low => {
                config.stream_config.max_concurrent_streams = 
                    config.stream_config.max_concurrent_streams / 2;
                optimizations.push("low priority");
            }
            _ => {}
        }

        // CPU utilization optimizations
        if let Some(metrics) = metrics {
            if metrics.cpu_utilization > 80.0 {
                // Reduce CPU load
                config.stream_config.max_concurrent_streams = 
                    config.stream_config.max_concurrent_streams / 2;
                config.stream_config.enable_compression = false;
                optimizations.push("high CPU utilization");
            }

            // Memory usage optimizations
            if metrics.memory_usage_mb > 1024 {
                // Reduce memory usage
                config.stream_config.stream_buffer_size = 
                    config.stream_config.stream_buffer_size / 2;
                optimizations.push("high memory usage");
            }
        }

        config.optimization_reason = format!(
            "Optimized for: {}", 
            optimizations.join(", ")
        );

        config
    }

    /// Assess network conditions based on metrics
    fn assess_network_condition(&self, metrics: &PerformanceMetrics) -> NetworkCondition {
        // Excellent: Low latency, high throughput, no loss
        if metrics.rtt_ms < 50 && metrics.throughput_mbps > 100.0 && metrics.packet_loss_rate < 0.001 {
            return NetworkCondition::Excellent;
        }

        // Poor: High latency or significant packet loss
        if metrics.rtt_ms > 200 || metrics.packet_loss_rate > 0.05 {
            return NetworkCondition::Poor;
        }

        // Congested: Low throughput despite reasonable latency
        if metrics.rtt_ms < 100 && metrics.throughput_mbps < 10.0 {
            return NetworkCondition::Congested;
        }

        // Default to good conditions
        NetworkCondition::Good
    }

    /// Create optimized configuration for small files
    fn create_small_file_config() -> OptimizedConfig {
        OptimizedConfig {
            stream_config: StreamTransferConfig {
                max_concurrent_streams: 4,
                stream_buffer_size: 256 * 1024, // 256KB
                adaptive_flow_control: false, // Overhead not worth it for small files
                flow_control_window: 4 * 1024 * 1024, // 4MB
                backpressure_threshold: 0.9,
                enable_stream_priority: false,
                max_retransmit_attempts: 3,
                stream_timeout: Duration::from_secs(10),
                enable_compression: false, // Not worth it for small files
            },
            pool_config: PoolConfig {
                max_connections_per_peer: 1, // One connection sufficient
                max_total_connections: 20,
                max_idle_time: Duration::from_secs(120),
                connect_timeout: Duration::from_secs(5),
                max_retry_attempts: 2,
                retry_delay: Duration::from_millis(100),
            },
            quic_config: QuicConfig::small_file_optimized(),
            optimization_reason: "Small file baseline config".to_string(),
        }
    }

    /// Create optimized configuration for medium files
    fn create_medium_file_config() -> OptimizedConfig {
        OptimizedConfig {
            stream_config: StreamTransferConfig::file_sync_optimized(),
            pool_config: PoolConfig::default(),
            quic_config: QuicConfig::file_sync_optimized(),
            optimization_reason: "Medium file baseline config".to_string(),
        }
    }

    /// Create optimized configuration for large files
    fn create_large_file_config() -> OptimizedConfig {
        OptimizedConfig {
            stream_config: StreamTransferConfig {
                max_concurrent_streams: 64,
                stream_buffer_size: 16 * 1024 * 1024, // 16MB
                adaptive_flow_control: true,
                flow_control_window: 128 * 1024 * 1024, // 128MB
                backpressure_threshold: 0.75,
                enable_stream_priority: true,
                max_retransmit_attempts: 5,
                stream_timeout: Duration::from_secs(60),
                enable_compression: false, // Usually not beneficial for large files
            },
            pool_config: PoolConfig {
                max_connections_per_peer: 4,
                max_total_connections: 100,
                max_idle_time: Duration::from_secs(600),
                connect_timeout: Duration::from_secs(15),
                max_retry_attempts: 3,
                retry_delay: Duration::from_millis(250),
            },
            quic_config: QuicConfig::large_file_optimized(),
            optimization_reason: "Large file baseline config".to_string(),
        }
    }

    /// Create optimized configuration for huge files
    fn create_huge_file_config() -> OptimizedConfig {
        OptimizedConfig {
            stream_config: StreamTransferConfig {
                max_concurrent_streams: 32, // Fewer streams but larger buffers
                stream_buffer_size: 32 * 1024 * 1024, // 32MB
                adaptive_flow_control: true,
                flow_control_window: 256 * 1024 * 1024, // 256MB
                backpressure_threshold: 0.7,
                enable_stream_priority: true,
                max_retransmit_attempts: 7,
                stream_timeout: Duration::from_secs(120),
                enable_compression: false,
            },
            pool_config: PoolConfig {
                max_connections_per_peer: 2, // Fewer connections but more stable
                max_total_connections: 50,
                max_idle_time: Duration::from_secs(1800), // 30 minutes
                connect_timeout: Duration::from_secs(30),
                max_retry_attempts: 5,
                retry_delay: Duration::from_millis(500),
            },
            quic_config: QuicConfig::huge_file_optimized(),
            optimization_reason: "Huge file baseline config".to_string(),
        }
    }

    /// Get performance statistics
    pub async fn get_performance_stats(&self) -> PerformanceStats {
        let metrics_map = self.metrics.read().await;
        let active_optimizations = self.active_optimizations.lock().await;

        let mut total_throughput = 0.0;
        let mut avg_rtt = 0.0;
        let mut avg_packet_loss = 0.0;
        let peer_count = metrics_map.len();

        for metrics in metrics_map.values() {
            total_throughput += metrics.throughput_mbps;
            avg_rtt += metrics.rtt_ms as f64;
            avg_packet_loss += metrics.packet_loss_rate;
        }

        if peer_count > 0 {
            avg_rtt /= peer_count as f64;
            avg_packet_loss /= peer_count as f64;
        }

        PerformanceStats {
            total_peers: peer_count,
            total_throughput_mbps: total_throughput,
            average_rtt_ms: avg_rtt,
            average_packet_loss_rate: avg_packet_loss,
            active_optimizations: active_optimizations.len(),
        }
    }
}

/// Performance statistics
#[derive(Debug, Clone)]
pub struct PerformanceStats {
    pub total_peers: usize,
    pub total_throughput_mbps: f64,
    pub average_rtt_ms: f64,
    pub average_packet_loss_rate: f64,
    pub active_optimizations: usize,
}

// Extension trait for QuicConfig with optimization presets
impl QuicConfig {
    /// Configuration optimized for small files
    pub fn small_file_optimized() -> Self {
        let mut config = Self::default();
        config.initial_rtt_us = 50_000; // 50ms in microseconds
        config.idle_timeout = Duration::from_secs(30);
        config
    }

    /// Configuration optimized for large files
    pub fn large_file_optimized() -> Self {
        let mut config = Self::file_sync_optimized();
        config.initial_rtt_us = 100_000; // 100ms in microseconds
        config.idle_timeout = Duration::from_secs(600);
        config
    }

    /// Configuration optimized for huge files
    pub fn huge_file_optimized() -> Self {
        let mut config = Self::large_file_optimized();
        config.idle_timeout = Duration::from_secs(1800);
        config
    }
}

impl TransferProfile {
    /// Create transfer profile from file size
    pub fn from_file_size(file_size: u64) -> Self {
        let file_type = match file_size {
            0..=1_048_576 => FileType::SmallFile,        // <= 1MB
            1_048_577..=104_857_600 => FileType::MediumFile, // 1MB - 100MB
            104_857_601..=1_073_741_824 => FileType::LargeFile, // 100MB - 1GB
            _ => FileType::HugeFile,                     // > 1GB
        };

        Self {
            file_size,
            file_type,
            priority: TransferPriority::Medium,
            estimated_duration: None,
        }
    }

    /// Set transfer priority
    pub fn with_priority(mut self, priority: TransferPriority) -> Self {
        self.priority = priority;
        self
    }
}

impl Default for PerformanceOptimizer {
    fn default() -> Self {
        Self::new()
    }
}