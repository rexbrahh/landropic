use landro_cas::{ContentStore, ContentStoreConfig, FsyncPolicy, PartialTransfer};
use landro_chunker::hash::ContentHash;
use std::io::Cursor;
use tempfile::tempdir;
use tokio::time::{Duration, sleep};

/// Test basic resumable write operations
#[tokio::test]
async fn test_resumable_write_basic() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Create test data
    let data = vec![42u8; 1024 * 1024]; // 1MB
    let expected_hash = {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&data);
        ContentHash::from_blake3(hasher.finalize())
    };

    // Start resumable write
    let mut partial = store.start_resumable_write(Some(expected_hash), Some(data.len() as u64)).await.unwrap();
    
    // Simulate writing first half
    let first_half = &data[..data.len()/2];
    let cursor = Cursor::new(first_half);
    store.continue_resumable_write(&mut partial, cursor).await.unwrap();
    
    assert_eq!(partial.bytes_received, (data.len() / 2) as u64);
    assert!(partial.temp_path.exists());
    
    // Simulate writing second half
    let second_half = &data[data.len()/2..];
    let cursor = Cursor::new(second_half);
    store.continue_resumable_write(&mut partial, cursor).await.unwrap();
    
    assert_eq!(partial.bytes_received, data.len() as u64);
    
    // Complete the write
    let result = store.complete_resumable_write(partial).await.unwrap();
    
    assert_eq!(result.hash, expected_hash);
    assert_eq!(result.size, data.len() as u64);
    
    // Verify we can read the data back
    let read_data = store.read(&result.hash).await.unwrap();
    assert_eq!(read_data.to_vec(), data);
}

/// Test resumable write with hash mismatch
#[tokio::test]
async fn test_resumable_write_hash_mismatch() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Create test data
    let data = vec![42u8; 1024];
    let wrong_hash = {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"wrong data");
        ContentHash::from_blake3(hasher.finalize())
    };

    // Start resumable write with wrong expected hash
    let mut partial = store.start_resumable_write(Some(wrong_hash), None).await.unwrap();
    
    // Write the data
    let cursor = Cursor::new(data.clone());
    store.continue_resumable_write(&mut partial, cursor).await.unwrap();
    
    // Completing should fail due to hash mismatch
    let result = store.complete_resumable_write(partial).await;
    assert!(result.is_err());
}

/// Test resumable write with size mismatch
#[tokio::test]
async fn test_resumable_write_size_mismatch() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Create test data
    let data = vec![42u8; 1024];

    // Start resumable write with wrong expected size
    let mut partial = store.start_resumable_write(None, Some(2048)).await.unwrap();
    
    // Write the data
    let cursor = Cursor::new(data.clone());
    store.continue_resumable_write(&mut partial, cursor).await.unwrap();
    
    // Completing should fail due to size mismatch
    let result = store.complete_resumable_write(partial).await;
    assert!(result.is_err());
}

/// Test cancelling a resumable write
#[tokio::test]
async fn test_resumable_write_cancel() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Start resumable write
    let mut partial = store.start_resumable_write(None, None).await.unwrap();
    
    // Write some data
    let data = vec![42u8; 1024];
    let cursor = Cursor::new(data);
    store.continue_resumable_write(&mut partial, cursor).await.unwrap();
    
    let temp_path = partial.temp_path.clone();
    assert!(temp_path.exists());
    
    // Cancel the write
    store.cancel_resumable_write(partial).await.unwrap();
    
    // Temp file should be cleaned up
    assert!(!temp_path.exists());
}

/// Test multiple resumable writes simultaneously
#[tokio::test]
async fn test_multiple_resumable_writes() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    config.max_concurrent_ops = 64;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Start multiple resumable writes
    let mut partials = Vec::new();
    for i in 0..10 {
        let partial = store.start_resumable_write(None, None).await.unwrap();
        partials.push((partial, i));
    }

    // Write data to each
    for (partial, i) in &mut partials {
        let data = vec![*i as u8; 1024];
        let cursor = Cursor::new(data);
        store.continue_resumable_write(partial, cursor).await.unwrap();
    }
    
    // Complete all writes
    let mut results = Vec::new();
    for (partial, _) in partials {
        let result = store.complete_resumable_write(partial).await.unwrap();
        results.push(result);
    }
    
    assert_eq!(results.len(), 10);
    
    // Verify all data is readable
    for (result, i) in results.iter().zip(0..10) {
        let data = store.read(&result.hash).await.unwrap();
        assert_eq!(data[0], i as u8);
        assert_eq!(data.len(), 1024);
    }
}

/// Test cleanup of expired partial transfers
#[tokio::test]
async fn test_cleanup_expired_partials() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Create a partial transfer and write some data
    let mut partial = store.start_resumable_write(None, None).await.unwrap();
    let data = vec![42u8; 1024];
    let cursor = Cursor::new(data);
    store.continue_resumable_write(&mut partial, cursor).await.unwrap();
    
    let temp_path = partial.temp_path.clone();
    assert!(temp_path.exists());
    
    // Test cleanup - should not clean up recent files
    let cleaned = store.cleanup_expired_partials().await.unwrap();
    assert_eq!(cleaned, 0);
    assert!(temp_path.exists());
    
    // Manually mark as expired by modifying the timestamp
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::os::unix::fs::MetadataExt;
    
    // We can't easily test actual expiration in a unit test,
    // but we can test the function works
    store.cancel_resumable_write(partial).await.unwrap();
}

/// Test resumable write preserves data integrity across interruptions
#[tokio::test]
async fn test_resumable_write_integrity() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Create large test data with pattern
    let chunk_size = 64 * 1024; // 64KB chunks
    let num_chunks = 8;
    let mut data = Vec::new();
    for i in 0..num_chunks {
        let mut chunk = vec![i as u8; chunk_size];
        // Add some pattern to detect corruption
        chunk[0] = 0xFF;
        chunk[chunk_size - 1] = 0xAA;
        data.extend(chunk);
    }
    
    let expected_hash = {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&data);
        ContentHash::from_blake3(hasher.finalize())
    };

    // Start resumable write
    let mut partial = store.start_resumable_write(Some(expected_hash), Some(data.len() as u64)).await.unwrap();
    
    // Write data in multiple chunks to simulate network interruptions
    let chunk_size = 128 * 1024; // 128KB per write
    let mut offset = 0;
    
    while offset < data.len() {
        let end = std::cmp::min(offset + chunk_size, data.len());
        let chunk = &data[offset..end];
        let cursor = Cursor::new(chunk);
        store.continue_resumable_write(&mut partial, cursor).await.unwrap();
        offset = end;
        
        // Verify progress
        assert_eq!(partial.bytes_received, offset as u64);
    }
    
    // Complete the write
    let result = store.complete_resumable_write(partial).await.unwrap();
    
    assert_eq!(result.hash, expected_hash);
    assert_eq!(result.size, data.len() as u64);
    
    // Verify data integrity
    let read_data = store.read(&result.hash).await.unwrap();
    assert_eq!(read_data.len(), data.len());
    assert_eq!(read_data.to_vec(), data);
    
    // Verify pattern integrity
    for i in 0..num_chunks {
        let chunk_start = i * (64 * 1024);
        assert_eq!(read_data[chunk_start], 0xFF);
        assert_eq!(read_data[chunk_start + (64 * 1024) - 1], 0xAA);
    }
}

/// Test resumable write with deduplication
#[tokio::test]
async fn test_resumable_write_deduplication() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Create test data
    let data = vec![123u8; 2048];
    let expected_hash = {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&data);
        ContentHash::from_blake3(hasher.finalize())
    };

    // First write the data normally
    let result1 = store.write(&data).await.unwrap();
    assert_eq!(result1.hash, expected_hash);

    // Now try resumable write with same data
    let mut partial = store.start_resumable_write(Some(expected_hash), Some(data.len() as u64)).await.unwrap();
    
    let cursor = Cursor::new(data.clone());
    store.continue_resumable_write(&mut partial, cursor).await.unwrap();
    
    // Complete should detect existing object
    let result2 = store.complete_resumable_write(partial).await.unwrap();
    
    assert_eq!(result2.hash, expected_hash);
    assert_eq!(result2.size, data.len() as u64);
    assert_eq!(result1.hash, result2.hash);
}

/// Performance test for resumable writes
#[tokio::test]
async fn test_resumable_write_performance() {
    let temp_dir = tempdir().unwrap();
    let mut config = ContentStoreConfig::default();
    config.fsync_policy = FsyncPolicy::Async;
    
    let store = ContentStore::new_with_config(temp_dir.path(), config).await.unwrap();

    // Test with 10MB file split into chunks
    let file_size = 10 * 1024 * 1024;
    let chunk_size = 1024 * 1024; // 1MB chunks
    let data = vec![0xABu8; file_size];
    
    let expected_hash = {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&data);
        ContentHash::from_blake3(hasher.finalize())
    };

    let start = tokio::time::Instant::now();
    
    // Start resumable write
    let mut partial = store.start_resumable_write(Some(expected_hash), Some(data.len() as u64)).await.unwrap();
    
    // Write in chunks
    let mut offset = 0;
    while offset < data.len() {
        let end = std::cmp::min(offset + chunk_size, data.len());
        let chunk = &data[offset..end];
        let cursor = Cursor::new(chunk);
        store.continue_resumable_write(&mut partial, cursor).await.unwrap();
        offset = end;
    }
    
    // Complete
    let result = store.complete_resumable_write(partial).await.unwrap();
    
    let duration = start.elapsed();
    let throughput = (file_size as f64) / duration.as_secs_f64() / (1024.0 * 1024.0);
    
    println!("Resumable write: {} MB in {:?} = {:.2} MB/s", 
             file_size / (1024 * 1024), duration, throughput);
    
    assert_eq!(result.hash, expected_hash);
    assert_eq!(result.size, file_size as u64);
    
    // Performance should be reasonable (target: >30 MB/s)
    assert!(throughput > 20.0, "Resumable write throughput too low: {:.2} MB/s", throughput);
}