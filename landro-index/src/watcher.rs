//! File system watcher for detecting changes in monitored folders

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, trace, warn};

use crate::errors::{IndexError, Result};
use crate::indexer::{FileIndexer, IndexerConfig};
use crate::manifest::Manifest;

/// Event types for file system changes
#[derive(Debug, Clone)]
pub enum FsEvent {
    /// File or directory created
    Created(PathBuf),
    /// File content modified
    Modified(PathBuf),
    /// File or directory removed
    Removed(PathBuf),
    /// File or directory renamed
    Renamed { from: PathBuf, to: PathBuf },
    /// Multiple changes detected, trigger full rescan
    Rescan(PathBuf),
}

/// Debounces file system events to avoid processing rapid changes
struct EventDebouncer {
    pending: HashMap<PathBuf, (FsEvent, Instant)>,
    delay: Duration,
}

impl EventDebouncer {
    fn new(delay: Duration) -> Self {
        Self {
            pending: HashMap::new(),
            delay,
        }
    }

    /// Add an event to the debouncer
    fn add_event(&mut self, event: FsEvent) {
        let path = match &event {
            FsEvent::Created(p) | FsEvent::Modified(p) | FsEvent::Removed(p) | FsEvent::Rescan(p) => p.clone(),
            FsEvent::Renamed { to, .. } => to.clone(),
        };

        self.pending.insert(path, (event, Instant::now()));
    }

    /// Get events that have been stable for the debounce delay
    fn get_ready_events(&mut self) -> Vec<FsEvent> {
        let now = Instant::now();
        let mut ready = Vec::new();

        self.pending.retain(|_path, (event, timestamp)| {
            if now.duration_since(*timestamp) >= self.delay {
                ready.push(event.clone());
                false // Remove from pending
            } else {
                true // Keep in pending
            }
        });

        ready
    }

    /// Check if there are any pending events
    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// Configuration for the folder watcher
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Debounce delay for file system events (default: 500ms)
    pub debounce_delay: Duration,
    /// Whether to watch recursively
    pub recursive: bool,
    /// Patterns to ignore (same as IndexerConfig)
    pub ignore_patterns: Vec<String>,
    /// Maximum events to batch before processing
    pub batch_size: usize,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            debounce_delay: Duration::from_millis(500),
            recursive: true,
            ignore_patterns: vec![
                ".git".to_string(),
                ".DS_Store".to_string(),
                "Thumbs.db".to_string(),
                "*.tmp".to_string(),
            ],
            batch_size: 100,
        }
    }
}

/// Watches folders for changes and triggers re-indexing
pub struct FolderWatcher {
    watcher: RecommendedWatcher,
    event_tx: mpsc::UnboundedSender<FsEvent>,
    event_rx: Arc<RwLock<mpsc::UnboundedReceiver<FsEvent>>>,
    watched_folders: Arc<RwLock<HashMap<PathBuf, String>>>, // Path -> FolderID
    config: WatcherConfig,
    debouncer: Arc<RwLock<EventDebouncer>>,
}

impl FolderWatcher {
    /// Create a new folder watcher
    pub fn new(config: WatcherConfig) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let event_tx_clone = event_tx.clone();
        
        // Create the notify watcher
        let watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                match res {
                    Ok(event) => {
                        if let Some(fs_event) = Self::convert_notify_event(event) {
                            let _ = event_tx_clone.send(fs_event);
                        }
                    }
                    Err(e) => {
                        error!("File watcher error: {}", e);
                    }
                }
            },
            Config::default(),
        ).map_err(|e| IndexError::WatcherError(e.to_string()))?;

        Ok(Self {
            watcher,
            event_tx,
            event_rx: Arc::new(RwLock::new(event_rx)),
            watched_folders: Arc::new(RwLock::new(HashMap::new())),
            debouncer: Arc::new(RwLock::new(EventDebouncer::new(config.debounce_delay))),
            config,
        })
    }

    /// Start watching a folder
    pub async fn watch_folder(&mut self, path: impl AsRef<Path>, folder_id: String) -> Result<()> {
        let path = path.as_ref();
        let canonical_path = path.canonicalize()
            .map_err(|e| IndexError::Io(e))?;

        info!("Starting to watch folder: {:?} (ID: {})", canonical_path, folder_id);

        // Add to notify watcher
        let mode = if self.config.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        self.watcher
            .watch(&canonical_path, mode)
            .map_err(|e| IndexError::WatcherError(e.to_string()))?;

        // Track the folder
        self.watched_folders.write().await.insert(canonical_path, folder_id);

        Ok(())
    }

    /// Stop watching a folder
    pub async fn unwatch_folder(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let canonical_path = path.canonicalize()
            .map_err(|e| IndexError::Io(e))?;

        info!("Stopping watch on folder: {:?}", canonical_path);

        // Remove from notify watcher
        self.watcher
            .unwatch(&canonical_path)
            .map_err(|e| IndexError::WatcherError(e.to_string()))?;

        // Remove from tracking
        self.watched_folders.write().await.remove(&canonical_path);

        Ok(())
    }

    /// Process events with debouncing and batching
    pub async fn process_events<F>(&self, mut handler: F) -> Result<()>
    where
        F: FnMut(Vec<FsEvent>) -> Result<()>,
    {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        let mut batch = Vec::new();

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Check for debounced events ready to process
                    let ready_events = self.debouncer.write().await.get_ready_events();
                    
                    for event in ready_events {
                        batch.push(event);
                        
                        // Process batch if it reaches the configured size
                        if batch.len() >= self.config.batch_size {
                            handler(std::mem::take(&mut batch))?;
                        }
                    }
                    
                    // Process any remaining events if debouncer is empty
                    if !batch.is_empty() && !self.debouncer.read().await.has_pending() {
                        handler(std::mem::take(&mut batch))?;
                    }
                }
                
                // Receive new events from the watcher
                Some(event) = async {
                    self.event_rx.write().await.recv().await
                } => {
                    // Filter ignored paths
                    if !self.should_ignore_event(&event) {
                        self.debouncer.write().await.add_event(event);
                    }
                }
            }
        }
    }

    /// Start the watcher with automatic re-indexing
    /// Note: This should be called from a dedicated task due to Send constraints with FileIndexer
    pub async fn start_with_indexer(
        self: Arc<Self>,
        indexer: Arc<RwLock<FileIndexer>>,
    ) -> Result<()> {
        info!("Starting folder watcher with automatic indexing");

        let watched_folders = self.watched_folders.clone();
        
        self.process_events(move |events| {
            // Process events directly in the handler without spawning
            // This is necessary because FileIndexer contains rusqlite::Connection which is not Send
            for event in events {
                let indexer = indexer.clone();
                let watched_folders = watched_folders.clone();
                
                // Using block_on is not ideal but necessary due to Send constraints
                // In production, consider using a channel-based approach
                let runtime = tokio::runtime::Handle::current();
                runtime.block_on(async {
                    if let Err(e) = Self::handle_fs_event(event, &indexer, &watched_folders).await {
                        error!("Error handling file system event: {}", e);
                    }
                });
            }
            
            Ok(())
        }).await
    }

    /// Handle a single file system event
    async fn handle_fs_event(
        event: FsEvent,
        indexer: &Arc<RwLock<FileIndexer>>,
        watched_folders: &Arc<RwLock<HashMap<PathBuf, String>>>,
    ) -> Result<()> {
        let path = match &event {
            FsEvent::Created(p) | FsEvent::Modified(p) | FsEvent::Removed(p) => p,
            FsEvent::Renamed { to, .. } => to,
            FsEvent::Rescan(p) => p,
        };

        // Find which watched folder this path belongs to
        let folders = watched_folders.read().await;
        let folder_entry = folders.iter()
            .find(|(folder_path, _)| path.starts_with(folder_path));

        if let Some((folder_path, folder_id)) = folder_entry {
            debug!("Event {:?} in folder {} ({})", event, folder_id, folder_path.display());

            // Trigger re-indexing based on event type
            match event {
                FsEvent::Created(_) | FsEvent::Modified(_) | FsEvent::Renamed { .. } => {
                    // Re-index the specific file or directory
                    // In a real implementation, we'd do incremental indexing
                    info!("Re-indexing folder {} due to changes", folder_id);
                    let mut indexer = indexer.write().await;
                    let _manifest = indexer.index_folder(folder_path).await?;
                }
                FsEvent::Removed(_) => {
                    // Handle file removal - update manifest
                    info!("File removed in folder {}, updating manifest", folder_id);
                    // TODO: Implement removal handling in indexer
                }
                FsEvent::Rescan(_) => {
                    // Full rescan needed
                    info!("Full rescan triggered for folder {}", folder_id);
                    let mut indexer = indexer.write().await;
                    let _manifest = indexer.index_folder(folder_path).await?;
                }
            }
        }

        Ok(())
    }

    /// Convert notify event to our FsEvent type
    fn convert_notify_event(event: Event) -> Option<FsEvent> {
        match event.kind {
            EventKind::Create(_) => {
                event.paths.first().map(|p| FsEvent::Created(p.clone()))
            }
            EventKind::Modify(modify_kind) => {
                use notify::event::ModifyKind;
                match modify_kind {
                    ModifyKind::Data(_) | ModifyKind::Any => {
                        event.paths.first().map(|p| FsEvent::Modified(p.clone()))
                    }
                    ModifyKind::Name(rename_mode) => {
                        use notify::event::RenameMode;
                        match rename_mode {
                            RenameMode::Both if event.paths.len() >= 2 => {
                                Some(FsEvent::Renamed {
                                    from: event.paths[0].clone(),
                                    to: event.paths[1].clone(),
                                })
                            }
                            _ => event.paths.first().map(|p| FsEvent::Modified(p.clone()))
                        }
                    }
                    _ => None,
                }
            }
            EventKind::Remove(_) => {
                event.paths.first().map(|p| FsEvent::Removed(p.clone()))
            }
            EventKind::Any | EventKind::Other => {
                // Trigger rescan for unknown events
                event.paths.first().map(|p| FsEvent::Rescan(p.clone()))
            }
            _ => None,
        }
    }

    /// Check if an event should be ignored based on patterns
    fn should_ignore_event(&self, event: &FsEvent) -> bool {
        let path = match event {
            FsEvent::Created(p) | FsEvent::Modified(p) | FsEvent::Removed(p) | FsEvent::Rescan(p) => p,
            FsEvent::Renamed { to, .. } => to,
        };

        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        for pattern in &self.config.ignore_patterns {
            if pattern.contains('*') {
                if Self::glob_match(pattern, name) {
                    trace!("Ignoring event for: {:?} (matches pattern: {})", path, pattern);
                    return true;
                }
            } else if name == pattern {
                trace!("Ignoring event for: {:?} (matches pattern: {})", path, pattern);
                return true;
            }
        }

        false
    }

    /// Simple glob pattern matching
    fn glob_match(pattern: &str, name: &str) -> bool {
        if pattern.starts_with('*') {
            let suffix = &pattern[1..];
            return name.ends_with(suffix);
        }
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            return name.starts_with(prefix);
        }
        pattern == name
    }

    /// Get list of currently watched folders
    pub async fn watched_folders(&self) -> Vec<(PathBuf, String)> {
        self.watched_folders.read().await
            .iter()
            .map(|(p, id)| (p.clone(), id.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_event_debouncer() {
        let mut debouncer = EventDebouncer::new(Duration::from_millis(100));
        
        let path = PathBuf::from("/test/file.txt");
        debouncer.add_event(FsEvent::Created(path.clone()));
        
        // No events ready immediately
        assert!(debouncer.get_ready_events().is_empty());
        assert!(debouncer.has_pending());
        
        // Wait for debounce delay
        sleep(Duration::from_millis(150)).await;
        
        // Event should be ready now
        let ready = debouncer.get_ready_events();
        assert_eq!(ready.len(), 1);
        assert!(!debouncer.has_pending());
    }

    #[tokio::test]
    async fn test_watch_folder() {
        let temp_dir = tempdir().unwrap();
        let config = WatcherConfig::default();
        let mut watcher = FolderWatcher::new(config).unwrap();
        
        // Start watching the temp directory
        watcher.watch_folder(temp_dir.path(), "test-folder".to_string())
            .await
            .unwrap();
        
        // Verify it's being watched
        let watched = watcher.watched_folders().await;
        assert_eq!(watched.len(), 1);
        assert_eq!(watched[0].1, "test-folder");
        
        // Stop watching
        watcher.unwatch_folder(temp_dir.path()).await.unwrap();
        
        let watched = watcher.watched_folders().await;
        assert!(watched.is_empty());
    }

    #[test]
    fn test_glob_matching() {
        assert!(FolderWatcher::glob_match("*.tmp", "test.tmp"));
        assert!(FolderWatcher::glob_match("test*", "test.txt"));
        assert!(!FolderWatcher::glob_match("*.tmp", "test.txt"));
        assert!(!FolderWatcher::glob_match("test*", "other.txt"));
    }

    #[test]
    fn test_event_filtering() {
        let config = WatcherConfig {
            ignore_patterns: vec!["*.tmp".to_string(), ".git".to_string()],
            ..Default::default()
        };
        
        let watcher = FolderWatcher::new(config).unwrap();
        
        // Should ignore .tmp files
        let event = FsEvent::Created(PathBuf::from("/test/file.tmp"));
        assert!(watcher.should_ignore_event(&event));
        
        // Should ignore .git directory
        let event = FsEvent::Created(PathBuf::from("/test/.git"));
        assert!(watcher.should_ignore_event(&event));
        
        // Should not ignore regular files
        let event = FsEvent::Created(PathBuf::from("/test/file.txt"));
        assert!(!watcher.should_ignore_event(&event));
    }
}