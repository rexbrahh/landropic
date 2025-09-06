# QUIC Network Optimizations for Landropic File Sync

## Overview
Comprehensive QUIC transport layer optimizations implemented to maximize file sync performance on LAN and WAN networks.

## Key Optimizations

### 1. QUIC Configuration Tuning (config.rs)
- **File Sync Profile**: 
  - 4GB receive window for massive parallel transfers
  - 256MB per-stream windows for large chunks
  - 2048 bidirectional streams for maximum parallelism
  - BBR congestion control for better throughput
  - 25μs initial RTT for ultra-fast LAN response
  - 128MB send/receive buffers for chunk batching

- **LAN Profile Enhancements**:
  - Switched from CUBIC to BBR for consistent throughput
  - Doubled stream windows to 128MB
  - Reduced initial RTT to 50μs
  - Increased concurrent streams to 1024/2048 (bidi/uni)

### 2. Adaptive Congestion Control (parallel_transfer.rs)
- **Four-State Controller**:
  - SlowStart: Exponential growth (2x per RTT)
  - CongestionAvoidance: Linear growth
  - Recovery: Backoff after loss (50% reduction)
  - FastRecovery: Gradual recovery (85% capacity)

- **Dynamic Stream Allocation**:
  - Bandwidth-based scaling (25% to 100% of max streams)
  - File size awareness (more streams for many chunks)
  - Load-based adjustment (reduces with active transfers)
  - Minimum 4 streams for parallelism

### 3. Connection Pooling
- **Pool Management**:
  - Max 2 connections per peer
  - 50 total connections pool-wide
  - 5-minute idle timeout
  - Automatic cleanup of stale connections
  - Health checking and retry logic

- **Integration**:
  - ParallelTransferManager::with_pool() for multi-peer
  - Automatic fallback to primary connection
  - Pooled connection reuse for efficiency

### 4. Chunk Provider Optimizations (chunk_provider.rs)
- **LRU Cache**:
  - 256 chunk in-memory cache
  - Cache hits avoid CAS reads
  - Automatic eviction of old chunks
  - Configurable cache size

- **Prefetch Queue**:
  - 64-chunk prefetch capacity
  - 8 concurrent prefetch operations
  - Anticipatory loading for sequential access

### 5. Stream Priority Management
- **Priority Levels**:
  - Large batches (>10 chunks): High priority
  - Medium batches (5-10 chunks): Normal priority  
  - Small batches (<5 chunks): Lower priority
  - Control messages: Highest priority (via datagrams)

### 6. Flow Control & Pacing
- **Adaptive Pacing**:
  - Brief pauses every 8 chunks or 16MB
  - Prevents overwhelming receivers
  - Maintains steady throughput

- **Timeout Management**:
  - 5-second memory acquisition timeout
  - 30-second chunk transfer timeout
  - Automatic retry with exponential backoff

## Performance Impact

### Expected Improvements
- **LAN Transfers**: 2-3x throughput increase
- **Concurrent Transfers**: 4x more parallel streams
- **Cache Hit Rate**: 30-50% for common chunks
- **Connection Reuse**: 60-80% pooled connection usage
- **Latency Reduction**: 50% lower RTT on LAN

### Bandwidth Utilization
- **>2 GB/s**: Maximum parallelism (100% streams)
- **500MB-2GB/s**: 85-95% stream utilization
- **100-500MB/s**: 65-85% stream utilization
- **<100MB/s**: Conservative 25-50% utilization

## Integration Points

### With Storage Layer
- Chunk provider integrates with CAS
- Cache reduces storage I/O
- Prefetch improves sequential read patterns

### With Sync Engine
- Connection pool shared across sync sessions
- Adaptive congestion control responds to sync patterns
- Priority system favors large file transfers

### With Daemon
- Configuration profiles selectable at startup
- Statistics available for monitoring
- Graceful shutdown of pooled connections

## Future Enhancements
1. Smart prefetching based on access patterns
2. Multi-path QUIC for redundancy
3. Dynamic MTU discovery and adaptation
4. Per-peer congestion control tuning
5. Compression for compressible chunks

## Testing Recommendations
1. Benchmark with various file sizes (1MB to 10GB)
2. Test concurrent transfers (10-100 peers)
3. Validate cache effectiveness with repeated syncs
4. Measure CPU/memory usage under load
5. Test recovery from connection failures

## Configuration Examples

```rust
// For large file sync on fast LAN
let config = QuicConfig::file_sync_optimized();

// For many small files
let transfer_config = ParallelTransferConfig::small_files_optimized();

// With connection pooling
let pool = ConnectionPool::new(client, PoolConfig::default());
let manager = ParallelTransferManager::with_pool(conn, pool, config);
```

## Monitoring Metrics
- Chunks served/second
- Cache hit rate
- Average chunk latency
- Bandwidth utilization
- Connection pool usage
- Congestion state distribution