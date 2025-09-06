use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber;

use landro_cas::ContentStore;
use landro_chunker::{Chunker, ChunkerConfig};
use landro_crypto::{CertificateVerifier, DeviceIdentity};
use landro_daemon::{
    discovery::DiscoveryService,
    orchestrator::{OrchestratorConfig, OrchestratorMessage, SyncOrchestrator},
    watcher::{FileEvent, FileWatcher},
};
use landro_index::async_indexer::AsyncIndexer;
use landro_quic::{QuicConfig, QuicServer};
use landro_sync::protocol::{SyncSession, SessionState};
use landro_quic::Connection;

/// Messages for communication between QUIC and orchestrator
#[derive(Debug)]
enum QuicMessage {
    StartSync {
        peer_id: String,
        folders: Vec<PathBuf>,
    },
    ManifestReady {
        path: PathBuf,
    },
    ChunkRequest {
        peer_id: String,
        chunk_hash: String,
    },
}

/// Handle an incoming QUIC connection
async fn handle_quic_connection(
    connection: Connection,
    store: Arc<ContentStore>,
    indexer: Arc<AsyncIndexer>,
    quic_tx: mpsc::Sender<QuicMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Handling new QUIC connection");
    
    // Open bidirectional stream for sync protocol
    let (mut send_stream, mut recv_stream) = connection.open_bi().await?;
    
    // Create sync session
    let session = SyncSession::new(
        connection.remote_device_id().unwrap_or_default(),
        store.clone(),
        indexer.clone(),
    );
    
    // Handle sync protocol messages
    loop {
        // Read message from stream
        let mut len_buf = [0u8; 4];
        if recv_stream.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        
        let msg_len = u32::from_be_bytes(len_buf) as usize;
        let mut msg_buf = vec![0u8; msg_len];
        if recv_stream.read_exact(&mut msg_buf).await.is_err() {
            break;
        }
        
        // Process message based on type
        match process_sync_message(&session, &msg_buf, &store).await {
            Ok(response) => {
                // Send response if any
                if let Some(resp_data) = response {
                    let len_bytes = (resp_data.len() as u32).to_be_bytes();
                    send_stream.write_all(&len_bytes).await?;
                    send_stream.write_all(&resp_data).await?;
                }
            }
            Err(e) => {
                error!("Error processing sync message: {}", e);
                break;
            }
        }
    }
    
    info!("QUIC connection handler completed");
    Ok(())
}

/// Process a sync protocol message
async fn process_sync_message(
    session: &SyncSession,
    msg_data: &[u8],
    store: &Arc<ContentStore>,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
    // This would decode the protobuf message and handle it
    // For now, return a placeholder
    Ok(None)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting Landropic daemon");

    // Parse command line arguments (simplified for now)
    let storage_path = PathBuf::from(
        std::env::var("LANDROPIC_STORAGE").unwrap_or_else(|_| ".landropic".to_string()),
    );
    let device_name = whoami::devicename();

    // Create storage directories
    tokio::fs::create_dir_all(&storage_path).await?;
    let cas_path = storage_path.join("objects");
    let db_path = storage_path.join("index.sqlite");
    let identity_path = storage_path.join("identity");

    // Load or generate device identity
    info!("Loading device identity");
    let identity = Arc::new(DeviceIdentity::load_or_generate(&device_name).await?);
    info!("Device ID: {}", identity.device_id());
    
    // Create certificate verifier for peer connections
    // In production, this would verify against trusted device certificates
    let verifier = Arc::new(CertificateVerifier::for_pairing());

    // Initialize core components
    info!("Initializing content store at {:?}", cas_path);
    let store = Arc::new(ContentStore::new(&cas_path).await?);

    info!("Initializing chunker");
    let chunker_config = ChunkerConfig {
        min_size: 16 * 1024,  // 16KB minimum
        avg_size: 64 * 1024,  // 64KB average
        max_size: 256 * 1024, // 256KB maximum
        mask_bits: 16,        // For 64KB average (2^16)
    };
    let chunker = Arc::new(Chunker::new(chunker_config)?);

    info!("Initializing indexer");
    let indexer = Arc::new(AsyncIndexer::new(&cas_path, &db_path, Default::default()).await?);

    // Create orchestrator
    let config = OrchestratorConfig::default();
    let (mut orchestrator, tx) =
        SyncOrchestrator::new(config, store.clone(), chunker.clone(), indexer.clone(), &storage_path).await?;

    // Initialize QUIC server with proper configuration
    let quic_config = QuicConfig {
        bind_addr: SocketAddr::from(([0, 0, 0, 0], 9876)),
        idle_timeout: std::time::Duration::from_secs(60),
        keep_alive_interval: std::time::Duration::from_secs(15),
        max_concurrent_streams: 100,
        ..QuicConfig::default()
    };
    
    let mut quic_server = QuicServer::new(identity.clone(), verifier.clone(), quic_config);
    quic_server.start().await?;
    info!("QUIC server listening on port 9876");
    
    // Create channels for QUIC<->orchestrator communication
    let (quic_tx, mut quic_rx) = mpsc::channel::<QuicMessage>(100);
    let tx_for_quic = tx.clone();
    
    // Spawn QUIC connection handler
    let quic_server_arc = Arc::new(tokio::sync::RwLock::new(quic_server));
    let quic_handle = tokio::spawn({
        let quic_server = quic_server_arc.clone();
        let store = store.clone();
        let indexer = indexer.clone();
        async move {
            loop {
                match quic_server.read().await.accept().await {
                    Ok(connection) => {
                        info!("New QUIC connection accepted");
                        // Handle connection in separate task
                        tokio::spawn(handle_quic_connection(
                            connection,
                            store.clone(),
                            indexer.clone(),
                            quic_tx.clone(),
                        ));
                    }
                    Err(e) => {
                        error!("Failed to accept QUIC connection: {}", e);
                        break;
                    }
                }
            }
        }
    });

    // Set up callbacks for integration with network layer
    let quic_tx_clone = quic_tx.clone();
    orchestrator.set_peer_sync_callback(move |peer_id, folders| {
        info!(
            "Sync needed with peer {} for {} folders",
            peer_id,
            folders.len()
        );
        // Trigger sync via QUIC
        let _ = quic_tx_clone.try_send(QuicMessage::StartSync {
            peer_id,
            folders,
        });
    });

    let quic_tx_clone2 = quic_tx.clone();
    orchestrator.set_manifest_ready_callback(move |path| {
        info!("Manifest ready for path: {}", path.display());
        // Trigger manifest exchange via QUIC
        let _ = quic_tx_clone2.try_send(QuicMessage::ManifestReady {
            path,
        });
    });

    // Start discovery service
    info!("Starting mDNS discovery service");
    let mut discovery = DiscoveryService::new(&device_name)?;
    discovery
        .start_advertising(9876, vec!["sync".to_string(), "transfer".to_string()])
        .await?;

    // Monitor peer discovery
    let tx_discovery = tx.clone();
    let discovery_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            match discovery.browse_peers().await {
                Ok(peers) => {
                    for peer in peers {
                        let _ = tx_discovery
                            .send(OrchestratorMessage::PeerDiscovered(peer))
                            .await;
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
    tx.send(OrchestratorMessage::AddSyncFolder(sync_folder.clone()))
        .await?;

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

    // Stop QUIC server
    {
        let mut server = quic_server_arc.write().await;
        server.stop();
    }
    
    // Abort async tasks
    discovery_handle.abort();
    quic_handle.abort();

    info!("Daemon shutdown complete");
    Ok(())
}
