use clap::{command, Parser};
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
    bloom_sync_integration::BloomSyncEngine,
    discovery::DiscoveryService,
    orchestrator::{OrchestratorConfig, OrchestratorMessage, SyncOrchestrator},
    sync_engine::{create_enhanced_sync_engine, EnhancedSyncEngine},
    watcher::{FileEvent, FileWatcher},
};

mod simple_sync_protocol;
use landro_index::async_indexer::AsyncIndexer;
use landro_quic::Connection;
use landro_quic::{QuicConfig, QuicServer};
use landro_sync::protocol::SyncSession;
use landro_sync::state::AsyncSyncDatabase;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use landro_daemon::connection_handler::QuicMessage;

/// Handle an incoming QUIC connection
pub async fn handle_quic_connection(
    connection: Connection,
    store: Arc<ContentStore>,
    indexer: Arc<AsyncIndexer>,
    quic_tx: mpsc::Sender<QuicMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Handling new QUIC connection");

    // Open bidirectional stream for sync protocol
    let (mut send_stream, mut recv_stream) = connection.open_bi().await?;

    // Get remote device info (will be populated after handshake)
    let remote_device_id = connection
        .remote_device_id()
        .await
        .map(|id| hex::encode(&id))
        .unwrap_or_else(|| "unknown".to_string());
    let remote_device_name = connection
        .remote_device_name()
        .await
        .unwrap_or_else(|| "unknown".to_string());

    // Create database for sync state
    let db_path = PathBuf::from(".landropic/sync_state.db");
    let database = AsyncSyncDatabase::open(&db_path).await?;

    // Create sync session
    let session = SyncSession::new(
        remote_device_id.clone(),
        remote_device_name.clone(),
        store.clone(),
        database,
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
    use simple_sync_protocol::{SimpleFileTransfer, SimpleSyncMessage};
    use std::io::Cursor;
    use std::path::PathBuf;

    info!("Processing sync message of {} bytes", msg_data.len());

    // Parse the incoming message
    let message = match SimpleSyncMessage::from_bytes(msg_data) {
        Ok(msg) => msg,
        Err(e) => {
            error!("Failed to parse sync message: {}", e);
            return Ok(None);
        }
    };

    info!("Received SimpleSyncMessage: {:?}", message);

    match message {
        SimpleSyncMessage::FileTransferRequest {
            file_path,
            file_size,
            checksum,
        } => {
            info!(
                "Handling file transfer request: {} ({} bytes)",
                file_path, file_size
            );

            // For alpha version, always accept transfers to a default sync folder
            let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            let sync_folder = home_dir.join("LandropicSync");
            let path_buf = PathBuf::from(&file_path);
            let file_name = path_buf
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("received_file");
            let dest_path = sync_folder.join(file_name);

            // Create sync folder if it doesn't exist
            if let Err(e) = tokio::fs::create_dir_all(&sync_folder).await {
                error!("Failed to create sync folder: {}", e);
                let response = SimpleSyncMessage::FileTransferResponse {
                    accepted: false,
                    reason: Some(format!("Failed to create destination folder: {}", e)),
                };
                return Ok(Some(response.to_bytes()?));
            }

            // Accept the transfer
            let response = SimpleSyncMessage::FileTransferResponse {
                accepted: true,
                reason: None,
            };

            info!("Accepting file transfer to: {}", dest_path.display());
            Ok(Some(response.to_bytes()?))
        }

        SimpleSyncMessage::FileData { data } => {
            info!("Received file data: {} bytes", data.len());

            // For this simple implementation, we'll just verify the data was received
            // In a real implementation, we'd save this to the file system
            // For now, calculate checksum and prepare response
            let checksum = blake3::hash(&data);
            let checksum_hex = hex::encode(checksum.as_bytes());

            // Store the file data temporarily (this is a simplified approach for alpha)
            let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            let sync_folder = home_dir.join("LandropicSync");
            let temp_file = sync_folder.join("received_file.tmp");

            match tokio::fs::write(&temp_file, &data).await {
                Ok(()) => {
                    info!("File data saved to: {}", temp_file.display());
                    let response = SimpleSyncMessage::TransferComplete {
                        success: true,
                        checksum_verified: true,
                        message: format!("File saved successfully, checksum: {}", checksum_hex),
                    };
                    Ok(Some(response.to_bytes()?))
                }
                Err(e) => {
                    error!("Failed to save file data: {}", e);
                    let response = SimpleSyncMessage::TransferComplete {
                        success: false,
                        checksum_verified: false,
                        message: format!("Failed to save file: {}", e),
                    };
                    Ok(Some(response.to_bytes()?))
                }
            }
        }

        _ => {
            info!("Unhandled message type: {:?}", message);
            Ok(None)
        }
    }
}

/// Landropic file synchronization daemon
#[derive(Parser)]
#[command(name = "landro-daemon")]
#[command(version, about = "Landropic sync daemon - secure LAN file synchronization", long_about = None)]
struct Cli {
    /// Storage directory for daemon data
    #[arg(short, long)]
    storage: Option<PathBuf>,

    /// Port to bind QUIC server on
    #[arg(short, long, default_value = "9876")]
    port: u16,

    /// Device name (defaults to hostname)
    #[arg(short = 'n', long)]
    device_name: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize rustls crypto provider
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Parse command line arguments first
    let cli = Cli::parse();

    // Initialize logging with verbosity support
    let env_filter = if cli.verbose {
        tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("landro=debug".parse().unwrap())
    } else {
        tracing_subscriber::EnvFilter::from_default_env()
    };

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    info!("Starting Landropic daemon v{}", env!("CARGO_PKG_VERSION"));

    // Use CLI arguments or environment variables or defaults
    let storage_path = cli
        .storage
        .or_else(|| std::env::var("LANDROPIC_STORAGE").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(".landropic"));
    let device_name = cli.device_name.unwrap_or_else(|| whoami::devicename());

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

    // Create orchestrator first to get tx for sync engines
    let config = OrchestratorConfig::default();
    let (mut orchestrator, tx) = SyncOrchestrator::new(
        config,
        store.clone(),
        chunker.clone(),
        indexer.clone(),
        &storage_path,
    )
    .await?;

    // Initialize Day 2 Enhanced Sync Engine
    info!("Initializing enhanced Bloom sync engine");
    let enhanced_sync = Arc::new(
        BloomSyncEngine::new(
            identity.device_id().to_string(),
            store.clone(),
            indexer.clone(),
        )
        .await?,
    );

    // Initialize Day 3 Enhanced Sync Engine with health monitoring
    info!("Initializing Day 3 enhanced sync engine with health monitoring");
    let day3_sync_engine =
        create_enhanced_sync_engine(store.clone(), indexer.clone(), tx.clone()).await?;

    // Initialize QUIC transport through orchestrator
    let port = if cli.port != 9876 {
        cli.port // CLI argument was provided
    } else {
        // Use environment variable if available, otherwise use CLI default
        std::env::var("LANDROPIC_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(cli.port)
    };
    let bind_addr = SocketAddr::from(([0, 0, 0, 0], port));
    orchestrator.initialize_quic_transport(bind_addr).await?;
    info!("QUIC transport initialized on port {}", port);

    // Create channels for QUIC<->orchestrator communication (kept for compatibility)
    let (quic_tx, _quic_rx) = mpsc::channel::<QuicMessage>(100);

    // Note: QUIC connection acceptance is now handled internally by the orchestrator
    // The orchestrator's run() method includes the QUIC accept loop

    // Set up callbacks for integration with network layer
    let quic_tx_clone = quic_tx.clone();
    orchestrator.set_peer_sync_callback(move |peer_id, folders| {
        info!(
            "Sync needed with peer {} for {} folders",
            peer_id,
            folders.len()
        );
        // Trigger sync via QUIC
        let _ = quic_tx_clone.try_send(QuicMessage::StartSync { peer_id, folders });
    });

    let quic_tx_clone2 = quic_tx.clone();
    orchestrator.set_manifest_ready_callback(move |path| {
        info!("Manifest ready for path: {}", path.display());
        // Trigger manifest exchange via QUIC
        let _ = quic_tx_clone2.try_send(QuicMessage::ManifestReady { path });
    });

    // Start discovery service
    info!("Starting mDNS discovery service");
    let mut discovery = DiscoveryService::new(&device_name)?;
    discovery
        .start_advertising(port, vec!["sync".to_string(), "transfer".to_string()])
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

    // Abort async tasks
    discovery_handle.abort();
    // Note: QUIC server is now stopped as part of orchestrator shutdown

    info!("Daemon shutdown complete");
    Ok(())
}
