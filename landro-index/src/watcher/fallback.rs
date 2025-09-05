//! Fallback file watcher implementation using the notify crate
//!
//! This provides a cross-platform implementation when platform-specific
//! optimizations are unavailable or disabled.

use async_trait::async_trait;
use notify::{
    Config, Event, EventKind as NotifyEventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, trace, warn};

use super::{
    EventKind, FsEvent, PlatformFlags, PlatformWatcher, WatcherCapabilities, WatcherStats,
};
use crate::errors::{IndexError, Result};

/// Fallback watcher using the notify crate
pub struct FallbackWatcher {
    /// notify watcher instance
    watcher: Option<RecommendedWatcher>,
    /// Channel for receiving events
    event_rx: Arc<RwLock<mpsc::UnboundedReceiver<FsEvent>>>,
    event_tx: mpsc::UnboundedSender<FsEvent>,
    /// Paths currently being watched
    watched_paths: Arc<RwLock<HashSet<PathBuf>>>,
    /// Statistics
    stats: Arc<RwLock<WatcherStats>>,
}

impl FallbackWatcher {
    /// Create a new fallback watcher
    pub async fn new() -> Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let event_tx_clone = event_tx.clone();

        // Create notify watcher
        let watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                match res {
                    Ok(event) => {
                        if let Some(fs_event) = Self::convert_notify_event(event) {
                            if let Err(_) = event_tx_clone.send(fs_event) {
                                // Channel closed, stop processing
                                error!("Event channel closed");
                            }
                        }
                    }
                    Err(e) => {
                        error!("notify watcher error: {}", e);
                    }
                }
            },
            Config::default(),
        )
        .map_err(|e| IndexError::WatcherError(format!("Failed to create notify watcher: {}", e)))?;

        Ok(Self {
            watcher: Some(watcher),
            event_rx: Arc::new(RwLock::new(event_rx)),
            event_tx,
            watched_paths: Arc::new(RwLock::new(HashSet::new())),
            stats: Arc::new(RwLock::new(WatcherStats::default())),
        })
    }

    /// Convert notify Event to our FsEvent
    fn convert_notify_event(event: Event) -> Option<FsEvent> {
        let kind = match event.kind {
            NotifyEventKind::Create(_) => EventKind::Created,
            NotifyEventKind::Modify(modify_kind) => {
                use notify::event::ModifyKind;
                match modify_kind {
                    ModifyKind::Data(_) | ModifyKind::Any => EventKind::Modified,
                    ModifyKind::Name(rename_mode) => {
                        use notify::event::RenameMode;
                        match rename_mode {
                            RenameMode::Both if event.paths.len() >= 2 => {
                                // Handle rename with both old and new paths
                                return Some(FsEvent {
                                    kind: EventKind::Renamed,
                                    path: event.paths[1].clone(),
                                    old_path: Some(event.paths[0].clone()),
                                    file_id: None,
                                    timestamp: SystemTime::now(),
                                    flags: PlatformFlags::default(),
                                });
                            }
                            _ => EventKind::Modified,
                        }
                    }
                    ModifyKind::Metadata(_) => EventKind::AttributeChanged,
                    _ => EventKind::Modified,
                }
            }
            NotifyEventKind::Remove(_) => EventKind::Removed,
            NotifyEventKind::Any | NotifyEventKind::Other => {
                // Trigger rescan for unknown events
                EventKind::Rescan
            }
            _ => return None,
        };

        let path = event.paths.first()?.clone();

        // Get basic file information if possible
        let (is_directory, is_symlink, file_size) = match std::fs::metadata(&path) {
            Ok(metadata) => (
                metadata.is_dir(),
                metadata.file_type().is_symlink(),
                Some(metadata.len()),
            ),
            Err(_) => (false, false, None),
        };

        Some(FsEvent {
            kind,
            path,
            old_path: None,
            file_id: None, // notify doesn't provide file IDs
            timestamp: SystemTime::now(),
            flags: PlatformFlags {
                #[cfg(target_os = "macos")]
                fsevent_flags: None,
                #[cfg(target_os = "linux")]
                inotify_mask: None,
                #[cfg(target_os = "windows")]
                usn_record: None,
                is_directory,
                is_symlink,
                file_size,
            },
        })
    }
}

#[async_trait]
impl PlatformWatcher for FallbackWatcher {
    async fn watch(&mut self, path: &Path, recursive: bool) -> Result<()> {
        let canonical_path = path
            .canonicalize()
            .map_err(|e| IndexError::WatcherError(format!("Cannot canonicalize path: {}", e)))?;

        debug!(
            "Adding fallback watch for path: {:?} (recursive: {})",
            canonical_path, recursive
        );

        if let Some(ref mut watcher) = self.watcher {
            let mode = if recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };

            watcher
                .watch(&canonical_path, mode)
                .map_err(|e| IndexError::WatcherError(format!("Failed to watch path: {}", e)))?;

            self.watched_paths.write().await.insert(canonical_path);

            let mut stats = self.stats.write().await;
            stats.paths_watched += 1;
        }

        Ok(())
    }

    async fn unwatch(&mut self, path: &Path) -> Result<()> {
        let canonical_path = path
            .canonicalize()
            .map_err(|e| IndexError::WatcherError(format!("Cannot canonicalize path: {}", e)))?;

        debug!("Removing fallback watch for path: {:?}", canonical_path);

        if let Some(ref mut watcher) = self.watcher {
            watcher
                .unwatch(&canonical_path)
                .map_err(|e| IndexError::WatcherError(format!("Failed to unwatch path: {}", e)))?;

            let removed = self.watched_paths.write().await.remove(&canonical_path);

            if removed {
                let mut stats = self.stats.write().await;
                stats.paths_watched = stats.paths_watched.saturating_sub(1);
            }
        }

        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<FsEvent>> {
        let mut events = Vec::new();
        let mut rx = self.event_rx.write().await;

        // Collect all available events
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        if !events.is_empty() {
            let mut stats = self.stats.write().await;
            stats.events_processed += events.len() as u64;
            trace!("Processed {} fallback events", events.len());
        }

        Ok(events)
    }

    fn capabilities(&self) -> WatcherCapabilities {
        WatcherCapabilities {
            supports_rename_tracking: true, // notify can track renames on some platforms
            supports_file_id: false,        // notify doesn't provide file IDs
            max_watches: None,              // notify handles platform limits internally
            supports_overflow_detection: false, // notify doesn't expose overflow info
            supports_network_paths: true,   // Depends on platform, but generally yes
            max_path_length: None,          // Platform dependent
            provides_file_size: true,       // We stat the file ourselves
        }
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(self.watcher.is_some())
    }

    async fn get_stats(&self) -> WatcherStats {
        let mut stats = self.stats.read().await.clone();
        stats.paths_watched = self.watched_paths.read().await.len();
        stats
    }
}

impl Drop for FallbackWatcher {
    fn drop(&mut self) {
        // notify watcher will be dropped automatically
        self.watcher.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::time::{sleep, timeout, Duration};

    #[tokio::test]
    async fn test_fallback_watcher_creation() {
        let watcher = FallbackWatcher::new().await;
        assert!(watcher.is_ok());

        let watcher = watcher.unwrap();
        let capabilities = watcher.capabilities();
        assert!(capabilities.supports_rename_tracking);
        assert!(!capabilities.supports_file_id);
    }

    #[tokio::test]
    async fn test_watch_unwatch() {
        let temp_dir = tempdir().unwrap();
        let mut watcher = FallbackWatcher::new().await.unwrap();

        // Start watching
        watcher.watch(temp_dir.path(), true).await.unwrap();
        assert!(watcher.health_check().await.unwrap());

        let stats = watcher.get_stats().await;
        assert_eq!(stats.paths_watched, 1);

        // Stop watching
        watcher.unwatch(temp_dir.path()).await.unwrap();
        let stats = watcher.get_stats().await;
        assert_eq!(stats.paths_watched, 0);
    }

    #[tokio::test]
    async fn test_file_events() {
        let temp_dir = tempdir().unwrap();
        let mut watcher = FallbackWatcher::new().await.unwrap();

        watcher.watch(temp_dir.path(), true).await.unwrap();

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "Hello, World!").await.unwrap();

        // Give notify time to process
        sleep(Duration::from_millis(200)).await;

        // Check for events
        let events = timeout(Duration::from_secs(1), async {
            loop {
                let events = watcher.poll_events().await.unwrap();
                if !events.is_empty() {
                    return events;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await;

        assert!(events.is_ok());
        let events = events.unwrap();

        // Should have at least one event
        assert!(!events.is_empty());

        // Should be a creation event
        let create_event = events.iter().find(|e| e.kind == EventKind::Created);
        assert!(create_event.is_some());
    }

    #[test]
    fn test_convert_notify_event() {
        use notify::event::{CreateKind, EventKind as NotifyEventKind};

        let notify_event = Event {
            kind: NotifyEventKind::Create(CreateKind::File),
            paths: vec![PathBuf::from("/test/file.txt")],
            attrs: Default::default(),
        };

        let fs_event = FallbackWatcher::convert_notify_event(notify_event);
        assert!(fs_event.is_some());

        let fs_event = fs_event.unwrap();
        assert_eq!(fs_event.kind, EventKind::Created);
        assert_eq!(fs_event.path, PathBuf::from("/test/file.txt"));
    }
}
