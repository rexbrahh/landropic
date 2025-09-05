use landro_cas::{ContentStore, ContentStoreConfig, FsyncPolicy, CompressionType};
use tokio;

/// Demonstrates atomic write capabilities and recovery features
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure the content store with different fsync policies
    let configs = vec![
        ("Always", ContentStoreConfig {
            fsync_policy: FsyncPolicy::Always,
            enable_recovery: true,
            compression: CompressionType::None,
        }),
        ("Batch", ContentStoreConfig {
            fsync_policy: FsyncPolicy::Batch(5),
            enable_recovery: true,
            compression: CompressionType::None,
        }),
        ("Never", ContentStoreConfig {
            fsync_policy: FsyncPolicy::Never,
            enable_recovery: true,
            compression: CompressionType::None,
        }),
    ];

    for (name, config) in configs {
        println!("\n=== Testing {} fsync policy ===", name);
        
        // Create temp directory for this test
        let temp_dir = tempfile::tempdir()?;
        let store = ContentStore::new_with_config(temp_dir.path(), config).await?;
        
        // Write some test data
        let test_data = format!("Test data for {} policy", name);
        let obj_ref = store.write(test_data.as_bytes()).await?;
        
        println!("Wrote object: {} ({} bytes)", obj_ref.hash, obj_ref.size);
        
        // Read it back
        let read_data = store.read(&obj_ref.hash).await?;
        assert_eq!(read_data.as_ref(), test_data.as_bytes());
        
        println!("Successfully verified object integrity");
        
        // Test recovery capabilities
        let stats = store.recover().await?;
        println!("Recovery stats: recovered={}, cleaned={}, errors={}",
                stats.recovered, stats.cleaned, stats.errors.len());
                
        // Get storage stats
        let storage_stats = store.stats().await?;
        println!("Storage stats: {} objects, {} bytes total",
                storage_stats.object_count, storage_stats.total_size);
    }

    println!("\n=== All atomic write tests completed successfully! ===");
    Ok(())
}