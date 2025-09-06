#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::semicolon_if_nothing_returned)]
#![allow(clippy::uninlined_format_args)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use landro_chunker::{Chunker, ChunkerConfig};
use std::io::Cursor;
use tokio::runtime::Runtime;

fn bench_chunk_bytes(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunk_bytes");

    // Test different data sizes
    let sizes = [
        64 * 1024,        // 64 KB
        256 * 1024,       // 256 KB
        1024 * 1024,      // 1 MB
        4 * 1024 * 1024,  // 4 MB
        16 * 1024 * 1024, // 16 MB
    ];

    let chunker = Chunker::default();

    for &size in &sizes {
        group.throughput(Throughput::Bytes(size as u64));

        // Create test data with some variation to ensure realistic chunking
        let mut data = vec![0u8; size];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = ((i * 7) % 256) as u8; // Create pseudo-random pattern
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(format_size(size)),
            &data,
            |b, data| {
                b.iter(|| {
                    let chunks = chunker.chunk_bytes(black_box(data)).unwrap();
                    black_box(chunks);
                });
            },
        );
    }

    group.finish();
}

fn bench_chunk_stream(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("chunk_stream");

    let sizes = [
        256 * 1024,      // 256 KB
        1024 * 1024,     // 1 MB
        4 * 1024 * 1024, // 4 MB
    ];

    let chunker = Chunker::default();

    for &size in &sizes {
        group.throughput(Throughput::Bytes(size as u64));

        // Create test data
        let mut data = vec![0u8; size];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = ((i * 13) % 256) as u8;
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(format_size(size)),
            &data,
            |b, data| {
                b.iter(|| {
                    rt.block_on(async {
                        let cursor = Cursor::new(data.clone());
                        let chunks = chunker.chunk_stream(black_box(cursor)).await.unwrap();
                        black_box(chunks);
                    })
                });
            },
        );
    }

    group.finish();
}

fn bench_different_configurations(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunk_configurations");

    let test_data = {
        let size = 1024 * 1024; // 1MB
        let mut data = vec![0u8; size];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = ((i * 11) % 256) as u8;
        }
        data
    };

    group.throughput(Throughput::Bytes(test_data.len() as u64));

    // Test different chunk size configurations
    let configs = vec![
        (
            "small_chunks",
            ChunkerConfig {
                min_size: 16 * 1024,  // 16KB
                avg_size: 64 * 1024,  // 64KB
                max_size: 256 * 1024, // 256KB
                mask_bits: 16,
            },
        ),
        ("default", ChunkerConfig::default()),
        (
            "large_chunks",
            ChunkerConfig {
                min_size: 128 * 1024,  // 128KB
                avg_size: 512 * 1024,  // 512KB
                max_size: 2048 * 1024, // 2MB
                mask_bits: 19,
            },
        ),
    ];

    for (name, config) in configs {
        let chunker = Chunker::new(config).unwrap();
        group.bench_with_input(BenchmarkId::from_parameter(name), &test_data, |b, data| {
            b.iter(|| {
                let chunks = chunker.chunk_bytes(black_box(data)).unwrap();
                black_box(chunks);
            });
        });
    }

    group.finish();
}

fn bench_chunk_consistency(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunk_consistency");

    let chunker = Chunker::default();

    // Create identical data to test deduplication effectiveness
    let identical_data = vec![42u8; 512 * 1024]; // 512KB of identical bytes

    // Create data with repeating patterns
    let mut pattern_data = Vec::with_capacity(512 * 1024);
    let pattern = b"Hello, World! This is a test pattern that repeats.";
    while pattern_data.len() < 512 * 1024 {
        pattern_data.extend_from_slice(pattern);
    }
    pattern_data.truncate(512 * 1024);

    group.throughput(Throughput::Bytes(512 * 1024));

    group.bench_function("identical_bytes", |b| {
        b.iter(|| {
            let chunks = chunker.chunk_bytes(black_box(&identical_data)).unwrap();
            black_box(chunks);
        });
    });

    group.bench_function("repeating_pattern", |b| {
        b.iter(|| {
            let chunks = chunker.chunk_bytes(black_box(&pattern_data)).unwrap();
            black_box(chunks);
        });
    });

    group.finish();
}

fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{}MB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{}KB", bytes / 1024)
    } else {
        format!("{}B", bytes)
    }
}

criterion_group!(
    benches,
    bench_chunk_bytes,
    bench_chunk_stream,
    bench_different_configurations,
    bench_chunk_consistency
);
criterion_main!(benches);
