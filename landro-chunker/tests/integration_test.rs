#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::explicit_iter_loop)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::redundant_clone)]
#![allow(clippy::cast_lossless)]

use landro_chunker::{Chunker, ChunkerConfig};
use std::collections::HashMap;

#[test]
fn test_deduplication_effectiveness() {
    // Test that identical data produces identical chunks (deduplication)
    // Use smaller chunks for this test to see better deduplication
    let config = ChunkerConfig {
        min_size: 16 * 1024,  // 16KB
        avg_size: 64 * 1024,  // 64KB
        max_size: 256 * 1024, // 256KB
        mask_bits: 16,
    };
    let chunker = Chunker::new(config).unwrap();

    // Create data with repeating patterns (make it larger for better testing)
    let mut data = Vec::new();
    let pattern = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ";
    for _ in 0..10000 {
        data.extend_from_slice(pattern);
    }

    // Add the same pattern again
    let mut data_with_duplicate = data.clone();
    data_with_duplicate.extend_from_slice(&data);

    let chunks1 = chunker.chunk_bytes(&data).unwrap();
    let chunks2 = chunker.chunk_bytes(&data_with_duplicate).unwrap();

    // Count unique chunks by hash
    let mut unique_chunks = HashMap::new();
    for chunk in chunks1.iter() {
        *unique_chunks.entry(chunk.hash).or_insert(0) += 1;
    }
    for chunk in chunks2.iter() {
        *unique_chunks.entry(chunk.hash).or_insert(0) += 1;
    }

    // The duplicate data should reuse most chunks
    let total_chunks = chunks1.len() + chunks2.len();
    let unique_count = unique_chunks.len();
    let dedup_ratio = 1.0 - (unique_count as f64 / total_chunks as f64);

    println!(
        "Deduplication test: {} total chunks, {} unique, {:.1}% dedup ratio",
        total_chunks,
        unique_count,
        dedup_ratio * 100.0
    );

    // We should have significant deduplication since we duplicated the data
    assert!(
        dedup_ratio > 0.3,
        "Deduplication ratio {:.1}% is too low",
        dedup_ratio * 100.0
    );
}

#[test]
fn test_incremental_changes() {
    // Test that small changes produce mostly the same chunks
    let chunker = Chunker::default();

    // Create large file (10MB to get multiple chunks)
    let mut data = Vec::new();
    for i in 0..(10 * 1024 * 1024) {
        data.push((i % 256) as u8);
    }

    let chunks_original = chunker.chunk_bytes(&data).unwrap();

    // Make a small change in the middle
    let change_pos = 5 * 1024 * 1024; // Change at 5MB mark
    data[change_pos] = 255;
    data[change_pos + 1] = 255;
    data[change_pos + 2] = 255;

    let chunks_modified = chunker.chunk_bytes(&data).unwrap();

    // Count how many chunks are identical
    let mut identical_chunks = 0;
    let original_hashes: HashMap<_, _> = chunks_original.iter().map(|c| (c.hash, c)).collect();

    for chunk in chunks_modified.iter() {
        if original_hashes.contains_key(&chunk.hash) {
            identical_chunks += 1;
        }
    }

    // With content-defined chunking, we expect most chunks to remain unchanged
    // Only the chunk containing the change (and possibly one adjacent) should differ
    let expected_unchanged = chunks_original.len().saturating_sub(2);

    println!(
        "Incremental change test: {}/{} chunks unchanged (expected at least {})",
        identical_chunks,
        chunks_original.len(),
        expected_unchanged
    );

    // At least all but 2 chunks should be unchanged
    assert!(
        identical_chunks >= expected_unchanged,
        "Only {} chunks unchanged, expected at least {}",
        identical_chunks,
        expected_unchanged
    );
}

#[test]
fn test_chunk_alignment() {
    // Test that chunks align properly when data is concatenated
    let chunker = Chunker::default();

    // Create two separate files
    let data1: Vec<u8> = (0..(512 * 1024)).map(|i| (i % 256) as u8).collect();
    let data2: Vec<u8> = (0..(512 * 1024)).map(|i| ((i + 128) % 256) as u8).collect();

    // Chunk them separately
    let chunks1 = chunker.chunk_bytes(&data1).unwrap();
    let chunks2 = chunker.chunk_bytes(&data2).unwrap();

    // Concatenate and chunk together
    let mut combined = data1.clone();
    combined.extend_from_slice(&data2);
    let chunks_combined = chunker.chunk_bytes(&combined).unwrap();

    // Verify total size is preserved
    let size1: usize = chunks1.iter().map(|c| c.data.len()).sum();
    let size2: usize = chunks2.iter().map(|c| c.data.len()).sum();
    let size_combined: usize = chunks_combined.iter().map(|c| c.data.len()).sum();

    assert_eq!(size1 + size2, size_combined);
    assert_eq!(size_combined, combined.len());

    println!(
        "Chunk alignment test: {} + {} chunks = {} chunks combined",
        chunks1.len(),
        chunks2.len(),
        chunks_combined.len()
    );
}

#[tokio::test]
async fn test_async_streaming() {
    // Test that async streaming produces same results as sync
    let chunker = Chunker::default();

    // Create test data
    let data: Vec<u8> = (0..(3 * 1024 * 1024))
        .map(|i| ((i * 17 + 31) % 256) as u8)
        .collect();

    // Chunk synchronously
    let chunks_sync = chunker.chunk_bytes(&data).unwrap();

    // Chunk asynchronously
    let cursor = std::io::Cursor::new(data.clone());
    let stream_chunks = chunker.chunk_stream(cursor).await.unwrap();

    // Results should be identical
    assert_eq!(chunks_sync.len(), stream_chunks.len());
    for (sync_chunk, async_chunk) in chunks_sync.iter().zip(stream_chunks.iter()) {
        assert_eq!(sync_chunk.hash, async_chunk.hash);
        assert_eq!(sync_chunk.offset, async_chunk.offset);
        assert_eq!(sync_chunk.data, async_chunk.data);
    }

    println!(
        "Async streaming test: {} chunks match between sync and async",
        chunks_sync.len()
    );
}

#[test]
fn test_production_config_boundaries() {
    // Test that production config respects min/max boundaries
    let config = ChunkerConfig {
        min_size: 256 * 1024,      // 256KB
        avg_size: 1024 * 1024,     // 1MB
        max_size: 4 * 1024 * 1024, // 4MB
        mask_bits: 20,
    };
    let chunker = Chunker::new(config.clone()).unwrap();

    // Create 20MB of varied data
    let mut data = Vec::new();
    for i in 0..(20 * 1024 * 1024) {
        let byte = match i % 4096 {
            0..=1023 => (i % 256) as u8,
            1024..=2047 => ((i / 256) % 256) as u8,
            2048..=3071 => ((i / 512) % 256) as u8,
            _ => ((i / 1024) % 256) as u8,
        };
        data.push(byte);
    }

    let chunks = chunker.chunk_bytes(&data).unwrap();

    // Verify all chunks respect boundaries
    let mut min_size = usize::MAX;
    let mut max_size = 0;
    let mut total_size = 0;

    for (i, chunk) in chunks.iter().enumerate() {
        let size = chunk.data.len();
        min_size = min_size.min(size);
        max_size = max_size.max(size);
        total_size += size;

        // All chunks except possibly the last must be >= min_size
        if i < chunks.len() - 1 {
            assert!(
                size >= config.min_size,
                "Chunk {} size {} is below min_size {}",
                i,
                size,
                config.min_size
            );
        }

        // All chunks must be <= max_size
        assert!(
            size <= config.max_size,
            "Chunk {} size {} exceeds max_size {}",
            i,
            size,
            config.max_size
        );
    }

    let avg_size = total_size as f64 / chunks.len() as f64;
    println!("Production config boundaries test:");
    println!("  Chunks: {}", chunks.len());
    println!("  Min size: {} KB", min_size / 1024);
    println!("  Avg size: {:.0} KB", avg_size / 1024.0);
    println!("  Max size: {} KB", max_size / 1024);

    // Average should be within reasonable range of target
    assert!(avg_size > (config.avg_size as f64 * 0.3));
    assert!(avg_size < (config.avg_size as f64 * 3.0));
}
