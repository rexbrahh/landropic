//! Stream-based data transfer implementation for optimized QUIC file synchronization
//! 
//! This module provides high-performance streaming capabilities for chunk transfers
//! with support for multiplexing, flow control, and adaptive optimization.

use bytes::{Bytes, BytesMut};
use futures::stream::{Stream, StreamExt};
use quinn::{RecvStream, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

use crate::connection::Connection;
use crate::errors::{QuicError, Result};
use crate::protocol::MessageType;
use landro_proto::ChunkData;

/// Stream transfer configuration for optimized data flow
#[derive(Clone, Debug)]
pub struct StreamTransferConfig {
    /// Maximum concurrent streams per transfer session
    pub max_concurrent_streams: usize,
    
    /// Stream buffer size for efficient batching
    pub stream_buffer_size: usize,
    
    /// Enable adaptive flow control based on network conditions
    pub adaptive_flow_control: bool,
    
    /// Flow control window size (bytes)
    pub flow_control_window: usize,
    
    /// Backpressure threshold (percentage of window)
    pub backpressure_threshold: f32,
    
    /// Stream priority levels (0-255, higher = more priority)
    pub enable_stream_priority: bool,
    
    /// Maximum retransmission attempts
    pub max_retransmit_attempts: u32,
    
    /// Stream timeout duration
    pub stream_timeout: Duration,
    
    /// Enable stream compression
    pub enable_compression: bool,
}

impl Default for StreamTransferConfig {
    fn default() -> Self {
        Self::file_sync_optimized()
    }
}

impl StreamTransferConfig {
    /// Configuration optimized for file sync workloads
    pub fn file_sync_optimized() -> Self {
        Self {
            max_concurrent_streams: 64,
            stream_buffer_size: 8 * 1024 * 1024,  // 8MB buffers
            adaptive_flow_control: true,
            flow_control_window: 64 * 1024 * 1024,  // 64MB window
            backpressure_threshold: 0.8,  // Apply backpressure at 80% capacity
            enable_stream_priority: true,
            max_retransmit_attempts: 3,
            stream_timeout: Duration::from_secs(30),
            enable_compression: false,  // Chunks already compressed
        }
    }
    
    /// Configuration for diff-based sync operations
    pub fn diff_sync_optimized() -> Self {
        Self {
            max_concurrent_streams: 32,
            stream_buffer_size: 4 * 1024 * 1024,  // 4MB buffers for diffs
            adaptive_flow_control: true,
            flow_control_window: 32 * 1024 * 1024,  // 32MB window
            backpressure_threshold: 0.75,
            enable_stream_priority: true,
            max_retransmit_attempts: 5,  // More retries for critical diffs
            stream_timeout: Duration::from_secs(60),
            enable_compression: true,  // Compress diff data
        }
    }
}

/// Stream transfer manager for coordinated data streaming
pub struct StreamTransferManager {
    connection: Arc<Connection>,
    config: StreamTransferConfig,
    active_streams: Arc<AtomicUsize>,
    bytes_transferred: Arc<AtomicU64>,
    flow_controller: Arc<RwLock<FlowController>>,
    stream_pool: Arc<StreamPool>,
    priority_scheduler: Arc<PriorityScheduler>,
}

impl StreamTransferManager {
    /// Create a new stream transfer manager
    pub fn new(connection: Arc<Connection>, config: StreamTransferConfig) -> Self {
        let flow_controller = Arc::new(RwLock::new(FlowController::new(
            config.flow_control_window,
            config.backpressure_threshold,
        )));
        
        let stream_pool = Arc::new(StreamPool::new(
            config.max_concurrent_streams,
            config.stream_buffer_size,
        ));
        
        let priority_scheduler = Arc::new(PriorityScheduler::new(
            config.enable_stream_priority,
        ));
        
        Self {
            connection,
            config,
            active_streams: Arc::new(AtomicUsize::new(0)),
            bytes_transferred: Arc::new(AtomicU64::new(0)),
            flow_controller,
            stream_pool,
            priority_scheduler,
        }
    }
    
    /// Stream chunks with multiplexing and flow control
    pub async fn stream_chunks(
        &self,
        chunks: Vec<ChunkData>,
        priority: u8,
    ) -> Result<mpsc::Receiver<TransferResult>> {
        info!("Starting stream transfer of {} chunks with priority {}", chunks.len(), priority);
        
        let (result_tx, result_rx) = mpsc::channel(chunks.len());
        let mut tasks = JoinSet::new();
        
        // Schedule chunks based on priority
        let scheduled_chunks = if self.config.enable_stream_priority {
            self.priority_scheduler.schedule(chunks, priority).await
        } else {
            chunks
        };
        
        // Process chunks in batches to respect stream limits
        for batch in scheduled_chunks.chunks(self.config.max_concurrent_streams) {
            for chunk in batch {
                let stream_handle = self.stream_pool.acquire().await?;
                let flow_permit = self.flow_controller.write().await
                    .acquire_capacity(chunk.data.len()).await?;
                
                let connection = self.connection.clone();
                let chunk = chunk.clone();
                let result_tx = result_tx.clone();
                let active_streams = self.active_streams.clone();
                let bytes_transferred = self.bytes_transferred.clone();
                let config = self.config.clone();
                
                tasks.spawn(async move {
                    active_streams.fetch_add(1, Ordering::Relaxed);
                    
                    let result = Self::transfer_chunk_stream(
                        connection,
                        chunk,
                        stream_handle,
                        flow_permit,
                        config,
                    ).await;
                    
                    if let Ok(ref transfer_result) = result {
                        bytes_transferred.fetch_add(transfer_result.bytes_transferred as u64, Ordering::Relaxed);
                    }
                    
                    active_streams.fetch_sub(1, Ordering::Relaxed);
                    
                    let _ = result_tx.send(result.unwrap_or_else(|e| TransferResult {
                        chunk_hash: vec![],
                        success: false,
                        bytes_transferred: 0,
                        duration: Duration::default(),
                        error: Some(e.to_string()),
                    })).await;
                });
            }
            
            // Wait for some streams to complete before starting next batch
            if self.active_streams.load(Ordering::Relaxed) >= self.config.max_concurrent_streams / 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        
        // Spawn task to collect results
        tokio::spawn(async move {
            while let Some(result) = tasks.join_next().await {
                if let Err(e) = result {
                    error!("Stream task failed: {:?}", e);
                }
            }
        });
        
        Ok(result_rx)
    }
    
    /// Transfer a single chunk over a stream with retries
    async fn transfer_chunk_stream(
        connection: Arc<Connection>,
        chunk: ChunkData,
        mut stream_handle: StreamHandle,
        flow_permit: FlowPermit,
        config: StreamTransferConfig,
    ) -> Result<TransferResult> {
        let start = Instant::now();
        let chunk_hash = chunk.hash.clone();
        let chunk_size = chunk.data.len();
        
        let mut attempts = 0;
        let mut last_error = None;
        
        while attempts < config.max_retransmit_attempts {
            attempts += 1;
            
            match Self::send_chunk_with_timeout(
                &connection,
                &chunk,
                &mut stream_handle,
                config.stream_timeout,
            ).await {
                Ok(_) => {
                    debug!("Successfully transferred chunk {} ({} bytes)", 
                           hex::encode(&chunk_hash), chunk_size);
                    
                    return Ok(TransferResult {
                        chunk_hash,
                        success: true,
                        bytes_transferred: chunk_size,
                        duration: start.elapsed(),
                        error: None,
                    });
                }
                Err(e) => {
                    warn!("Transfer attempt {} failed for chunk {}: {}", 
                          attempts, hex::encode(&chunk_hash), e);
                    last_error = Some(e.to_string());
                    
                    if attempts < config.max_retransmit_attempts {
                        tokio::time::sleep(Duration::from_millis(100 * attempts as u64)).await;
                    }
                }
            }
        }
        
        Err(QuicError::Stream(format!(
            "Failed to transfer chunk after {} attempts: {}",
            attempts,
            last_error.unwrap_or_else(|| "Unknown error".to_string())
        )))
    }
    
    /// Send chunk with timeout protection
    async fn send_chunk_with_timeout(
        connection: &Arc<Connection>,
        chunk: &ChunkData,
        stream_handle: &mut StreamHandle,
        timeout: Duration,
    ) -> Result<()> {
        tokio::time::timeout(timeout, async {
            let (mut send, mut recv) = connection.open_bi().await?;
            
            // Write message header
            send.write_all(&[MessageType::ChunkData as u8]).await
                .map_err(|e| QuicError::Stream(format!("Failed to write header: {}", e)))?;
            
            // Write chunk data
            use prost::Message;
            let mut buf = Vec::with_capacity(chunk.encoded_len());
            chunk.encode(&mut buf)
                .map_err(|e| QuicError::Protocol(format!("Failed to encode chunk: {}", e)))?;
            
            send.write_all(&(buf.len() as u32).to_be_bytes()).await
                .map_err(|e| QuicError::Stream(format!("Failed to write size: {}", e)))?;
            
            send.write_all(&buf).await
                .map_err(|e| QuicError::Stream(format!("Failed to write data: {}", e)))?;
            
            send.finish()
                .map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;
            
            Ok(())
        })
        .await
        .map_err(|_| QuicError::Stream("Stream transfer timeout".to_string()))?
    }
    
    /// Handle diff protocol integration for incremental sync
    pub async fn stream_diff(
        &self,
        diff_request: DiffRequest,
    ) -> Result<DiffResponse> {
        debug!("Processing diff request for {} chunks", diff_request.chunk_hashes.len());
        
        let (mut send, mut recv) = self.connection.open_bi().await?;
        
        // Send diff request
        use prost::Message;
        let mut buf = Vec::with_capacity(diff_request.encoded_len());
        diff_request.encode(&mut buf)
            .map_err(|e| QuicError::Protocol(format!("Failed to encode diff request: {}", e)))?;
        
        send.write_all(&[MessageType::DiffRequest as u8]).await
            .map_err(|e| QuicError::Stream(format!("Failed to write header: {}", e)))?;
        
        send.write_all(&(buf.len() as u32).to_be_bytes()).await
            .map_err(|e| QuicError::Stream(format!("Failed to write size: {}", e)))?;
        
        send.write_all(&buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to write diff request: {}", e)))?;
        
        send.finish()
            .map_err(|e| QuicError::Stream(format!("Failed to finish send stream: {}", e)))?;
        
        // Read diff response
        let msg_type = recv.read_u8().await
            .map_err(|e| QuicError::Stream(format!("Failed to read response type: {}", e)))?;
        
        if msg_type != MessageType::DiffResponse as u8 {
            return Err(QuicError::Protocol(format!(
                "Expected DiffResponse, got message type {}",
                msg_type
            )));
        }
        
        let size = recv.read_u32().await
            .map_err(|e| QuicError::Stream(format!("Failed to read response size: {}", e)))?;
        
        let mut response_buf = vec![0u8; size as usize];
        recv.read_exact(&mut response_buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to read response data: {}", e)))?;
        
        DiffResponse::decode(&response_buf[..])
            .map_err(|e| QuicError::Protocol(format!("Failed to decode diff response: {}", e)))
    }
    
    /// Create a multiplexed stream for parallel chunk transfers
    pub async fn create_multiplexed_stream(
        &self,
        num_substreams: usize,
    ) -> Result<MultiplexedStream> {
        let substreams = Arc::new(RwLock::new(Vec::with_capacity(num_substreams)));
        
        for _ in 0..num_substreams {
            let (send, recv) = self.connection.open_bi().await?;
            substreams.write().await.push((
                Arc::new(Mutex::new(send)), 
                Arc::new(Mutex::new(recv))
            ));
        }
        
        Ok(MultiplexedStream {
            substreams,
            current_index: Arc::new(AtomicUsize::new(0)),
            config: self.config.clone(),
        })
    }
}

/// Multiplexed stream for parallel data transfer
pub struct MultiplexedStream {
    substreams: Arc<RwLock<Vec<(Arc<Mutex<SendStream>>, Arc<Mutex<RecvStream>>)>>>,
    current_index: Arc<AtomicUsize>,
    config: StreamTransferConfig,
}

impl MultiplexedStream {
    /// Send data on the next available substream
    pub async fn send(&self, data: &[u8]) -> Result<()> {
        let streams = self.substreams.read().await;
        let index = self.current_index.fetch_add(1, Ordering::Relaxed) % streams.len();
        
        // Lock the send stream for writing
        let stream_mutex = &streams[index].0;
        let mut stream = stream_mutex.lock().await;
        stream.write_all(data).await
            .map_err(|e| QuicError::Stream(format!("Failed to write to substream: {}", e)))?;
        
        Ok(())
    }
    
    /// Receive data from any substream
    pub async fn recv(&self) -> Result<Vec<u8>> {
        let streams = self.substreams.read().await;
        let index = self.current_index.load(Ordering::Relaxed) % streams.len();
        
        // Lock the recv stream for reading
        let stream_mutex = &streams[index].1;
        let mut stream = stream_mutex.lock().await;
        let mut buf = vec![0u8; self.config.stream_buffer_size];
        
        let n = stream.read(&mut buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to read from substream: {}", e)))?
            .ok_or_else(|| QuicError::Protocol("Substream closed".to_string()))?;
        
        buf.truncate(n);
        Ok(buf)
    }
}

/// Flow controller for managing stream bandwidth
struct FlowController {
    window_size: usize,
    available_capacity: Arc<AtomicUsize>,
    backpressure_threshold: f32,
    waiters: Arc<RwLock<VecDeque<mpsc::Sender<()>>>>,
}

impl FlowController {
    fn new(window_size: usize, backpressure_threshold: f32) -> Self {
        Self {
            window_size,
            available_capacity: Arc::new(AtomicUsize::new(window_size)),
            backpressure_threshold,
            waiters: Arc::new(RwLock::new(VecDeque::new())),
        }
    }
    
    async fn acquire_capacity(&mut self, size: usize) -> Result<FlowPermit> {
        // Check if we need to apply backpressure
        let current = self.available_capacity.load(Ordering::Relaxed);
        let threshold = (self.window_size as f32 * self.backpressure_threshold) as usize;
        
        if current < threshold {
            debug!("Applying backpressure: {} / {} bytes available", current, self.window_size);
            
            // Wait for capacity to become available
            let (tx, mut rx) = mpsc::channel(1);
            self.waiters.write().await.push_back(tx);
            
            rx.recv().await
                .ok_or_else(|| QuicError::Stream("Flow control waiter dropped".to_string()))?;
        }
        
        // Try to acquire capacity
        let mut current = self.available_capacity.load(Ordering::Relaxed);
        loop {
            if current < size {
                // Not enough capacity, wait
                tokio::time::sleep(Duration::from_millis(10)).await;
                current = self.available_capacity.load(Ordering::Relaxed);
                continue;
            }
            
            match self.available_capacity.compare_exchange(
                current,
                current - size,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Ok(FlowPermit {
                        size,
                        capacity: self.available_capacity.clone(),
                        released: Arc::new(AtomicBool::new(false)),
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }
    
    async fn notify_waiters(&self) {
        let mut waiters = self.waiters.write().await;
        while let Some(waiter) = waiters.pop_front() {
            let _ = waiter.send(()).await;
        }
    }
}

/// Flow control permit that releases capacity when dropped
struct FlowPermit {
    size: usize,
    capacity: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
}

impl Drop for FlowPermit {
    fn drop(&mut self) {
        if !self.released.swap(true, Ordering::Relaxed) {
            self.capacity.fetch_add(self.size, Ordering::Relaxed);
        }
    }
}

/// Stream pool for efficient stream reuse
struct StreamPool {
    max_streams: usize,
    buffer_size: usize,
    available: Arc<RwLock<Vec<StreamHandle>>>,
    semaphore: Arc<Semaphore>,
}

impl StreamPool {
    fn new(max_streams: usize, buffer_size: usize) -> Self {
        Self {
            max_streams,
            buffer_size,
            available: Arc::new(RwLock::new(Vec::with_capacity(max_streams))),
            semaphore: Arc::new(Semaphore::new(max_streams)),
        }
    }
    
    async fn acquire(&self) -> Result<StreamHandle> {
        let _permit = self.semaphore.acquire().await
            .map_err(|e| QuicError::Stream(format!("Failed to acquire stream permit: {}", e)))?;
        
        let mut available = self.available.write().await;
        
        if let Some(handle) = available.pop() {
            Ok(handle)
        } else {
            Ok(StreamHandle {
                id: uuid::Uuid::new_v4().to_string(),
                buffer: BytesMut::with_capacity(self.buffer_size),
                created_at: Instant::now(),
            })
        }
    }
    
    async fn release(&self, mut handle: StreamHandle) {
        handle.buffer.clear();
        
        let mut available = self.available.write().await;
        if available.len() < self.max_streams {
            available.push(handle);
        }
    }
}

/// Handle to a pooled stream
struct StreamHandle {
    id: String,
    buffer: BytesMut,
    created_at: Instant,
}

/// Priority scheduler for stream prioritization
struct PriorityScheduler {
    enabled: bool,
    queues: Arc<RwLock<HashMap<u8, VecDeque<ChunkData>>>>,
}

impl PriorityScheduler {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            queues: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    async fn schedule(&self, chunks: Vec<ChunkData>, priority: u8) -> Vec<ChunkData> {
        if !self.enabled {
            return chunks;
        }
        
        let mut queues = self.queues.write().await;
        let queue = queues.entry(priority).or_insert_with(VecDeque::new);
        
        for chunk in chunks {
            queue.push_back(chunk);
        }
        
        // Return chunks in priority order
        let mut scheduled = Vec::new();
        
        // Process from highest to lowest priority
        for priority in (0..=255).rev() {
            if let Some(queue) = queues.get_mut(&priority) {
                while let Some(chunk) = queue.pop_front() {
                    scheduled.push(chunk);
                    
                    // Yield occasionally to prevent blocking
                    if scheduled.len() % 100 == 0 {
                        tokio::task::yield_now().await;
                    }
                }
            }
        }
        
        scheduled
    }
}

/// Result of a chunk transfer operation
#[derive(Debug, Clone)]
pub struct TransferResult {
    pub chunk_hash: Vec<u8>,
    pub success: bool,
    pub bytes_transferred: usize,
    pub duration: Duration,
    pub error: Option<String>,
}

/// Stream for receiving transfer results
pub struct TransferResultStream {
    receiver: mpsc::Receiver<TransferResult>,
}

impl Stream for TransferResultStream {
    type Item = TransferResult;
    
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_stream_config_defaults() {
        let config = StreamTransferConfig::default();
        assert_eq!(config.max_concurrent_streams, 64);
        assert!(config.adaptive_flow_control);
    }
    
    #[test]
    fn test_diff_sync_config() {
        let config = StreamTransferConfig::diff_sync_optimized();
        assert_eq!(config.max_concurrent_streams, 32);
        assert!(config.enable_compression);
    }
    
    #[tokio::test]
    async fn test_flow_controller() {
        let mut controller = FlowController::new(1000, 0.8);
        
        // Acquire some capacity
        let permit1 = controller.acquire_capacity(300).await.unwrap();
        assert_eq!(controller.available_capacity.load(Ordering::Relaxed), 700);
        
        // Acquire more capacity
        let permit2 = controller.acquire_capacity(400).await.unwrap();
        assert_eq!(controller.available_capacity.load(Ordering::Relaxed), 300);
        
        // Release first permit
        drop(permit1);
        assert_eq!(controller.available_capacity.load(Ordering::Relaxed), 600);
        
        // Release second permit
        drop(permit2);
        assert_eq!(controller.available_capacity.load(Ordering::Relaxed), 1000);
    }
    
    #[tokio::test]
    async fn test_priority_scheduler() {
        let scheduler = PriorityScheduler::new(true);
        
        let chunk1 = ChunkData {
            hash: vec![1],
            data: vec![],
            compressed: false,
            uncompressed_size: 0,
        };
        
        let chunk2 = ChunkData {
            hash: vec![2],
            data: vec![],
            compressed: false,
            uncompressed_size: 0,
        };
        
        let chunk3 = ChunkData {
            hash: vec![3],
            data: vec![],
            compressed: false,
            uncompressed_size: 0,
        };
        
        // Schedule with different priorities
        let _ = scheduler.schedule(vec![chunk1.clone()], 100).await;
        let _ = scheduler.schedule(vec![chunk2.clone()], 200).await;
        let _ = scheduler.schedule(vec![chunk3.clone()], 150).await;
        
        // Get scheduled chunks
        let scheduled = scheduler.schedule(vec![], 0).await;
        
        // Should be in priority order: 200, 150, 100
        assert_eq!(scheduled[0].hash, vec![2]);
        assert_eq!(scheduled[1].hash, vec![3]);
        assert_eq!(scheduled[2].hash, vec![1]);
    }
}

/// Placeholder diff request type - TODO: implement proper protobuf schema
#[derive(Clone, Debug)]
pub struct DiffRequest {
    pub chunk_hashes: Vec<Vec<u8>>,
}

impl DiffRequest {
    pub fn encoded_len(&self) -> usize {
        // Simple placeholder implementation
        4 + self.chunk_hashes.iter().map(|h| h.len() + 4).sum::<usize>()
    }
    
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        // Simple placeholder implementation
        buf.extend_from_slice(&(self.chunk_hashes.len() as u32).to_be_bytes());
        for hash in &self.chunk_hashes {
            buf.extend_from_slice(&(hash.len() as u32).to_be_bytes());
            buf.extend_from_slice(hash);
        }
        Ok(())
    }
}

/// Placeholder diff response type - TODO: implement proper protobuf schema
#[derive(Clone, Debug)]
pub struct DiffResponse {
    pub available_chunks: Vec<Vec<u8>>,
}

impl DiffResponse {
    pub fn decode(_buf: &[u8]) -> Result<Self> {
        // Simple placeholder implementation
        Ok(DiffResponse {
            available_chunks: Vec::new(),
        })
    }
}