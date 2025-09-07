use landro_cas::{ContentStore, ContentStoreConfig, FsyncPolicy};
use landro_chunker::hash::ContentHash;
use std::io::Cursor;
use tempfile::tempdir;
use tokio::time::Instant;
use tracing_test::traced_test;

/// Performance test for streaming write operations
#[tokio::test]
async fn test_streaming_write_performance() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async; // Faster for testing
    config.max_concurrent_ops = 16;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Test with 10MB chunk
    let data = vec![0u8; 10 * 1024 * 1024];
    let cursor = Cursor::new(data.clone());
    
    let start = Instant::now();
    let result = store.write_stream(cursor, None).await.unwrap();
    let duration = start.elapsed();
    
    let throughput = (data.len() as f64) / duration.as_secs_f64() / (1024.0 * 1024.0);
    
    println!("Streaming write: {} MB in {:?} = {:.2} MB/s", 
             data.len() / (1024 * 1024), 
             duration, 
             throughput);
    
    // Target: 100 MB/s
    assert!(throughput > 50.0, "Throughput too low: {:.2} MB/s", throughput);
    assert_eq!(result.size, data.len() as u64);
}

/// Performance test comparing streaming vs legacy write
#[tokio::test]
async fn test_streaming_vs_legacy_write() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Test with 5MB chunk to avoid memory pressure
    let data = vec![1u8; 5 * 1024 * 1024];
    
    // Test streaming write
    let cursor = Cursor::new(data.clone());
    let start = Instant::now();
    let _result1 = store.write_stream(cursor, None).await.unwrap();
    let streaming_duration = start.elapsed();
    
    // Test legacy write (modify data slightly to avoid dedup)
    let mut legacy_data = data.clone();
    legacy_data[0] = 2u8;
    let start = Instant::now();
    let _result2 = store.write(&legacy_data).await.unwrap();
    let legacy_duration = start.elapsed();
    
    let streaming_throughput = (data.len() as f64) / streaming_duration.as_secs_f64() / (1024.0 * 1024.0);
    let legacy_throughput = (data.len() as f64) / legacy_duration.as_secs_f64() / (1024.0 * 1024.0);
    
    println!("Streaming write: {:.2} MB/s", streaming_throughput);
    println!("Legacy write: {:.2} MB/s", legacy_throughput);
    
    // Streaming should be competitive or better
    assert!(streaming_throughput > 30.0, "Streaming throughput too low");
    assert!(legacy_throughput > 30.0, "Legacy throughput too low");
}

/// Performance test for streaming read operations
#[tokio::test]
async fn test_streaming_read_performance() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Write a 10MB chunk first
    let data = vec![3u8; 10 * 1024 * 1024];
    let result = store.write(&data).await.unwrap();
    
    // Test streaming read
    let start = Instant::now();
    let mut reader = store.read_stream(&result.hash).await.unwrap();
    let mut buffer = Vec::new();
    
    use tokio::io::AsyncReadExt;
    reader.read_to_end(&mut buffer).await.unwrap();
    let duration = start.elapsed();
    
    let throughput = (buffer.len() as f64) / duration.as_secs_f64() / (1024.0 * 1024.0);
    
    println!("Streaming read: {} MB in {:?} = {:.2} MB/s", 
             buffer.len() / (1024 * 1024), 
             duration, 
             throughput);
    
    // Target: 200 MB/s
    assert!(throughput > 80.0, "Read throughput too low: {:.2} MB/s", throughput);
    assert_eq!(buffer, data);
}

/// Performance test for batch operations
#[tokio::test]
async fn test_batch_performance() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    config.max_concurrent_ops = 64;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Create 100 chunks of 64KB each
    let chunk_size = 64 * 1024;
    let num_chunks = 100;
    
    let chunk_data: Vec<Vec<u8>> = (0..num_chunks).map(|i| {
        let mut data = vec![0u8; chunk_size];
        data[0] = i as u8; // Make each chunk unique
        data
    }).collect();
    
    let chunks: Vec<(&[u8], Option<ContentHash>)> = chunk_data.iter().map(|data| (data.as_slice(), None)).collect();
    
    let start = Instant::now();
    let results = store.write_batch(chunks).await.unwrap();
    let duration = start.elapsed();
    
    let chunks_per_sec = (num_chunks as f64) / duration.as_secs_f64();
    let total_mb = (num_chunks * chunk_size) as f64 / (1024.0 * 1024.0);
    let throughput = total_mb / duration.as_secs_f64();
    
    println!("Batch write: {} chunks in {:?} = {:.2} chunks/sec, {:.2} MB/s", 
             num_chunks, duration, chunks_per_sec, throughput);
    
    assert_eq!(results.len(), num_chunks);
    // Target: 1000 chunks/sec
    assert!(chunks_per_sec > 200.0, "Batch throughput too low: {:.2} chunks/sec", chunks_per_sec);
}

/// Performance test for streaming batch operations
#[tokio::test]
#[traced_test] 
async fn test_batch_streaming_performance() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    config.max_concurrent_ops = 32;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Create 50 chunks of 128KB each as streams
    let chunk_size = 128 * 1024;
    let num_chunks = 50;
    let mut chunks = Vec::new();
    
    for i in 0..num_chunks {
        let mut data = vec![0u8; chunk_size];
        data[0] = i as u8; // Make each chunk unique
        let cursor = Cursor::new(data);
        chunks.push((cursor, None));
    }
    
    let start = Instant::now();
    let results = store.write_batch_stream(chunks).await.unwrap();
    let duration = start.elapsed();
    
    let chunks_per_sec = (num_chunks as f64) / duration.as_secs_f64();
    let total_mb = (num_chunks * chunk_size) as f64 / (1024.0 * 1024.0);
    let throughput = total_mb / duration.as_secs_f64();
    
    println!("Streaming batch write: {} chunks in {:?} = {:.2} chunks/sec, {:.2} MB/s", 
             num_chunks, duration, chunks_per_sec, throughput);
    
    assert_eq!(results.len(), num_chunks);
    // Should be competitive with regular batch
    assert!(chunks_per_sec > 100.0, "Streaming batch throughput too low: {:.2} chunks/sec", chunks_per_sec);
}

/// Test memory usage for streaming vs legacy operations
#[tokio::test]
async fn test_memory_efficiency() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Test with a large 50MB chunk
    let data_size = 50 * 1024 * 1024;
    let data = vec![4u8; data_size];
    
    // For streaming, we should only use buffer memory (64KB)
    let cursor = Cursor::new(data.clone());
    let result = store.write_stream(cursor, None).await.unwrap();
    
    // Verify we can read it back
    let mut reader = store.read_stream(&result.hash).await.unwrap();
    let mut buffer = Vec::new();
    
    use tokio::io::AsyncReadExt;
    reader.read_to_end(&mut buffer).await.unwrap();
    
    assert_eq!(buffer.len(), data_size);
    assert_eq!(result.size, data_size as u64);
    
    // This test primarily verifies functionality; memory measurement
    // would require external tools or memory profiling
    println!("Successfully processed 50MB chunk with streaming operations");
}

/// Integration test: Large file handling
#[tokio::test]
async fn test_large_file_streaming() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Test with 100MB file
    let file_size = 100 * 1024 * 1024;
    let data = vec![5u8; file_size];
    
    let start = Instant::now();
    let cursor = Cursor::new(data.clone());
    let result = store.write_stream(cursor, None).await.unwrap();
    let write_duration = start.elapsed();
    
    let start = Instant::now();
    let mut reader = store.read_stream(&result.hash).await.unwrap();
    let mut buffer = Vec::new();
    
    use tokio::io::AsyncReadExt;
    reader.read_to_end(&mut buffer).await.unwrap();
    let read_duration = start.elapsed();
    
    let write_throughput = (file_size as f64) / write_duration.as_secs_f64() / (1024.0 * 1024.0);
    let read_throughput = (file_size as f64) / read_duration.as_secs_f64() / (1024.0 * 1024.0);
    
    println!("Large file (100MB) - Write: {:.2} MB/s, Read: {:.2} MB/s", 
             write_throughput, read_throughput);
    
    assert_eq!(buffer, data);
    assert_eq!(result.size, file_size as u64);
    
    // Reasonable performance targets for large files
    assert!(write_throughput > 50.0, "Large file write too slow");
    assert!(read_throughput > 80.0, "Large file read too slow");
}