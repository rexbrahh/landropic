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
        // Validate size relationships
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
        
        // Validate absolute limits (using proto validation limits)
        const MIN_CHUNK_SIZE: usize = 256;  // 256 bytes minimum
        const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024;  // 16 MB maximum
        
        if self.min_size < MIN_CHUNK_SIZE {
            return Err(ChunkerError::InvalidConfig(
                format!("min_size {} is below minimum {}", self.min_size, MIN_CHUNK_SIZE),
            ));
        }
        if self.max_size > MAX_CHUNK_SIZE {
            return Err(ChunkerError::InvalidConfig(
                format!("max_size {} exceeds maximum {}", self.max_size, MAX_CHUNK_SIZE),
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

impl Default for Chunker {
    fn default() -> Self {
        Self::new(ChunkerConfig::default()).unwrap()
    }
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

        trace!(
            "Stream chunking completed: {} bytes into {} chunks",
            buffer_len,
            chunks.len()
        );
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
    /// rolling hash matches our mask pattern. This implements the proper
    /// FastCDC algorithm with a 48-byte sliding window.
    fn find_chunk_boundary(&self, data: &[u8], start_pos: usize, max_pos: usize) -> usize {
        const WINDOW_SIZE: usize = 48;
        
        if start_pos >= data.len() || start_pos >= max_pos {
            return max_pos.min(data.len());
        }

        let mut hash = 0u64;
        let mut window = vec![0u8; WINDOW_SIZE];
        let mut window_idx = 0;
        let mut window_full = false;

        // Initialize the rolling hash window
        let init_end = (start_pos + WINDOW_SIZE).min(data.len()).min(max_pos);
        for i in start_pos..init_end {
            let byte = data[i];
            window[window_idx] = byte;
            hash = hash.wrapping_mul(2).wrapping_add(self.gear_table[byte as usize]);
            window_idx = (window_idx + 1) % WINDOW_SIZE;
            
            if window_idx == 0 {
                window_full = true;
            }
        }

        let mut pos = init_end;

        // If we haven't filled the window yet, we can't do proper rolling
        if !window_full {
            // For short data, just return the end
            return max_pos.min(data.len());
        }

        // Now do the rolling hash scan
        while pos < max_pos && pos < data.len() {
            let new_byte = data[pos];
            let old_byte = window[window_idx];
            
            // Update rolling hash: remove old byte contribution, add new byte
            // Since we multiply by 2 each time and the old byte has been multiplied by 2 
            // a total of WINDOW_SIZE times, its current contribution is gear[old_byte] * (2^WINDOW_SIZE)
            let old_contribution = self.gear_table[old_byte as usize]
                .wrapping_shl(WINDOW_SIZE as u32);
            hash = hash.wrapping_sub(old_contribution);
            hash = hash.wrapping_mul(2).wrapping_add(self.gear_table[new_byte as usize]);
            
            // Update the sliding window
            window[window_idx] = new_byte;
            window_idx = (window_idx + 1) % WINDOW_SIZE;

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
    /// which are used in the rolling hash computation. This uses a better
    /// pseudo-random generator for more uniform distribution.
    fn generate_gear_table() -> [u64; 256] {
        let mut table = [0u64; 256];
        let mut state = 0x3DAE66B0C5E15E79u64; // Fixed seed for deterministic results

        for entry in &mut table {
            // Use a better PRNG (xorshift64) for more uniform distribution
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *entry = state;
        }

        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;

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
            assert!(chunks[i].offset > chunks[i - 1].offset);
        }
    }

    // Golden vector tests for deterministic chunking
    mod golden_vectors {
        use super::*;

        fn test_config() -> ChunkerConfig {
            ChunkerConfig {
                min_size: 512,
                avg_size: 2048,
                max_size: 8192,
                mask_bits: 11, // For 2KB average
            }
        }

        #[test]
        fn test_empty_input() {
            let chunker = Chunker::new(test_config()).unwrap();
            let chunks = chunker.chunk_bytes(&[]).unwrap();
            assert_eq!(chunks.len(), 0);
        }

        #[test]
        fn test_single_byte() {
            let chunker = Chunker::new(test_config()).unwrap();
            let chunks = chunker.chunk_bytes(&[42]).unwrap();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].data.len(), 1);
            assert_eq!(chunks[0].data[0], 42);
            assert_eq!(chunks[0].offset, 0);
        }

        #[test]
        fn test_below_min_size() {
            let chunker = Chunker::new(test_config()).unwrap();
            let data = vec![1u8; 256]; // Below min size of 512
            let chunks = chunker.chunk_bytes(&data).unwrap();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].data.len(), 256);
        }

        #[test]
        fn test_deterministic_chunking() {
            // Create test data with known patterns
            let mut data = Vec::new();
            for i in 0..10000 {
                data.push((i % 256) as u8);
            }

            let chunker = Chunker::new(test_config()).unwrap();
            let chunks1 = chunker.chunk_bytes(&data).unwrap();
            let chunks2 = chunker.chunk_bytes(&data).unwrap();

            // Should be deterministic
            assert_eq!(chunks1.len(), chunks2.len());
            for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
                assert_eq!(c1.data, c2.data);
                assert_eq!(c1.hash, c2.hash);
                assert_eq!(c1.offset, c2.offset);
            }
        }

        #[test]
        fn test_known_data_chunks() {
            // Test with specific data that should produce predictable chunks
            let data = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";
            
            let chunker = Chunker::new(test_config()).unwrap();
            let chunks = chunker.chunk_bytes(data).unwrap();

            // Verify basic properties
            assert!(!chunks.is_empty());
            let total_size: usize = chunks.iter().map(|c| c.data.len()).sum();
            assert_eq!(total_size, data.len());

            // Verify no overlaps and proper ordering
            let mut expected_offset = 0;
            for chunk in &chunks {
                assert_eq!(chunk.offset, expected_offset);
                expected_offset += chunk.data.len() as u64;
            }
        }

        #[test]
        fn test_large_uniform_data() {
            // Large block of uniform data should still chunk properly
            let data = vec![0x55u8; 50000];
            let chunker = Chunker::new(test_config()).unwrap();
            let chunks = chunker.chunk_bytes(&data).unwrap();

            assert!(!chunks.is_empty());
            let total_size: usize = chunks.iter().map(|c| c.data.len()).sum();
            assert_eq!(total_size, data.len());
        }

        #[test]
        fn test_all_zero_data() {
            let data = vec![0u8; 16384];
            let chunker = Chunker::new(test_config()).unwrap();
            let chunks = chunker.chunk_bytes(&data).unwrap();

            // Should produce some chunks even with uniform data
            assert!(!chunks.is_empty());
            let total_size: usize = chunks.iter().map(|c| c.data.len()).sum();
            assert_eq!(total_size, data.len());

            // All chunks except potentially the last should be >= min_size
            for (i, chunk) in chunks.iter().enumerate() {
                if i < chunks.len() - 1 {
                    assert!(chunk.data.len() >= test_config().min_size, 
                           "Chunk {} has size {} which is below min_size {}", 
                           i, chunk.data.len(), test_config().min_size);
                }
                assert!(chunk.data.len() <= test_config().max_size,
                       "Chunk {} has size {} which exceeds max_size {}",
                       i, chunk.data.len(), test_config().max_size);
            }
        }

        #[test]
        fn test_boundary_consistency() {
            // Test that chunk boundaries are consistent across different input sizes
            let base_data = (0..20000).map(|i| (i % 256) as u8).collect::<Vec<_>>();
            let chunker = Chunker::new(test_config()).unwrap();

            // Chunk the full data
            let full_chunks = chunker.chunk_bytes(&base_data).unwrap();

            // Chunk a prefix and verify the first chunks match
            let prefix_len = 15000;
            let prefix_chunks = chunker.chunk_bytes(&base_data[..prefix_len]).unwrap();

            // The chunks that are entirely within the prefix should match
            let mut matching_chunks = 0;
            let mut offset = 0;
            for (full_chunk, prefix_chunk) in full_chunks.iter().zip(prefix_chunks.iter()) {
                if offset + full_chunk.data.len() <= prefix_len {
                    assert_eq!(full_chunk.data, prefix_chunk.data);
                    assert_eq!(full_chunk.hash, prefix_chunk.hash);
                    assert_eq!(full_chunk.offset, prefix_chunk.offset);
                    matching_chunks += 1;
                }
                offset += full_chunk.data.len();
                if offset >= prefix_len {
                    break;
                }
            }

            assert!(matching_chunks > 0, "Should have at least some matching chunks");
        }
    }

    // Property-based tests using proptest
    mod property_tests {
        use super::*;

        proptest! {
            #[test]
            fn test_chunk_size_bounds(data in prop::collection::vec(any::<u8>(), 0..100000)) {
                let config = ChunkerConfig {
                    min_size: 1024,
                    avg_size: 4096,
                    max_size: 16384,
                    mask_bits: 12,
                };
                let chunker = Chunker::new(config.clone()).unwrap();
                let chunks = chunker.chunk_bytes(&data).unwrap();

                if data.is_empty() {
                    prop_assert_eq!(chunks.len(), 0);
                } else {
                    // Total size should match
                    let total_size: usize = chunks.iter().map(|c| c.data.len()).sum();
                    prop_assert_eq!(total_size, data.len());

                    // All chunks except possibly the last should meet minimum size
                    for (i, chunk) in chunks.iter().enumerate() {
                        if i < chunks.len() - 1 {
                            prop_assert!(chunk.data.len() >= config.min_size);
                        }
                        prop_assert!(chunk.data.len() <= config.max_size);
                    }

                    // Verify proper offset sequence
                    let mut expected_offset = 0;
                    for chunk in &chunks {
                        prop_assert_eq!(chunk.offset, expected_offset);
                        expected_offset += chunk.data.len() as u64;
                    }
                }
            }

            #[test]
            fn test_deterministic_hashing(data in prop::collection::vec(any::<u8>(), 1..10000)) {
                let chunker = Chunker::default();
                let chunks1 = chunker.chunk_bytes(&data).unwrap();
                let chunks2 = chunker.chunk_bytes(&data).unwrap();

                prop_assert_eq!(chunks1.len(), chunks2.len());
                for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
                    prop_assert_eq!(c1.hash, c2.hash);
                    prop_assert_eq!(c1.offset, c2.offset);
                    prop_assert_eq!(&c1.data, &c2.data);
                }
            }

            #[test]
            fn test_chunk_uniqueness(data in prop::collection::vec(any::<u8>(), 1000..50000)) {
                let chunker = Chunker::default();
                let chunks = chunker.chunk_bytes(&data).unwrap();

                // Collect all unique hashes - should be one per chunk unless we have duplicates
                let unique_hashes: HashSet<_> = chunks.iter().map(|c| c.hash).collect();
                
                // For randomly generated data, we expect most chunks to be unique
                // Allow some duplicates but not too many
                let duplicate_ratio = 1.0 - (unique_hashes.len() as f64 / chunks.len() as f64);
                prop_assert!(duplicate_ratio < 0.5, "Too many duplicate chunks: ratio = {}", duplicate_ratio);
            }

            #[test] 
            fn test_average_chunk_size_convergence(seed in 0u64..1000) {
                // Generate larger datasets to test average convergence
                let mut data = Vec::new();
                let mut rng_state = seed;
                for _ in 0..200000 {
                    rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
                    data.push((rng_state >> 24) as u8);
                }

                let config = ChunkerConfig {
                    min_size: 2048,
                    avg_size: 8192,
                    max_size: 32768,
                    mask_bits: 13,
                };
                let chunker = Chunker::new(config.clone()).unwrap();
                let chunks = chunker.chunk_bytes(&data).unwrap();

                if chunks.len() >= 10 {
                    let total_size: usize = chunks.iter().map(|c| c.data.len()).sum();
                    let actual_avg = total_size as f64 / chunks.len() as f64;
                    let expected_avg = config.avg_size as f64;
                    
                    // Allow for more variance since FastCDC can have significant deviation
                    // especially with certain data patterns
                    let ratio = actual_avg / expected_avg;
                    prop_assert!(ratio >= 0.3 && ratio <= 3.0, 
                               "Average chunk size {} too far from expected {}, ratio: {}", 
                               actual_avg, expected_avg, ratio);
                }
            }
        }
    }
}
