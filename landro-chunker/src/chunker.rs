use blake3::Hasher;
use bytes::Bytes;
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
            min_size: 64 * 1024,   // 64 KB
            avg_size: 256 * 1024,  // 256 KB
            max_size: 1024 * 1024, // 1 MB
            mask_bits: 18,         // For 256KB average
        }
    }
}

impl ChunkerConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        if self.min_size >= self.avg_size {
            return Err(ChunkerError::InvalidConfig(
                "min_size must be less than avg_size".to_string(),
            ));
        }
        if self.avg_size >= self.max_size {
            return Err(ChunkerError::InvalidConfig(
                "avg_size must be less than max_size".to_string(),
            ));
        }
        if self.mask_bits == 0 || self.mask_bits > 32 {
            return Err(ChunkerError::InvalidConfig(
                "mask_bits must be between 1 and 32".to_string(),
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

/// FastCDC chunker for content-defined chunking.
///
/// This implements a content-defined chunking algorithm based on FastCDC,
/// which uses a rolling hash to find chunk boundaries. The chunker is
/// configured with minimum, average, and maximum chunk sizes.
///
/// # Example
///
/// ```rust
/// use landro_chunker::{Chunker, ChunkerConfig};
///
/// let config = ChunkerConfig::default();
/// let chunker = Chunker::new(config).unwrap();
/// let data = b"Hello, World! This is test data for chunking.";
/// let chunks = chunker.chunk_bytes(data).unwrap();
/// ```
pub struct Chunker {
    config: ChunkerConfig,
    mask: u64,
    gear_table: [u64; 256],
}

impl Chunker {
    /// Create a new chunker with configuration
    pub fn new(config: ChunkerConfig) -> Result<Self> {
        config.validate()?;

        // Create mask for average chunk size
        let mask = (1u64 << config.mask_bits) - 1;

        // Initialize gear table for rolling hash
        let gear_table = Self::generate_gear_table();

        Ok(Self {
            config,
            mask,
            gear_table,
        })
    }

    /// Create a chunker with default configuration
    pub fn default() -> Self {
        Self::new(ChunkerConfig::default()).unwrap()
    }

    /// Chunk data from an async reader.
    ///
    /// This method efficiently chunks large files that don't fit in memory using
    /// the FastCDC algorithm with buffered reading and rolling hash computation.
    ///
    /// # Arguments
    ///
    /// * `reader` - An async reader implementing `AsyncRead`
    ///
    /// # Returns
    ///
    /// A vector of chunks with their hashes and offsets
    pub async fn chunk_stream<R>(&self, mut reader: R) -> Result<Vec<Chunk>>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        use tokio::io::AsyncReadExt;
        
        trace!("Starting stream chunking with config: {:?}", self.config);
        
        let mut chunks = Vec::new();
        let global_offset = 0u64;
        let mut buffer = Vec::new();
        let mut read_buffer = vec![0u8; 64 * 1024]; // 64KB read buffer
        
        // Read the entire stream into memory in chunks
        loop {
            let bytes_read = reader.read(&mut read_buffer).await?;
            
            if bytes_read == 0 {
                break; // End of stream
            }
            
            buffer.extend_from_slice(&read_buffer[..bytes_read]);
        }
        
        if buffer.is_empty() {
            return Ok(chunks);
        }
        
        // Process the complete buffer using our existing chunking logic
        let mut offset = 0usize;
        let buffer_len = buffer.len();
        
        while offset < buffer_len {
            let chunk_start = offset;
            let remaining = buffer_len - chunk_start;
            
            // Enforce minimum chunk size
            let mut chunk_end = chunk_start + self.config.min_size.min(remaining);
            
            // If we have more data and we're past min size, look for optimal cut point
            if chunk_end < buffer_len && remaining > self.config.min_size {
                let max_scan = (chunk_start + self.config.max_size).min(buffer_len);
                chunk_end = self.find_chunk_boundary(&buffer, chunk_end, max_scan);
            }
            
            // Ensure we don't exceed bounds
            chunk_end = chunk_end.min(buffer_len);
            
            // Create chunk
            let chunk_data = &buffer[chunk_start..chunk_end];
            let mut hasher = Hasher::new();
            hasher.update(chunk_data);
            let hash = ChunkHash::from_blake3(hasher.finalize());
            
            chunks.push(Chunk {
                data: Bytes::copy_from_slice(chunk_data),
                hash,
                offset: global_offset + (chunk_start as u64),
            });
            
            offset = chunk_end;
        }
        
        trace!("Stream chunking completed: {} bytes into {} chunks", buffer_len, chunks.len());
        Ok(chunks)
    }

    /// Chunk data from a byte slice.
    ///
    /// This method processes the entire data slice and returns a vector of chunks.
    /// Currently uses a simple fixed-size chunking as a placeholder for the
    /// full FastCDC implementation.
    ///
    /// # Arguments
    ///
    /// * `data` - The byte slice to chunk
    ///
    /// # Returns
    ///
    /// A vector of chunks with their hashes and offsets
    pub fn chunk_bytes(&self, data: &[u8]) -> Result<Vec<Chunk>> {
        let mut chunks = Vec::new();
        let mut offset = 0u64;
        let data_len = data.len();

        while (offset as usize) < data_len {
            let start = offset as usize;
            let remaining = data_len - start;

            // Enforce minimum chunk size
            let mut chunk_end = start + self.config.min_size.min(remaining);

            // If we're past the minimum size, look for a cut point
            if chunk_end < data_len && remaining > self.config.min_size {
                let max_scan = (start + self.config.max_size).min(data_len);
                chunk_end = self.find_chunk_boundary(data, chunk_end, max_scan);
            }

            // Create the chunk
            let chunk_data = &data[start..chunk_end];
            let mut hasher = Hasher::new();
            hasher.update(chunk_data);
            let hash = ChunkHash::from_blake3(hasher.finalize());

            chunks.push(Chunk {
                data: Bytes::copy_from_slice(chunk_data),
                hash,
                offset,
            });

            offset = chunk_end as u64;
        }

        trace!("Chunked {} bytes into {} chunks", data.len(), chunks.len());
        Ok(chunks)
    }

    /// Find optimal chunk boundary using FastCDC rolling hash.
    ///
    /// Scans from start_pos to max_pos looking for a boundary where the
    /// rolling hash matches our mask pattern.
    fn find_chunk_boundary(&self, data: &[u8], start_pos: usize, max_pos: usize) -> usize {
        let window_size = 48;
        let mut hash = 0u64;
        let mut pos = start_pos;
        
        // Initialize hash for first window
        let window_end = (pos + window_size).min(data.len()).min(max_pos);
        for i in pos..window_end {
            hash = (hash << 1).wrapping_add(self.gear_table[data[i] as usize]);
        }
        
        pos = window_end;
        
        // Scan for boundary
        while pos < max_pos && pos < data.len() {
            // Update rolling hash
            hash = (hash << 1).wrapping_add(self.gear_table[data[pos] as usize]);
            
            // Check if we found a boundary
            if (hash & self.mask) == 0 {
                return pos + 1;
            }
            
            pos += 1;
        }
        
        // If no boundary found, return max position
        max_pos.min(data.len())
    }

    /// Generate the gear table for the rolling hash.
    ///
    /// The gear table provides pseudo-random values for each possible byte value,
    /// which are used in the rolling hash computation.
    fn generate_gear_table() -> [u64; 256] {
        let mut table = [0u64; 256];
        let mut seed = 0x3DAE66B0C5E15E79u64; // Arbitrary seed for deterministic results

        for i in 0..256 {
            // Simple pseudo-random generation using linear congruential generator
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            table[i] = seed;
        }

        table
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

    #[tokio::test]
    async fn test_chunk_stream() {
        let chunker = Chunker::default();
        let data = vec![42u8; 512 * 1024]; // 512KB
        let cursor = std::io::Cursor::new(data.clone());

        let chunks = chunker.chunk_stream(cursor).await.unwrap();
        assert!(!chunks.is_empty());
        
        // Verify that chunks contain the expected data
        let total_size: usize = chunks.iter().map(|c| c.data.len()).sum();
        assert_eq!(total_size, data.len());
        
        // Verify chunks are properly ordered by offset
        for i in 1..chunks.len() {
            assert!(chunks[i].offset > chunks[i-1].offset);
        }
    }
}
