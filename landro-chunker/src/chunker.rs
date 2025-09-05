use blake3::Hasher;
use bytes::Bytes;
use tokio::io::AsyncRead;
use tracing::trace;

use crate::errors::{ChunkerError, Result};
use crate::hash::ChunkHash;

/// FastCDC chunking configuration
#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    /// Minimum chunk size in bytes
    pub min_size: usize,
    /// Average chunk size in bytes
    pub avg_size: usize,
    /// Maximum chunk size in bytes
    pub max_size: usize,
    /// Bits for the rolling hash mask
    pub mask_bits: u32,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            min_size: 64 * 1024,      // 64 KB
            avg_size: 256 * 1024,     // 256 KB
            max_size: 1024 * 1024,    // 1 MB
            mask_bits: 18,            // For 256KB average
        }
    }
}

impl ChunkerConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        if self.min_size >= self.avg_size {
            return Err(ChunkerError::InvalidConfig(
                "min_size must be less than avg_size".to_string()
            ));
        }
        if self.avg_size >= self.max_size {
            return Err(ChunkerError::InvalidConfig(
                "avg_size must be less than max_size".to_string()
            ));
        }
        if self.mask_bits == 0 || self.mask_bits > 32 {
            return Err(ChunkerError::InvalidConfig(
                "mask_bits must be between 1 and 32".to_string()
            ));
        }
        Ok(())
    }
}

/// A content-defined chunk
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Chunk data
    pub data: Bytes,
    /// Blake3 hash of the chunk
    pub hash: ChunkHash,
    /// Offset in the original stream
    pub offset: u64,
}

/// FastCDC chunker for content-defined chunking
pub struct Chunker {
    config: ChunkerConfig,
    mask: u64,
}

impl Chunker {
    /// Create a new chunker with configuration
    pub fn new(config: ChunkerConfig) -> Result<Self> {
        config.validate()?;
        
        // Create mask for average chunk size
        let mask = (1u64 << config.mask_bits) - 1;
        
        Ok(Self {
            config,
            mask,
        })
    }
    
    /// Create a chunker with default configuration
    pub fn default() -> Self {
        Self::new(ChunkerConfig::default()).unwrap()
    }
    
    /// Chunk data from an async reader
    pub async fn chunk_stream<R>(&self, mut reader: R) -> Result<Vec<Chunk>>
    where
        R: AsyncRead + Unpin,
    {
        // TODO: Implement actual FastCDC chunking algorithm
        // For now, this is a stub that returns empty vec
        trace!("Chunking stream with config: {:?}", self.config);
        Ok(Vec::new())
    }
    
    /// Chunk data from bytes
    pub fn chunk_bytes(&self, data: &[u8]) -> Result<Vec<Chunk>> {
        let mut chunks = Vec::new();
        let mut offset = 0u64;
        
        // Stub implementation - just split into fixed chunks
        // TODO: Implement actual FastCDC algorithm with rolling hash
        let chunk_size = self.config.avg_size;
        
        for chunk_data in data.chunks(chunk_size) {
            let mut hasher = Hasher::new();
            hasher.update(chunk_data);
            let hash = ChunkHash::from_blake3(hasher.finalize());
            
            chunks.push(Chunk {
                data: Bytes::copy_from_slice(chunk_data),
                hash,
                offset,
            });
            
            offset += chunk_data.len() as u64;
        }
        
        trace!("Chunked {} bytes into {} chunks", data.len(), chunks.len());
        Ok(chunks)
    }
    
    /// Calculate rolling hash (gear-based)
    fn rolling_hash(&self, window: &[u8]) -> u64 {
        // TODO: Implement gear-based rolling hash
        // For now, simple placeholder
        let mut hash = 0u64;
        for &byte in window {
            hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_validation() {
        let valid = ChunkerConfig::default();
        assert!(valid.validate().is_ok());
        
        let invalid = ChunkerConfig {
            min_size: 1000,
            avg_size: 500,
            max_size: 2000,
            mask_bits: 18,
        };
        assert!(invalid.validate().is_err());
    }
    
    #[test]
    fn test_chunk_bytes() {
        let chunker = Chunker::default();
        let data = vec![0u8; 1024 * 1024]; // 1MB
        
        let chunks = chunker.chunk_bytes(&data).unwrap();
        assert!(!chunks.is_empty());
    }
}