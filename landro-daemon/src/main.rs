use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber;

use landro_cas::ContentStore;
use landro_chunker::{Chunker, ChunkerConfig};
use landro_daemon::{
    orchestrator::{OrchestratorConfig, OrchestratorMessage, SyncOrchestrator},
    discovery::DiscoveryService,
    watcher::{FileWatcher, FileEvent},
};
use landro_index::async_indexer::AsyncIndexer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting Landropic daemon");

    // Parse command line arguments (simplified for now)
    let storage_path = PathBuf::from(
        std::env::var("LANDROPIC_STORAGE").unwrap_or_else(|_| ".landropic".to_string())
    );
    let device_name = whoami::devicename();

    // Create storage directories
    tokio::fs::create_dir_all(&storage_path).await?;
    let cas_path = storage_path.join("objects");
    let db_path = storage_path.join("index.sqlite");

    // Initialize core components
    info!("Initializing content store at {:?}", cas_path);
    let store = Arc::new(ContentStore::new(&cas_path).await?);
    
    info!("Initializing chunker");
    let chunker_config = ChunkerConfig {
        min_size: 16 * 1024,      // 16KB minimum
        avg_size: 64 * 1024,      // 64KB average  
        max_size: 256 * 1024,     // 256KB maximum
        mask_bits: 16,            // For 64KB average (2^16)
    };
    let chunker = Arc::new(Chunker::new(chunker_config)?); 
    
    info!("Initializing indexer");
    let indexer = Arc::new(
        AsyncIndexer::new(
            &cas_path,
            &db_path,
            Default::default(),
        ).await?
    );

    // Create orchestrator
    let config = OrchestratorConfig::default();
    let (mut orchestrator, tx) = SyncOrchestrator::new(
        config,
        store,
        chunker,
        indexer,
        &storage_path,
    ).await?;

    // Set up callbacks for integration with network layer
    let tx_clone = tx.clone();
    orchestrator.set_peer_sync_callback(move |peer_id, folders| {
        info!("Sync needed with peer {} for {} folders", peer_id, folders.len());
        // In a real implementation, this would trigger network sync
    });

    orchestrator.set_manifest_ready_callback(move |path| {
        info!("Manifest ready for path: {}", path.display());
        // In a real implementation, this would trigger manifest exchange
    });

    // Start discovery service
    info!("Starting mDNS discovery service");
    let mut discovery = DiscoveryService::new(&device_name)?;
    discovery.start_advertising(9876, vec!["sync".to_string(), "transfer".to_string()]).await?;

    // Monitor peer discovery
    let tx_discovery = tx.clone();
    let discovery_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            match discovery.browse_peers().await {
                Ok(peers) => {
                    for peer in peers {
                        let _ = tx_discovery.send(OrchestratorMessage::PeerDiscovered(peer)).await;
                    }
                }
                Err(e) => {
                    error!("Failed to browse peers: {}", e);
                }
            }
        }
    });

    // Set up file watchers for default sync folders
    let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let sync_folder = home_dir.join("LandropicSync");
    
    // Create default sync folder if it doesn't exist
    if !sync_folder.exists() {
        info!("Creating default sync folder at {:?}", sync_folder);
        tokio::fs::create_dir_all(&sync_folder).await?;
    }

    // Start file watcher
    let tx_watcher = tx.clone();
    let watcher = FileWatcher::new(sync_folder.clone())?;
    watcher.start(move |events: Vec<FileEvent>| {
        let tx = tx_watcher.clone();
        tokio::spawn(async move {
            let _ = tx.send(OrchestratorMessage::FileChangesBatch(events)).await;
        });
    })?;

    // Add the sync folder to orchestrator
    tx.send(OrchestratorMessage::AddSyncFolder(sync_folder.clone())).await?;

    // Spawn the orchestrator
    let orchestrator_handle = tokio::spawn(async move {
        if let Err(e) = orchestrator.run().await {
            error!("Orchestrator error: {}", e);
        }
    });

    // Wait for shutdown signal
    info!("Daemon running. Press Ctrl+C to stop.");
    signal::ctrl_c().await?;

    // Graceful shutdown
    info!("Shutting down daemon...");
    
    // Stop file watcher
    watcher.stop()?;
    
    // Send shutdown message to orchestrator
    tx.send(OrchestratorMessage::Shutdown).await?;
    
    // Wait for orchestrator to finish
    orchestrator_handle.await?;
    
    // Abort discovery task
    discovery_handle.abort();

    info!("Daemon shutdown complete");
    Ok(())
}