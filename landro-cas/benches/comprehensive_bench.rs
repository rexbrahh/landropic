use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use landro_cas::{CompressionType, ContentStore, ContentStoreConfig, FsyncPolicy};
use std::io::Cursor;
use std::time::Duration;
use tempfile::tempdir;
use tokio::runtime::Runtime;

/// Comprehensive benchmark suite for all storage operations
pub fn criterion_benchmark(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    // Benchmark streaming operations
    benchmark_streaming_operations(c, &rt);

    // Benchmark resumable operations
    benchmark_resumable_operations(c, &rt);

    // Benchmark batch operations
    benchmark_batch_operations(c, &rt);

    // Benchmark compression algorithms
    benchmark_compression(c, &rt);

    // Benchmark different file sizes
    benchmark_file_sizes(c, &rt);

    // Benchmark concurrent operations
    benchmark_concurrency(c, &rt);
}

fn benchmark_streaming_operations(c: &mut Criterion, rt: &Runtime) {
    let mut group = c.benchmark_group("streaming_operations");

    // Test different chunk sizes
    for chunk_size in [64, 256, 1024, 4096].iter() {
        let chunk_size_kb = *chunk_size;
        let data_size = chunk_size_kb * 1024; // Convert to bytes

        group.throughput(Throughput::Bytes(data_size as u64));

        group.bench_with_input(
            BenchmarkId::new("streaming_write", chunk_size_kb),
            &data_size,
            |b, &size| {
                b.to_async(rt).iter_batched(
                    || {
                        let temp_dir = tempdir().unwrap();
                        let mut config = ContentStoreConfig::default();
                        config.fsync_policy = FsyncPolicy::Async;
                        let store = rt
                            .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                            .unwrap();
                        let data = vec![0u8; size];
                        (store, data)
                    },
                    |(store, data)| async move {
                        let cursor = Cursor::new(data);
                        let result = store.write_stream(cursor, None).await.unwrap();
                        black_box(result);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("streaming_read", chunk_size_kb),
            &data_size,
            |b, &size| {
                b.to_async(rt).iter_batched(
                    || {
                        let temp_dir = tempdir().unwrap();
                        let mut config = ContentStoreConfig::default();
                        config.fsync_policy = FsyncPolicy::Async;
                        let store = rt
                            .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                            .unwrap();
                        let data = vec![1u8; size];
                        let hash = rt.block_on(store.write(&data)).unwrap().hash;
                        (store, hash)
                    },
                    |(store, hash)| async move {
                        use tokio::io::AsyncReadExt;
                        let mut reader = store.read_stream(&hash).await.unwrap();
                        let mut buffer = Vec::new();
                        let bytes_read = reader.read_to_end(&mut buffer).await.unwrap();
                        black_box(bytes_read);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn benchmark_resumable_operations(c: &mut Criterion, rt: &Runtime) {
    let mut group = c.benchmark_group("resumable_operations");
    group.sample_size(20); // Fewer samples for expensive operations

    for file_size_mb in [1, 5, 10].iter() {
        let file_size = *file_size_mb * 1024 * 1024;

        group.throughput(Throughput::Bytes(file_size as u64));

        group.bench_with_input(
            BenchmarkId::new("resumable_complete", file_size_mb),
            &file_size,
            |b, &size| {
                b.to_async(rt).iter_batched(
                    || {
                        let temp_dir = tempdir().unwrap();
                        let mut config = ContentStoreConfig::default();
                        config.fsync_policy = FsyncPolicy::Async;
                        let store = rt
                            .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                            .unwrap();
                        let data = vec![2u8; size];
                        (store, data)
                    },
                    |(store, data)| async move {
                        // Start resumable write
                        let mut partial = store.start_resumable_write(None, None).await.unwrap();

                        // Write in chunks to simulate network transfer
                        let chunk_size = 256 * 1024; // 256KB chunks
                        let mut offset = 0;
                        while offset < data.len() {
                            let end = std::cmp::min(offset + chunk_size, data.len());
                            let chunk = &data[offset..end];
                            let cursor = Cursor::new(chunk);
                            store
                                .continue_resumable_write(&mut partial, cursor)
                                .await
                                .unwrap();
                            offset = end;
                        }

                        // Complete the transfer
                        let result = store.complete_resumable_write(partial).await.unwrap();
                        black_box(result);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("resumable_interrupted", file_size_mb),
            &file_size,
            |b, &size| {
                b.to_async(rt).iter_batched(
                    || {
                        let temp_dir = tempdir().unwrap();
                        let mut config = ContentStoreConfig::default();
                        config.fsync_policy = FsyncPolicy::Async;
                        let store = rt
                            .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                            .unwrap();
                        let data = vec![3u8; size];
                        (store, data)
                    },
                    |(store, data)| async move {
                        // Start resumable write
                        let mut partial = store.start_resumable_write(None, None).await.unwrap();

                        // Write half the data
                        let half_size = data.len() / 2;
                        let first_half = &data[..half_size];
                        let cursor = Cursor::new(first_half);
                        store
                            .continue_resumable_write(&mut partial, cursor)
                            .await
                            .unwrap();

                        // "Resume" by writing second half
                        let second_half = &data[half_size..];
                        let cursor = Cursor::new(second_half);
                        store
                            .continue_resumable_write(&mut partial, cursor)
                            .await
                            .unwrap();

                        // Complete
                        let result = store.complete_resumable_write(partial).await.unwrap();
                        black_box(result);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn benchmark_batch_operations(c: &mut Criterion, rt: &Runtime) {
    let mut group = c.benchmark_group("batch_operations");

    for batch_size in [10, 50, 100, 500].iter() {
        let chunk_size = 64 * 1024; // 64KB per chunk
        let total_size = *batch_size * chunk_size;

        group.throughput(Throughput::Bytes(total_size as u64));

        group.bench_with_input(
            BenchmarkId::new("batch_write", batch_size),
            batch_size,
            |b, &size| {
                b.to_async(rt).iter_batched(
                    || {
                        let temp_dir = tempdir().unwrap();
                        let mut config = ContentStoreConfig::default();
                        config.fsync_policy = FsyncPolicy::Async;
                        config.max_concurrent_ops = 64;
                        let store = rt
                            .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                            .unwrap();

                        let mut chunks = Vec::new();
                        for i in 0..size {
                            let mut data = vec![0u8; chunk_size];
                            data[0] = i as u8; // Make each chunk unique
                            chunks.push((data.as_slice(), None));
                        }
                        (store, chunks)
                    },
                    |(store, chunks)| async move {
                        let results = store.write_batch(chunks).await.unwrap();
                        black_box(results);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("batch_stream_write", batch_size),
            batch_size,
            |b, &size| {
                b.to_async(rt).iter_batched(
                    || {
                        let temp_dir = tempdir().unwrap();
                        let mut config = ContentStoreConfig::default();
                        config.fsync_policy = FsyncPolicy::Async;
                        config.max_concurrent_ops = 32;
                        let store = rt
                            .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                            .unwrap();

                        let mut chunks = Vec::new();
                        for i in 0..size {
                            let mut data = vec![1u8; chunk_size];
                            data[0] = i as u8; // Make each chunk unique
                            let cursor = Cursor::new(data);
                            chunks.push((cursor, None));
                        }
                        (store, chunks)
                    },
                    |(store, chunks)| async move {
                        let results = store.write_batch_stream(chunks).await.unwrap();
                        black_box(results);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn benchmark_compression(c: &mut Criterion, rt: &Runtime) {
    let mut group = c.benchmark_group("compression_algorithms");

    // Test data with different characteristics
    let test_data = [
        ("random", generate_random_data(1024 * 1024)),
        ("repetitive", vec![0xABu8; 1024 * 1024]),
        ("text_like", generate_text_like_data(1024 * 1024)),
    ];

    let compression_types = [
        ("none", CompressionType::None),
        ("lz4", CompressionType::Lz4),
        ("zstd_1", CompressionType::Zstd { level: 1 }),
        ("zstd_3", CompressionType::Zstd { level: 3 }),
        ("snappy", CompressionType::Snappy),
    ];

    for (data_name, data) in test_data.iter() {
        for (comp_name, comp_type) in compression_types.iter() {
            group.throughput(Throughput::Bytes(data.len() as u64));

            group.bench_with_input(
                BenchmarkId::new(format!("{}_{}", data_name, comp_name), data.len()),
                &(data, comp_type),
                |b, &(data, compression)| {
                    b.to_async(rt).iter_batched(
                        || {
                            let temp_dir = tempdir().unwrap();
                            let mut config = ContentStoreConfig::default();
                            config.fsync_policy = FsyncPolicy::Async;
                            config.compression = *compression;
                            let store = rt
                                .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                                .unwrap();
                            (store, data.clone())
                        },
                        |(store, data)| async move {
                            let result = store.write(&data).await.unwrap();
                            black_box(result);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn benchmark_file_sizes(c: &mut Criterion, rt: &Runtime) {
    let mut group = c.benchmark_group("file_sizes");
    group.sample_size(20);

    // Test different file sizes with streaming
    for size_mb in [1, 5, 10, 25, 50].iter() {
        let file_size = *size_mb * 1024 * 1024;

        group.throughput(Throughput::Bytes(file_size as u64));

        group.bench_with_input(
            BenchmarkId::new("large_file_streaming", size_mb),
            &file_size,
            |b, &size| {
                b.to_async(rt).iter_batched(
                    || {
                        let temp_dir = tempdir().unwrap();
                        let mut config = ContentStoreConfig::default();
                        config.fsync_policy = FsyncPolicy::Async;
                        let store = rt
                            .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                            .unwrap();
                        let data = vec![0xCDu8; size];
                        (store, data)
                    },
                    |(store, data)| async move {
                        let cursor = Cursor::new(data);
                        let result = store.write_stream(cursor, None).await.unwrap();
                        black_box(result);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn benchmark_concurrency(c: &mut Criterion, rt: &Runtime) {
    let mut group = c.benchmark_group("concurrency");
    group.sample_size(10);

    for concurrent_ops in [1, 4, 8, 16, 32].iter() {
        let chunk_size = 256 * 1024; // 256KB per operation

        group.throughput(Throughput::Bytes((*concurrent_ops * chunk_size) as u64));

        group.bench_with_input(
            BenchmarkId::new("concurrent_writes", concurrent_ops),
            concurrent_ops,
            |b, &ops| {
                b.to_async(rt).iter_batched(
                    || {
                        let temp_dir = tempdir().unwrap();
                        let mut config = ContentStoreConfig::default();
                        config.fsync_policy = FsyncPolicy::Async;
                        config.max_concurrent_ops = ops;
                        let store = rt
                            .block_on(ContentStore::new_with_config(temp_dir.path(), config))
                            .unwrap();

                        let mut tasks = Vec::new();
                        for i in 0..ops {
                            let mut data = vec![0u8; chunk_size];
                            data[0] = i as u8; // Make each chunk unique
                            tasks.push(data);
                        }
                        (store, tasks)
                    },
                    |(store, tasks)| async move {
                        let mut handles = Vec::new();

                        for data in tasks {
                            let store_clone = store.clone();
                            let handle =
                                tokio::spawn(async move { store_clone.write(&data).await });
                            handles.push(handle);
                        }

                        let mut results = Vec::new();
                        for handle in handles {
                            let result = handle.await.unwrap().unwrap();
                            results.push(result);
                        }

                        black_box(results);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// Helper functions for test data generation
fn generate_random_data(size: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut rng = rand::thread_rng();
    let mut data = vec![0u8; size];
    rng.fill_bytes(&mut data);
    data
}

fn generate_text_like_data(size: usize) -> Vec<u8> {
    let pattern = b"The quick brown fox jumps over the lazy dog. ";
    let mut data = Vec::with_capacity(size);

    while data.len() < size {
        let remaining = size - data.len();
        if remaining < pattern.len() {
            data.extend_from_slice(&pattern[..remaining]);
        } else {
            data.extend_from_slice(pattern);
        }
    }

    data
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(30))
        .sample_size(50);
    targets = criterion_benchmark
);
criterion_main!(benches);
