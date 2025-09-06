use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn, error, debug};

use landro_quic::QuicServer;
use landro_sync::{SyncOrchestrator, SyncConfig};
use landro_index::async_indexer::AsyncIndexer;
use crate::discovery::DiscoveryService;
// use crate::network::{ConnectionManager, NetworkConfig};
use crate::watcher::{FileWatcher, FileEventKind};

/// Configuration for the daemon
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// QUIC server bind address
    pub bind_addr: SocketAddr,
    /// Storage directory for CAS and database
    pub storage_path: PathBuf,
    /// Device name for mDNS
    pub device_name: String,
    /// Enable auto-sync on file changes
    pub auto_sync: bool,
    /// Maximum concurrent transfers
    pub max_concurrent_transfers: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind_addr: "[::]:9876".parse().unwrap(),
            storage_path: PathBuf::from(".landropic"),
            device_name: whoami::devicename(),
            auto_sync: true,
            max_concurrent_transfers: 4,
        }
    }
}

/// State of all daemon subsystems
struct DaemonState {
    quic_server: Option<Arc<QuicServer>>,
    discovery_service: Option<Arc<Mutex<DiscoveryService>>>,
    // connection_manager: Option<Arc<ConnectionManager>>,
    sync_orchestrator: Option<Arc<SyncOrchestrator>>,
    file_watchers: Vec<FileWatcher>,
    indexer: Option<Arc<AsyncIndexer>>,
}

/// Main daemon orchestrator
pub struct Daemon {
    config: DaemonConfig,
    running: Arc<RwLock<bool>>,
    state: Arc<RwLock<DaemonState>>,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Daemon {
    /// Create a new daemon instance
    pub fn new(config: DaemonConfig) -> Self {
        Self {
            config,
            running: Arc::new(RwLock::new(false)),
            state: Arc::new(RwLock::new(DaemonState {
                quic_server: None,
                discovery_service: None,
                // connection_manager: None,
                sync_orchestrator: None,
                file_watchers: Vec::new(),
                indexer: None,
            })),
            tasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start the daemon and all subsystems
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut running = self.running.write().await;
        if *running {
            return Err("Daemon already running".into());
        }

        info!("Starting Landropic daemon");
        
        // Create storage directories
        tokio::fs::create_dir_all(&self.config.storage_path).await?;
        let _cas_path = self.config.storage_path.join("objects");
        let db_path = self.config.storage_path.join("index.sqlite");
        
        // Initialize database and indexer
        let cas_path = self.config.storage_path.join("objects");
        let indexer_config = landro_index::IndexerConfig::default();
        let indexer = AsyncIndexer::new(&cas_path, &db_path, indexer_config).await?;
        let indexer = Arc::new(indexer);
        
        // Initialize sync orchestrator
        let sync_config = SyncConfig {
            max_concurrent_transfers: self.config.max_concurrent_transfers,
            database_path: self.config.storage_path.join("sync.db").to_string_lossy().to_string(),
            auto_sync: self.config.auto_sync,
            ..Default::default()
        };
        let sync_orchestrator = Arc::new(SyncOrchestrator::new(sync_config).await?);
        
        // Start QUIC server
        let quic_server = self.start_quic_server().await?;
        
        // Start mDNS discovery
        let port = quic_server.local_addr().map(|addr| addr.port()).unwrap_or(9876);
        let discovery_service = self.start_discovery_service(port).await?;
        
        // Create connection manager
        // TODO: Re-enable when network module is ready
        // let identity = Arc::new(landro_crypto::DeviceIdentity::generate(&self.config.device_name).map_err(|e| e.to_string())?);
        // let verifier = Arc::new(landro_crypto::CertificateVerifier::for_pairing());
        // let network_config = NetworkConfig::default();
        // let connection_manager = Arc::new(ConnectionManager::new(
        //     identity,
        //     verifier,
        //     discovery_service.clone(),
        //     network_config,
        // ));
        
        // Start connection manager
        // connection_manager.start().await?;
        
        // Update state
        {
            let mut state = self.state.write().await;
            state.quic_server = Some(quic_server.clone());
            state.discovery_service = Some(discovery_service);
            // state.connection_manager = Some(connection_manager.clone());
            state.sync_orchestrator = Some(sync_orchestrator.clone());
            state.indexer = Some(indexer.clone());
        }
        
        // Start background tasks
        self.start_background_tasks(quic_server, sync_orchestrator).await?;
        
        *running = true;
        info!("Landropic daemon started successfully");
        
        Ok(())
    }

    /// Start QUIC server
    async fn start_quic_server(&self) -> Result<Arc<QuicServer>, String> {
        info!("Starting QUIC server on {}", self.config.bind_addr);
        
        // Create device identity
        let identity = Arc::new(landro_crypto::DeviceIdentity::generate(&self.config.device_name).map_err(|e| e.to_string())?);
        
        // Create certificate verifier for pairing/testing
        let verifier = Arc::new(landro_crypto::CertificateVerifier::for_pairing());
        
        // Create QUIC configuration
        let quic_config = landro_quic::QuicConfig::default().bind_addr(self.config.bind_addr);

        let mut server = QuicServer::new(identity, verifier, quic_config);
        
        // Start the server
        server.start().await.map_err(|e| e.to_string())?;
        
        // Start listening
        let server = Arc::new(server);
        let server_clone = server.clone();
        
        let task = tokio::spawn(async move {
            if let Err(e) = server_clone.run().await {
                error!("QUIC server error: {}", e);
            }
        });
        
        self.tasks.lock().await.push(task);
        
        Ok(server)
    }

    /// Start mDNS discovery service
    async fn start_discovery_service(&self, port: u16) -> Result<Arc<Mutex<DiscoveryService>>, String> {
        info!("Starting mDNS discovery service");
        
        let mut discovery = DiscoveryService::new(&self.config.device_name).map_err(|e| e.to_string())?;
        
        // Start advertising with our capabilities
        discovery.start_advertising(
            port,
            vec![
                "sync".to_string(),
                "transfer".to_string(),
                "v0.1.0".to_string(),
            ],
        ).await.map_err(|e| e.to_string())?;
        
        Ok(Arc::new(Mutex::new(discovery)))
    }

    /// Start background tasks
    async fn start_background_tasks(
        &self,
        _quic_server: Arc<QuicServer>,
        _sync_orchestrator: Arc<SyncOrchestrator>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Re-enable when connection manager is ready
        // // Periodic sync with discovered peers using ConnectionManager
        // let connection_manager = self.state.read().await.connection_manager.clone().unwrap();
        // let sync_orch = sync_orchestrator.clone();
        // 
        // let task = tokio::spawn(async move {
        //     let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        //     loop {
        //         interval.tick().await;
        //         
        //         // Get connection statistics and trigger sync for healthy peers
        //         let stats = connection_manager.get_stats().await;
        //         debug!("Connection stats: {} total peers, {} healthy", 
        //                stats.total_peers, stats.healthy_peers);
        //         
        //         for peer_stat in stats.peer_stats {
        //             if peer_stat.is_healthy {
        //                 // Attempt to sync with healthy peer
        //                 match connection_manager.get_connection(&peer_stat.peer_id).await {
        //                     Ok(_conn) => {
        //                         if let Err(e) = sync_orch.start_sync(
        //                             peer_stat.peer_id.clone(),
        //                             peer_stat.peer_name.clone(),
        //                         ).await {
        //                             warn!("Failed to start sync with peer {}: {}", 
        //                                   peer_stat.peer_id, e);
        //                         }
        //                     }
        //                     Err(e) => {
        //                         warn!("Failed to get connection for peer {}: {}", 
        //                               peer_stat.peer_id, e);
        //                     }
        //                 }
        //             }
        //         }
        //     }
        // });
        // 
        // self.tasks.lock().await.push(task);
        
        // TODO: Re-enable connection handler when ready
        // // Connection handler
        // let sync_orch = sync_orchestrator.clone();
        // let task = tokio::spawn(async move {
        //     loop {
        //         // Accept incoming connections
        //         match quic_server.accept().await {
        //             Ok(connection) => {
        //                 let sync_orch = sync_orch.clone();
        //                 tokio::spawn(async move {
        //                     if let Err(e) = handle_connection(connection, sync_orch).await {
        //                         error!("Connection handling error: {}", e);
        //                     }
        //                 });
        //             }
        //             Err(e) => {
        //                 error!("Failed to accept connection: {}", e);
        //                 break;
        //             }
        //         }
        //     }
        // });
        // 
        // self.tasks.lock().await.push(task);
        
        Ok(())
    }

    /// Add a folder to watch and sync
    pub async fn add_sync_folder(&self, path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let path = path.as_ref().to_path_buf();
        let path_for_indexing = path.clone();
        
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()).into());
        }
        
        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()).into());
        }
        
        info!("Adding sync folder: {}", path.display());
        
        let mut state = self.state.write().await;
        
        // Create file watcher
        let watcher = FileWatcher::new(path.clone())?;
        
        // Start watching with complete sync pipeline
        let indexer = state.indexer.clone().unwrap();
        let sync_orch = state.sync_orchestrator.clone().unwrap();
        let discovery = state.discovery_service.clone().unwrap();
        let auto_sync = self.config.auto_sync;
        
        watcher.start(move |events| {
            let indexer = indexer.clone();
            let sync_orch = sync_orch.clone();
            let discovery = discovery.clone();
            let _path = path.clone();
            
            tokio::spawn(async move {
                for event in events {
                    debug!("File event: {:?}", event);
                    
                    // Step 1: Update index for the affected file/directory
                    match &event.kind {
                        FileEventKind::Created | FileEventKind::Modified => {
                            if let Err(e) = indexer.index_folder(&event.path).await {
                                error!("Failed to index path {}: {}", event.path.display(), e);
                                continue;
                            }
                            info!("Indexed file change: {}", event.path.display());
                        }
                        FileEventKind::Deleted => {
                            // For deleted files, we should update the manifest to reflect the deletion
                            debug!("File deleted: {}", event.path.display());
                            // TODO: Implement deletion in manifest
                        }
                        FileEventKind::Renamed { from, to } => {
                            debug!("File renamed: {} -> {}", from.display(), to.display());
                            if let Err(e) = indexer.index_folder(to).await {
                                error!("Failed to index renamed file {}: {}", to.display(), e);
                                continue;
                            }
                        }
                    }
                    
                    // Step 2: Trigger sync with discovered peers if auto-sync is enabled
                    if auto_sync {
                        debug!("Auto-sync triggered for path: {}", event.path.display());
                        
                        // Get current peers from discovery
                        if let Ok(peers) = discovery.lock().await.browse_peers().await {
                            for peer in peers {
                                // Start sync with this peer for the changed files
                                if let Err(e) = sync_orch.start_sync(
                                    peer.device_id.clone(),
                                    peer.device_name.clone(),
                                ).await {
                                    warn!("Failed to start sync with peer {} after file change: {}", 
                                        peer.device_name, e);
                                } else {
                                    info!("Started sync with peer {} for file change", peer.device_name);
                                }
                            }
                        } else {
                            debug!("No peers available for auto-sync");
                        }
                    }
                }
            });
        })?;
        
        state.file_watchers.push(watcher);
        
        // Index the folder immediately
        state.indexer.as_ref().unwrap().index_folder(&path_for_indexing).await?;
        
        Ok(())
    }

    /// Remove a folder from sync
    pub async fn remove_sync_folder(&self, path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let path = path.as_ref();
        info!("Removing sync folder: {}", path.display());
        
        let mut state = self.state.write().await;
        
        // Remove watcher for this path
        state.file_watchers.retain(|w| w.path() != path);
        
        Ok(())
    }

    /// Get list of synced folders
    pub async fn list_sync_folders(&self) -> Vec<PathBuf> {
        let state = self.state.read().await;
        state.file_watchers.iter().map(|w| w.path().to_path_buf()).collect()
    }

    /// Stop the daemon
    pub async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut running = self.running.write().await;
        if !*running {
            return Err("Daemon not running".into());
        }

        info!("Stopping Landropic daemon");
        
        // Stop discovery service
        if let Some(discovery) = &self.state.read().await.discovery_service {
            discovery.lock().await.stop().await?;
        }
        
        // Stop file watchers
        {
            let mut state = self.state.write().await;
            for watcher in &state.file_watchers {
                watcher.stop()?;
            }
            state.file_watchers.clear();
        }
        
        // Cancel background tasks
        for task in self.tasks.lock().await.drain(..) {
            task.abort();
        }
        
        // Clear state
        {
            let mut state = self.state.write().await;
            state.quic_server = None;
            state.discovery_service = None;
            state.sync_orchestrator = None;
            state.indexer = None;
        }
        
        *running = false;
        info!("Landropic daemon stopped");
        
        Ok(())
    }

    /// Check if daemon is running
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
    
    /// Get daemon status information
    pub async fn get_status(&self) -> DaemonStatus {
        let running = self.is_running().await;
        let state = self.state.read().await;
        
        let peer_count = if let Some(sync_orch) = &state.sync_orchestrator {
            sync_orch.get_peer_states().await.len()
        } else {
            0
        };
        
        DaemonStatus {
            running,
            device_name: self.config.device_name.clone(),
            bind_addr: self.config.bind_addr,
            peer_count,
            sync_folders: state.file_watchers.len(),
        }
    }
}

/// Daemon status information
#[derive(Debug, Clone)]
pub struct DaemonStatus {
    pub running: bool,
    pub device_name: String,
    pub bind_addr: SocketAddr,
    pub peer_count: usize,
    pub sync_folders: usize,
}

/// Handle an incoming QUIC connection
// TODO: Implement proper connection handling when sync protocol is ready
#[allow(dead_code)]
async fn handle_connection(
    _connection: landro_quic::Connection,
    _sync_orchestrator: Arc<SyncOrchestrator>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // TODO: Implement when sync protocol is ready
    Ok(())
    
    /* 
    // Load device identity for handshake
    let identity = landro_crypto::DeviceIdentity::load(None).await?;
    
    // Perform server-side handshake
    connection.server_handshake(
        &identity.device_id().0,
        &whoami::devicename(),
    ).await?;
    
    // Get remote device info
    let device_id = connection.remote_device_id().await
        .ok_or("No remote device ID")?;
    let device_name = connection.remote_device_name().await
        .ok_or("No remote device name")?;
    
    info!("Connection from {} ({})", device_name, hex::encode(&device_id[..8]));
    
    // Start sync with this peer
    let peer_id_str = hex::encode(&device_id);
    sync_orchestrator.start_sync(
        peer_id_str.clone(),
        device_name.clone(),
    ).await?;
    
    // Handle sync protocol streams
    loop {
        match connection.accept_bi().await {
            Ok((send, recv)) => {
                debug!("Accepted bidirectional stream for sync protocol");
                
                // Handle the sync protocol stream
                let sync_orch = sync_orchestrator.clone();
                let peer_id = peer_id_str.clone();
                
                tokio::spawn(async move {
                    if let Err(e) = handle_sync_stream(
                        send,
                        recv,
                        peer_id,
                        sync_orch,
                    ).await {
                        error!("Sync stream handling error: {}", e);
                    }
                });
            }
            Err(e) => {
                debug!("Connection closed: {}", e);
                break;
            }
        }
    }
    
    // Complete sync when connection closes
    if let Err(e) = sync_orchestrator.complete_sync(&peer_id_str).await {
        warn!("Failed to complete sync with peer {}: {}", peer_id_str, e);
    }
    
    Ok(())
    */
}

/// Handle a sync protocol stream
#[allow(dead_code)]
async fn handle_sync_stream(
    mut _send: quinn::SendStream,
    mut _recv: quinn::RecvStream,
    _peer_id: String,
    _sync_orchestrator: Arc<SyncOrchestrator>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // TODO: Implement when sync protocol is ready
    Ok(())
    
    /*
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    
    // Read stream type/message type
    let mut msg_type = [0u8; 1];
    recv.read_exact(&mut msg_type).await?;
    
    match msg_type[0] {
        0 => {
            // Manifest exchange request
            debug!("Handling manifest exchange for peer: {}", peer_id);
            
            // TODO: Implement proper manifest exchange
            // This should:
            // 1. Read the peer's manifest from the stream
            // 2. Load our local manifest from the indexer
            // 3. Compare manifests to find differences (missing/updated files)
            // 4. Schedule transfers for the differences through sync orchestrator
            // 5. Send back our manifest or diff response
            
            // For now, simulate manifest exchange
            let response = serde_json::json!({
                "type": "manifest_response",
                "status": "success",
                "message": "Manifest exchange completed - would schedule actual transfers here"
            }).to_string();
            
            send.write_all(response.as_bytes()).await?;
            send.finish()?;
            
            info!("Completed manifest exchange with peer: {} (simulated)", peer_id);
        }
        1 => {
            // File transfer request  
            debug!("Handling file transfer request for peer: {}", peer_id);
            
            // Read transfer metadata (file path, chunk hashes, etc.)
            let mut metadata_len = [0u8; 4];
            recv.read_exact(&mut metadata_len).await?;
            let len = u32::from_be_bytes(metadata_len) as usize;
            
            let mut metadata = vec![0u8; len];
            recv.read_exact(&mut metadata).await?;
            
            // Parse metadata (in a real implementation, this would be protobuf)
            let metadata_str = String::from_utf8(metadata)?;
            debug!("Received transfer metadata: {}", metadata_str);
            
            // TODO: Use QuicTransferEngine for actual transfer
            // This should:
            // 1. Parse transfer request (file path, chunks needed, etc.)
            // 2. Create QuicTransferEngine for this connection
            // 3. Start resumable transfer with the engine
            // 4. Handle chunk transfers and progress updates
            
            // For now, acknowledge the transfer request
            let response = serde_json::json!({
                "type": "transfer_response", 
                "status": "accepted",
                "message": "Transfer request accepted - would use QuicTransferEngine here"
            }).to_string();
            
            send.write_all(response.as_bytes()).await?;
            send.finish()?;
            
            debug!("Completed file transfer handling for peer: {} (simulated)", peer_id);
        }
        2 => {
            // Chunk data transfer
            debug!("Handling chunk data transfer for peer: {}", peer_id);
            
            // This would handle actual chunk content transfer
            // using the QuicTransferEngine and resumable transfer protocols
            
            let response = b"chunk_ack";
            send.write_all(response).await?;
            send.finish()?;
            
            debug!("Completed chunk transfer for peer: {}", peer_id);
        }
        _ => {
            warn!("Unknown message type: {}", msg_type[0]);
            return Err("Unknown message type".into());
        }
    }
    
    Ok(())
    */
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new(DaemonConfig::default())
    }
}

use quinn;