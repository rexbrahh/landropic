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