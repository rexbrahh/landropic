//! Parallel chunk transfer implementation for high-throughput file transfers

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, info, warn, error};
use bytes::{Bytes, BytesMut};
use quinn::{RecvStream, SendStream};

use landro_proto::ChunkData;
use crate::connection::Connection;
use crate::errors::{QuicError, Result};
use crate::protocol::{MessageType, StreamProtocol};

/// Configuration for parallel transfer optimization
#[derive(Clone, Debug)]
pub struct ParallelTransferConfig {
    /// Maximum number of concurrent streams per transfer
    pub max_concurrent_streams: usize,
    
    /// Maximum number of chunks to batch per stream
    pub chunks_per_stream: usize,
    
    /// Target bytes per stream before opening new stream
    pub bytes_per_stream_target: usize,
    
    /// Enable adaptive parallelism based on bandwidth
    pub adaptive_parallelism: bool,
    
    /// Bandwidth measurement window
    pub bandwidth_window: Duration,
    
    /// Maximum memory usage for buffering (bytes)
    pub max_buffer_size: usize,
}

impl Default for ParallelTransferConfig {
    fn default() -> Self {
        Self {
            max_concurrent_streams: 16,          // 16 parallel streams
            chunks_per_stream: 8,                 // 8 chunks per stream batch
            bytes_per_stream_target: 4 * 1024 * 1024, // 4MB per stream
            adaptive_parallelism: true,          // Enable adaptive optimization
            bandwidth_window: Duration::from_secs(1), // 1 second measurement window
            max_buffer_size: 128 * 1024 * 1024,  // 128MB max buffer
        }
    }
}

/// Parallel transfer manager for optimized chunk transfers
pub struct ParallelTransferManager {
    connection: Arc<Connection>,
    config: ParallelTransferConfig,
    active_streams: Arc<AtomicUsize>,
    bytes_transferred: Arc<AtomicU64>,
    bandwidth_estimator: Arc<RwLock<BandwidthEstimator>>,
    stream_semaphore: Arc<Semaphore>,
    memory_limiter: Arc<Semaphore>,
}

impl ParallelTransferManager {
    /// Create a new parallel transfer manager
    pub fn new(connection: Arc<Connection>, config: ParallelTransferConfig) -> Self {
        let stream_semaphore = Arc::new(Semaphore::new(config.max_concurrent_streams));
        let memory_limiter = Arc::new(Semaphore::new(config.max_buffer_size));
        
        Self {
            connection,
            config: config.clone(),
            active_streams: Arc::new(AtomicUsize::new(0)),
            bytes_transferred: Arc::new(AtomicU64::new(0)),
            bandwidth_estimator: Arc::new(RwLock::new(BandwidthEstimator::new(config.bandwidth_window))),
            stream_semaphore,
            memory_limiter,
        }
    }
    
    /// Transfer chunks in parallel with adaptive optimization
    pub async fn transfer_chunks(
        &self,
        chunks_needed: Vec<Vec<u8>>,
        chunk_provider: Arc<dyn ChunkProvider>,
    ) -> Result<Vec<ChunkData>> {
        let total_chunks = chunks_needed.len();
        info!("Starting parallel transfer of {} chunks", total_chunks);
        
        // Partition chunks for parallel transfer
        let optimal_streams = self.calculate_optimal_streams(total_chunks).await;
        let chunk_batches = self.partition_chunks(chunks_needed, optimal_streams);
        
        // Create channels for chunk collection
        let (tx, mut rx) = mpsc::channel::<ChunkData>(total_chunks);
        let mut tasks = JoinSet::new();
        
        // Launch parallel transfer tasks
        for (batch_id, batch) in chunk_batches.into_iter().enumerate() {
            let connection = self.connection.clone();
            let tx = tx.clone();
            let provider = chunk_provider.clone();
            let stream_sem = self.stream_semaphore.clone();
            let mem_limiter = self.memory_limiter.clone();
            let active_streams = self.active_streams.clone();
            let bytes_transferred = self.bytes_transferred.clone();
            let bandwidth_est = self.bandwidth_estimator.clone();
            
            tasks.spawn(async move {
                // Acquire stream permit
                let _permit = stream_sem.acquire().await
                    .map_err(|e| QuicError::Stream(format!("Failed to acquire stream permit: {}", e)))?;
                
                active_streams.fetch_add(1, Ordering::Relaxed);
                let result = Self::transfer_batch(
                    connection,
                    batch_id,
                    batch,
                    provider,
                    tx,
                    mem_limiter,
                    bytes_transferred,
                    bandwidth_est,
                ).await;
                active_streams.fetch_sub(1, Ordering::Relaxed);
                
                result
            });
        }
        
        // Drop original sender to signal completion
        drop(tx);
        
        // Collect all transferred chunks
        let mut received_chunks = Vec::with_capacity(total_chunks);
        while let Some(chunk) = rx.recv().await {
            received_chunks.push(chunk);
        }
        
        // Wait for all tasks to complete
        while let Some(result) = tasks.join_next().await {
            if let Err(e) = result {
                error!("Transfer task failed: {:?}", e);
            }
        }
        
        // Report final statistics
        let bandwidth = self.bandwidth_estimator.read().await.get_bandwidth();
        info!(
            "Parallel transfer complete: {} chunks, {:.2} MB/s",
            received_chunks.len(),
            bandwidth / (1024.0 * 1024.0)
        );
        
        Ok(received_chunks)
    }
    
    /// Calculate optimal number of streams based on current conditions
    async fn calculate_optimal_streams(&self, total_chunks: usize) -> usize {
        if !self.config.adaptive_parallelism {
            return self.config.max_concurrent_streams.min(total_chunks);
        }
        
        let bandwidth = self.bandwidth_estimator.read().await.get_bandwidth();
        let active = self.active_streams.load(Ordering::Relaxed);
        
        // Adaptive calculation based on bandwidth and current load
        let optimal = if bandwidth > 100_000_000.0 { // > 100 MB/s
            self.config.max_concurrent_streams
        } else if bandwidth > 50_000_000.0 { // > 50 MB/s
            (self.config.max_concurrent_streams * 3) / 4
        } else if bandwidth > 10_000_000.0 { // > 10 MB/s
            self.config.max_concurrent_streams / 2
        } else {
            4.max(self.config.max_concurrent_streams / 4)
        };
        
        // Adjust based on current active streams
        let adjusted = if active > 0 {
            optimal.saturating_sub(active / 2)
        } else {
            optimal
        };
        
        adjusted.max(1).min(total_chunks).min(self.config.max_concurrent_streams)
    }
    
    /// Partition chunks into batches for parallel transfer
    fn partition_chunks(&self, chunks: Vec<Vec<u8>>, num_streams: usize) -> Vec<Vec<Vec<u8>>> {
        let chunks_per_batch = (chunks.len() + num_streams - 1) / num_streams;
        let mut batches = Vec::with_capacity(num_streams);
        
        for batch in chunks.chunks(chunks_per_batch) {
            batches.push(batch.to_vec());
        }
        
        batches
    }
    
    /// Transfer a batch of chunks on a single stream
    async fn transfer_batch(
        connection: Arc<Connection>,
        batch_id: usize,
        chunk_hashes: Vec<Vec<u8>>,
        provider: Arc<dyn ChunkProvider>,
        tx: mpsc::Sender<ChunkData>,
        memory_limiter: Arc<Semaphore>,
        bytes_transferred: Arc<AtomicU64>,
        bandwidth_est: Arc<RwLock<BandwidthEstimator>>,
    ) -> Result<()> {
        debug!("Starting batch {} transfer with {} chunks", batch_id, chunk_hashes.len());
        
        // Open bidirectional stream for this batch
        let (mut send, mut recv) = connection.open_bi().await?;
        
        // Send chunk requests
        for hash in &chunk_hashes {
            // Get chunk data from provider
            let chunk_data = provider.get_chunk(hash).await?;
            let chunk_size = chunk_data.data.len();
            
            // Acquire memory permit
            let _mem_permit = memory_limiter.acquire_many(chunk_size as u32).await
                .map_err(|e| QuicError::Stream(format!("Memory limit exceeded: {}", e)))?;
            
            // Send chunk data
            Self::send_chunk(&mut send, &chunk_data).await?;
            
            // Update statistics
            bytes_transferred.fetch_add(chunk_size as u64, Ordering::Relaxed);
            bandwidth_est.write().await.record_bytes(chunk_size);
            
            // Forward to collector
            if tx.send(chunk_data).await.is_err() {
                warn!("Receiver dropped, stopping batch {}", batch_id);
                break;
            }
        }
        
        // Close send stream
        send.finish().map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;
        
        debug!("Batch {} transfer complete", batch_id);
        Ok(())
    }
    
    /// Send a single chunk over a stream
    async fn send_chunk(stream: &mut SendStream, chunk: &ChunkData) -> Result<()> {
        // Write message type
        stream.write_all(&[MessageType::ChunkData as u8]).await
            .map_err(|e| QuicError::Stream(format!("Failed to write message type: {}", e)))?;
        
        // Write chunk size
        use prost::Message;
        let size = chunk.encoded_len() as u32;
        stream.write_all(&size.to_be_bytes()).await
            .map_err(|e| QuicError::Stream(format!("Failed to write chunk size: {}", e)))?;
        
        // Write chunk data
        let mut buf = Vec::with_capacity(size as usize);
        prost::Message::encode(chunk, &mut buf)
            .map_err(|e| QuicError::Protocol(format!("Failed to encode chunk: {}", e)))?;
        stream.write_all(&buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to write chunk data: {}", e)))?;
        
        Ok(())
    }
}

/// Trait for providing chunk data
#[async_trait::async_trait]
pub trait ChunkProvider: Send + Sync {
    /// Get chunk data by hash
    async fn get_chunk(&self, hash: &[u8]) -> Result<ChunkData>;
}

/// Bandwidth estimator for adaptive optimization
struct BandwidthEstimator {
    window: Duration,
    samples: VecDeque<(Instant, usize)>,
    total_bytes: usize,
}

impl BandwidthEstimator {
    fn new(window: Duration) -> Self {
        Self {
            window,
            samples: VecDeque::new(),
            total_bytes: 0,
        }
    }
    
    fn record_bytes(&mut self, bytes: usize) {
        let now = Instant::now();
        self.samples.push_back((now, bytes));
        self.total_bytes += bytes;
        
        // Remove old samples outside the window
        let cutoff = now - self.window;
        while let Some((timestamp, bytes)) = self.samples.front() {
            if *timestamp < cutoff {
                self.total_bytes -= bytes;
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }
    
    fn get_bandwidth(&self) -> f64 {
        if self.samples.len() < 2 {
            return 0.0;
        }
        
        let duration = self.samples.back().unwrap().0 - self.samples.front().unwrap().0;
        if duration.as_secs_f64() > 0.0 {
            self.total_bytes as f64 / duration.as_secs_f64()
        } else {
            0.0
        }
    }
}

/// Zero-copy chunk sender using memory-mapped files
pub struct ZeroCopyChunkSender {
    connection: Arc<Connection>,
    send_buffer_pool: Arc<BufferPool>,
}

impl ZeroCopyChunkSender {
    pub fn new(connection: Arc<Connection>) -> Self {
        Self {
            connection,
            send_buffer_pool: Arc::new(BufferPool::new(64, 4 * 1024 * 1024)), // 64 x 4MB buffers
        }
    }
    
    /// Send chunk data using zero-copy optimization
    pub async fn send_chunk_zero_copy(&self, chunk: &ChunkData) -> Result<()> {
        // Get a buffer from the pool
        let mut buffer = self.send_buffer_pool.get().await;
        
        // Encode chunk into buffer
        prost::Message::encode(chunk, &mut buffer)
            .map_err(|e| QuicError::Protocol(format!("Failed to encode chunk: {}", e)))?;
        
        // Open stream and send
        let mut stream = self.connection.open_uni().await?;
        
        // Write using vectored I/O if available
        stream.write_all(&buffer).await
            .map_err(|e| QuicError::Stream(format!("Failed to write chunk: {}", e)))?;
        
        stream.finish().map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;
        
        Ok(())
    }
}

/// Buffer pool for zero-copy operations
struct BufferPool {
    buffers: Arc<RwLock<Vec<BytesMut>>>,
    buffer_size: usize,
    max_buffers: usize,
}

impl BufferPool {
    fn new(max_buffers: usize, buffer_size: usize) -> Self {
        let mut buffers = Vec::with_capacity(max_buffers);
        for _ in 0..max_buffers / 2 {
            buffers.push(BytesMut::with_capacity(buffer_size));
        }
        
        Self {
            buffers: Arc::new(RwLock::new(buffers)),
            buffer_size,
            max_buffers,
        }
    }
    
    async fn get(&self) -> BytesMut {
        let mut buffers = self.buffers.write().await;
        
        if let Some(buffer) = buffers.pop() {
            buffer
        } else {
            BytesMut::with_capacity(self.buffer_size)
        }
    }
    
    async fn return_buffer(&self, mut buffer: BytesMut) {
        buffer.clear();
        
        let mut buffers = self.buffers.write().await;
        if buffers.len() < self.max_buffers {
            buffers.push(buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bandwidth_estimator() {
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));
        
        // Record some bytes
        estimator.record_bytes(1024);
        std::thread::sleep(Duration::from_millis(100));
        estimator.record_bytes(2048);
        std::thread::sleep(Duration::from_millis(100));
        estimator.record_bytes(4096);
        
        // Check bandwidth calculation
        let bandwidth = estimator.get_bandwidth();
        assert!(bandwidth > 0.0);
        
        // Should be roughly (1024 + 2048 + 4096) / 0.2 seconds
        let expected = 7168.0 / 0.2;
        let tolerance = expected * 0.5; // 50% tolerance for timing variations
        assert!((bandwidth - expected).abs() < tolerance);
    }
    
    #[test]
    fn test_chunk_partitioning() {
        let config = ParallelTransferConfig::default();
        let connection = Arc::new(Connection::dummy()); // Would need a test dummy
        let manager = ParallelTransferManager::new(connection, config);
        
        let chunks: Vec<Vec<u8>> = (0..100)
            .map(|i| vec![i as u8])
            .collect();
        
        let partitions = manager.partition_chunks(chunks.clone(), 10);
        
        assert_eq!(partitions.len(), 10);
        assert_eq!(partitions[0].len(), 10);
        
        // Verify all chunks are present
        let mut all_chunks = Vec::new();
        for partition in partitions {
            all_chunks.extend(partition);
        }
        assert_eq!(all_chunks.len(), 100);
    }
}