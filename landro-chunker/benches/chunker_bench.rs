use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use landro_chunker::{Chunker, ChunkerConfig};
use std::hint::black_box as std_black_box;

fn generate_test_data(size: usize, pattern: u8) -> Vec<u8> {
    match pattern {
        0 => vec![0u8; size], // All zeros
        1 => (0..size).map(|i| (i % 256) as u8).collect(), // Sequential pattern
        2 => (0..size).map(|i| ((i * 123456789) % 256) as u8).collect(), // Pseudo-random pattern
        _ => {
            // Real random-ish pattern using simple PRNG
            let mut data = Vec::with_capacity(size);
            let mut seed = 0x3DAE66B0C5E15E79u64;
            for _ in 0..size {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                data.push((seed >> 24) as u8);
            }
            data
        }
    }
}

fn bench_chunk_sizes(c: &mut Criterion) {
    let test_data = generate_test_data(10 * 1024 * 1024, 3); // 10MB of varied data
    
    let configs = [
        ("small", ChunkerConfig {
            min_size: 4 * 1024,    // 4KB
            avg_size: 16 * 1024,   // 16KB
            max_size: 64 * 1024,   // 64KB
            mask_bits: 14,         // For 16KB average
        }),
        ("medium", ChunkerConfig {
            min_size: 64 * 1024,   // 64KB
            avg_size: 256 * 1024,  // 256KB
            max_size: 1024 * 1024, // 1MB
            mask_bits: 18,         // For 256KB average
        }),
        ("large", ChunkerConfig {
            min_size: 256 * 1024,  // 256KB
            avg_size: 1024 * 1024, // 1MB
            max_size: 4 * 1024 * 1024, // 4MB
            mask_bits: 20,         // For 1MB average
        }),
    ];
    
    let mut group = c.benchmark_group("chunk_sizes");
    group.throughput(Throughput::Bytes(test_data.len() as u64));
    group.sample_size(10); // Quick benchmarks
    
    for (name, config) in configs.iter() {
        let chunker = Chunker::new(config.clone()).unwrap();
        
        group.bench_with_input(BenchmarkId::new("chunk_bytes", name), &test_data, |b, data| {
            b.iter(|| {
                let chunks = chunker.chunk_bytes(std_black_box(data)).unwrap();
                std_black_box(chunks);
            });
        });
    }
    
    group.finish();
}

fn bench_data_patterns(c: &mut Criterion) {
    let size = 5 * 1024 * 1024; // 5MB
    let config = ChunkerConfig::default();
    let chunker = Chunker::new(config).unwrap();
    
    let patterns = [
        ("zeros", generate_test_data(size, 0)),
        ("sequential", generate_test_data(size, 1)),
        ("pseudo_random", generate_test_data(size, 2)),
        ("varied", generate_test_data(size, 3)),
    ];
    
    let mut group = c.benchmark_group("data_patterns");
    group.throughput(Throughput::Bytes(size as u64));
    
    for (name, data) in patterns.iter() {
        group.bench_with_input(BenchmarkId::new("chunk_bytes", name), data, |b, data| {
            b.iter(|| {
                let chunks = chunker.chunk_bytes(std_black_box(data)).unwrap();
                std_black_box(chunks);
            });
        });
    }
    
    group.finish();
}

fn bench_input_sizes(c: &mut Criterion) {
    let chunker = Chunker::default();
    let sizes = [
        1024,        // 1KB
        64 * 1024,   // 64KB
        1024 * 1024, // 1MB
        10 * 1024 * 1024, // 10MB
    ];
    
    let mut group = c.benchmark_group("input_sizes");
    
    for size in sizes.iter() {
        let data = generate_test_data(*size, 3);
        group.throughput(Throughput::Bytes(*size as u64));
        
        group.bench_with_input(BenchmarkId::new("chunk_bytes", format!("{}KB", size / 1024)), &data, |b, data| {
            b.iter(|| {
                let chunks = chunker.chunk_bytes(std_black_box(data)).unwrap();
                std_black_box(chunks);
            });
        });
    }
    
    group.finish();
}

fn bench_rolling_hash_window_sizes(c: &mut Criterion) {
    // This benchmark tests the efficiency of different window sizes in the rolling hash
    // by using different mask_bits values which effectively change how the chunker behaves
    let test_data = generate_test_data(5 * 1024 * 1024, 3); // 5MB
    
    let configs = [
        ("aggressive", ChunkerConfig {
            min_size: 32 * 1024,   // 32KB
            avg_size: 128 * 1024,  // 128KB
            max_size: 512 * 1024,  // 512KB
            mask_bits: 17,         // More frequent boundaries
        }),
        ("balanced", ChunkerConfig {
            min_size: 64 * 1024,   // 64KB
            avg_size: 256 * 1024,  // 256KB
            max_size: 1024 * 1024, // 1MB
            mask_bits: 18,         // Default FastCDC behavior
        }),
        ("conservative", ChunkerConfig {
            min_size: 128 * 1024,  // 128KB
            avg_size: 512 * 1024,  // 512KB
            max_size: 2 * 1024 * 1024, // 2MB
            mask_bits: 19,         // Fewer boundaries
        }),
    ];
    
    let mut group = c.benchmark_group("window_efficiency");
    group.throughput(Throughput::Bytes(test_data.len() as u64));
    
    for (name, config) in configs.iter() {
        let chunker = Chunker::new(config.clone()).unwrap();
        
        group.bench_with_input(BenchmarkId::new("chunk_bytes", name), &test_data, |b, data| {
            b.iter(|| {
                let chunks = chunker.chunk_bytes(std_black_box(data)).unwrap();
                std_black_box(chunks);
            });
        });
    }
    
    group.finish();
}

fn bench_async_streaming(c: &mut Criterion) {
    let test_data = generate_test_data(10 * 1024 * 1024, 3); // 10MB
    let chunker = Chunker::default();
    
    let mut group = c.benchmark_group("async_streaming");
    group.throughput(Throughput::Bytes(test_data.len() as u64));
    
    let rt = tokio::runtime::Runtime::new().unwrap();
    group.bench_function("chunk_stream", |b| {
        b.iter(|| {
            rt.block_on(async {
                let cursor = std::io::Cursor::new(test_data.clone());
                let chunks = chunker.chunk_stream(std_black_box(cursor)).await.unwrap();
                std_black_box(chunks);
            })
        });
    });
    
    group.finish();
}

// Benchmark the raw rolling hash performance
fn bench_rolling_hash_raw(c: &mut Criterion) {
    use landro_chunker::Chunker;
    
    let chunker = Chunker::default();
    let test_data = generate_test_data(1024 * 1024, 3); // 1MB
    
    let mut group = c.benchmark_group("rolling_hash_raw");
    group.throughput(Throughput::Bytes(test_data.len() as u64));
    
    // This tests the find_chunk_boundary function directly through chunking small segments
    group.bench_function("boundary_search", |b| {
        b.iter(|| {
            let chunks = chunker.chunk_bytes(std_black_box(&test_data)).unwrap();
            std_black_box(chunks);
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_chunk_sizes,
    bench_data_patterns,
    bench_input_sizes,
    bench_rolling_hash_window_sizes,
    bench_async_streaming,
    bench_rolling_hash_raw
);

criterion_main!(benches);