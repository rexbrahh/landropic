//! File system watcher for detecting changes in monitored folders

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, trace};

use crate::errors::{IndexError, Result};
use crate::indexer::FileIndexer;

// Re-export platform watcher types for backward compatibility
pub use crate::watcher::{
    create_platform_watcher, should_ignore_path, EventDebouncer, EventKind, FsEvent, PlatformFlags,
    PlatformWatcher, WatcherCapabilities, WatcherConfig, WatcherStats,
};

// Legacy FsEvent enum for backward compatibility
#[derive(Debug, Clone)]
#[deprecated(note = "Use crate::watcher::FsEvent instead")]
pub enum LegacyFsEvent {
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

impl From<FsEvent> for LegacyFsEvent {
    fn from(event: FsEvent) -> Self {
        match event.kind {
            EventKind::Created => LegacyFsEvent::Created(event.path),
            EventKind::Modified => LegacyFsEvent::Modified(event.path),
            EventKind::Removed => LegacyFsEvent::Removed(event.path),
            EventKind::AttributeChanged => LegacyFsEvent::Modified(event.path),
            EventKind::Renamed => {
                if let Some(old_path) = event.old_path {
                    LegacyFsEvent::Renamed {
                        from: old_path,
                        to: event.path,
                    }
                } else {
                    LegacyFsEvent::Modified(event.path)
                }
            }
            EventKind::Rescan => LegacyFsEvent::Rescan(event.path),
        }
    }
}

// EventDebouncer is now provided by the watcher module

// WatcherConfig is now provided by the watcher module

/// Enhanced folder watcher using platform-specific optimizations
pub struct FolderWatcher {
    /// Platform-specific watcher implementation
    platform_watcher: Box<dyn PlatformWatcher>,
    /// Watched folders mapping (Path -> FolderID)
    watched_folders: Arc<RwLock<HashMap<PathBuf, String>>>,
    /// Watcher configuration
    config: WatcherConfig,
    /// Event debouncer
    debouncer: Arc<RwLock<EventDebouncer>>,
}

impl FolderWatcher {
    /// Create a new folder watcher with platform-specific optimizations
    pub async fn new(config: WatcherConfig) -> Result<Self> {
        let platform_watcher = if config.use_platform_optimizations {
            create_platform_watcher().await?
        } else {
            // Force use of fallback implementation
            Box::new(crate::watcher::fallback::FallbackWatcher::new().await?)
        };

        info!(
            "Created folder watcher with capabilities: {:?}",
            platform_watcher.capabilities()
        );

        Ok(Self {
            platform_watcher,
            watched_folders: Arc::new(RwLock::new(HashMap::new())),
            debouncer: Arc::new(RwLock::new(EventDebouncer::new(config.debounce_delay))),
            config,
        })
    }

    /// Create a new folder watcher (sync version for backward compatibility)
    pub fn new_sync(config: WatcherConfig) -> Result<Self> {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(Self::new(config))
        })
    }

    /// Start watching a folder
    pub async fn watch_folder(&mut self, path: impl AsRef<Path>, folder_id: String) -> Result<()> {
        let path = path.as_ref();
        let canonical_path = path.canonicalize().map_err(|e| IndexError::Io(e))?;

        info!(
            "Starting to watch folder: {:?} (ID: {}) using platform watcher",
            canonical_path, folder_id
        );

        // Add to platform watcher
        self.platform_watcher
            .watch(&canonical_path, self.config.recursive)
            .await?;

        // Track the folder
        self.watched_folders
            .write()
            .await
            .insert(canonical_path, folder_id);

        Ok(())
    }

    /// Stop watching a folder
    pub async fn unwatch_folder(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let canonical_path = path.canonicalize().map_err(|e| IndexError::Io(e))?;

        info!("Stopping watch on folder: {:?}", canonical_path);

        // Remove from platform watcher
        self.platform_watcher.unwatch(&canonical_path).await?;

        // Remove from tracking
        self.watched_folders.write().await.remove(&canonical_path);

        Ok(())
    }

    /// Process events with debouncing and batching
    pub async fn process_events<F>(&mut self, mut handler: F) -> Result<()>
    where
        F: FnMut(Vec<FsEvent>) -> Result<()>,
    {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        let mut batch = Vec::new();

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Poll for new events from platform watcher
                    let new_events = self.platform_watcher.poll_events().await?;

                    // Filter and debounce events
                    for event in new_events {
                        if !self.should_ignore_event(&event).await {
                            self.debouncer.write().await.add_event(event);
                        }
                    }

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
            }
        }
    }

    /// Start the watcher with automatic re-indexing
    pub async fn start_with_indexer(mut self, indexer: Arc<RwLock<FileIndexer>>) -> Result<()> {
        info!("Starting enhanced folder watcher with automatic indexing");

        let watched_folders = self.watched_folders.clone();

        // Use a channel to handle events asynchronously
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<FsEvent>();

        // Spawn a task to handle events from the channel
        let indexer_clone = indexer.clone();
        let watched_folders_clone = watched_folders.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let Err(e) =
                    Self::handle_fs_event(event, &indexer_clone, &watched_folders_clone).await
                {
                    error!("Error handling file system event: {}", e);
                }
            }
        });

        // Process events and forward them to the channel
        self.process_events(move |events| {
            for event in events {
                let _ = event_tx.send(event);
            }
            Ok(())
        })
        .await
    }

    /// Handle a single file system event
    async fn handle_fs_event(
        event: FsEvent,
        indexer: &Arc<RwLock<FileIndexer>>,
        watched_folders: &Arc<RwLock<HashMap<PathBuf, String>>>,
    ) -> Result<()> {
        let path = &event.path;

        // Find which watched folder this path belongs to
        let folders = watched_folders.read().await;
        let folder_entry = folders
            .iter()
            .find(|(folder_path, _)| path.starts_with(folder_path));

        if let Some((folder_path, folder_id)) = folder_entry {
            debug!(
                "Event {:?} ({:?}) in folder {} ({}) - file_id: {:?}",
                event.kind,
                path,
                folder_id,
                folder_path.display(),
                event.file_id
            );

            // Trigger re-indexing based on event type
            match event.kind {
                EventKind::Created | EventKind::Modified | EventKind::Renamed => {
                    // Re-index the specific file or directory
                    // In a real implementation, we'd do incremental indexing
                    info!("Re-indexing folder {} due to {:?}", folder_id, event.kind);
                    let mut indexer = indexer.write().await;
                    let _manifest = indexer.index_folder(folder_path).await?;
                }
                EventKind::Removed => {
                    // Handle file removal - update manifest
                    info!("File removed in folder {}, updating manifest", folder_id);
                    // TODO: Implement removal handling in indexer
                }
                EventKind::AttributeChanged => {
                    // For now, treat attribute changes as modifications
                    info!("Attributes changed in folder {}, re-indexing", folder_id);
                    let mut indexer = indexer.write().await;
                    let _manifest = indexer.index_folder(folder_path).await?;
                }
                EventKind::Rescan => {
                    // Full rescan needed
                    info!("Full rescan triggered for folder {}", folder_id);
                    let mut indexer = indexer.write().await;
                    let _manifest = indexer.index_folder(folder_path).await?;
                }
            }
        }

        Ok(())
    }

    /// Check if an event should be ignored based on patterns
    async fn should_ignore_event(&self, event: &FsEvent) -> bool {
        should_ignore_path(&event.path, &self.config.ignore_patterns)
    }

    /// Get watcher capabilities
    pub async fn capabilities(&self) -> WatcherCapabilities {
        self.platform_watcher.capabilities()
    }

    /// Get watcher statistics
    pub async fn get_stats(&self) -> WatcherStats {
        self.platform_watcher.get_stats().await
    }

    /// Perform a health check on the watcher
    pub async fn health_check(&self) -> Result<bool> {
        self.platform_watcher.health_check().await
    }

    // glob_match and other utility methods are now provided by the watcher module

    /// Get list of currently watched folders
    pub async fn watched_folders(&self) -> Vec<(PathBuf, String)> {
        self.watched_folders
            .read()
            .await
            .iter()
            .map(|(p, id)| (p.clone(), id.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_event_debouncer() {
        let mut debouncer = EventDebouncer::new(Duration::from_millis(100));

        let path = PathBuf::from("/test/file.txt");
        let event = FsEvent {
            kind: EventKind::Created,
            path: path.clone(),
            old_path: None,
            file_id: None,
            timestamp: SystemTime::now(),
            flags: PlatformFlags::default(),
        };

        debouncer.add_event(event);

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
        let mut watcher = FolderWatcher::new(config).await.unwrap();

        // Start watching the temp directory
        watcher
            .watch_folder(temp_dir.path(), "test-folder".to_string())
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
        use crate::watcher::should_ignore_path;
        let patterns = vec!["*.tmp".to_string()];

        assert!(should_ignore_path(
            &PathBuf::from("/test/file.tmp"),
            &patterns
        ));
        assert!(!should_ignore_path(
            &PathBuf::from("/test/file.txt"),
            &patterns
        ));
    }

    #[tokio::test]
    async fn test_event_filtering() {
        let config = WatcherConfig {
            ignore_patterns: vec!["*.tmp".to_string(), ".git".to_string()],
            ..Default::default()
        };

        let watcher = FolderWatcher::new(config).await.unwrap();

        // Should ignore .tmp files
        let event = FsEvent {
            kind: EventKind::Created,
            path: PathBuf::from("/test/file.tmp"),
            old_path: None,
            file_id: None,
            timestamp: SystemTime::now(),
            flags: PlatformFlags::default(),
        };
        assert!(watcher.should_ignore_event(&event).await);

        // Should ignore .git directory
        let event = FsEvent {
            kind: EventKind::Created,
            path: PathBuf::from("/test/.git"),
            old_path: None,
            file_id: None,
            timestamp: SystemTime::now(),
            flags: PlatformFlags::default(),
        };
        assert!(watcher.should_ignore_event(&event).await);

        // Should not ignore regular files
        let event = FsEvent {
            kind: EventKind::Created,
            path: PathBuf::from("/test/file.txt"),
            old_path: None,
            file_id: None,
            timestamp: SystemTime::now(),
            flags: PlatformFlags::default(),
        };
        assert!(!watcher.should_ignore_event(&event).await);
    }

    #[tokio::test]
    async fn test_watcher_capabilities() {
        let config = WatcherConfig::default();
        let watcher = FolderWatcher::new(config).await.unwrap();

        let capabilities = watcher.capabilities().await;

        // All platforms should support basic functionality
        assert!(
            capabilities.max_path_length.is_none() || capabilities.max_path_length.unwrap() > 100
        );
    }

    #[tokio::test]
    async fn test_watcher_health_check() {
        let config = WatcherConfig::default();
        let watcher = FolderWatcher::new(config).await.unwrap();

        // Health check should pass for a newly created watcher
        assert!(watcher.health_check().await.unwrap());
    }
}
