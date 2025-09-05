use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn, error, debug};

use landro_quic::Server as QuicServer;
use landro_sync::{SyncOrchestrator, SyncConfig};
use landro_index::async_indexer::AsyncIndexer;
use crate::discovery::DiscoveryService;
use crate::watcher::FileWatcher;

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
                sync_orchestrator: None,
                file_watchers: Vec::new(),
                indexer: None,
            })),
            tasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start the daemon and all subsystems
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut running = self.running.write().await;
        if *running {
            return Err("Daemon already running".into());
        }

        info!("Starting Landropic daemon");
        
        // Create storage directories
        tokio::fs::create_dir_all(&self.config.storage_path).await?;
        let cas_path = self.config.storage_path.join("objects");
        let db_path = self.config.storage_path.join("index.sqlite");
        
        // Initialize database and indexer
        let indexer = AsyncIndexer::new(db_path).await?;
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
        let discovery_service = self.start_discovery_service(quic_server.port()).await?;
        
        // Update state
        {
            let mut state = self.state.write().await;
            state.quic_server = Some(quic_server.clone());
            state.discovery_service = Some(discovery_service);
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
    async fn start_quic_server(&self) -> Result<Arc<QuicServer>, Box<dyn std::error::Error>> {
        info!("Starting QUIC server on {}", self.config.bind_addr);
        
        // Load device identity
        let identity = landro_crypto::DeviceIdentity::load(None).await?;
        
        // Create server
        let mut server = QuicServer::new(
            self.config.bind_addr,
            identity.signing_key().to_bytes().to_vec(),
            &self.config.device_name,
        ).await?;
        
        // Start listening
        let server = Arc::new(server);
        let server_clone = server.clone();
        
        let task = tokio::spawn(async move {
            if let Err(e) = server_clone.listen().await {
                error!("QUIC server error: {}", e);
            }
        });
        
        self.tasks.lock().await.push(task);
        
        Ok(server)
    }

    /// Start mDNS discovery service
    async fn start_discovery_service(&self, port: u16) -> Result<Arc<Mutex<DiscoveryService>>, Box<dyn std::error::Error>> {
        info!("Starting mDNS discovery service");
        
        let mut discovery = DiscoveryService::new(&self.config.device_name)?;
        
        // Start advertising with our capabilities
        discovery.start_advertising(
            port,
            vec![
                "sync".to_string(),
                "transfer".to_string(),
                "v0.1.0".to_string(),
            ],
        ).await?;
        
        Ok(Arc::new(Mutex::new(discovery)))
    }

    /// Start background tasks
    async fn start_background_tasks(
        &self,
        quic_server: Arc<QuicServer>,
        sync_orchestrator: Arc<SyncOrchestrator>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Periodic peer discovery
        let discovery = self.state.read().await.discovery_service.clone().unwrap();
        let sync_orch = sync_orchestrator.clone();
        
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                
                // Browse for peers
                match discovery.lock().await.browse_peers().await {
                    Ok(peers) => {
                        debug!("Found {} peers via mDNS", peers.len());
                        for peer in peers {
                            // Attempt to connect and sync
                            if let Err(e) = sync_orch.start_sync(
                                peer.device_id,
                                peer.device_name,
                            ).await {
                                warn!("Failed to start sync with peer: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to browse peers: {}", e);
                    }
                }
            }
        });
        
        self.tasks.lock().await.push(task);
        
        // Connection handler
        let sync_orch = sync_orchestrator.clone();
        let task = tokio::spawn(async move {
            loop {
                // Accept incoming connections
                match quic_server.accept().await {
                    Ok(connection) => {
                        let sync_orch = sync_orch.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(connection, sync_orch).await {
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
        
        let mut state = self.state.write().await;
        
        // Create file watcher
        let watcher = FileWatcher::new(path.clone())?;
        
        // Start watching
        let indexer = state.indexer.clone().unwrap();
        let sync_orch = state.sync_orchestrator.clone().unwrap();
        
        watcher.start(move |events| {
            let indexer = indexer.clone();
            let sync_orch = sync_orch.clone();
            let path = path.clone();
            
            tokio::spawn(async move {
                for event in events {
                    debug!("File event: {:?}", event);
                    
                    // Update index
                    if let Err(e) = indexer.index_directory(&path).await {
                        error!("Failed to update index: {}", e);
                    }
                    
                    // Trigger sync if auto-sync is enabled
                    // The sync orchestrator will handle the actual syncing
                }
            });
        })?;
        
        state.file_watchers.push(watcher);
        
        // Index the folder immediately
        state.indexer.as_ref().unwrap().index_directory(&path).await?;
        
        Ok(())
    }

    /// Remove a folder from sync
    pub async fn remove_sync_folder(&self, path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error>> {
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
    pub async fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
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
async fn handle_connection(
    mut connection: landro_quic::Connection,
    sync_orchestrator: Arc<SyncOrchestrator>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Receive handshake
    connection.receive_hello().await?;
    
    // Get remote device info
    let device_id = connection.remote_device_id().await
        .ok_or("No remote device ID")?;
    let device_name = connection.remote_device_name().await
        .ok_or("No remote device name")?;
    
    info!("Connection from {} ({})", device_name, hex::encode(&device_id[..8]));
    
    // Start sync with this peer
    sync_orchestrator.start_sync(
        hex::encode(&device_id),
        device_name,
    ).await?;
    
    // Handle sync protocol
    loop {
        match connection.accept_bi().await {
            Ok((mut send, mut recv)) => {
                // Handle bidirectional stream for sync protocol
                // This would involve manifest exchange, chunk transfer, etc.
                debug!("Accepted bidirectional stream");
                
                // TODO: Implement full sync protocol handling
                // For now, just echo back
                let mut buf = vec![0u8; 1024];
                if let Ok(n) = recv.read(&mut buf).await {
                    if n > 0 {
                        send.write_all(&buf[..n]).await?;
                    }
                }
            }
            Err(e) => {
                debug!("Connection closed: {}", e);
                break;
            }
        }
    }
    
    Ok(())
}

impl Default for Daemon {
    fn default() -> Self {
        Self::new(DaemonConfig::default())
    }
}