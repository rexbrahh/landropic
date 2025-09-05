//! Example demonstrating the async-safe usage pattern for FileIndexer
//!
//! This example shows how to properly use the FileIndexer with async operations
//! without encountering the !Send constraint issues from rusqlite::Connection.

use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::fs;

use landro_index::{
    AsyncIndexer, AsyncIndexerBuilder, FolderWatcher, IndexerConfig, WatcherConfig,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Create temporary directories for testing
    let temp_dir = tempdir()?;
    let cas_dir = temp_dir.path().join("cas");
    let db_path = temp_dir.path().join("index.db");
    let watch_dir = temp_dir.path().join("watched");
    fs::create_dir_all(&watch_dir).await?;

    // Method 1: Using AsyncIndexer (recommended for most use cases)
    println!("=== Using AsyncIndexer (Recommended) ===");
    demo_async_indexer(&cas_dir, &db_path, &watch_dir).await?;

    // Method 2: Using AsyncIndexerBuilder for more control
    println!("\n=== Using AsyncIndexerBuilder ===");
    demo_async_indexer_builder(&cas_dir, &db_path, &watch_dir).await?;

    // Method 3: Integration with FolderWatcher
    println!("\n=== Using with FolderWatcher ===");
    demo_with_watcher(&cas_dir, &db_path, &watch_dir).await?;

    Ok(())
}

/// Demonstrates basic AsyncIndexer usage
async fn demo_async_indexer(
    cas_dir: &Path,
    db_path: &Path,
    watch_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create the async indexer
    let indexer = AsyncIndexer::new(cas_dir, db_path, IndexerConfig::default()).await?;

    // Create some test files
    let file1 = watch_dir.join("test1.txt");
    let file2 = watch_dir.join("test2.txt");
    fs::write(&file1, b"Hello from async indexer!").await?;
    fs::write(&file2, b"This is properly thread-safe!").await?;

    // Index the folder
    let manifest = indexer.index_folder(watch_dir).await?;
    println!("Indexed {} files", manifest.file_count());
    println!("Total size: {} bytes", manifest.total_size());

    // Get storage statistics
    let stats = indexer.storage_stats().await?;
    println!("Storage objects: {}", stats.total_objects);

    // Verify integrity
    let report = indexer.verify_integrity().await?;
    println!(
        "Integrity check: {}",
        if report.is_healthy() { "✓" } else { "✗" }
    );

    Ok(())
}

/// Demonstrates using the builder pattern
async fn demo_async_indexer_builder(
    cas_dir: &Path,
    db_path: &Path,
    watch_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // Configure indexer with builder
    let config = IndexerConfig {
        follow_symlinks: false,
        ignore_patterns: vec!["*.tmp".to_string(), ".DS_Store".to_string()],
        ..Default::default()
    };

    let indexer = AsyncIndexerBuilder::new()
        .cas_path(cas_dir)
        .database_path(db_path)
        .config(config)
        .build()
        .await?;

    // Create a test file
    let test_file = watch_dir.join("builder_test.txt");
    fs::write(&test_file, b"Created with builder pattern").await?;

    // Index and report
    let manifest = indexer.index_folder(watch_dir).await?;
    println!("Builder indexer found {} files", manifest.file_count());

    Ok(())
}

/// Demonstrates integration with FolderWatcher for real-time monitoring
async fn demo_with_watcher(
    cas_dir: &Path,
    db_path: &Path,
    watch_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create the async indexer
    let indexer = AsyncIndexer::new(cas_dir, db_path, IndexerConfig::default()).await?;

    // Configure the watcher
    let watcher_config = WatcherConfig {
        debounce_delay: std::time::Duration::from_millis(500),
        recursive: true,
        ..Default::default()
    };

    // Create the folder watcher
    let mut watcher = FolderWatcher::new(watcher_config)?;

    // Start watching the folder
    watcher
        .watch_folder(watch_dir, "demo-folder".to_string())
        .await?;
    println!("Started watching: {:?}", watch_dir);

    // Create a file to trigger the watcher
    let trigger_file = watch_dir.join("trigger.txt");
    fs::write(&trigger_file, b"This should trigger re-indexing").await?;

    // In a real application, you would use:
    // ```
    // let watcher = Arc::new(watcher);
    // let indexer_inner = indexer.inner();
    // watcher.start_with_indexer(indexer_inner).await?;
    // ```

    // For this demo, we'll just show that the setup works
    println!("Watcher is ready for real-time indexing");

    // Clean up
    watcher.unwatch_folder(watch_dir).await?;
    println!("Stopped watching folder");

    Ok(())
}

/// Demonstrates concurrent access patterns
#[allow(dead_code)]
async fn demo_concurrent_access(
    indexer: AsyncIndexer,
    folders: Vec<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    use futures::future::join_all;

    // Clone the indexer for concurrent use
    let handles: Vec<_> = folders
        .into_iter()
        .map(|folder| {
            let indexer_clone = indexer.clone();
            let folder = folder.to_path_buf();
            tokio::spawn(async move { indexer_clone.index_folder(folder).await })
        })
        .collect();

    // Wait for all indexing operations to complete
    let results = join_all(handles).await;

    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(Ok(manifest)) => {
                println!("Folder {} indexed: {} files", i, manifest.file_count());
            }
            Ok(Err(e)) => eprintln!("Folder {} failed: {}", i, e),
            Err(e) => eprintln!("Task {} panicked: {}", i, e),
        }
    }

    Ok(())
}
