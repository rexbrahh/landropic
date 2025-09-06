use landro_chunker::{Chunker, ChunkerConfig};
use std::collections::HashMap;

#[allow(clippy::cast_precision_loss)]
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::uninlined_format_args)]
#[allow(clippy::cast_lossless)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("FastCDC Implementation Demo");
    println!("===========================\n");

    // Test with different configurations
    let configs = vec![
        (
            "Small chunks",
            ChunkerConfig {
                min_size: 1024,  // 1KB
                avg_size: 4096,  // 4KB
                max_size: 16384, // 16KB
                mask_bits: 12,
            },
        ),
        (
            "Medium chunks",
            ChunkerConfig {
                min_size: 8192,   // 8KB
                avg_size: 32768,  // 32KB
                max_size: 131_072, // 128KB
                mask_bits: 15,
            },
        ),
        (
            "Large chunks",
            ChunkerConfig {
                min_size: 65536,   // 64KB
                avg_size: 262_144,  // 256KB
                max_size: 1_048_576, // 1MB
                mask_bits: 18,
            },
        ),
    ];

    // Generate test data with different patterns
    let test_data = generate_varied_test_data(1024 * 1024); // 1MB

    for (name, config) in &configs {
        println!("Configuration: {}", name);
        println!(
            "  Min: {}KB, Avg: {}KB, Max: {}KB",
            config.min_size / 1024,
            config.avg_size / 1024,
            config.max_size / 1024
        );

        let chunker = Chunker::new(config.clone())?;
        let chunks = chunker.chunk_bytes(&test_data)?;

        // Calculate statistics
        let total_size: usize = chunks.iter().map(|c| c.data.len()).sum();
        let avg_chunk_size = total_size as f64 / chunks.len() as f64;
        let min_chunk_size = chunks.iter().map(|c| c.data.len()).min().unwrap_or(0);
        let max_chunk_size = chunks.iter().map(|c| c.data.len()).max().unwrap_or(0);

        // Distribution analysis
        let mut size_buckets = HashMap::new();
        for chunk in &chunks {
            let bucket = (chunk.data.len() / 4096) * 4096; // Round to 4KB buckets
            *size_buckets.entry(bucket).or_insert(0) += 1;
        }

        println!("  Results:");
        println!("    Total chunks: {}", chunks.len());
        println!(
            "    Average size: {:.1}KB (target: {}KB)",
            avg_chunk_size / 1024.0,
            config.avg_size / 1024
        );
        println!(
            "    Size range: {}KB - {}KB",
            min_chunk_size / 1024,
            max_chunk_size / 1024
        );

        // Show size distribution
        println!("    Size distribution:");
        let mut sorted_buckets: Vec<_> = size_buckets.iter().collect();
        sorted_buckets.sort_by_key(|(size, _)| *size);
        for (size, count) in sorted_buckets.iter().take(5) {
            println!("      {}KB: {} chunks", *size / 1024, count);
        }

        // Verify deterministic behavior
        let chunks2 = chunker.chunk_bytes(&test_data)?;
        assert_eq!(chunks.len(), chunks2.len(), "Non-deterministic chunking!");
        for (c1, c2) in chunks.iter().zip(chunks2.iter()) {
            assert_eq!(c1.hash, c2.hash, "Hash mismatch!");
            assert_eq!(c1.data.len(), c2.data.len(), "Size mismatch!");
        }
        println!("    ✓ Deterministic chunking verified");

        println!();
    }

    // Test deduplication effectiveness
    println!("Deduplication Test");
    println!("==================");

    let repeated_data = generate_repeated_pattern_data(512 * 1024); // 512KB with repeated patterns
    let chunker = Chunker::new(ChunkerConfig::default())?;
    let chunks = chunker.chunk_bytes(&repeated_data)?;

    let mut unique_hashes = std::collections::HashSet::new();
    let mut duplicate_count = 0;

    for chunk in &chunks {
        if !unique_hashes.insert(chunk.hash) {
            duplicate_count += 1;
        }
    }

    println!("Total chunks: {}", chunks.len());
    println!("Unique chunks: {}", unique_hashes.len());
    println!(
        "Duplicate chunks: {} ({:.1}%)",
        duplicate_count,
        (duplicate_count as f64 / chunks.len() as f64) * 100.0
    );

    Ok(())
}

fn generate_varied_test_data(size: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(size);
    let mut seed = 0x3DAE_66B0_C5E1_5E79_u64;

    // Create sections with different patterns
    let section_size = size / 4;

    // Section 1: Sequential pattern
    for i in 0..section_size {
        data.push((i % 256) as u8);
    }

    // Section 2: Repeated pattern
    let pattern = b"The quick brown fox jumps over the lazy dog. ";
    for i in 0..section_size {
        data.push(pattern[i % pattern.len()]);
    }

    // Section 3: Pseudo-random
    for _ in 0..section_size {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        data.push((seed >> 24) as u8);
    }

    // Section 4: Mixed patterns
    for i in 0..section_size {
        if i % 100 < 50 {
            data.push((i % 256) as u8);
        } else {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            data.push((seed >> 24) as u8);
        }
    }

    data
}

fn generate_repeated_pattern_data(size: usize) -> Vec<u8> {
    let patterns = [
        b"This is a repeated pattern that should create duplicate chunks. ".as_slice(),
        b"Another pattern with different content but similar length. ".as_slice(),
        b"The quick brown fox jumps over the lazy dog repeatedly. ".as_slice(),
    ];

    let mut data = Vec::with_capacity(size);
    let mut pattern_idx = 0;

    while data.len() < size {
        let pattern = patterns[pattern_idx % patterns.len()];

        // Repeat each pattern multiple times to increase chance of duplicates
        for _ in 0..10 {
            if data.len() + pattern.len() <= size {
                data.extend_from_slice(pattern);
            } else {
                data.extend_from_slice(&pattern[..size - data.len()]);
                break;
            }
        }

        pattern_idx += 1;
    }

    data
}
