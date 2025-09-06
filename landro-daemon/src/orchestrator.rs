//! Actor-based sync orchestrator to avoid deadlocks
//!
//! This module implements the main orchestration logic using an actor pattern
//! with message passing to coordinate file watching, chunking, storage, and sync.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, warn};

use crate::discovery::PeerInfo;
use crate::watcher::{FileEvent, FileEventKind};
use crate::network::{ConnectionManager, NetworkConfig};
use landro_cas::ContentStore;
use landro_chunker::Chunker;
use landro_index::async_indexer::AsyncIndexer;
use landro_quic::{QuicServer, QuicConfig, Connection};
use landro_crypto::{DeviceIdentity, CertificateVerifier};

/// Messages that can be sent to the orchestrator
#[derive(Debug, Clone)]
pub enum OrchestratorMessage {
    /// File system change detected
    FileChanged(FileEvent),

    /// Multiple file changes (batched)
    FileChangesBatch(Vec<FileEvent>),

    /// New peer discovered via mDNS
    PeerDiscovered(PeerInfo),

    /// Peer disconnected or lost
    PeerLost(String),

    /// Explicit sync request for a path
    SyncRequested { peer_id: String, path: PathBuf },

    /// Request to add a folder for syncing
    AddSyncFolder(PathBuf),

    /// Request to remove a folder from syncing
    RemoveSyncFolder(PathBuf),

    /// Get current status
    GetStatus(mpsc::Sender<OrchestratorStatus>),

    /// Graceful shutdown
    Shutdown,
}

/// Status information from the orchestrator
#[derive(Debug, Clone)]
pub struct OrchestratorStatus {
    pub running: bool,
    pub active_peers: usize,
    pub pending_changes: usize,
    pub synced_folders: Vec<PathBuf>,
    pub total_bytes_synced: u64,
    pub current_operations: Vec<String>,
}

/// Configuration for the orchestrator
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Maximum number of concurrent chunk operations
    pub max_concurrent_chunks: usize,

    /// Debounce duration for file changes
    pub debounce_duration: Duration,

    /// Enable auto-sync on file changes
    pub auto_sync: bool,

    /// Maximum retries for failed operations
    pub max_retries: usize,

    /// Batch size for processing file changes
    pub batch_size: usize,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_chunks: 4,
            debounce_duration: Duration::from_millis(500),
            auto_sync: true,
            max_retries: 3,
            batch_size: 100,
        }
    }
}

/// Internal state for a pending file operation
#[derive(Debug)]
struct PendingOperation {
    path: PathBuf,
    kind: FileEventKind,
    retry_count: usize,
    last_attempt: std::time::Instant,
}

/// Main actor-based sync orchestrator
pub struct SyncOrchestrator {
    config: OrchestratorConfig,
    receiver: mpsc::Receiver<OrchestratorMessage>,
    sender: mpsc::Sender<OrchestratorMessage>,

    // Core components
    store: Arc<ContentStore>,
    chunker: Arc<Chunker>,
    indexer: Arc<AsyncIndexer>,

    // Network components
    connection_manager: Option<Arc<ConnectionManager>>,
    quic_server: Option<Arc<tokio::sync::RwLock<QuicServer>>>,
    device_identity: Arc<DeviceIdentity>,
    certificate_verifier: Arc<CertificateVerifier>,

    // Callbacks for external integration
    on_peer_sync_needed: Option<Arc<dyn Fn(String, Vec<PathBuf>) + Send + Sync>>,
    on_manifest_ready: Option<Arc<dyn Fn(PathBuf) + Send + Sync>>,

    // Internal state
    synced_folders: Vec<PathBuf>,
    active_peers: HashMap<String, PeerInfo>,
    pending_operations: Vec<PendingOperation>,
    current_operations: Vec<String>,
    total_bytes_synced: u64,

    // Debouncing state
    debounce_buffer: Vec<FileEvent>,
    last_debounce: std::time::Instant,
}

impl SyncOrchestrator {
    /// Initialize QUIC server and connection manager
    pub async fn initialize_quic_transport(&mut self, bind_addr: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Initializing QUIC transport on {}", bind_addr);
        
        // Configure QUIC for file sync workloads
        let quic_config = QuicConfig::file_sync_optimized()
            .bind_addr(bind_addr)
            .idle_timeout(Duration::from_secs(300))
            .stream_receive_window(128 * 1024 * 1024)  // 128MB for large chunks
            .receive_window(2 * 1024 * 1024 * 1024);   // 2GB for bulk transfers
        
        // Create and start QUIC server
        let mut quic_server = QuicServer::new(
            self.device_identity.clone(),
            self.certificate_verifier.clone(),
            quic_config.clone(),
        );
        quic_server.start().await?;
        self.quic_server = Some(Arc::new(tokio::sync::RwLock::new(quic_server)));
        
        // Create connection manager for outbound connections
        let discovery = crate::discovery::DiscoveryService::new(&whoami::devicename())?;
        let network_config = NetworkConfig {
            max_connections_per_peer: 4,  // More connections for parallel transfers
            connection_idle_timeout: Duration::from_secs(300),
            health_check_interval: Duration::from_secs(30),
            ..Default::default()
        };
        
        let connection_manager = Arc::new(ConnectionManager::new(
            self.device_identity.clone(),
            self.certificate_verifier.clone(),
            Arc::new(tokio::sync::Mutex::new(discovery)),
            network_config,
        ));
        
        connection_manager.start().await?;
        self.connection_manager = Some(connection_manager);
        
        info!("QUIC transport initialized successfully");
        Ok(())
    }

    /// Handle incoming QUIC connection
    pub async fn handle_incoming_connection(&self, connection: Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Handling incoming QUIC connection from {}", connection.remote_address());
        
        // Perform protocol handshake
        connection.receive_hello().await?;
        connection.send_hello(
            self.device_identity.device_id_bytes(),
            &self.device_identity.device_name(),
        ).await?;
        
        // Spawn handler for this connection
        let store = self.store.clone();
        let indexer = self.indexer.clone();
        let chunker = self.chunker.clone();
        
        tokio::spawn(async move {
            if let Err(e) = Self::handle_connection_streams(connection, store, indexer, chunker).await {
                error!("Error handling connection streams: {}", e);
            }
        });
        
        Ok(())
    }
    
    /// Handle streams on an established connection
    async fn handle_connection_streams(
        connection: Connection,
        store: Arc<ContentStore>,
        _indexer: Arc<AsyncIndexer>,
        _chunker: Arc<Chunker>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            tokio::select! {
                // Accept bidirectional streams for data transfer
                result = connection.accept_bi() => {
                    match result {
                        Ok((send, recv)) => {
                            // Handle data transfer stream
                            let store_clone = store.clone();
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_data_stream(send, recv, store_clone).await {
                                    error!("Error handling data stream: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            warn!("Error accepting bidirectional stream: {}", e);
                            break;
                        }
                    }
                }
                
                // Accept unidirectional streams for control messages
                result = connection.accept_uni() => {
                    match result {
                        Ok(recv) => {
                            // Handle control stream
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_control_stream(recv).await {
                                    error!("Error handling control stream: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            warn!("Error accepting unidirectional stream: {}", e);
                            break;
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Handle data transfer stream
    async fn handle_data_stream(
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
        store: Arc<ContentStore>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        
        // Read chunk request
        let mut len_buf = [0u8; 4];
        recv.read_exact(&mut len_buf).await?;
        let chunk_hash_len = u32::from_be_bytes(len_buf) as usize;
        
        let mut chunk_hash = vec![0u8; chunk_hash_len];
        recv.read_exact(&mut chunk_hash).await?;
        
        // Retrieve chunk from store
        let hash = landro_cas::Hash::from_bytes(&chunk_hash)?;
        let chunk_data = store.read(&hash).await?;
        
        // Send chunk data
        let len_bytes = (chunk_data.len() as u32).to_be_bytes();
        send.write_all(&len_bytes).await?;
        send.write_all(&chunk_data).await?;
        send.finish()?;
        
        debug!("Sent chunk {} ({} bytes)", hex::encode(&chunk_hash), chunk_data.len());
        Ok(())
    }
    
    /// Handle control stream
    async fn handle_control_stream(
        mut recv: quinn::RecvStream,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use tokio::io::AsyncReadExt;
        
        // Read control message type
        let mut msg_type = [0u8; 1];
        recv.read_exact(&mut msg_type).await?;
        
        match msg_type[0] {
            0 => {
                // Manifest exchange
                debug!("Received manifest request");
            }
            1 => {
                // Sync status
                debug!("Received sync status");
            }
            _ => {
                warn!("Unknown control message type: {}", msg_type[0]);
            }
        }
        
        Ok(())
    }

    /// Set the callback for when peer sync is needed
    pub fn set_peer_sync_callback<F>(&mut self, callback: F)
    where
        F: Fn(String, Vec<PathBuf>) + Send + Sync + 'static,
    {
        self.on_peer_sync_needed = Some(Arc::new(callback));
    }

    /// Set the callback for when a manifest is ready
    pub fn set_manifest_ready_callback<F>(&mut self, callback: F)
    where
        F: Fn(PathBuf) + Send + Sync + 'static,
    {
        self.on_manifest_ready = Some(Arc::new(callback));
    }

    /// Create a new orchestrator
    pub async fn new(
        config: OrchestratorConfig,
        store: Arc<ContentStore>,
        chunker: Arc<Chunker>,
        indexer: Arc<AsyncIndexer>,
        _storage_path: &PathBuf,
    ) -> Result<(Self, mpsc::Sender<OrchestratorMessage>), Box<dyn std::error::Error + Send + Sync>>
    {
        let (sender, receiver) = mpsc::channel(1000);

        // Load or generate device identity
        let device_name = whoami::devicename();
        let device_identity = Arc::new(DeviceIdentity::load_or_generate(&device_name).await?);
        let certificate_verifier = Arc::new(CertificateVerifier::for_pairing());

        // Callbacks will be set after creation

        let orchestrator = Self {
            config,
            receiver,
            sender: sender.clone(),
            store,
            chunker,
            indexer,
            connection_manager: None,
            quic_server: None,
            device_identity,
            certificate_verifier,
            on_peer_sync_needed: None,
            on_manifest_ready: None,
            synced_folders: Vec::new(),
            active_peers: HashMap::new(),
            pending_operations: Vec::new(),
            current_operations: Vec::new(),
            total_bytes_synced: 0,
            debounce_buffer: Vec::new(),
            last_debounce: std::time::Instant::now(),
        };

        Ok((orchestrator, sender))
    }

    /// Run the orchestrator event loop
    pub async fn run(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting sync orchestrator actor");

        let mut debounce_interval = interval(self.config.debounce_duration);
        let mut retry_interval = interval(Duration::from_secs(30));
        let mut quic_accept_task = None;

        // Start accepting QUIC connections if server is initialized
        if let Some(ref server) = self.quic_server {
            let server_clone = server.clone();
            let store = self.store.clone();
            let indexer = self.indexer.clone();
            let chunker = self.chunker.clone();
            let identity = self.device_identity.clone();
            
            quic_accept_task = Some(tokio::spawn(async move {
                loop {
                    match server_clone.read().await.accept().await {
                        Ok(connection) => {
                            let conn = Connection::new(connection);
                            let store_clone = store.clone();
                            let indexer_clone = indexer.clone();
                            let chunker_clone = chunker.clone();
                            let identity_clone = identity.clone();
                            
                            tokio::spawn(async move {
                                // Handle connection with proper error recovery
                                if let Err(e) = Self::handle_incoming_connection_with_recovery(
                                    conn, 
                                    store_clone, 
                                    indexer_clone, 
                                    chunker_clone,
                                    identity_clone
                                ).await {
                                    error!("Failed to handle incoming connection: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("Failed to accept QUIC connection: {}", e);
                            // Don't break on error, continue accepting
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            }));
        }

        loop {
            tokio::select! {
                // Handle incoming messages
                msg = self.receiver.recv() => {
                    match msg {
                        Some(message) => {
                            // Wrap message handling with error recovery
                            match self.handle_message_with_recovery(message).await {
                                Ok(should_continue) => {
                                    if !should_continue {
                                        break; // Shutdown requested
                                    }
                                }
                                Err(e) => {
                                    error!("Error handling message: {}", e);
                                    // Continue running despite errors
                                }
                            }
                        }
                        None => {
                            warn!("Orchestrator channel closed");
                            break;
                        }
                    }
                }

                // Process debounced file changes
                _ = debounce_interval.tick() => {
                    if let Err(e) = self.process_debounced_changes().await {
                        error!("Error processing debounced changes: {}", e);
                        // Continue running
                    }
                }

                // Retry failed operations
                _ = retry_interval.tick() => {
                    if let Err(e) = self.retry_failed_operations().await {
                        error!("Error retrying failed operations: {}", e);
                        // Continue running
                    }
                }
            }
        }

        // Clean up QUIC accept task
        if let Some(task) = quic_accept_task {
            task.abort();
        }

        info!("Sync orchestrator shutting down");
        self.shutdown().await?;
        Ok(())
    }

    /// Handle incoming connection with error recovery
    async fn handle_incoming_connection_with_recovery(
        connection: Connection,
        store: Arc<ContentStore>,
        indexer: Arc<AsyncIndexer>,
        chunker: Arc<Chunker>,
        identity: Arc<DeviceIdentity>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Perform handshake with timeout
        match timeout(Duration::from_secs(10), connection.receive_hello()).await {
            Ok(Ok(_)) => {
                // Send our hello
                if let Err(e) = connection.send_hello(
                    identity.device_id().as_bytes(),
                    &identity.device_name(),
                ).await {
                    error!("Failed to send hello: {}", e);
                    connection.close(1, b"handshake_failed");
                    return Err(e.into());
                }
            }
            Ok(Err(e)) => {
                error!("Failed to receive hello: {}", e);
                connection.close(1, b"handshake_failed");
                return Err(e.into());
            }
            Err(_) => {
                error!("Handshake timeout");
                connection.close(2, b"handshake_timeout");
                return Err("Handshake timeout".into());
            }
        }
        
        // Handle connection streams
        Self::handle_connection_streams(connection, store, indexer, chunker).await
    }

    /// Handle message with error recovery
    async fn handle_message_with_recovery(
        &mut self,
        message: OrchestratorMessage,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // Add retry logic for critical operations
        match message {
            OrchestratorMessage::PeerDiscovered(ref peer) => {
                // Retry peer connection with exponential backoff
                let mut retry_count = 0;
                let max_retries = 3;
                
                while retry_count < max_retries {
                    match self.handle_message(message.clone()).await {
                        Ok(result) => return Ok(result),
                        Err(e) => {
                            retry_count += 1;
                            if retry_count < max_retries {
                                let backoff = Duration::from_millis(100 * (1 << retry_count));
                                warn!("Failed to handle peer discovery (attempt {}/{}): {}, retrying in {:?}", 
                                      retry_count, max_retries, e, backoff);
                                tokio::time::sleep(backoff).await;
                            } else {
                                error!("Failed to handle peer discovery after {} attempts: {}", max_retries, e);
                                return Err(e);
                            }
                        }
                    }
                }
                Ok(true)
            }
            _ => {
                // For other messages, use single attempt
                self.handle_message(message).await
            }
        }
    }

    /// Handle a single message
    async fn handle_message(
        &mut self,
        message: OrchestratorMessage,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        match message {
            OrchestratorMessage::FileChanged(event) => {
                debug!("File changed: {:?}", event);
                self.debounce_buffer.push(event);
                Ok(true)
            }

            OrchestratorMessage::FileChangesBatch(events) => {
                debug!("Batch of {} file changes", events.len());
                self.debounce_buffer.extend(events);
                Ok(true)
            }

            OrchestratorMessage::PeerDiscovered(peer) => {
                info!("Peer discovered: {} ({})", peer.device_name, peer.device_id);
                self.handle_peer_discovered(peer).await?;
                Ok(true)
            }

            OrchestratorMessage::PeerLost(peer_id) => {
                info!("Peer lost: {}", peer_id);
                self.handle_peer_lost(&peer_id).await?;
                Ok(true)
            }

            OrchestratorMessage::SyncRequested { peer_id, path } => {
                info!(
                    "Sync requested for {} with peer {}",
                    path.display(),
                    peer_id
                );
                self.handle_sync_request(&peer_id, &path).await?;
                Ok(true)
            }

            OrchestratorMessage::AddSyncFolder(path) => {
                info!("Adding sync folder: {}", path.display());
                self.add_sync_folder(path).await?;
                Ok(true)
            }

            OrchestratorMessage::RemoveSyncFolder(path) => {
                info!("Removing sync folder: {}", path.display());
                self.remove_sync_folder(&path).await?;
                Ok(true)
            }

            OrchestratorMessage::GetStatus(response_tx) => {
                let status = self.get_status();
                let _ = response_tx.send(status).await;
                Ok(true)
            }

            OrchestratorMessage::Shutdown => {
                info!("Shutdown requested");
                Ok(false) // Signal to exit the loop
            }
        }
    }

    /// Process debounced file changes
    async fn process_debounced_changes(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.debounce_buffer.is_empty() {
            return Ok(());
        }

        if self.last_debounce.elapsed() < self.config.debounce_duration {
            return Ok(()); // Still within debounce window
        }

        // Take all pending changes
        let changes = std::mem::take(&mut self.debounce_buffer);
        self.last_debounce = std::time::Instant::now();

        info!("Processing {} debounced file changes", changes.len());

        // Group changes by operation type for efficient processing
        let mut creates = Vec::new();
        let mut modifies = Vec::new();
        let mut deletes = Vec::new();

        for event in changes {
            match event.kind {
                FileEventKind::Created => creates.push(event.path),
                FileEventKind::Modified => modifies.push(event.path),
                FileEventKind::Deleted => deletes.push(event.path),
                FileEventKind::Renamed { from, to } => {
                    deletes.push(from);
                    creates.push(to);
                }
            }
        }

        // Process in batches
        for batch in creates.chunks(self.config.batch_size) {
            self.process_file_batch(batch, FileOperation::Create)
                .await?;
        }

        for batch in modifies.chunks(self.config.batch_size) {
            self.process_file_batch(batch, FileOperation::Modify)
                .await?;
        }

        for batch in deletes.chunks(self.config.batch_size) {
            self.process_file_batch(batch, FileOperation::Delete)
                .await?;
        }

        // Trigger sync if auto-sync is enabled and we have peers
        if self.config.auto_sync && !self.active_peers.is_empty() {
            self.trigger_auto_sync().await?;
        }

        Ok(())
    }

    /// Process a batch of files for a specific operation
    async fn process_file_batch(
        &mut self,
        paths: &[PathBuf],
        operation: FileOperation,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Spawn concurrent tasks for processing
        let mut tasks = Vec::new();

        for path in paths {
            let path = path.clone();
            let store = self.store.clone();
            let chunker = self.chunker.clone();
            let indexer = self.indexer.clone();
            let op = operation.clone();

            let task = tokio::spawn(async move {
                process_single_file(path, op, store, chunker, indexer).await
            });

            tasks.push(task);

            // Limit concurrent operations
            if tasks.len() >= self.config.max_concurrent_chunks {
                // Wait for some to complete
                let results = futures::future::join_all(tasks).await;
                tasks = Vec::new();

                // Log any errors
                for result in results {
                    if let Err(e) = result {
                        error!("Task join error: {}", e);
                    } else if let Ok(Err(e)) = result {
                        error!("File processing error: {}", e);
                    }
                }
            }
        }

        // Wait for remaining tasks
        if !tasks.is_empty() {
            let results = futures::future::join_all(tasks).await;
            for result in results {
                if let Err(e) = result {
                    error!("Task join error: {}", e);
                } else if let Ok(Err(e)) = result {
                    error!("File processing error: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Handle peer discovered
    async fn handle_peer_discovered(
        &mut self,
        peer: PeerInfo,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.active_peers
            .insert(peer.device_id.clone(), peer.clone());

        info!(
            "Peer discovered: {} - preparing sync folders",
            peer.device_name
        );

        // If we have folders to sync and QUIC is initialized, establish connection
        if !self.synced_folders.is_empty() && self.config.auto_sync {
            if let Some(ref conn_mgr) = self.connection_manager {
                // Try to establish connection to the peer
                match conn_mgr.get_connection(&peer.device_id).await {
                    Ok(connection) => {
                        info!("Established QUIC connection to peer {}", peer.device_name);
                        // Trigger sync through the connection
                        self.initiate_sync_with_peer(&peer.device_id, connection).await?;
                    }
                    Err(e) => {
                        warn!("Failed to connect to peer {}: {}", peer.device_name, e);
                    }
                }
            } else if let Some(ref callback) = self.on_peer_sync_needed {
                // Fallback to callback if QUIC not initialized
                callback(peer.device_id.clone(), self.synced_folders.clone());
            }
        }

        Ok(())
    }

    /// Handle peer lost
    async fn handle_peer_lost(
        &mut self,
        peer_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.active_peers.remove(peer_id);

        info!("Peer lost: {} - cleaning up state", peer_id);

        // Clean up any pending operations for this peer
        self.pending_operations.retain(|_op| {
            // Keep operations not related to this peer
            // In a real implementation, we'd track peer associations
            true
        });

        Ok(())
    }

    /// Handle explicit sync request
    async fn handle_sync_request(
        &mut self,
        peer_id: &str,
        path: &PathBuf,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.active_peers.contains_key(peer_id) {
            warn!("Sync requested with unknown peer: {}", peer_id);
            return Ok(());
        }

        info!(
            "Processing sync request for {} with peer {}",
            path.display(),
            peer_id
        );

        // Process the folder through our pipeline
        self.process_folder_for_sync(path).await?;

        // Notify that we have data ready to sync
        if let Some(ref callback) = self.on_peer_sync_needed {
            callback(peer_id.to_string(), vec![path.clone()]);
        }

        Ok(())
    }

    /// Initiate sync with a specific peer through QUIC
    async fn initiate_sync_with_peer(
        &self,
        peer_id: &str,
        connection: Arc<Connection>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Initiating sync with peer {} via QUIC", peer_id);
        
        // Open control stream for sync negotiation
        let mut control_stream = connection.open_uni().await?;
        
        // Send sync request for each folder
        for folder in &self.synced_folders {
            // Build manifest for this folder
            let manifest = self.indexer.index_folder(folder).await?;
            
            // Serialize manifest (using a simple format for now)
            let manifest_data = serde_json::to_vec(&manifest)?;
            
            // Send sync request message
            use tokio::io::AsyncWriteExt;
            control_stream.write_all(&[1u8]).await?; // Message type: sync request
            control_stream.write_all(&(manifest_data.len() as u32).to_be_bytes()).await?;
            control_stream.write_all(&manifest_data).await?;
        }
        
        control_stream.finish()?;
        
        Ok(())
    }

    /// Trigger auto-sync with all connected peers
    async fn trigger_auto_sync(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!(
            "Triggering auto-sync with {} peers",
            self.active_peers.len()
        );

        // Use QUIC connections if available
        if let Some(ref conn_mgr) = self.connection_manager {
            for (peer_id, _peer_info) in &self.active_peers {
                match conn_mgr.get_connection(peer_id).await {
                    Ok(connection) => {
                        if let Err(e) = self.initiate_sync_with_peer(peer_id, connection).await {
                            error!("Failed to sync with {}: {}", peer_id, e);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to get connection to {}: {}", peer_id, e);
                    }
                }
            }
        } else if let Some(ref callback) = self.on_peer_sync_needed {
            // Fallback to callback
            for peer_id in self.active_peers.keys() {
                callback(peer_id.clone(), self.synced_folders.clone());
            }
        }

        Ok(())
    }

    /// Retry failed operations
    async fn retry_failed_operations(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.pending_operations.is_empty() {
            return Ok(());
        }

        let mut still_pending = Vec::new();

        for mut op in std::mem::take(&mut self.pending_operations) {
            if op.retry_count >= self.config.max_retries {
                error!(
                    "Max retries exceeded for operation on: {}",
                    op.path.display()
                );
                continue;
            }

            if op.last_attempt.elapsed() < Duration::from_secs(30) {
                still_pending.push(op);
                continue;
            }

            debug!(
                "Retrying operation on: {} (attempt {})",
                op.path.display(),
                op.retry_count + 1
            );

            // Convert to FileEvent and reprocess
            let event = FileEvent {
                path: op.path.clone(),
                kind: op.kind.clone(),
            };

            op.retry_count += 1;
            op.last_attempt = std::time::Instant::now();

            self.debounce_buffer.push(event);
            still_pending.push(op);
        }

        self.pending_operations = still_pending;
        Ok(())
    }

    /// Process a folder for synchronization
    async fn process_folder_for_sync(
        &mut self,
        path: &PathBuf,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Processing folder for sync: {}", path.display());

        // Index the folder to build manifest
        self.indexer.index_folder(path).await?;

        // Use iterative approach with a stack to avoid recursion
        let mut dir_stack = vec![path.clone()];
        let mut total_file_count = 0;

        while let Some(current_dir) = dir_stack.pop() {
            let mut entries = tokio::fs::read_dir(&current_dir).await?;
            let mut file_count = 0;

            while let Some(entry) = entries.next_entry().await? {
                let entry_path = entry.path();
                let metadata = entry.metadata().await?;

                if metadata.is_file() {
                    // Process individual file
                    if let Err(e) = self
                        .process_single_file_internal(&entry_path, FileOperation::Create)
                        .await
                    {
                        error!("Failed to process file {}: {}", entry_path.display(), e);
                    } else {
                        file_count += 1;
                    }
                } else if metadata.is_dir() {
                    // Add subdirectory to stack for processing
                    dir_stack.push(entry_path);
                }
            }

            if file_count > 0 {
                debug!(
                    "Processed {} files in directory {}",
                    file_count,
                    current_dir.display()
                );
            }
            total_file_count += file_count;
        }

        info!(
            "Processed {} total files in folder {}",
            total_file_count,
            path.display()
        );

        // Notify that manifest is ready
        if let Some(ref callback) = self.on_manifest_ready {
            callback(path.clone());
        }

        Ok(())
    }

    /// Process a single file internally
    async fn process_single_file_internal(
        &mut self,
        path: &PathBuf,
        operation: FileOperation,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match operation {
            FileOperation::Create | FileOperation::Modify => {
                if path.is_file() {
                    // Track operation
                    self.current_operations
                        .push(format!("Processing: {}", path.display()));

                    // Always use streaming to avoid memory issues
                    self.process_file_streaming(path).await?;

                    // Update index - index parent folder to include this file
                    if let Some(parent) = path.parent() {
                        self.indexer.index_folder(parent).await?;
                    }

                    // Remove from current operations
                    self.current_operations
                        .retain(|op| !op.contains(&path.display().to_string()));
                }
            }
            FileOperation::Delete => {
                // Mark as deleted in index
                debug!("File deleted: {}", path.display());
                // The indexer should handle deletion tracking
            }
        }

        Ok(())
    }

    /// Process files with streaming to avoid loading entire file into memory
    async fn process_file_streaming(
        &mut self,
        path: &PathBuf,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use tokio::io::AsyncReadExt;

        debug!("Processing file with streaming: {}", path.display());

        let file = tokio::fs::File::open(path).await?;
        let metadata = file.metadata().await?;
        let file_size = metadata.len();
        
        let mut file = file;
        // Use a rolling buffer approach - read in chunks that overlap
        // to ensure we don't miss chunk boundaries
        let read_size = 1024 * 1024; // 1MB read buffer
        let mut buffer = Vec::with_capacity(read_size * 2);
        let mut total_chunks = 0;
        let mut _file_offset = 0u64;

        loop {
            // Read next chunk from file
            let mut temp_buffer = vec![0u8; read_size];
            let bytes_read = file.read(&mut temp_buffer).await?;
            
            if bytes_read == 0 {
                // Process any remaining data
                if !buffer.is_empty() {
                    let chunks = self.chunker.chunk_bytes(&buffer)?;
                    for chunk in chunks {
                        self.store.write(&chunk.data).await?;
                        total_chunks += 1;
                    }
                }
                break;
            }

            // Add new data to buffer
            buffer.extend_from_slice(&temp_buffer[..bytes_read]);
            
            // Process buffer when it's large enough
            // Keep some data for overlap to catch boundaries
            if buffer.len() >= self.config.max_concurrent_chunks * 256 * 1024 {
                // Process most of the buffer, keeping last part for overlap
                let process_size = buffer.len() - (256 * 1024); // Keep 256KB for overlap
                let data_to_process = &buffer[..process_size];
                
                let chunks = self.chunker.chunk_bytes(data_to_process)?;
                for chunk in chunks {
                    self.store.write(&chunk.data).await?;
                    total_chunks += 1;
                }
                
                // Keep the unprocessed tail
                buffer.drain(..process_size);
            }
            
            file_offset += bytes_read as u64;
        }

        // Track bytes synced
        self.total_bytes_synced += file_size;

        debug!(
            "File {} processed: {} bytes into {} chunks",
            path.display(),
            file_size,
            total_chunks
        );
        Ok(())
    }

    /// Add a folder to sync
    async fn add_sync_folder(
        &mut self,
        path: PathBuf,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()).into());
        }

        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()).into());
        }

        if self.synced_folders.contains(&path) {
            return Ok(()); // Already syncing
        }

        info!("Adding sync folder: {}", path.display());

        // Process the folder immediately
        self.process_folder_for_sync(&path).await?;

        self.synced_folders.push(path);
        Ok(())
    }

    /// Remove a folder from sync
    async fn remove_sync_folder(
        &mut self,
        path: &PathBuf,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.synced_folders.retain(|p| p != path);
        Ok(())
    }

    /// Get current status
    fn get_status(&self) -> OrchestratorStatus {
        OrchestratorStatus {
            running: true,
            active_peers: self.active_peers.len(),
            pending_changes: self.debounce_buffer.len() + self.pending_operations.len(),
            synced_folders: self.synced_folders.clone(),
            total_bytes_synced: self.total_bytes_synced,
            current_operations: self.current_operations.clone(),
        }
    }

    /// Graceful shutdown
    async fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Orchestrator shutting down gracefully");

        // Process any remaining debounced changes
        if !self.debounce_buffer.is_empty() {
            info!(
                "Processing {} remaining changes before shutdown",
                self.debounce_buffer.len()
            );
            self.process_debounced_changes().await?;
        }

        // Complete all active operations
        info!(
            "Completing {} active operations",
            self.current_operations.len()
        );
        self.current_operations.clear();

        Ok(())
    }
}

/// File operation type
#[derive(Debug, Clone)]
enum FileOperation {
    Create,
    Modify,
    Delete,
}

/// Process a single file through the pipeline
async fn process_single_file(
    path: PathBuf,
    operation: FileOperation,
    store: Arc<ContentStore>,
    chunker: Arc<Chunker>,
    indexer: Arc<AsyncIndexer>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match operation {
        FileOperation::Create | FileOperation::Modify => {
            // Check if it's a file or directory
            if path.is_file() {
                debug!("Processing file: {}", path.display());

                // Read file content
                let content = tokio::fs::read(&path).await?;

                // Chunk the file
                let chunks = chunker.chunk_bytes(&content)?;
                debug!(
                    "File {} chunked into {} pieces",
                    path.display(),
                    chunks.len()
                );

                // Store chunks in CAS
                for chunk in &chunks {
                    let obj_ref = store.write(&chunk.data).await?;
                    debug!(
                        "Stored chunk with hash: {}",
                        hex::encode(obj_ref.hash.as_bytes())
                    );
                }

                // Update index - index parent folder to include this file
                if let Some(parent) = path.parent() {
                    indexer.index_folder(parent).await?;
                }
                info!("Indexed file: {}", path.display());
            } else if path.is_dir() {
                // Index the entire directory
                indexer.index_folder(&path).await?;
                info!("Indexed directory: {}", path.display());
            }
        }

        FileOperation::Delete => {
            // Update index to reflect deletion
            // TODO: Implement deletion in indexer
            debug!("File deleted: {}", path.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let temp_dir = TempDir::new().unwrap();
        let storage_path = temp_dir.path().to_path_buf();

        // Create test components
        let store = Arc::new(ContentStore::new(&storage_path.join("cas")).await.unwrap());
        let chunker_config = landro_chunker::ChunkerConfig {
            min_size: 16 * 1024,
            avg_size: 64 * 1024,
            max_size: 256 * 1024,
            mask_bits: 16,
        };
        let chunker = Arc::new(Chunker::new(chunker_config).unwrap());
        let indexer = Arc::new(
            AsyncIndexer::new(
                &storage_path.join("cas"),
                &storage_path.join("index.sqlite"),
                Default::default(),
            )
            .await
            .unwrap(),
        );

        let config = OrchestratorConfig::default();
        let (orchestrator, sender) =
            SyncOrchestrator::new(config, store, chunker, indexer, &storage_path)
                .await
                .unwrap();

        // Test sending a message
        sender.send(OrchestratorMessage::Shutdown).await.unwrap();

        // Run should exit immediately due to shutdown
        tokio::time::timeout(Duration::from_secs(1), orchestrator.run())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn test_file_change_debouncing() {
        let temp_dir = TempDir::new().unwrap();
        let storage_path = temp_dir.path().to_path_buf();

        // Create test components
        let store = Arc::new(ContentStore::new(&storage_path.join("cas")).await.unwrap());
        let chunker_config = landro_chunker::ChunkerConfig {
            min_size: 16 * 1024,
            avg_size: 64 * 1024,
            max_size: 256 * 1024,
            mask_bits: 16,
        };
        let chunker = Arc::new(Chunker::new(chunker_config).unwrap());
        let indexer = Arc::new(
            AsyncIndexer::new(
                &storage_path.join("cas"),
                &storage_path.join("index.sqlite"),
                Default::default(),
            )
            .await
            .unwrap(),
        );

        let mut config = OrchestratorConfig::default();
        config.debounce_duration = Duration::from_millis(100);

        let (orchestrator, sender) =
            SyncOrchestrator::new(config, store, chunker, indexer, &storage_path)
                .await
                .unwrap();

        // Spawn orchestrator in background
        let handle = tokio::spawn(async move { orchestrator.run().await });

        // Send multiple file changes quickly
        for i in 0..5 {
            let event = FileEvent {
                path: PathBuf::from(format!("test{}.txt", i)),
                kind: FileEventKind::Created,
            };
            sender
                .send(OrchestratorMessage::FileChanged(event))
                .await
                .unwrap();
        }

        // Wait for debouncing
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Get status to verify changes were processed
        let (status_tx, mut status_rx) = mpsc::channel(1);
        sender
            .send(OrchestratorMessage::GetStatus(status_tx))
            .await
            .unwrap();

        let status = status_rx.recv().await.unwrap();
        assert_eq!(status.pending_changes, 0); // Changes should be processed

        // Shutdown
        sender.send(OrchestratorMessage::Shutdown).await.unwrap();
        handle.await.unwrap().unwrap();
    }
}
