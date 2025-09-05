//! Platform-specific file watchers for optimal performance and reliability
//!
//! This module provides platform-optimized file watching implementations:
//! - macOS: FSEvents API for efficient recursive watching
//! - Linux: inotify with overflow handling and watch limit management  
//! - Windows: USN Journal for tracking all file system changes
//! - Fallback: notify crate for unsupported platforms

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

use crate::errors::{IndexError, Result};

// Platform-specific modules (temporarily disabled)
// TODO: Re-enable after fixing Send/Sync issues
// #[cfg(target_os = "macos")]
// mod macos;
// #[cfg(target_os = "linux")]
// mod linux;  
// #[cfg(target_os = "windows")]
// mod windows;
mod fallback; // notify-based fallback
pub mod legacy; // Legacy FolderWatcher for backward compatibility

// Re-export platform implementation (temporarily disabled)
// #[cfg(target_os = "macos")]
// pub use macos::*;
// #[cfg(target_os = "linux")]
// pub use linux::*;
// #[cfg(target_os = "windows")]
// pub use windows::*;

// Re-export legacy FolderWatcher for backward compatibility
pub use legacy::{FolderWatcher, LegacyFsEvent};

// Common types and traits

/// Enhanced file system event with platform-specific metadata
#[derive(Debug, Clone)]
pub struct FsEvent {
    /// Type of file system operation
    pub kind: EventKind,
    /// Primary path affected by the event
    pub path: PathBuf,
    /// Source path for rename operations
    pub old_path: Option<PathBuf>,
    /// Platform-specific file identifier (inode, file ID, etc.)
    pub file_id: Option<u64>,
    /// When the event occurred
    pub timestamp: SystemTime,
    /// Platform-specific flags and metadata
    pub flags: PlatformFlags,
}

/// Types of file system events
#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    /// File or directory created
    Created,
    /// File content or metadata modified
    Modified,
    /// File or directory removed
    Removed,
    /// File or directory renamed/moved
    Renamed,
    /// Permission or attribute changes
    AttributeChanged,
    /// Multiple rapid changes - suggests bulk operation
    Rescan,
}

/// Platform-specific event flags and metadata
#[derive(Debug, Clone, Default)]
pub struct PlatformFlags {
    /// FSEvents flags (macOS)
    #[cfg(target_os = "macos")]
    pub fsevent_flags: Option<u32>,
    
    /// inotify mask (Linux)  
    #[cfg(target_os = "linux")]
    pub inotify_mask: Option<u32>,
    
    /// USN record data (Windows)
    #[cfg(target_os = "windows")]
    pub usn_record: Option<WindowsUsnRecord>,
    
    /// Whether this event indicates a directory
    pub is_directory: bool,
    /// Whether the event represents a symbolic link
    pub is_symlink: bool,
    /// File size if available from the platform event
    pub file_size: Option<u64>,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
pub struct WindowsUsnRecord {
    pub usn: u64,
    pub reason: u32,
    pub file_attributes: u32,
}

/// Capabilities of a platform watcher implementation
#[derive(Debug, Clone)]
pub struct WatcherCapabilities {
    /// Can track file renames with both old and new paths
    pub supports_rename_tracking: bool,
    /// Provides stable file identifiers across renames
    pub supports_file_id: bool,
    /// Maximum number of concurrent watches (None = unlimited)
    pub max_watches: Option<usize>,
    /// Can detect when event queue overflows
    pub supports_overflow_detection: bool,
    /// Can watch network/remote file systems
    pub supports_network_paths: bool,
    /// Minimum supported path length
    pub max_path_length: Option<usize>,
    /// Can provide file size in events
    pub provides_file_size: bool,
}

/// Trait for platform-specific file watcher implementations
#[async_trait]
pub trait PlatformWatcher: Send + Sync {
    /// Start watching a path (recursively if specified)
    async fn watch(&mut self, path: &Path, recursive: bool) -> Result<()>;
    
    /// Stop watching a specific path
    async fn unwatch(&mut self, path: &Path) -> Result<()>;
    
    /// Poll for new file system events (non-blocking)
    async fn poll_events(&mut self) -> Result<Vec<FsEvent>>;
    
    /// Get the capabilities of this watcher implementation
    fn capabilities(&self) -> WatcherCapabilities;
    
    /// Check if the watcher is healthy and functioning
    async fn health_check(&self) -> Result<bool> {
        Ok(true) // Default implementation
    }
    
    /// Get statistics about the watcher's operation
    async fn get_stats(&self) -> WatcherStats {
        WatcherStats::default()
    }
}

/// Statistics about watcher operation
#[derive(Debug, Clone, Default)]
pub struct WatcherStats {
    /// Total events processed since creation
    pub events_processed: u64,
    /// Number of paths currently being watched
    pub paths_watched: usize,
    /// Number of events currently queued
    pub events_queued: usize,
    /// Whether any watch limits have been hit
    pub at_watch_limit: bool,
    /// Number of overflow events detected
    pub overflow_events: u64,
}

/// Create the best available watcher for the current platform
pub async fn create_platform_watcher() -> Result<Box<dyn PlatformWatcher>> {
    // For now, temporarily disable platform-specific implementations due to
    // complexity of making raw C pointers Send/Sync. These will be re-enabled
    // in a future iteration with proper thread safety.
    
    // TODO: Re-enable platform optimizations after fixing thread safety
    // #[cfg(target_os = "macos")]
    // {
    //     match MacosWatcher::new().await {
    //         Ok(watcher) => {
    //             info!("Using macOS FSEvents watcher");
    //             return Ok(Box::new(watcher));
    //         }
    //         Err(e) => {
    //             warn!("Failed to create FSEvents watcher, falling back to notify: {}", e);
    //         }
    //     }
    // }
    
    // #[cfg(target_os = "linux")]
    // {
    //     match LinuxWatcher::new().await {
    //         Ok(watcher) => {
    //             info!("Using Linux inotify watcher");
    //             return Ok(Box::new(watcher));
    //         }
    //         Err(e) => {
    //             warn!("Failed to create inotify watcher, falling back to notify: {}", e);
    //         }
    //     }
    // }
    
    // #[cfg(target_os = "windows")]
    // {
    //     match WindowsWatcher::new().await {
    //         Ok(watcher) => {
    //             info!("Using Windows USN Journal watcher");
    //             return Ok(Box::new(watcher));
    //         }
    //         Err(e) => {
    //             warn!("Failed to create USN Journal watcher, falling back to notify: {}", e);
    //         }
    //     }
    // }
    
    // Use fallback notify-based implementation for now
    info!("Using fallback notify-based watcher (platform optimizations disabled for now)");
    Ok(Box::new(fallback::FallbackWatcher::new().await?))
}

/// Event debouncer that coalesces rapid changes to the same path
pub struct EventDebouncer {
    pending: HashMap<PathBuf, (FsEvent, Instant)>,
    delay: Duration,
}

impl EventDebouncer {
    pub fn new(delay: Duration) -> Self {
        Self {
            pending: HashMap::new(),
            delay,
        }
    }

    /// Add an event to the debouncer
    pub fn add_event(&mut self, event: FsEvent) {
        let path = event.path.clone();
        self.pending.insert(path, (event, Instant::now()));
    }

    /// Get events that have been stable for the debounce delay
    pub fn get_ready_events(&mut self) -> Vec<FsEvent> {
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
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    
    /// Clear all pending events (useful for shutdown)
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

/// Configuration for the watcher system
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Debounce delay for file system events (default: 500ms)
    pub debounce_delay: Duration,
    /// Whether to watch recursively
    pub recursive: bool,
    /// Patterns to ignore
    pub ignore_patterns: Vec<String>,
    /// Maximum events to batch before processing
    pub batch_size: usize,
    /// Whether to use platform-specific optimizations (vs always use fallback)
    pub use_platform_optimizations: bool,
    /// Buffer size for event queues
    pub event_buffer_size: usize,
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
                "*.swp".to_string(),
                "*~".to_string(),
                ".#*".to_string(),  // Emacs lock files
                "#*#".to_string(),   // Emacs backup files
            ],
            batch_size: 100,
            use_platform_optimizations: true,
            event_buffer_size: 1000,
        }
    }
}

/// Check if a path should be ignored based on patterns
pub fn should_ignore_path(path: &Path, patterns: &[String]) -> bool {
    let name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    for pattern in patterns {
        if pattern.contains('*') {
            if glob_match(pattern, name) {
                trace!("Ignoring path: {:?} (matches pattern: {})", path, pattern);
                return true;
            }
        } else if name == pattern {
            trace!("Ignoring path: {:?} (matches pattern: {})", path, pattern);
            return true;
        }
    }

    false
}

/// Simple glob pattern matching for ignore patterns
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
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

    #[test]
    fn test_glob_matching() {
        assert!(glob_match("*.tmp", "test.tmp"));
        assert!(glob_match("test*", "test.txt"));
        assert!(!glob_match("*.tmp", "test.txt"));
        assert!(!glob_match("test*", "other.txt"));
    }

    #[test]
    fn test_ignore_patterns() {
        let patterns = vec!["*.tmp".to_string(), ".git".to_string()];

        // Should ignore .tmp files
        assert!(should_ignore_path(&PathBuf::from("/test/file.tmp"), &patterns));

        // Should ignore .git directory
        assert!(should_ignore_path(&PathBuf::from("/test/.git"), &patterns));

        // Should not ignore regular files
        assert!(!should_ignore_path(&PathBuf::from("/test/file.txt"), &patterns));
    }

    #[tokio::test]
    async fn test_create_platform_watcher() {
        // This should successfully create a watcher on any platform
        let watcher = create_platform_watcher().await;
        assert!(watcher.is_ok());
        
        let watcher = watcher.unwrap();
        let capabilities = watcher.capabilities();
        
        // All platforms should support basic watching
        assert!(capabilities.max_path_length.is_none() || capabilities.max_path_length.unwrap() > 100);
    }
}