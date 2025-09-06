//! Chunk provider implementation for serving chunks from CAS storage over QUIC streams

use async_trait::async_trait;
use bytes::Bytes;
use quinn::{RecvStream, SendStream};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

use landro_cas::{ContentStore, ObjectRef};
use landro_chunker::ContentHash;
use landro_proto::{ChunkData, ChunkRequest};

use crate::connection::Connection;
use crate::errors::{QuicError, Result};

/// Statistics for chunk serving
#[derive(Debug, Clone, Default)]
pub struct ChunkServerStats {
    pub chunks_served: u64,
    pub bytes_served: u64,
    pub chunks_not_found: u64,
    pub errors: u64,
    pub avg_latency_ms: f64,
}

/// Provides chunks from CAS storage to remote peers with optimizations
pub struct ChunkProvider {
    /// Content store for reading chunks
    content_store: Arc<ContentStore>,
    /// Active chunk requests being processed
    active_requests: Arc<RwLock<HashMap<String, ChunkRequestInfo>>>,
    /// Statistics
    stats: Arc<RwLock<ChunkServerStats>>,
    /// Chunk cache for frequently accessed chunks
    chunk_cache: Arc<RwLock<lru::LruCache<ContentHash, Bytes>>>,
    /// Prefetch queue for anticipated chunks
    prefetch_queue: Arc<RwLock<VecDeque<ContentHash>>>,
    /// Maximum concurrent prefetches
    prefetch_semaphore: Arc<Semaphore>,
}

#[derive(Debug)]
struct ChunkRequestInfo {
    transfer_id: String,
    chunk_hash: ContentHash,
    started_at: Instant,
    peer_id: String,
}

impl ChunkProvider {
    /// Create a new chunk provider with optimizations
    pub fn new(content_store: Arc<ContentStore>) -> Self {
        Self {
            content_store,
            active_requests: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(ChunkServerStats::default())),
            chunk_cache: Arc::new(RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(256).unwrap() // Cache 256 chunks
            ))),
            prefetch_queue: Arc::new(RwLock::new(VecDeque::with_capacity(64))),
            prefetch_semaphore: Arc::new(Semaphore::new(8)), // Max 8 concurrent prefetches
        }
    }
    
    /// Create with custom cache size
    pub fn with_cache_size(content_store: Arc<ContentStore>, cache_size: usize) -> Self {
        Self {
            content_store,
            active_requests: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(ChunkServerStats::default())),
            chunk_cache: Arc::new(RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(cache_size.max(1)).unwrap()
            ))),
            prefetch_queue: Arc::new(RwLock::new(VecDeque::with_capacity(cache_size / 4))),
            prefetch_semaphore: Arc::new(Semaphore::new(16)), // More prefetch capacity
        }
    }

    /// Handle incoming chunk request on a bidirectional stream
    pub async fn handle_chunk_request(
        &self,
        mut recv: RecvStream,
        mut send: SendStream,
        peer_id: String,
    ) -> Result<()> {
        // Read chunk request
        let request = self.read_chunk_request(&mut recv).await?;
        let chunk_hash = ContentHash::from_bytes(
            request.chunk_hash.as_slice().try_into()
                .map_err(|_| QuicError::Protocol("Invalid chunk hash".to_string()))?
        );

        debug!(
            "Received chunk request for {} from {} (transfer: {})",
            chunk_hash, peer_id, request.transfer_id
        );

        // Track request
        let request_id = format!("{}:{}", request.transfer_id, chunk_hash);
        {
            let mut active = self.active_requests.write().await;
            active.insert(
                request_id.clone(),
                ChunkRequestInfo {
                    transfer_id: request.transfer_id.clone(),
                    chunk_hash,
                    started_at: Instant::now(),
                    peer_id: peer_id.clone(),
                },
            );
        }

        // Serve the chunk
        let result = self.serve_chunk(&mut send, &chunk_hash).await;

        // Update stats and cleanup
        self.finalize_request(&request_id, result.is_ok()).await;

        result
    }

    /// Serve a chunk from storage with caching
    async fn serve_chunk(&self, stream: &mut SendStream, chunk_hash: &ContentHash) -> Result<()> {
        let start = Instant::now();

        // Check cache first
        if let Some(cached_data) = self.chunk_cache.write().await.get(chunk_hash) {
            debug!("Serving chunk {} from cache ({} bytes)", chunk_hash, cached_data.len());
            
            let chunk_data = ChunkData {
                hash: chunk_hash.to_vec(),
                data: cached_data.to_vec(),
                offset: 0,
                total_size: cached_data.len() as u64,
            };
            
            self.write_chunk_data(stream, &chunk_data).await?;
            
            // Update stats for cache hit
            let mut stats = self.stats.write().await;
            stats.chunks_served += 1;
            stats.bytes_served += cached_data.len() as u64;
            
            return Ok(());
        }

        // Read chunk from content store
        match self.content_store.read(chunk_hash).await {
            Ok(data) => {
                debug!("Serving chunk {} ({} bytes)", chunk_hash, data.len());

                // Add to cache for future requests
                let data_bytes = Bytes::from(data.to_vec());
                self.chunk_cache.write().await.put(*chunk_hash, data_bytes.clone());

                // Create chunk data message
                let chunk_data = ChunkData {
                    hash: chunk_hash.to_vec(),
                    data: data.to_vec(),
                    offset: 0,
                    total_size: data.len() as u64,
                };

                // Write chunk data to stream
                self.write_chunk_data(stream, &chunk_data).await?;

                // Update stats
                let mut stats = self.stats.write().await;
                stats.chunks_served += 1;
                stats.bytes_served += data.len() as u64;
                
                let latency_ms = start.elapsed().as_millis() as f64;
                stats.avg_latency_ms = 
                    (stats.avg_latency_ms * (stats.chunks_served - 1) as f64 + latency_ms) 
                    / stats.chunks_served as f64;

                info!(
                    "Served chunk {} ({} bytes) in {:.2}ms",
                    chunk_hash,
                    data.len(),
                    latency_ms
                );

                Ok(())
            }
            Err(landro_cas::CasError::ObjectNotFound(_)) => {
                warn!("Chunk not found: {}", chunk_hash);
                
                let mut stats = self.stats.write().await;
                stats.chunks_not_found += 1;

                // Send error response
                self.write_chunk_not_found(stream, chunk_hash).await?;
                
                Err(QuicError::Protocol(format!("Chunk not found: {}", chunk_hash)))
            }
            Err(e) => {
                error!("Failed to read chunk {}: {}", chunk_hash, e);
                
                let mut stats = self.stats.write().await;
                stats.errors += 1;

                Err(QuicError::Internal(format!("Storage error: {}", e)))
            }
        }
    }

    /// Read chunk request from stream
    async fn read_chunk_request(&self, stream: &mut RecvStream) -> Result<ChunkRequest> {
        use prost::Message;
        use tokio::io::AsyncReadExt;

        // Read message type
        let msg_type = stream.read_u8().await
            .map_err(|e| QuicError::Stream(format!("Failed to read type: {}", e)))?
            .ok_or_else(|| QuicError::Protocol("Stream closed unexpectedly".to_string()))?;

        if msg_type != 4 { // MessageType::ChunkRequest
            return Err(QuicError::Protocol(format!(
                "Expected ChunkRequest message, got type {}",
                msg_type
            )));
        }

        // Read message length
        let length = stream.read_u32().await
            .map_err(|e| QuicError::Stream(format!("Failed to read length: {}", e)))?
            .ok_or_else(|| QuicError::Protocol("Stream closed unexpectedly".to_string()))?;

        // Validate length
        if length > 1024 * 1024 { // 1MB max for request
            return Err(QuicError::Protocol(format!(
                "ChunkRequest too large: {} bytes",
                length
            )));
        }

        // Read message data
        let mut buf = vec![0u8; length as usize];
        stream.read_exact(&mut buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to read data: {}", e)))?;

        // Decode request
        ChunkRequest::decode(&buf[..])
            .map_err(|e| QuicError::Protocol(format!("Failed to decode request: {}", e)))
    }

    /// Write chunk data to stream
    async fn write_chunk_data(&self, stream: &mut SendStream, chunk: &ChunkData) -> Result<()> {
        use prost::Message;
        use tokio::io::AsyncWriteExt;

        let mut buf = Vec::with_capacity(chunk.encoded_len());
        chunk.encode(&mut buf)
            .map_err(|e| QuicError::Protocol(format!("Failed to encode chunk: {}", e)))?;

        // Write message type and length
        stream.write_u8(4).await // MessageType::ChunkData
            .map_err(|e| QuicError::Stream(format!("Failed to write type: {}", e)))?;
        stream.write_u32(buf.len() as u32).await
            .map_err(|e| QuicError::Stream(format!("Failed to write length: {}", e)))?;
        stream.write_all(&buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to write data: {}", e)))?;

        stream.finish()
            .map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        Ok(())
    }

    /// Write chunk not found error
    async fn write_chunk_not_found(
        &self,
        stream: &mut SendStream,
        chunk_hash: &ContentHash,
    ) -> Result<()> {
        use prost::Message;
        use tokio::io::AsyncWriteExt;

        let error = landro_proto::Error {
            code: 404,
            message: format!("Chunk not found: {}", chunk_hash),
            details: vec![],
        };

        let mut buf = Vec::with_capacity(error.encoded_len());
        error.encode(&mut buf)
            .map_err(|e| QuicError::Protocol(format!("Failed to encode error: {}", e)))?;

        // Write error message
        stream.write_u8(6).await // MessageType::Error
            .map_err(|e| QuicError::Stream(format!("Failed to write type: {}", e)))?;
        stream.write_u32(buf.len() as u32).await
            .map_err(|e| QuicError::Stream(format!("Failed to write length: {}", e)))?;
        stream.write_all(&buf).await
            .map_err(|e| QuicError::Stream(format!("Failed to write error: {}", e)))?;

        stream.finish()
            .map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        Ok(())
    }

    /// Finalize request tracking
    async fn finalize_request(&self, request_id: &str, success: bool) {
        let mut active = self.active_requests.write().await;
        if let Some(info) = active.remove(request_id) {
            let duration = info.started_at.elapsed();
            debug!(
                "Request {} completed in {:?} (success: {})",
                request_id, duration, success
            );
        }
    }

    /// Get current statistics
    pub async fn stats(&self) -> ChunkServerStats {
        self.stats.read().await.clone()
    }

    /// Reset statistics
    pub async fn reset_stats(&self) {
        *self.stats.write().await = ChunkServerStats::default();
    }
}

/// Chunk stream handler for accepting incoming chunk requests
pub struct ChunkStreamHandler {
    provider: Arc<ChunkProvider>,
    connection: Arc<Connection>,
}

impl ChunkStreamHandler {
    /// Create a new chunk stream handler
    pub fn new(provider: Arc<ChunkProvider>, connection: Arc<Connection>) -> Self {
        Self {
            provider,
            connection,
        }
    }

    /// Start handling incoming chunk requests
    pub async fn start(&self) -> Result<()> {
        info!("Starting chunk stream handler");

        loop {
            // Accept incoming bidirectional stream
            match self.connection.accept_bi().await {
                Ok((recv, send)) => {
                    let provider = self.provider.clone();
                    let peer_id = self.connection.remote_address().to_string();

                    // Handle request in background
                    tokio::spawn(async move {
                        if let Err(e) = provider.handle_chunk_request(recv, send, peer_id).await {
                            error!("Failed to handle chunk request: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept stream: {}", e);
                    // Check if connection is still alive
                    if self.connection.close_reason().is_some() {
                        info!("Connection closed, stopping chunk handler");
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_chunk_provider_creation() {
        let dir = tempdir().unwrap();
        let store = Arc::new(ContentStore::new(dir.path()).await.unwrap());
        let provider = ChunkProvider::new(store);

        let stats = provider.stats().await;
        assert_eq!(stats.chunks_served, 0);
        assert_eq!(stats.bytes_served, 0);
    }

    #[tokio::test]
    async fn test_chunk_stats() {
        let dir = tempdir().unwrap();
        let store = Arc::new(ContentStore::new(dir.path()).await.unwrap());
        let provider = ChunkProvider::new(store);

        // Check initial stats
        let stats = provider.stats().await;
        assert_eq!(stats.chunks_served, 0);

        // Reset stats
        provider.reset_stats().await;
        let stats = provider.stats().await;
        assert_eq!(stats.chunks_served, 0);
    }
}