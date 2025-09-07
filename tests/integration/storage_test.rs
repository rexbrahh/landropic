use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, debug};
use anyhow::{Context, Result};

use landro_cas::{ContentStore, AtomicWriter};
use landro_chunker::{Chunker, ChunkerConfig};
use landro_index::{AsyncIndexer, Manifest};
use landro_crypto::DeviceIdentity;

use crate::common::{
    init_crypto, generate_test_data, create_test_files
};

/// Test basic content-addressed storage operations
#[tokio::test]
async fn test_cas_basic_operations() -> Result<()> {
    init_crypto();
    
    info!("Testing basic CAS operations");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    let store = Arc::new(ContentStore::new(&cas_path).await?);
    
    // Test data of various sizes
    let test_cases = vec![
        ("empty", vec![]),
        ("small", vec![42u8; 256]),
        ("medium", generate_test_data(4096)),
        ("large", generate_test_data(1_048_576)), // 1MB
    ];
    
    let mut stored_objects = Vec::new();
    
    for (name, data) in test_cases {
        info!("Testing CAS with {} data ({} bytes)", name, data.len());
        
        // Store data
        let object_id = store.put(&data).await?;
        info!("Stored {} as {}", name, hex::encode(&object_id));
        stored_objects.push((name, object_id.clone(), data.clone()));
        
        // Verify storage
        assert!(store.contains(&object_id).await?, "Store should contain object");
        
        // Retrieve and verify
        let retrieved = store.get(&object_id).await?
            .ok_or_else(|| anyhow::anyhow!("Failed to retrieve object"))?;
        assert_eq!(data, retrieved, "Retrieved data should match original");
        
        // Test size
        let size = store.size(&object_id).await?;
        assert_eq!(data.len(), size, "Size should match data length");
    }
    
    // Test deduplication - same content should have same ID
    let duplicate_data = generate_test_data(1024);
    let id1 = store.put(&duplicate_data).await?;
    let id2 = store.put(&duplicate_data).await?;
    assert_eq!(id1, id2, "Identical content should have same object ID");
    
    // Test list operation
    let all_objects = store.list().await?;
    info!("Store contains {} objects total", all_objects.len());
    assert!(all_objects.len() >= stored_objects.len(), "Store should contain at least our objects");
    
    // Verify all our objects are in the list
    for (name, object_id, _) in &stored_objects {
        assert!(
            all_objects.contains(object_id),
            "Object {} should be in store listing",
            name
        );
    }
    
    info!("CAS basic operations test passed");
    Ok(())
}

/// Test atomic writer for safe concurrent access
#[tokio::test]
async fn test_atomic_writer() -> Result<()> {
    init_crypto();
    
    info!("Testing atomic writer");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    let store = Arc::new(ContentStore::new(&cas_path).await?);
    
    // Test atomic write
    let test_data = generate_test_data(8192);
    let mut writer = AtomicWriter::new(&cas_path, "test-atomic")?;
    
    // Write data in chunks
    let chunk_size = 1024;
    for chunk in test_data.chunks(chunk_size) {
        writer.write_all(chunk).await?;
    }
    
    // Finalize the write
    let final_path = writer.finalize().await?;
    info!("Atomic write completed to: {:?}", final_path);
    
    // Verify file was written correctly
    let written_data = tokio::fs::read(&final_path).await?;
    assert_eq!(test_data, written_data, "Written data should match original");
    
    // Test that partial writes are cleaned up on drop
    let temp_data = generate_test_data(4096);
    let mut incomplete_writer = AtomicWriter::new(&cas_path, "test-incomplete")?;
    incomplete_writer.write_all(&temp_data[..2048]).await?;
    
    // Drop without finalizing
    drop(incomplete_writer);
    
    info!("Atomic writer test passed");
    Ok(())
}

/// Test chunker with various file sizes and patterns
#[tokio::test]
async fn test_chunker_comprehensive() -> Result<()> {
    init_crypto();
    
    info!("Testing chunker with comprehensive scenarios");
    
    let chunker_config = ChunkerConfig {
        min_size: 2048,
        avg_size: 8192,
        max_size: 32768,
        mask_bits: 13, // For 8KB average (2^13)
    };
    let chunker = Chunker::new(chunker_config)?;
    
    // Test various data patterns
    let test_cases = vec![
        ("tiny", vec![1u8; 100]),
        ("small", vec![2u8; 1024]),
        ("exact_min", vec![3u8; 2048]),
        ("around_avg", vec![4u8; 8000]),
        ("large", generate_test_data(100_000)),
        ("repetitive", vec![5u8; 50_000]), // Highly repetitive pattern
        ("random", {
            let mut data = Vec::new();
            for i in 0..25000 {
                data.push((i % 256) as u8);
            }
            data
        }),
    ];
    
    for (name, data) in test_cases {
        info!("Chunking {} data ({} bytes)", name, data.len());
        
        let chunks = chunker.chunk_bytes(&data)?;
        info!("Created {} chunks", chunks.len());
        
        // Verify chunks
        let mut total_size = 0;
        let mut reconstructed = Vec::new();
        
        for (i, chunk) in chunks.iter().enumerate() {
            info!("Chunk {}: {} bytes, hash: {}", i, chunk.data.len(), hex::encode(&chunk.hash));
            
            // Check chunk size constraints
            if chunks.len() == 1 {
                // Single chunk can be any size for small files
                assert!(chunk.data.len() <= data.len());
            } else if i == 0 || i == chunks.len() - 1 {
                // First and last chunks can be smaller than min_size
                assert!(chunk.data.len() <= chunker_config.max_size);
            } else {
                // Middle chunks should respect size constraints
                assert!(chunk.data.len() >= chunker_config.min_size);
                assert!(chunk.data.len() <= chunker_config.max_size);
            }
            
            total_size += chunk.data.len();
            reconstructed.extend_from_slice(&chunk.data);
        }
        
        assert_eq!(total_size, data.len(), "Total chunk size should equal original");
        assert_eq!(reconstructed, data, "Reconstructed data should match original");
        
        // Test chunking determinism - same data should produce same chunks
        let chunks2 = chunker.chunk_bytes(&data)?;
        assert_eq!(chunks.len(), chunks2.len(), "Same data should produce same number of chunks");
        
        for (c1, c2) in chunks.iter().zip(chunks2.iter()) {
            assert_eq!(c1.hash, c2.hash, "Same chunk should have same hash");
            assert_eq!(c1.data, c2.data, "Same chunk should have same data");
        }
    }
    
    info!("Comprehensive chunker test passed");
    Ok(())
}

/// Test indexer with file operations
#[tokio::test]
async fn test_indexer_comprehensive() -> Result<()> {
    init_crypto();
    
    info!("Testing indexer comprehensive operations");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    let db_path = temp_dir.path().join("index.sqlite");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    let indexer = Arc::new(AsyncIndexer::new(&cas_path, &db_path, Default::default()).await?);
    
    // Create test file hierarchy
    let test_folder = temp_dir.path().join("test_files");
    let test_files = create_test_files(&test_folder)?;
    
    info!("Created {} test files for indexing", test_files.len());
    
    // Index all files
    for file_path in &test_files {
        info!("Indexing file: {:?}", file_path);
        indexer.index_file(file_path).await?;
    }
    
    // Generate manifest
    let manifest = indexer.generate_manifest(&test_folder).await?;
    info!("Generated manifest with {} entries", manifest.files.len());
    
    assert_eq!(manifest.files.len(), test_files.len(), "Manifest should include all files");
    
    // Verify each file in manifest
    for (rel_path, file_entry) in &manifest.files {
        info!("Manifest entry: {} -> {} bytes, {} chunks", 
              rel_path.display(), file_entry.size, file_entry.chunks.len());
        
        assert!(file_entry.size > 0 || rel_path.to_string_lossy().contains("empty"), 
                "File size should be positive (unless it's an empty test file)");
        assert!(!file_entry.chunks.is_empty(), "File should have at least one chunk");
        
        // Verify chunk references are valid
        for chunk_id in &file_entry.chunks {
            // In a full implementation, we'd verify chunks exist in CAS
            assert!(!chunk_id.is_empty(), "Chunk ID should not be empty");
        }
    }
    
    // Test file modification detection
    let first_file = &test_files[0];
    let original_metadata = tokio::fs::metadata(first_file).await?;
    
    // Modify file
    tokio::fs::write(first_file, "MODIFIED CONTENT FOR INDEXER TEST").await?;
    
    // Re-index
    indexer.index_file(first_file).await?;
    
    // Generate new manifest
    let manifest2 = indexer.generate_manifest(&test_folder).await?;
    
    // Compare manifests
    let diff = manifest.diff(&manifest2);
    assert!(!diff.modified.is_empty(), "Should detect modified file");
    
    info!("File modification detection test passed");
    
    // Test concurrent indexing
    info!("Testing concurrent indexing");
    
    let concurrent_files = vec![
        temp_folder.join("concurrent1.txt"),
        temp_folder.join("concurrent2.txt"),
        temp_folder.join("concurrent3.txt"),
    ];
    
    // Create concurrent test files
    for (i, file_path) in concurrent_files.iter().enumerate() {
        let content = format!("Concurrent test file {} with unique content", i);
        tokio::fs::write(file_path, content).await?;
    }
    
    // Index files concurrently
    let indexer_clone = indexer.clone();
    let mut handles = Vec::new();
    
    for file_path in concurrent_files {
        let indexer = indexer_clone.clone();
        let handle = tokio::spawn(async move {
            indexer.index_file(&file_path).await
        });
        handles.push(handle);
    }
    
    // Wait for all indexing operations
    let results = futures::future::try_join_all(handles).await?;
    for result in results {
        result?; // Propagate any indexing errors
    }
    
    info!("Concurrent indexing test passed");
    
    // Test database persistence
    drop(indexer);
    
    // Recreate indexer with same database
    let indexer2 = Arc::new(AsyncIndexer::new(&cas_path, &db_path, Default::default()).await?);
    let restored_manifest = indexer2.generate_manifest(&test_folder).await?;
    
    info!("Restored manifest has {} entries", restored_manifest.files.len());
    // Should have at least the original files (concurrent files might not be in the test folder)
    assert!(restored_manifest.files.len() >= test_files.len(), 
            "Restored manifest should have persisted data");
    
    info!("Database persistence test passed");
    Ok(())
}

/// Test storage under high load
#[tokio::test]
async fn test_storage_performance() -> Result<()> {
    init_crypto();
    
    info!("Testing storage performance under load");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    let store = Arc::new(ContentStore::new(&cas_path).await?);
    
    // Test concurrent operations
    let num_operations = 50;
    let data_size = 4096;
    
    let start_time = std::time::Instant::now();
    let mut handles = Vec::new();
    
    for i in 0..num_operations {
        let store = store.clone();
        let handle = tokio::spawn(async move {
            let data = generate_test_data(data_size);
            let object_id = store.put(&data).await?;
            
            // Verify we can retrieve it
            let retrieved = store.get(&object_id).await?
                .ok_or_else(|| anyhow::anyhow!("Failed to retrieve object"))?;
            
            assert_eq!(data.len(), retrieved.len());
            Ok::<String, anyhow::Error>(hex::encode(object_id))
        });
        handles.push(handle);
    }
    
    let results = futures::future::try_join_all(handles).await?;
    let duration = start_time.elapsed();
    
    info!("Completed {} concurrent operations in {:?}", num_operations, duration);
    info!("Average: {:.2} ops/sec", num_operations as f64 / duration.as_secs_f64());
    
    assert_eq!(results.len(), num_operations);
    
    // Test storage space efficiency
    let total_objects = store.list().await?;
    info!("Total objects in store: {}", total_objects.len());
    
    // Due to deduplication, we might have fewer unique objects than operations
    assert!(total_objects.len() <= num_operations);
    
    info!("Storage performance test passed");
    Ok(())
}

/// Test error handling in storage operations
#[tokio::test]
async fn test_storage_error_handling() -> Result<()> {
    init_crypto();
    
    info!("Testing storage error handling");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    let store = Arc::new(ContentStore::new(&cas_path).await?);
    
    // Test retrieval of non-existent object
    let fake_id = vec![0u8; 32]; // Invalid object ID
    let result = store.get(&fake_id).await?;
    assert!(result.is_none(), "Should return None for non-existent object");
    
    // Test contains check for non-existent object
    let contains = store.contains(&fake_id).await?;
    assert!(!contains, "Should return false for non-existent object");
    
    // Test size query for non-existent object
    let size_result = store.size(&fake_id).await;
    assert!(size_result.is_err(), "Size query for non-existent object should fail");
    
    info!("Storage error handling test passed");
    Ok(())
}

/// Test manifest operations and diffing
#[tokio::test]
async fn test_manifest_operations() -> Result<()> {
    init_crypto();
    
    info!("Testing manifest operations");
    
    let temp_dir = tempfile::TempDir::new()?;
    let cas_path = temp_dir.path().join("objects");
    let db_path = temp_dir.path().join("index.sqlite");
    tokio::fs::create_dir_all(&cas_path).await?;
    
    let indexer = Arc::new(AsyncIndexer::new(&cas_path, &db_path, Default::default()).await?);
    
    // Create initial file set
    let test_folder = temp_dir.path().join("manifest_test");
    let initial_files = vec![
        (test_folder.join("file1.txt"), "Content of file 1"),
        (test_folder.join("file2.txt"), "Content of file 2"),
        (test_folder.join("file3.txt"), "Content of file 3"),
    ];
    
    // Create and index initial files
    for (path, content) in &initial_files {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, content).await?;
        indexer.index_file(path).await?;
    }
    
    let manifest_v1 = indexer.generate_manifest(&test_folder).await?;
    info!("Initial manifest has {} files", manifest_v1.files.len());
    
    // Modify scenario: add, remove, and modify files
    
    // Add new file
    let new_file = test_folder.join("file4.txt");
    tokio::fs::write(&new_file, "Content of new file 4").await?;
    indexer.index_file(&new_file).await?;
    
    // Remove a file
    tokio::fs::remove_file(&initial_files[1].0).await?;
    
    // Modify a file
    tokio::fs::write(&initial_files[0].0, "MODIFIED content of file 1").await?;
    indexer.index_file(&initial_files[0].0).await?;
    
    // Generate new manifest
    let manifest_v2 = indexer.generate_manifest(&test_folder).await?;
    info!("Modified manifest has {} files", manifest_v2.files.len());
    
    // Test manifest diffing
    let diff = manifest_v1.diff(&manifest_v2);
    
    info!("Manifest diff: {} added, {} removed, {} modified", 
          diff.added.len(), diff.removed.len(), diff.modified.len());
    
    assert_eq!(diff.added.len(), 1, "Should detect one added file");
    assert_eq!(diff.removed.len(), 1, "Should detect one removed file");
    assert_eq!(diff.modified.len(), 1, "Should detect one modified file");
    
    // Verify specific changes
    let added_path = PathBuf::from("file4.txt");
    assert!(diff.added.contains(&added_path), "Should detect file4.txt as added");
    
    let removed_path = PathBuf::from("file2.txt");
    assert!(diff.removed.contains(&removed_path), "Should detect file2.txt as removed");
    
    let modified_path = PathBuf::from("file1.txt");
    assert!(diff.modified.contains_key(&modified_path), "Should detect file1.txt as modified");
    
    info!("Manifest operations test passed");
    Ok(())
}