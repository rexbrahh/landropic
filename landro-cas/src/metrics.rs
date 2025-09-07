//! Metrics collection and reporting for the CAS storage layer

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// Storage optimization metrics for performance tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetrics {
    /// Timestamp of metrics collection
    pub timestamp: u64,
    
    /// Cache performance metrics
    pub cache: CacheMetrics,
    
    /// Deduplication metrics
    pub dedup: DedupMetrics,
    
    /// Batch operation metrics
    pub batch: BatchMetrics,
    
    /// Compression metrics
    pub compression: CompressionMetrics,
    
    /// I/O performance metrics
    pub io: IoMetrics,
    
    /// Overall system metrics
    pub system: SystemMetrics,
    
    /// Resumable operation metrics  
    pub resumable: ResumableMetrics,
    
    /// Performance monitoring metrics
    pub performance: PerformanceMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetrics {
    pub hit_rate: f64,
    pub total_hits: u64,
    pub total_misses: u64,
    pub evictions: u64,
    pub cached_chunks: usize,
    pub cache_size_bytes: u64,
    pub avg_access_time_us: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupMetrics {
    pub dedup_ratio: f64,
    pub bytes_saved: u64,
    pub duplicate_chunks: u64,
    pub unique_chunks: u64,
    pub dedup_hits: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMetrics {
    pub total_batches: u64,
    pub avg_batch_size: f64,
    pub avg_batch_latency_ms: f64,
    pub max_batch_size: usize,
    pub total_chunks_batched: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionMetrics {
    pub compression_ratio: f64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub avg_compression_time_us: f64,
    pub avg_decompression_time_us: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoMetrics {
    pub total_reads: u64,
    pub total_writes: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub avg_read_latency_ms: f64,
    pub avg_write_latency_ms: f64,
    pub concurrent_operations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub uptime_seconds: u64,
    pub total_operations: u64,
    pub errors: u64,
    pub throughput_mbps: f64,
    pub storage_efficiency: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumableMetrics {
    pub active_partials: u64,
    pub completed_resumes: u64,
    pub cancelled_resumes: u64,
    pub expired_partials: u64,
    pub avg_resume_time_ms: f64,
    pub total_resumed_bytes: u64,
    pub resume_success_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub operations_per_second: f64,
    pub avg_latency_p50_ms: f64,
    pub avg_latency_p95_ms: f64,
    pub avg_latency_p99_ms: f64,
    pub memory_usage_bytes: u64,
    pub disk_usage_bytes: u64,
    pub temp_files_count: u64,
    pub health_score: f64, // 0.0 to 1.0
}

/// Storage health status for monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageHealth {
    pub status: HealthStatus,
    pub score: f64,
    pub issues: Vec<String>,
    pub recommendations: Vec<String>,
    pub last_check: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Warning,
    Critical,
    Unknown,
}

/// Metrics collector for real-time performance tracking
pub struct MetricsCollector {
    start_time: Instant,
    
    // Cache metrics
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cache_evictions: AtomicU64,
    
    // Dedup metrics
    dedup_hits: AtomicU64,
    unique_chunks: AtomicU64,
    duplicate_chunks: AtomicU64,
    bytes_saved: AtomicU64,
    
    // Batch metrics
    batch_count: AtomicU64,
    batch_chunks: AtomicU64,
    
    // Compression metrics
    bytes_before_compression: AtomicU64,
    bytes_after_compression: AtomicU64,
    
    // I/O metrics
    read_ops: AtomicU64,
    write_ops: AtomicU64,
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
    
    // Timing metrics
    read_latencies: Arc<RwLock<Vec<Duration>>>,
    write_latencies: Arc<RwLock<Vec<Duration>>>,
    compression_times: Arc<RwLock<Vec<Duration>>>,
    decompression_times: Arc<RwLock<Vec<Duration>>>,
    
    // Error tracking
    errors: AtomicU64,
    
    // Resumable operation metrics
    active_partials: AtomicU64,
    completed_resumes: AtomicU64,
    cancelled_resumes: AtomicU64,
    expired_partials: AtomicU64,
    resume_latencies: Arc<RwLock<Vec<Duration>>>,
    
    // Performance monitoring
    operation_latencies: Arc<RwLock<Vec<Duration>>>,
    memory_usage: AtomicU64,
    temp_files: AtomicU64,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            cache_evictions: AtomicU64::new(0),
            dedup_hits: AtomicU64::new(0),
            unique_chunks: AtomicU64::new(0),
            duplicate_chunks: AtomicU64::new(0),
            bytes_saved: AtomicU64::new(0),
            batch_count: AtomicU64::new(0),
            batch_chunks: AtomicU64::new(0),
            bytes_before_compression: AtomicU64::new(0),
            bytes_after_compression: AtomicU64::new(0),
            read_ops: AtomicU64::new(0),
            write_ops: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            read_latencies: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            write_latencies: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            compression_times: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            decompression_times: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            errors: AtomicU64::new(0),
            active_partials: AtomicU64::new(0),
            completed_resumes: AtomicU64::new(0),
            cancelled_resumes: AtomicU64::new(0),
            expired_partials: AtomicU64::new(0),
            resume_latencies: Arc::new(RwLock::new(Vec::with_capacity(100))),
            operation_latencies: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            memory_usage: AtomicU64::new(0),
            temp_files: AtomicU64::new(0),
        }
    }
    
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_cache_eviction(&self) {
        self.cache_evictions.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_dedup_hit(&self, bytes_saved: u64) {
        self.dedup_hits.fetch_add(1, Ordering::Relaxed);
        self.duplicate_chunks.fetch_add(1, Ordering::Relaxed);
        self.bytes_saved.fetch_add(bytes_saved, Ordering::Relaxed);
    }
    
    pub fn record_unique_chunk(&self) {
        self.unique_chunks.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_batch(&self, chunk_count: usize) {
        self.batch_count.fetch_add(1, Ordering::Relaxed);
        self.batch_chunks.fetch_add(chunk_count as u64, Ordering::Relaxed);
    }
    
    pub fn record_compression(&self, before: u64, after: u64, duration: Duration) {
        self.bytes_before_compression.fetch_add(before, Ordering::Relaxed);
        self.bytes_after_compression.fetch_add(after, Ordering::Relaxed);
        
        tokio::spawn({
            let times = Arc::clone(&self.compression_times);
            async move {
                let mut times = times.write().await;
                times.push(duration);
                if times.len() > 10000 {
                    times.drain(0..5000);
                }
            }
        });
    }
    
    pub async fn record_read(&self, bytes: u64, duration: Duration) {
        self.read_ops.fetch_add(1, Ordering::Relaxed);
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
        
        let mut latencies = self.read_latencies.write().await;
        latencies.push(duration);
        if latencies.len() > 10000 {
            latencies.drain(0..5000);
        }
    }
    
    pub async fn record_write(&self, bytes: u64, duration: Duration) {
        self.write_ops.fetch_add(1, Ordering::Relaxed);
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
        
        let mut latencies = self.write_latencies.write().await;
        latencies.push(duration);
        if latencies.len() > 10000 {
            latencies.drain(0..5000);
        }
    }
    
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Collect current metrics snapshot
    pub async fn collect(&self) -> StorageMetrics {
        let uptime = self.start_time.elapsed();
        let uptime_secs = uptime.as_secs();
        
        let cache_hits = self.cache_hits.load(Ordering::Relaxed);
        let cache_misses = self.cache_misses.load(Ordering::Relaxed);
        let total_cache_ops = cache_hits + cache_misses;
        
        let bytes_written = self.bytes_written.load(Ordering::Relaxed);
        let bytes_read = self.bytes_read.load(Ordering::Relaxed);
        let total_bytes = bytes_written + bytes_read;
        
        let throughput_mbps = if uptime_secs > 0 {
            (total_bytes as f64 / uptime_secs as f64) / (1024.0 * 1024.0)
        } else {
            0.0
        };
        
        let bytes_before = self.bytes_before_compression.load(Ordering::Relaxed);
        let bytes_after = self.bytes_after_compression.load(Ordering::Relaxed);
        let compression_ratio = if bytes_before > 0 {
            bytes_after as f64 / bytes_before as f64
        } else {
            1.0
        };
        
        let batch_count = self.batch_count.load(Ordering::Relaxed);
        let batch_chunks = self.batch_chunks.load(Ordering::Relaxed);
        let avg_batch_size = if batch_count > 0 {
            batch_chunks as f64 / batch_count as f64
        } else {
            0.0
        };
        
        let unique = self.unique_chunks.load(Ordering::Relaxed);
        let duplicate = self.duplicate_chunks.load(Ordering::Relaxed);
        let dedup_ratio = if unique + duplicate > 0 {
            duplicate as f64 / (unique + duplicate) as f64
        } else {
            0.0
        };
        
        let read_latencies = self.read_latencies.read().await;
        let avg_read_latency_ms = if !read_latencies.is_empty() {
            let sum: Duration = read_latencies.iter().sum();
            sum.as_millis() as f64 / read_latencies.len() as f64
        } else {
            0.0
        };
        
        let write_latencies = self.write_latencies.read().await;
        let avg_write_latency_ms = if !write_latencies.is_empty() {
            let sum: Duration = write_latencies.iter().sum();
            sum.as_millis() as f64 / write_latencies.len() as f64
        } else {
            0.0
        };
        
        StorageMetrics {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            cache: CacheMetrics {
                hit_rate: if total_cache_ops > 0 {
                    cache_hits as f64 / total_cache_ops as f64
                } else {
                    0.0
                },
                total_hits: cache_hits,
                total_misses: cache_misses,
                evictions: self.cache_evictions.load(Ordering::Relaxed),
                cached_chunks: 0, // Will be filled by actual cache stats
                cache_size_bytes: 0, // Will be filled by actual cache stats
                avg_access_time_us: 0.0, // Will be calculated from actual measurements
            },
            dedup: DedupMetrics {
                dedup_ratio,
                bytes_saved: self.bytes_saved.load(Ordering::Relaxed),
                duplicate_chunks: duplicate,
                unique_chunks: unique,
                dedup_hits: self.dedup_hits.load(Ordering::Relaxed),
            },
            batch: BatchMetrics {
                total_batches: batch_count,
                avg_batch_size,
                avg_batch_latency_ms: 0.0, // Will be calculated from actual measurements
                max_batch_size: 100, // Configured max
                total_chunks_batched: batch_chunks,
            },
            compression: CompressionMetrics {
                compression_ratio,
                bytes_before,
                bytes_after,
                avg_compression_time_us: 0.0, // Will be calculated
                avg_decompression_time_us: 0.0, // Will be calculated
            },
            io: IoMetrics {
                total_reads: self.read_ops.load(Ordering::Relaxed),
                total_writes: self.write_ops.load(Ordering::Relaxed),
                bytes_read,
                bytes_written,
                avg_read_latency_ms,
                avg_write_latency_ms,
                concurrent_operations: 32, // Configured max
            },
            system: SystemMetrics {
                uptime_seconds: uptime_secs,
                total_operations: self.read_ops.load(Ordering::Relaxed) 
                    + self.write_ops.load(Ordering::Relaxed),
                errors: self.errors.load(Ordering::Relaxed),
                throughput_mbps,
                storage_efficiency: 1.0 - dedup_ratio,
            },
            resumable: {
                let completed = self.completed_resumes.load(Ordering::Relaxed);
                let cancelled = self.cancelled_resumes.load(Ordering::Relaxed);
                let total_resumes = completed + cancelled;
                let resume_success_rate = if total_resumes > 0 {
                    completed as f64 / total_resumes as f64
                } else {
                    1.0
                };
                
                let resume_latencies = self.resume_latencies.read().await;
                let avg_resume_time_ms = if !resume_latencies.is_empty() {
                    let sum: Duration = resume_latencies.iter().sum();
                    sum.as_millis() as f64 / resume_latencies.len() as f64
                } else {
                    0.0
                };
                
                ResumableMetrics {
                    active_partials: self.active_partials.load(Ordering::Relaxed),
                    completed_resumes: completed,
                    cancelled_resumes: cancelled,
                    expired_partials: self.expired_partials.load(Ordering::Relaxed),
                    avg_resume_time_ms,
                    total_resumed_bytes: 0, // TODO: Track this
                    resume_success_rate,
                }
            },
            performance: {
                let operation_latencies = self.operation_latencies.read().await;
                
                // Calculate percentiles
                let mut sorted_latencies: Vec<Duration> = operation_latencies.clone();
                sorted_latencies.sort();
                
                let (p50, p95, p99) = if !sorted_latencies.is_empty() {
                    let len = sorted_latencies.len();
                    let p50 = sorted_latencies[len * 50 / 100].as_millis() as f64;
                    let p95 = sorted_latencies[len * 95 / 100].as_millis() as f64;
                    let p99 = sorted_latencies[len * 99 / 100].as_millis() as f64;
                    (p50, p95, p99)
                } else {
                    (0.0, 0.0, 0.0)
                };
                
                let total_ops = self.read_ops.load(Ordering::Relaxed) 
                    + self.write_ops.load(Ordering::Relaxed);
                let ops_per_second = if uptime_secs > 0 {
                    total_ops as f64 / uptime_secs as f64
                } else {
                    0.0
                };
                
                // Simple health score based on error rate and performance
                let error_rate = if total_ops > 0 {
                    self.errors.load(Ordering::Relaxed) as f64 / total_ops as f64
                } else {
                    0.0
                };
                
                let health_score = (1.0 - error_rate).max(0.0);
                
                PerformanceMetrics {
                    operations_per_second: ops_per_second,
                    avg_latency_p50_ms: p50,
                    avg_latency_p95_ms: p95,
                    avg_latency_p99_ms: p99,
                    memory_usage_bytes: self.memory_usage.load(Ordering::Relaxed),
                    disk_usage_bytes: 0, // TODO: Calculate disk usage
                    temp_files_count: self.temp_files.load(Ordering::Relaxed),
                    health_score,
                }
            },
        }
    }
    
    /// Export metrics as JSON
    pub async fn export_json(&self) -> String {
        let metrics = self.collect().await;
        serde_json::to_string_pretty(&metrics).unwrap_or_else(|_| "{}".to_string())
    }
    
    /// Reset all metrics
    pub async fn reset(&self) {
        self.cache_hits.store(0, Ordering::Relaxed);
        self.cache_misses.store(0, Ordering::Relaxed);
        self.cache_evictions.store(0, Ordering::Relaxed);
        self.dedup_hits.store(0, Ordering::Relaxed);
        self.unique_chunks.store(0, Ordering::Relaxed);
        self.duplicate_chunks.store(0, Ordering::Relaxed);
        self.bytes_saved.store(0, Ordering::Relaxed);
        self.batch_count.store(0, Ordering::Relaxed);
        self.batch_chunks.store(0, Ordering::Relaxed);
        self.bytes_before_compression.store(0, Ordering::Relaxed);
        self.bytes_after_compression.store(0, Ordering::Relaxed);
        self.read_ops.store(0, Ordering::Relaxed);
        self.write_ops.store(0, Ordering::Relaxed);
        self.bytes_read.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        
        self.read_latencies.write().await.clear();
        self.write_latencies.write().await.clear();
        self.compression_times.write().await.clear();
        self.decompression_times.write().await.clear();
        
        self.active_partials.store(0, Ordering::Relaxed);
        self.completed_resumes.store(0, Ordering::Relaxed);
        self.cancelled_resumes.store(0, Ordering::Relaxed);
        self.expired_partials.store(0, Ordering::Relaxed);
        self.resume_latencies.write().await.clear();
        self.operation_latencies.write().await.clear();
        self.memory_usage.store(0, Ordering::Relaxed);
        self.temp_files.store(0, Ordering::Relaxed);
    }
    
    // Resumable operation metrics
    pub fn record_partial_started(&self) {
        self.active_partials.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_partial_completed(&self, duration: Duration) {
        self.active_partials.fetch_sub(1, Ordering::Relaxed);
        self.completed_resumes.fetch_add(1, Ordering::Relaxed);
        
        tokio::spawn({
            let latencies = self.resume_latencies.clone();
            async move {
                let mut vec = latencies.write().await;
                vec.push(duration);
                if vec.len() > 100 {
                    vec.remove(0);
                }
            }
        });
    }
    
    pub fn record_partial_cancelled(&self) {
        self.active_partials.fetch_sub(1, Ordering::Relaxed);
        self.cancelled_resumes.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_partial_expired(&self) {
        self.active_partials.fetch_sub(1, Ordering::Relaxed);
        self.expired_partials.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_operation_latency(&self, duration: Duration) {
        tokio::spawn({
            let latencies = self.operation_latencies.clone();
            async move {
                let mut vec = latencies.write().await;
                vec.push(duration);
                if vec.len() > 1000 {
                    vec.remove(0);
                }
            }
        });
    }
    
    pub fn update_memory_usage(&self, bytes: u64) {
        self.memory_usage.store(bytes, Ordering::Relaxed);
    }
    
    pub fn record_temp_file_created(&self) {
        self.temp_files.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_temp_file_removed(&self) {
        self.temp_files.fetch_sub(1, Ordering::Relaxed);
    }
    
    /// Calculate storage health based on metrics
    pub async fn calculate_health(&self) -> StorageHealth {
        let metrics = self.collect().await;
        let mut issues = Vec::new();
        let mut recommendations = Vec::new();
        let mut score: f64 = 1.0;
        
        // Check error rate
        let total_ops = metrics.system.total_operations;
        if total_ops > 0 {
            let error_rate = metrics.system.errors as f64 / total_ops as f64;
            if error_rate > 0.05 {
                issues.push("High error rate detected".to_string());
                recommendations.push("Check system logs for recurring errors".to_string());
                score -= 0.3;
            } else if error_rate > 0.01 {
                issues.push("Elevated error rate".to_string());
                score -= 0.1;
            }
        }
        
        // Check cache hit rate
        if metrics.cache.hit_rate < 0.7 {
            issues.push("Low cache hit rate".to_string());
            recommendations.push("Consider increasing cache size".to_string());
            score -= 0.2;
        } else if metrics.cache.hit_rate < 0.8 {
            score -= 0.1;
        }
        
        // Check throughput
        if metrics.system.throughput_mbps < 50.0 {
            issues.push("Low throughput performance".to_string());
            recommendations.push("Check disk I/O and consider SSD storage".to_string());
            score -= 0.2;
        }
        
        // Check resumable operations
        let active = metrics.resumable.active_partials;
        if active > 100 {
            issues.push("Many active partial transfers".to_string());
            recommendations.push("Monitor for stuck transfers and consider cleanup".to_string());
            score -= 0.1;
        }
        
        // Check temp files
        if metrics.performance.temp_files_count > 1000 {
            issues.push("High number of temporary files".to_string());
            recommendations.push("Run cleanup_expired_partials() more frequently".to_string());
            score -= 0.1;
        }
        
        // Determine status
        let status = if score >= 0.9 {
            HealthStatus::Healthy
        } else if score >= 0.7 {
            HealthStatus::Warning
        } else if score >= 0.4 {
            HealthStatus::Critical
        } else {
            HealthStatus::Unknown
        };
        
        StorageHealth {
            status,
            score: score.max(0.0),
            issues,
            recommendations,
            last_check: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_metrics_collection() {
        let collector = MetricsCollector::new();
        
        // Record some metrics
        collector.record_cache_hit();
        collector.record_cache_hit();
        collector.record_cache_miss();
        collector.record_dedup_hit(1024);
        collector.record_unique_chunk();
        collector.record_batch(10);
        
        // Collect metrics
        let metrics = collector.collect().await;
        
        // Verify metrics
        assert_eq!(metrics.cache.total_hits, 2);
        assert_eq!(metrics.cache.total_misses, 1);
        assert!(metrics.cache.hit_rate > 0.6);
        assert_eq!(metrics.dedup.dedup_hits, 1);
        assert_eq!(metrics.dedup.bytes_saved, 1024);
        assert_eq!(metrics.batch.total_batches, 1);
        assert_eq!(metrics.batch.total_chunks_batched, 10);
    }
    
    #[tokio::test]
    async fn test_metrics_reset() {
        let collector = MetricsCollector::new();
        
        // Record metrics
        collector.record_cache_hit();
        collector.record_error();
        
        // Reset
        collector.reset().await;
        
        // Verify reset
        let metrics = collector.collect().await;
        assert_eq!(metrics.cache.total_hits, 0);
        assert_eq!(metrics.system.errors, 0);
    }
}