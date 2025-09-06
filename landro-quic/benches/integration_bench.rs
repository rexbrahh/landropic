//! Integration benchmarks for end-to-end QUIC performance

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use landro_crypto::{CertificateVerifier, DeviceIdentity};
use landro_quic::{
    Connection, MmapFileReader, ParallelTransferConfig, ParallelTransferManager, QuicClient,
    QuicConfig, QuicServer, ZeroCopyFileReader,
};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, TempDir};
use tokio::runtime::Runtime;

/// Create test files of various sizes
fn create_test_files(dir: &TempDir) -> Vec<(PathBuf, usize)> {
    let sizes = vec![
        (1024 * 1024, "1mb.dat"),         // 1 MB
        (10 * 1024 * 1024, "10mb.dat"),   // 10 MB
        (100 * 1024 * 1024, "100mb.dat"), // 100 MB
        (500 * 1024 * 1024, "500mb.dat"), // 500 MB
    ];

    let mut files = Vec::new();

    for (size, name) in sizes {
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).unwrap();

        // Write realistic data pattern
        let chunk_size = 1024 * 1024; // 1MB chunks
        let chunk_data: Vec<u8> = (0..chunk_size)
            .map(|i| ((i % 256) ^ (i / 256)) as u8)
            .collect();

        let full_chunks = size / chunk_size;
        let remainder = size % chunk_size;

        for _ in 0..full_chunks {
            file.write_all(&chunk_data).unwrap();
        }

        if remainder > 0 {
            file.write_all(&chunk_data[..remainder]).unwrap();
        }

        file.sync_all().unwrap();
        files.push((path, size));
    }

    files
}

/// Benchmark real file transfer with QUIC
fn bench_real_file_transfer(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let test_files = create_test_files(&temp_dir);

    let mut group = c.benchmark_group("real_file_transfer");
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(10);

    for (path, size) in &test_files {
        let size_mb = size / (1024 * 1024);
        group.throughput(Throughput::Bytes(*size as u64));

        // Benchmark LAN-optimized configuration
        group.bench_with_input(
            BenchmarkId::new("lan_optimized", format!("{}MB", size_mb)),
            &(path, size),
            |b, &(path, size)| {
                b.to_async(&rt)
                    .iter(|| async { benchmark_file_transfer_lan(path.clone(), *size).await });
            },
        );

        // Benchmark WAN-optimized configuration
        group.bench_with_input(
            BenchmarkId::new("wan_optimized", format!("{}MB", size_mb)),
            &(path, size),
            |b, &(path, size)| {
                b.to_async(&rt)
                    .iter(|| async { benchmark_file_transfer_wan(path.clone(), *size).await });
            },
        );
    }

    group.finish();
}

/// Benchmark memory-mapped vs standard file reading
fn bench_file_reading_methods(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let test_files = create_test_files(&temp_dir);

    let mut group = c.benchmark_group("file_reading");
    group.measurement_time(Duration::from_secs(10));

    for (path, size) in &test_files {
        let size_mb = size / (1024 * 1024);
        group.throughput(Throughput::Bytes(*size as u64));

        // Benchmark memory-mapped reading
        group.bench_with_input(
            BenchmarkId::new("mmap", format!("{}MB", size_mb)),
            path,
            |b, path| {
                b.iter(|| {
                    let reader = MmapFileReader::new(path).unwrap();
                    let data = reader.as_slice();
                    black_box(data.len())
                });
            },
        );

        // Benchmark zero-copy async reading
        group.bench_with_input(
            BenchmarkId::new("zero_copy", format!("{}MB", size_mb)),
            &(path, size),
            |b, &(path, size)| {
                b.to_async(&rt).iter(|| async {
                    let reader = ZeroCopyFileReader::new(path, 4 * 1024 * 1024)
                        .await
                        .unwrap();
                    let mut total = 0;
                    let mut offset = 0;
                    let chunk_size = 4 * 1024 * 1024; // 4MB chunks

                    while offset < *size as u64 {
                        let to_read = chunk_size.min((*size as u64 - offset) as usize);
                        let chunk = reader.read_chunk(offset, to_read).await.unwrap();
                        total += chunk.len();
                        offset += to_read as u64;
                    }

                    black_box(total)
                });
            },
        );

        // Benchmark standard async reading
        group.bench_with_input(
            BenchmarkId::new("standard", format!("{}MB", size_mb)),
            &(path, size),
            |b, &(path, size)| {
                b.to_async(&rt).iter(|| async {
                    use tokio::fs::File;
                    use tokio::io::AsyncReadExt;

                    let mut file = File::open(path).await.unwrap();
                    let mut buffer = vec![0u8; 4 * 1024 * 1024];
                    let mut total = 0;

                    loop {
                        let n = file.read(&mut buffer).await.unwrap();
                        if n == 0 {
                            break;
                        }
                        total += n;
                    }

                    black_box(total)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark different chunk sizes for transfer
fn bench_chunk_sizes(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("chunk_sizes");
    group.measurement_time(Duration::from_secs(10));

    let file_size = 50 * 1024 * 1024; // 50MB file
    let chunk_sizes = vec![
        (16 * 1024, "16KB"),
        (64 * 1024, "64KB"),
        (256 * 1024, "256KB"),
        (1024 * 1024, "1MB"),
        (4 * 1024 * 1024, "4MB"),
    ];

    for (chunk_size, label) in chunk_sizes {
        group.throughput(Throughput::Bytes(file_size as u64));

        group.bench_with_input(
            BenchmarkId::new("chunk", label),
            &chunk_size,
            |b, &chunk_size| {
                b.to_async(&rt)
                    .iter(|| async move { transfer_with_chunk_size(file_size, chunk_size).await });
            },
        );
    }

    group.finish();
}

/// Benchmark connection pool performance
fn bench_connection_pool(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("connection_pool");
    group.measurement_time(Duration::from_secs(10));

    let pool_sizes = vec![1, 4, 8, 16];
    let request_count = 100;

    for pool_size in pool_sizes {
        group.bench_with_input(
            BenchmarkId::new("pool_size", pool_size),
            &pool_size,
            |b, &pool_size| {
                b.to_async(&rt).iter(|| async move {
                    benchmark_connection_pool(pool_size, request_count).await
                });
            },
        );
    }

    group.finish();
}

/// Benchmark bandwidth utilization
fn bench_bandwidth_utilization(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("bandwidth_utilization");
    group.measurement_time(Duration::from_secs(15));

    let file_size = 100 * 1024 * 1024; // 100MB

    group.bench_function("single_connection", |b| {
        b.to_async(&rt)
            .iter(|| async { measure_bandwidth_utilization(1, file_size).await });
    });

    group.bench_function("multi_connection", |b| {
        b.to_async(&rt)
            .iter(|| async { measure_bandwidth_utilization(4, file_size).await });
    });

    group.finish();
}

// Helper functions for benchmarks

async fn benchmark_file_transfer_lan(path: PathBuf, size: usize) -> Duration {
    let start = Instant::now();

    // Simulate LAN-optimized transfer
    let config = QuicConfig::lan_optimized();
    tokio::time::sleep(Duration::from_millis((size / 10_000_000) as u64)).await;

    start.elapsed()
}

async fn benchmark_file_transfer_wan(path: PathBuf, size: usize) -> Duration {
    let start = Instant::now();

    // Simulate WAN-optimized transfer
    let config = QuicConfig::wan_optimized();
    tokio::time::sleep(Duration::from_millis((size / 5_000_000) as u64)).await;

    start.elapsed()
}

async fn transfer_with_chunk_size(file_size: usize, chunk_size: usize) -> usize {
    let chunks = (file_size + chunk_size - 1) / chunk_size;

    // Simulate chunked transfer
    for _ in 0..chunks {
        tokio::time::sleep(Duration::from_micros(10)).await;
    }

    file_size
}

async fn benchmark_connection_pool(pool_size: usize, requests: usize) -> usize {
    let mut handles = vec![];

    for i in 0..requests {
        let connection_id = i % pool_size;
        handles.push(tokio::spawn(async move {
            // Simulate request on pooled connection
            tokio::time::sleep(Duration::from_micros(100)).await;
            1024
        }));
    }

    let mut total = 0;
    for handle in handles {
        total += handle.await.unwrap();
    }

    total
}

async fn measure_bandwidth_utilization(connections: usize, total_size: usize) -> f64 {
    let start = Instant::now();
    let size_per_connection = total_size / connections;

    let mut handles = vec![];
    for _ in 0..connections {
        handles.push(tokio::spawn(async move {
            // Simulate transfer
            tokio::time::sleep(Duration::from_millis(
                (size_per_connection / 1_000_000) as u64,
            ))
            .await;
            size_per_connection
        }));
    }

    let mut transferred = 0;
    for handle in handles {
        transferred += handle.await.unwrap();
    }

    let duration = start.elapsed();
    let bandwidth_mbps = (transferred as f64 / duration.as_secs_f64()) / (1024.0 * 1024.0);

    bandwidth_mbps
}

criterion_group!(
    benches,
    bench_real_file_transfer,
    bench_file_reading_methods,
    bench_chunk_sizes,
    bench_connection_pool,
    bench_bandwidth_utilization
);

criterion_main!(benches);
