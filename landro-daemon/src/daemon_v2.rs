//! Daemon implementation using the actor-based orchestrator
//! 
//! This version uses message passing to avoid deadlocks and improve concurrency

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn, error, debug};

use landro_cas::ContentStore;
use landro_chunker::Chunker;
use landro_crypto::{DeviceIdentity, CertificateVerifier};
use landro_index::async_indexer::AsyncIndexer;
use landro_quic::{QuicServer, QuicConfig, Connection};

use crate::discovery::{DiscoveryService, PeerInfo};
use crate::orchestrator::{SyncOrchestrator, OrchestratorMessage, OrchestratorConfig, OrchestratorStatus};
use crate::watcher::{FileWatcher, FileEvent};

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
    /// File change debounce duration in milliseconds
    pub debounce_ms: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind_addr: "[::]:9876".parse().unwrap(),
            storage_path: PathBuf::from(".landropic"),
            device_name: whoami::devicename(),
            auto_sync: true,
            max_concurrent_transfers: 4,
            debounce_ms: 500,
        }
    }
}

/// Main daemon with actor-based orchestration
pub struct Daemon {
    config: DaemonConfig,
    running: Arc<RwLock<bool>>,
    orchestrator_sender: Arc<Mutex<Option<mpsc::Sender<OrchestratorMessage>>>>,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    quic_server: Arc<Mutex<Option<Arc<QuicServer>>>>,
    discovery_service: Arc<Mutex<Option<Arc<Mutex<DiscoveryService>>>>>,
    file_watchers: Arc<Mutex<Vec<FileWatcher>>>,
}

impl Daemon {
    /// Create a new daemon instance
    pub fn new(config: DaemonConfig) -> Self {
        Self {
            config,
            running: Arc::new(RwLock::new(false)),
            orchestrator_sender: Arc::new(Mutex::new(None)),
            tasks: Arc::new(Mutex::new(Vec::new())),
            quic_server: Arc::new(Mutex::new(None)),
            discovery_service: Arc::new(Mutex::new(None)),
            file_watchers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start the daemon and all subsystems
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut running = self.running.write().await;
        if *running {
            return Err("Daemon already running".into());
        }

        info!("Starting Landropic daemon (actor-based)");
        
        // Create storage directories
        tokio::fs::create_dir_all(&self.config.storage_path).await?;
        let cas_path = self.config.storage_path.join("objects");
        let db_path = self.config.storage_path.join("index.sqlite");
        
        // Initialize core components
        let store = Arc::new(ContentStore::open(&cas_path).await?);
        let chunker = Arc::new(Chunker::new(64 * 1024)); // 64KB average chunk size
        let indexer = Arc::new(AsyncIndexer::new(
            &cas_path,
            &db_path,
            Default::default(),
        ).await?);
        
        // Create orchestrator config
        let orchestrator_config = OrchestratorConfig {
            max_concurrent_chunks: self.config.max_concurrent_transfers,
            debounce_duration: std::time::Duration::from_millis(self.config.debounce_ms),
            auto_sync: self.config.auto_sync,
            ..Default::default()
        };
        
        // Create and start orchestrator
        let (orchestrator, sender) = SyncOrchestrator::new(
            orchestrator_config,
            store,
            chunker,
            indexer,
            &self.config.storage_path,
        ).await?;
        
        // Store the sender for communication
        *self.orchestrator_sender.lock().await = Some(sender.clone());
        
        // Spawn orchestrator task
        let orchestrator_task = tokio::spawn(async move {
            if let Err(e) = orchestrator.run().await {
                error!("Orchestrator error: {}", e);
            }
        });
        self.tasks.lock().await.push(orchestrator_task);
        
        // Start QUIC server
        let quic_server = self.start_quic_server().await?;
        *self.quic_server.lock().await = Some(quic_server.clone());
        
        // Start mDNS discovery
        let port = quic_server.local_addr().map(|addr| addr.port()).unwrap_or(9876);
        let discovery_service = self.start_discovery_service(port, sender.clone()).await?;
        *self.discovery_service.lock().await = Some(discovery_service);
        
        // Start connection handler
        self.start_connection_handler(quic_server, sender.clone()).await?;
        
        *running = true;
        info!("Landropic daemon started successfully");
        
        Ok(())
    }

    /// Start QUIC server
    async fn start_quic_server(&self) -> Result<Arc<QuicServer>, Box<dyn std::error::Error>> {
        info!("Starting QUIC server on {}", self.config.bind_addr);
        
        // Create device identity
        let identity = Arc::new(DeviceIdentity::generate(&self.config.device_name)?);
        
        // Create certificate verifier
        let verifier = Arc::new(CertificateVerifier::for_pairing());
        
        // Create QUIC configuration
        let quic_config = QuicConfig::default().bind_addr(self.config.bind_addr);

        let mut server = QuicServer::new(identity, verifier, quic_config);
        
        // Start the server
        server.start().await?;
        
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
    async fn start_discovery_service(
        &self,
        port: u16,
        orchestrator: mpsc::Sender<OrchestratorMessage>,
    ) -> Result<Arc<Mutex<DiscoveryService>>, Box<dyn std::error::Error>> {
        info!("Starting mDNS discovery service");
        
        let mut discovery = DiscoveryService::new(&self.config.device_name)?;
        
        // Start advertising
        discovery.start_advertising(
            port,
            vec![
                "sync".to_string(),
                "transfer".to_string(),
                "v0.1.0".to_string(),
            ],
        ).await?;
        
        let discovery = Arc::new(Mutex::new(discovery));
        
        // Start periodic peer discovery
        let discovery_clone = discovery.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            
            loop {
                interval.tick().await;
                
                // Browse for peers
                let peers = match discovery_clone.lock().await.browse_peers().await {
                    Ok(peers) => peers,
                    Err(e) => {
                        warn!("Failed to browse peers: {}", e);
                        continue;
                    }
                };
                
                debug!("Found {} peers via mDNS", peers.len());
                
                // Send discovered peers to orchestrator
                for peer in peers {
                    if let Err(e) = orchestrator.send(OrchestratorMessage::PeerDiscovered(peer)).await {
                        error!("Failed to send peer discovery to orchestrator: {}", e);
                        break;
                    }
                }
            }
        });
        
        self.tasks.lock().await.push(task);
        
        Ok(discovery)
    }

    /// Start connection handler
    async fn start_connection_handler(
        &self,
        quic_server: Arc<QuicServer>,
        orchestrator: mpsc::Sender<OrchestratorMessage>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let task = tokio::spawn(async move {
            loop {
                // Accept incoming connections
                match quic_server.accept().await {
                    Ok(connection) => {
                        let orchestrator = orchestrator.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(connection, orchestrator).await {
                                error!("Connection handling error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {}", e);
                        break;
                    }
                }
            }
        });
        
        self.tasks.lock().await.push(task);
        Ok(())
    }

    /// Add a folder to watch and sync
    pub async fn add_sync_folder(&self, path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error>> {
        let path = path.as_ref().to_path_buf();
        
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()).into());
        }
        
        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()).into());
        }
        
        info!("Adding sync folder: {}", path.display());
        
        // Send to orchestrator
        if let Some(sender) = &*self.orchestrator_sender.lock().await {
            sender.send(OrchestratorMessage::AddSyncFolder(path.clone())).await?;
        }
        
        // Create file watcher
        let watcher = FileWatcher::new(path.clone())?;
        
        // Start watching
        let orchestrator = self.orchestrator_sender.lock().await.clone();
        watcher.start(move |events| {
            if let Some(sender) = &orchestrator {
                let sender = sender.clone();
                tokio::spawn(async move {
                    // Send file changes to orchestrator
                    if let Err(e) = sender.send(OrchestratorMessage::FileChangesBatch(events)).await {
                        error!("Failed to send file changes to orchestrator: {}", e);
                    }
                });
            }
        })?;
        
        self.file_watchers.lock().await.push(watcher);
        
        Ok(())
    }

    /// Remove a folder from sync
    pub async fn remove_sync_folder(&self, path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error>> {
        let path = path.as_ref();
        info!("Removing sync folder: {}", path.display());
        
        // Send to orchestrator
        if let Some(sender) = &*self.orchestrator_sender.lock().await {
            sender.send(OrchestratorMessage::RemoveSyncFolder(path.to_path_buf())).await?;
        }
        
        // Stop and remove watcher
        let mut watchers = self.file_watchers.lock().await;
        watchers.retain(|w| {
            if w.path() == path {
                let _ = w.stop();
                false
            } else {
                true
            }
        });
        
        Ok(())
    }

    /// Get list of synced folders
    pub async fn list_sync_folders(&self) -> Vec<PathBuf> {
        // Get from orchestrator status
        if let Some(sender) = &*self.orchestrator_sender.lock().await {
            let (tx, mut rx) = mpsc::channel(1);
            if sender.send(OrchestratorMessage::GetStatus(tx)).await.is_ok() {
                if let Some(status) = rx.recv().await {
                    return status.synced_folders;
                }
            }
        }
        
        Vec::new()
    }

    /// Stop the daemon
    pub async fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut running = self.running.write().await;
        if !*running {
            return Err("Daemon not running".into());
        }

        info!("Stopping Landropic daemon");
        
        // Send shutdown to orchestrator
        if let Some(sender) = &*self.orchestrator_sender.lock().await {
            let _ = sender.send(OrchestratorMessage::Shutdown).await;
        }
        
        // Stop discovery service
        if let Some(discovery) = &*self.discovery_service.lock().await {
            discovery.lock().await.stop().await?;
        }
        
        // Stop file watchers
        for watcher in self.file_watchers.lock().await.iter() {
            watcher.stop()?;
        }
        self.file_watchers.lock().await.clear();
        
        // Cancel background tasks
        for task in self.tasks.lock().await.drain(..) {
            task.abort();
        }
        
        // Clear state
        *self.orchestrator_sender.lock().await = None;
        *self.quic_server.lock().await = None;
        *self.discovery_service.lock().await = None;
        
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
        
        // Get orchestrator status
        let mut peer_count = 0;
        let mut sync_folders = 0;
        
        if let Some(sender) = &*self.orchestrator_sender.lock().await {
            let (tx, mut rx) = mpsc::channel(1);
            if sender.send(OrchestratorMessage::GetStatus(tx)).await.is_ok() {
                if let Some(status) = rx.recv().await {
                    peer_count = status.active_peers;
                    sync_folders = status.synced_folders.len();
                }
            }
        }
        
        DaemonStatus {
            running,
            device_name: self.config.device_name.clone(),
            bind_addr: self.config.bind_addr,
            peer_count,
            sync_folders,
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
async fn handle_connection(
    connection: Connection,
    orchestrator: mpsc::Sender<OrchestratorMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Load device identity for handshake
    let identity = DeviceIdentity::load(None).await?;
    
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
    
    let peer_id = hex::encode(&device_id);
    info!("Connection from {} ({})", device_name, &peer_id[..8]);
    
    // Notify orchestrator of new peer
    let peer_info = PeerInfo {
        device_id: peer_id.clone(),
        device_name: device_name.clone(),
        addresses: vec![],
        port: 0,
        txt_records: vec![],
    };
    
    orchestrator.send(OrchestratorMessage::PeerDiscovered(peer_info)).await?;
    
    // Handle protocol streams
    loop {
        match connection.accept_bi().await {
            Ok((send, recv)) => {
                debug!("Accepted bidirectional stream");
                
                // Handle the stream (simplified for now)
                let peer_id = peer_id.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_sync_stream(send, recv, peer_id).await {
                        error!("Stream handling error: {}", e);
                    }
                });
            }
            Err(e) => {
                debug!("Connection closed: {}", e);
                break;
            }
        }
    }
    
    // Notify orchestrator that peer is lost
    orchestrator.send(OrchestratorMessage::PeerLost(peer_id)).await?;
    
    Ok(())
}

/// Handle a sync protocol stream (simplified)
async fn handle_sync_stream(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    peer_id: String,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    
    // Read message type
    let mut msg_type = [0u8; 1];
    recv.read_exact(&mut msg_type).await?;
    
    debug!("Handling sync stream type {} for peer {}", msg_type[0], peer_id);
    
    // Simple acknowledgment for now
    let response = b"ACK";
    send.write_all(response).await?;
    send.finish()?;
    
    Ok(())
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new(DaemonConfig::default())
    }
}