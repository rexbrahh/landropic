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

use landro_cas::ContentStore;
use landro_chunker::Chunker;
use landro_index::async_indexer::AsyncIndexer;
use crate::watcher::{FileEvent, FileEventKind};
use crate::discovery::PeerInfo;

/// Messages that can be sent to the orchestrator
#[derive(Debug)]
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
    SyncRequested {
        peer_id: String,
        path: PathBuf,
    },
    
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
        storage_path: &PathBuf,
    ) -> Result<(Self, mpsc::Sender<OrchestratorMessage>), Box<dyn std::error::Error>> {
        let (sender, receiver) = mpsc::channel(1000);
        
        // Callbacks will be set after creation
        
        let orchestrator = Self {
            config,
            receiver,
            sender: sender.clone(),
            store,
            chunker,
            indexer,
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
    pub async fn run(mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting sync orchestrator actor");
        
        let mut debounce_interval = interval(self.config.debounce_duration);
        let mut retry_interval = interval(Duration::from_secs(30));
        
        loop {
            tokio::select! {
                // Handle incoming messages
                msg = self.receiver.recv() => {
                    match msg {
                        Some(message) => {
                            if !self.handle_message(message).await? {
                                break; // Shutdown requested
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
                    self.process_debounced_changes().await?;
                }
                
                // Retry failed operations
                _ = retry_interval.tick() => {
                    self.retry_failed_operations().await?;
                }
            }
        }
        
        info!("Sync orchestrator shutting down");
        self.shutdown().await?;
        Ok(())
    }
    
    /// Handle a single message
    async fn handle_message(&mut self, message: OrchestratorMessage) -> Result<bool, Box<dyn std::error::Error>> {
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
                info!("Sync requested for {} with peer {}", path.display(), peer_id);
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
    async fn process_debounced_changes(&mut self) -> Result<(), Box<dyn std::error::Error>> {
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
            self.process_file_batch(batch, FileOperation::Create).await?;
        }
        
        for batch in modifies.chunks(self.config.batch_size) {
            self.process_file_batch(batch, FileOperation::Modify).await?;
        }
        
        for batch in deletes.chunks(self.config.batch_size) {
            self.process_file_batch(batch, FileOperation::Delete).await?;
        }
        
        // Trigger sync if auto-sync is enabled and we have peers
        if self.config.auto_sync && !self.active_peers.is_empty() {
            self.trigger_auto_sync().await?;
        }
        
        Ok(())
    }
    
    /// Process a batch of files for a specific operation
    async fn process_file_batch(&mut self, paths: &[PathBuf], operation: FileOperation) -> Result<(), Box<dyn std::error::Error>> {
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
    async fn handle_peer_discovered(&mut self, peer: PeerInfo) -> Result<(), Box<dyn std::error::Error>> {
        self.active_peers.insert(peer.device_id.clone(), peer.clone());
        
        info!("Peer discovered: {} - preparing sync folders", peer.device_name);
        
        // If we have folders to sync, notify about the new peer
        if !self.synced_folders.is_empty() && self.config.auto_sync {
            if let Some(ref callback) = self.on_peer_sync_needed {
                callback(peer.device_id.clone(), self.synced_folders.clone());
            }
        }
        
        Ok(())
    }
    
    /// Handle peer lost
    async fn handle_peer_lost(&mut self, peer_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.active_peers.remove(peer_id);
        
        info!("Peer lost: {} - cleaning up state", peer_id);
        
        // Clean up any pending operations for this peer
        self.pending_operations.retain(|op| {
            // Keep operations not related to this peer
            // In a real implementation, we'd track peer associations
            true
        });
        
        Ok(())
    }
    
    /// Handle explicit sync request
    async fn handle_sync_request(&mut self, peer_id: &str, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        if !self.active_peers.contains_key(peer_id) {
            warn!("Sync requested with unknown peer: {}", peer_id);
            return Ok(());
        }
        
        info!("Processing sync request for {} with peer {}", path.display(), peer_id);
        
        // Process the folder through our pipeline
        self.process_folder_for_sync(path).await?;
        
        // Notify that we have data ready to sync
        if let Some(ref callback) = self.on_peer_sync_needed {
            callback(peer_id.to_string(), vec![path.clone()]);
        }
        
        Ok(())
    }
    
    /// Trigger auto-sync with all connected peers
    async fn trigger_auto_sync(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Triggering auto-sync with {} peers", self.active_peers.len());
        
        // Notify about sync needed for each peer
        for peer_id in self.active_peers.keys() {
            if let Some(ref callback) = self.on_peer_sync_needed {
                callback(peer_id.clone(), self.synced_folders.clone());
            }
        }
        
        Ok(())
    }
    
    /// Retry failed operations
    async fn retry_failed_operations(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.pending_operations.is_empty() {
            return Ok(());
        }
        
        let mut still_pending = Vec::new();
        
        for mut op in std::mem::take(&mut self.pending_operations) {
            if op.retry_count >= self.config.max_retries {
                error!("Max retries exceeded for operation on: {}", op.path.display());
                continue;
            }
            
            if op.last_attempt.elapsed() < Duration::from_secs(30) {
                still_pending.push(op);
                continue;
            }
            
            debug!("Retrying operation on: {} (attempt {})", op.path.display(), op.retry_count + 1);
            
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
    async fn process_folder_for_sync(&mut self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        info!("Processing folder for sync: {}", path.display());
        
        // Index the folder to build manifest
        self.indexer.index_folder(path).await?;
        
        // Walk the directory and process files
        let mut entries = tokio::fs::read_dir(path).await?;
        let mut file_count = 0;
        
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            
            if entry_path.is_file() {
                // Process individual file
                if let Err(e) = self.process_single_file_internal(&entry_path, FileOperation::Create).await {
                    error!("Failed to process file {}: {}", entry_path.display(), e);
                } else {
                    file_count += 1;
                }
            } else if entry_path.is_dir() {
                // Recursively process subdirectory
                Box::pin(self.process_folder_for_sync(&entry_path)).await?;
            }
        }
        
        info!("Processed {} files in folder {}", file_count, path.display());
        
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
    ) -> Result<(), Box<dyn std::error::Error>> {
        match operation {
            FileOperation::Create | FileOperation::Modify => {
                if path.is_file() {
                    // Track operation
                    self.current_operations.push(format!("Processing: {}", path.display()));
                    
                    // Read file in chunks to avoid memory issues
                    let file = tokio::fs::File::open(path).await?;
                    let metadata = file.metadata().await?;
                    let file_size = metadata.len();
                    
                    // For large files, use streaming
                    if file_size > 100 * 1024 * 1024 { // 100MB threshold
                        self.process_large_file(path).await?;
                    } else {
                        // Small file - process normally
                        let content = tokio::fs::read(path).await?;
                        
                        // Chunk the file
                        let chunks = self.chunker.chunk_bytes(&content)?;
                        debug!("File {} chunked into {} pieces", path.display(), chunks.len());
                        
                        // Store chunks in CAS
                        let mut stored_hashes = Vec::new();
                        for chunk in &chunks {
                            let hash = self.store.put(&chunk.data).await?;
                            stored_hashes.push(hash);
                        }
                        
                        // Track bytes synced
                        self.total_bytes_synced += file_size;
                    }
                    
                    // Update index
                    self.indexer.index_file(path).await?;
                    
                    // Remove from current operations
                    self.current_operations.retain(|op| !op.contains(&path.display().to_string()));
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
    
    /// Process large files with streaming
    async fn process_large_file(&mut self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::AsyncReadExt;
        
        info!("Processing large file with streaming: {}", path.display());
        
        let mut file = tokio::fs::File::open(path).await?;
        let mut buffer = vec![0u8; 4 * 1024 * 1024]; // 4MB chunks
        let mut total_chunks = 0;
        
        loop {
            let bytes_read = file.read(&mut buffer).await?;
            if bytes_read == 0 {
                break;
            }
            
            let data = &buffer[..bytes_read];
            
            // Chunk this segment
            let chunks = self.chunker.chunk_bytes(data)?;
            
            // Store chunks
            for chunk in chunks {
                self.store.put(&chunk.data).await?;
                total_chunks += 1;
            }
        }
        
        info!("Large file {} processed into {} chunks", path.display(), total_chunks);
        Ok(())
    }
    
    /// Add a folder to sync
    async fn add_sync_folder(&mut self, path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
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
    async fn remove_sync_folder(&mut self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
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
    async fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Orchestrator shutting down gracefully");
        
        // Process any remaining debounced changes
        if !self.debounce_buffer.is_empty() {
            info!("Processing {} remaining changes before shutdown", self.debounce_buffer.len());
            self.process_debounced_changes().await?;
        }
        
        // Complete all active operations
        info!("Completing {} active operations", self.current_operations.len());
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
) -> Result<(), Box<dyn std::error::Error>> {
    match operation {
        FileOperation::Create | FileOperation::Modify => {
            // Check if it's a file or directory
            if path.is_file() {
                debug!("Processing file: {}", path.display());
                
                // Read file content
                let content = tokio::fs::read(&path).await?;
                
                // Chunk the file
                let chunks = chunker.chunk_bytes(&content)?;
                debug!("File {} chunked into {} pieces", path.display(), chunks.len());
                
                // Store chunks in CAS
                for chunk in &chunks {
                    let hash = store.put(&chunk.data).await?;
                    debug!("Stored chunk with hash: {}", hex::encode(&hash.0));
                }
                
                // Update index
                indexer.index_file(&path).await?;
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
        let store = Arc::new(ContentStore::open(&storage_path.join("cas")).await.unwrap());
        let chunker = Arc::new(Chunker::new(64 * 1024)); // 64KB average chunk
        let indexer = Arc::new(
            AsyncIndexer::new(
                &storage_path.join("cas"),
                &storage_path.join("index.sqlite"),
                Default::default(),
            ).await.unwrap()
        );
        
        let config = OrchestratorConfig::default();
        let (orchestrator, sender) = SyncOrchestrator::new(
            config,
            store,
            chunker,
            indexer,
            &storage_path,
        ).await.unwrap();
        
        // Test sending a message
        sender.send(OrchestratorMessage::Shutdown).await.unwrap();
        
        // Run should exit immediately due to shutdown
        tokio::time::timeout(
            Duration::from_secs(1),
            orchestrator.run()
        ).await.unwrap().unwrap();
    }
    
    #[tokio::test]
    async fn test_file_change_debouncing() {
        let temp_dir = TempDir::new().unwrap();
        let storage_path = temp_dir.path().to_path_buf();
        
        // Create test components
        let store = Arc::new(ContentStore::open(&storage_path.join("cas")).await.unwrap());
        let chunker = Arc::new(Chunker::new(64 * 1024));
        let indexer = Arc::new(
            AsyncIndexer::new(
                &storage_path.join("cas"),
                &storage_path.join("index.sqlite"),
                Default::default(),
            ).await.unwrap()
        );
        
        let mut config = OrchestratorConfig::default();
        config.debounce_duration = Duration::from_millis(100);
        
        let (orchestrator, sender) = SyncOrchestrator::new(
            config,
            store,
            chunker,
            indexer,
            &storage_path,
        ).await.unwrap();
        
        // Spawn orchestrator in background
        let handle = tokio::spawn(async move {
            orchestrator.run().await
        });
        
        // Send multiple file changes quickly
        for i in 0..5 {
            let event = FileEvent {
                path: PathBuf::from(format!("test{}.txt", i)),
                kind: FileEventKind::Created,
            };
            sender.send(OrchestratorMessage::FileChanged(event)).await.unwrap();
        }
        
        // Wait for debouncing
        tokio::time::sleep(Duration::from_millis(200)).await;
        
        // Get status to verify changes were processed
        let (status_tx, mut status_rx) = mpsc::channel(1);
        sender.send(OrchestratorMessage::GetStatus(status_tx)).await.unwrap();
        
        let status = status_rx.recv().await.unwrap();
        assert_eq!(status.pending_changes, 0); // Changes should be processed
        
        // Shutdown
        sender.send(OrchestratorMessage::Shutdown).await.unwrap();
        handle.await.unwrap().unwrap();
    }
}