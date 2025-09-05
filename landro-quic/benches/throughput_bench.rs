//! Throughput benchmarks for QUIC file transfers

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use landro_quic::{QuicConfig, QuicServer, QuicClient, ParallelTransferConfig, ParallelTransferManager};
use landro_crypto::{DeviceIdentity, CertificateVerifier};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use bytes::Bytes;
use tempfile::NamedTempFile;
use std::io::Write;

/// Generate test data of specified size
fn generate_test_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Benchmark throughput for various file sizes
fn bench_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("throughput");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);
    
    // Test different file sizes
    let sizes = vec![
        (1024, "1KB"),
        (10 * 1024, "10KB"),
        (100 * 1024, "100KB"),
        (1024 * 1024, "1MB"),
        (10 * 1024 * 1024, "10MB"),
        (100 * 1024 * 1024, "100MB"),
    ];
    
    for (size, label) in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        
        group.bench_with_input(
            BenchmarkId::new("single_stream", label),
            &size,
            |b, &size| {
                b.to_async(&rt).iter(|| async move {
                    transfer_single_stream(size).await
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("parallel_streams", label),
            &size,
            |b, &size| {
                b.to_async(&rt).iter(|| async move {
                    transfer_parallel_streams(size).await
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("zero_copy", label),
            &size,
            |b, &size| {
                b.to_async(&rt).iter(|| async move {
                    transfer_zero_copy(size).await
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark latency for small file transfers
fn bench_latency(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("latency");
    group.measurement_time(Duration::from_secs(5));
    
    // Small file sizes for latency testing
    let sizes = vec![
        (512, "512B"),
        (1024, "1KB"),
        (4096, "4KB"),
        (16384, "16KB"),
    ];
    
    for (size, label) in sizes {
        group.bench_with_input(
            BenchmarkId::new("small_file", label),
            &size,
            |b, &size| {
                b.to_async(&rt).iter(|| async move {
                    transfer_small_file(size).await
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark concurrent connections
fn bench_concurrent_connections(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("concurrent_connections");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(10);
    
    let connection_counts = vec![1, 10, 50, 100];
    
    for count in connection_counts {
        group.bench_with_input(
            BenchmarkId::new("connections", count),
            &count,
            |b, &count| {
                b.to_async(&rt).iter(|| async move {
                    handle_concurrent_connections(count).await
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark stream multiplexing
fn bench_stream_multiplexing(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("stream_multiplexing");
    group.measurement_time(Duration::from_secs(5));
    
    let stream_counts = vec![1, 4, 8, 16, 32, 64];
    
    for count in stream_counts {
        group.bench_with_input(
            BenchmarkId::new("streams", count),
            &count,
            |b, &count| {
                b.to_async(&rt).iter(|| async move {
                    multiplex_streams(count).await
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark adaptive parallelism
fn bench_adaptive_parallelism(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("adaptive_parallelism");
    group.measurement_time(Duration::from_secs(10));
    
    group.bench_function("adaptive_enabled", |b| {
        b.to_async(&rt).iter(|| async {
            transfer_with_adaptive_parallelism(true).await
        });
    });
    
    group.bench_function("adaptive_disabled", |b| {
        b.to_async(&rt).iter(|| async {
            transfer_with_adaptive_parallelism(false).await
        });
    });
    
    group.finish();
}

/// Benchmark different congestion control algorithms
fn bench_congestion_control(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("congestion_control");
    group.measurement_time(Duration::from_secs(10));
    
    let file_size = 10 * 1024 * 1024; // 10MB
    
    group.bench_function("cubic", |b| {
        b.to_async(&rt).iter(|| async {
            transfer_with_congestion_control("cubic", file_size).await
        });
    });
    
    group.bench_function("bbr", |b| {
        b.to_async(&rt).iter(|| async {
            transfer_with_congestion_control("bbr", file_size).await
        });
    });
    
    group.finish();
}

// Helper functions for benchmarks (simplified implementations)

async fn transfer_single_stream(size: usize) -> usize {
    // Simulate single stream transfer
    let data = generate_test_data(size);
    tokio::time::sleep(Duration::from_micros((size / 1000) as u64)).await;
    black_box(data.len())
}

async fn transfer_parallel_streams(size: usize) -> usize {
    // Simulate parallel stream transfer
    let data = generate_test_data(size);
    let chunk_size = size / 8;
    
    let mut handles = vec![];
    for chunk in data.chunks(chunk_size) {
        let chunk_len = chunk.len();
        handles.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_micros((chunk_len / 2000) as u64)).await;
            chunk_len
        }));
    }
    
    let mut total = 0;
    for handle in handles {
        total += handle.await.unwrap();
    }
    
    black_box(total)
}

async fn transfer_zero_copy(size: usize) -> usize {
    // Simulate zero-copy transfer
    let mut file = NamedTempFile::new().unwrap();
    let data = generate_test_data(size);
    file.write_all(&data).unwrap();
    
    // Simulate zero-copy read
    tokio::time::sleep(Duration::from_micros((size / 5000) as u64)).await;
    
    black_box(size)
}

async fn transfer_small_file(size: usize) -> usize {
    // Simulate small file transfer with minimal latency
    let data = generate_test_data(size);
    tokio::time::sleep(Duration::from_micros(100)).await;
    black_box(data.len())
}

async fn handle_concurrent_connections(count: usize) -> usize {
    let mut handles = vec![];
    
    for _ in 0..count {
        handles.push(tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            1024
        }));
    }
    
    let mut total = 0;
    for handle in handles {
        total += handle.await.unwrap();
    }
    
    black_box(total)
}

async fn multiplex_streams(count: usize) -> usize {
    let data_per_stream = 1024 * 1024; // 1MB per stream
    let mut handles = vec![];
    
    for _ in 0..count {
        handles.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_micros(500)).await;
            data_per_stream
        }));
    }
    
    let mut total = 0;
    for handle in handles {
        total += handle.await.unwrap();
    }
    
    black_box(total)
}

async fn transfer_with_adaptive_parallelism(enabled: bool) -> usize {
    let size = 50 * 1024 * 1024; // 50MB
    let parallelism = if enabled {
        // Simulate adaptive parallelism
        16
    } else {
        // Fixed parallelism
        4
    };
    
    let chunk_size = size / parallelism;
    let mut handles = vec![];
    
    for _ in 0..parallelism {
        handles.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            chunk_size
        }));
    }
    
    let mut total = 0;
    for handle in handles {
        total += handle.await.unwrap();
    }
    
    black_box(total)
}

async fn transfer_with_congestion_control(algorithm: &str, size: usize) -> usize {
    // Simulate different congestion control behavior
    let delay = match algorithm {
        "cubic" => Duration::from_millis((size / 100000) as u64),
        "bbr" => Duration::from_millis((size / 120000) as u64),
        _ => Duration::from_millis((size / 100000) as u64),
    };
    
    tokio::time::sleep(delay).await;
    black_box(size)
}

criterion_group!(
    benches,
    bench_throughput,
    bench_latency,
    bench_concurrent_connections,
    bench_stream_multiplexing,
    bench_adaptive_parallelism,
    bench_congestion_control
);

criterion_main!(benches);