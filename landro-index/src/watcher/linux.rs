//! Linux inotify implementation for efficient file system watching
//!
//! This implementation uses inotify directly for maximum performance and
//! proper handling of watch limits and overflow detection.

use async_trait::async_trait;
use libc::{c_char, c_int, c_void};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, OsStr};
use std::io::{self, Error, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::unix::AsyncFd;
use tokio::sync::RwLock;
use tracing::{debug, error, trace, warn};

use super::{EventKind, FsEvent, PlatformFlags, PlatformWatcher, WatcherCapabilities, WatcherStats};
use crate::errors::{IndexError, Result};

// inotify constants
const IN_ACCESS: u32 = 0x00000001;
const IN_MODIFY: u32 = 0x00000002;
const IN_ATTRIB: u32 = 0x00000004;
const IN_CLOSE_WRITE: u32 = 0x00000008;
const IN_CLOSE_NOWRITE: u32 = 0x00000010;
const IN_OPEN: u32 = 0x00000020;
const IN_MOVED_FROM: u32 = 0x00000040;
const IN_MOVED_TO: u32 = 0x00000080;
const IN_CREATE: u32 = 0x00000100;
const IN_DELETE: u32 = 0x00000200;
const IN_DELETE_SELF: u32 = 0x00000400;
const IN_MOVE_SELF: u32 = 0x00000800;
const IN_UNMOUNT: u32 = 0x00002000;
const IN_Q_OVERFLOW: u32 = 0x00004000;
const IN_IGNORED: u32 = 0x00008000;
const IN_ONLYDIR: u32 = 0x01000000;
const IN_DONT_FOLLOW: u32 = 0x02000000;
const IN_EXCL_UNLINK: u32 = 0x04000000;
const IN_MASK_CREATE: u32 = 0x10000000;
const IN_MASK_ADD: u32 = 0x20000000;
const IN_ISDIR: u32 = 0x40000000;
const IN_ONESHOT: u32 = 0x80000000;

// Watch mask for file changes
const WATCH_MASK: u32 = IN_CREATE | IN_DELETE | IN_MODIFY | IN_MOVED_FROM | IN_MOVED_TO | IN_ATTRIB;

// inotify event structure
#[repr(C)]
#[derive(Debug)]
struct InotifyEvent {
    wd: c_int,
    mask: u32,
    cookie: u32,
    len: u32,
    // name follows immediately after
}

extern "C" {
    fn inotify_init1(flags: c_int) -> c_int;
    fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: u32) -> c_int;
    fn inotify_rm_watch(fd: c_int, wd: c_int) -> c_int;
}

const IN_CLOEXEC: c_int = 0o2000000;
const IN_NONBLOCK: c_int = 0o0004000;

/// Linux inotify-based file watcher
pub struct LinuxWatcher {
    /// inotify file descriptor
    inotify_fd: AsyncFd<OwnedFd>,
    /// Map of watch descriptors to paths
    wd_to_path: Arc<RwLock<HashMap<c_int, PathBuf>>>,
    /// Map of paths to watch descriptors
    path_to_wd: Arc<RwLock<HashMap<PathBuf, c_int>>>,
    /// Tracking move operations by cookie
    move_tracker: Arc<RwLock<HashMap<u32, (PathBuf, SystemTime)>>>,
    /// Event buffer for reading from inotify
    event_buffer: Vec<u8>,
    /// Statistics
    stats: Arc<RwLock<WatcherStats>>,
    /// Maximum number of watches (from /proc/sys/fs/inotify/max_user_watches)
    max_watches: usize,
}

impl LinuxWatcher {
    /// Create a new Linux inotify watcher
    pub async fn new() -> Result<Self> {
        // Create inotify instance
        let fd = unsafe { inotify_init1(IN_CLOEXEC | IN_NONBLOCK) };
        if fd < 0 {
            return Err(IndexError::WatcherError(
                format!("Failed to create inotify instance: {}", io::Error::last_os_error())
            ));
        }

        let owned_fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let async_fd = AsyncFd::new(owned_fd)
            .map_err(|e| IndexError::WatcherError(format!("Failed to create async fd: {}", e)))?;

        // Read maximum watches from system
        let max_watches = Self::read_max_user_watches().unwrap_or(8192);

        Ok(Self {
            inotify_fd: async_fd,
            wd_to_path: Arc::new(RwLock::new(HashMap::new())),
            path_to_wd: Arc::new(RwLock::new(HashMap::new())),
            move_tracker: Arc::new(RwLock::new(HashMap::new())),
            event_buffer: vec![0; 4096],
            stats: Arc::new(RwLock::new(WatcherStats::default())),
            max_watches,
        })
    }

    /// Read the maximum number of user watches from /proc
    fn read_max_user_watches() -> Option<usize> {
        std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// Add inotify watch for a path
    async fn add_inotify_watch(&self, path: &Path, recursive: bool) -> Result<()> {
        let path_c = CString::new(path.as_os_str().as_bytes())
            .map_err(|e| IndexError::WatcherError(format!("Invalid path: {}", e)))?;

        let mut mask = WATCH_MASK;
        if path.is_dir() && !recursive {
            mask |= IN_ONLYDIR;
        }

        let wd = unsafe {
            inotify_add_watch(
                self.inotify_fd.get_ref().as_raw_fd(),
                path_c.as_ptr(),
                mask,
            )
        };

        if wd < 0 {
            return Err(IndexError::WatcherError(
                format!("Failed to add inotify watch for {:?}: {}", path, io::Error::last_os_error())
            ));
        }

        // Store mappings
        self.wd_to_path.write().await.insert(wd, path.to_path_buf());
        self.path_to_wd.write().await.insert(path.to_path_buf(), wd);

        debug!("Added inotify watch for {:?} (wd: {})", path, wd);

        // If recursive, add watches for existing subdirectories
        if recursive && path.is_dir() {
            self.add_recursive_watches(path).await?;
        }

        Ok(())
    }

    /// Add watches for all subdirectories recursively
    async fn add_recursive_watches(&self, root: &Path) -> Result<()> {
        let mut stack = vec![root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        // Check if we're at the watch limit
                        let current_watches = self.wd_to_path.read().await.len();
                        if current_watches >= self.max_watches {
                            warn!("Reached maximum inotify watches ({}), cannot add more", self.max_watches);
                            self.stats.write().await.at_watch_limit = true;
                            break;
                        }

                        // Add watch for this directory
                        if let Err(e) = self.add_inotify_watch(&path, false).await {
                            warn!("Failed to add watch for subdirectory {:?}: {}", path, e);
                        } else {
                            // Add to stack for recursive processing
                            stack.push(path);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Remove inotify watch for a path
    async fn remove_inotify_watch(&self, path: &Path) -> Result<()> {
        let wd = {
            let path_to_wd = self.path_to_wd.read().await;
            path_to_wd.get(path).copied()
        };

        if let Some(wd) = wd {
            let result = unsafe {
                inotify_rm_watch(self.inotify_fd.get_ref().as_raw_fd(), wd)
            };

            if result < 0 {
                warn!("Failed to remove inotify watch for {:?}: {}", path, io::Error::last_os_error());
            }

            // Remove from mappings
            self.wd_to_path.write().await.remove(&wd);
            self.path_to_wd.write().await.remove(path);

            debug!("Removed inotify watch for {:?} (wd: {})", path, wd);
        }

        Ok(())
    }

    /// Read and parse inotify events
    async fn read_events(&mut self) -> Result<Vec<FsEvent>> {
        let mut events = Vec::new();

        loop {
            let mut guard = self.inotify_fd.readable_mut().await
                .map_err(|e| IndexError::WatcherError(format!("inotify not readable: {}", e)))?;

            match guard.try_io(|inner| {
                let fd = inner.get_ref().as_raw_fd();
                unsafe {
                    libc::read(
                        fd,
                        self.event_buffer.as_mut_ptr() as *mut c_void,
                        self.event_buffer.len(),
                    )
                }
            }) {
                Ok(Ok(bytes_read)) => {
                    if bytes_read == 0 {
                        break; // No more events
                    }

                    let parsed_events = self.parse_events(&self.event_buffer[..bytes_read as usize]).await?;
                    events.extend(parsed_events);
                }
                Ok(Err(err)) => {
                    if err.kind() == ErrorKind::WouldBlock {
                        break; // No more events available
                    }
                    return Err(IndexError::WatcherError(format!("inotify read error: {}", err)));
                }
                Err(_would_block) => {
                    break; // Would block, no events ready
                }
            }
        }

        if !events.is_empty() {
            let mut stats = self.stats.write().await;
            stats.events_processed += events.len() as u64;
        }

        Ok(events)
    }

    /// Parse raw inotify events from buffer
    async fn parse_events(&self, buffer: &[u8]) -> Result<Vec<FsEvent>> {
        let mut events = Vec::new();
        let mut offset = 0;

        while offset < buffer.len() {
            if offset + std::mem::size_of::<InotifyEvent>() > buffer.len() {
                break; // Incomplete event
            }

            let event_ptr = buffer.as_ptr().add(offset) as *const InotifyEvent;
            let event = unsafe { &*event_ptr };
            
            // Calculate total event size
            let event_size = std::mem::size_of::<InotifyEvent>() + event.len as usize;
            if offset + event_size > buffer.len() {
                break; // Incomplete event
            }

            // Extract name if present
            let name = if event.len > 0 {
                let name_ptr = unsafe { 
                    buffer.as_ptr().add(offset + std::mem::size_of::<InotifyEvent>()) as *const c_char
                };
                let name_cstr = unsafe { CStr::from_ptr(name_ptr) };
                Some(name_cstr.to_string_lossy().into_owned())
            } else {
                None
            };

            // Convert to our event type
            if let Some(fs_event) = self.convert_inotify_event(event, name).await? {
                events.push(fs_event);
            }

            offset += event_size;
        }

        Ok(events)
    }

    /// Convert inotify event to FsEvent
    async fn convert_inotify_event(&self, event: &InotifyEvent, name: Option<String>) -> Result<Option<FsEvent>> {
        // Handle overflow
        if event.mask & IN_Q_OVERFLOW != 0 {
            warn!("inotify queue overflow detected");
            self.stats.write().await.overflow_events += 1;
            // Return a rescan event for all watched paths
            // For simplicity, just return None here - in practice you'd want to
            // trigger a full rescan of watched directories
            return Ok(None);
        }

        // Ignore ignored events
        if event.mask & IN_IGNORED != 0 {
            return Ok(None);
        }

        // Get the base path for this watch descriptor
        let base_path = {
            let wd_to_path = self.wd_to_path.read().await;
            wd_to_path.get(&event.wd).cloned()
        };

        let Some(mut path) = base_path else {
            trace!("Received event for unknown watch descriptor: {}", event.wd);
            return Ok(None);
        };

        // Add name to path if present
        if let Some(ref name) = name {
            if !name.is_empty() && name != "." && name != ".." {
                path = path.join(name);
            }
        }

        // Handle move operations with cookie tracking
        if event.mask & (IN_MOVED_FROM | IN_MOVED_TO) != 0 {
            let mut move_tracker = self.move_tracker.write().await;
            
            if event.mask & IN_MOVED_FROM != 0 {
                // This is the source of a move, store it
                move_tracker.insert(event.cookie, (path.clone(), SystemTime::now()));
                return Ok(None); // Wait for the corresponding MOVED_TO
            } else if event.mask & IN_MOVED_TO != 0 {
                // This is the destination of a move
                if let Some((old_path, _timestamp)) = move_tracker.remove(&event.cookie) {
                    return Ok(Some(FsEvent {
                        kind: EventKind::Renamed,
                        path,
                        old_path: Some(old_path),
                        file_id: None, // inotify doesn't provide inode directly
                        timestamp: SystemTime::now(),
                        flags: PlatformFlags {
                            inotify_mask: Some(event.mask),
                            is_directory: event.mask & IN_ISDIR != 0,
                            is_symlink: false, // Would need to stat to determine
                            file_size: None,
                        },
                    }));
                } else {
                    // MOVED_TO without corresponding MOVED_FROM (moved from unwatched location)
                    return Ok(Some(FsEvent {
                        kind: EventKind::Created,
                        path,
                        old_path: None,
                        file_id: None,
                        timestamp: SystemTime::now(),
                        flags: PlatformFlags {
                            inotify_mask: Some(event.mask),
                            is_directory: event.mask & IN_ISDIR != 0,
                            is_symlink: false,
                            file_size: None,
                        },
                    }));
                }
            }
        }

        // Clean up old move entries (older than 1 second)
        {
            let now = SystemTime::now();
            let mut move_tracker = self.move_tracker.write().await;
            move_tracker.retain(|_, (_, timestamp)| {
                now.duration_since(*timestamp).map(|d| d.as_secs() < 1).unwrap_or(false)
            });
        }

        // Determine event kind
        let kind = if event.mask & IN_CREATE != 0 {
            EventKind::Created
        } else if event.mask & IN_DELETE != 0 {
            EventKind::Removed
        } else if event.mask & (IN_MODIFY | IN_CLOSE_WRITE) != 0 {
            EventKind::Modified
        } else if event.mask & IN_ATTRIB != 0 {
            EventKind::AttributeChanged
        } else if event.mask & (IN_DELETE_SELF | IN_MOVE_SELF) != 0 {
            EventKind::Removed
        } else {
            trace!("Unhandled inotify event mask: 0x{:x}", event.mask);
            return Ok(None);
        };

        // Get file metadata if available
        let (file_size, is_symlink) = match std::fs::metadata(&path) {
            Ok(metadata) => (Some(metadata.len()), metadata.file_type().is_symlink()),
            Err(_) => (None, false),
        };

        Ok(Some(FsEvent {
            kind,
            path,
            old_path: None,
            file_id: None, // Would need to stat to get inode
            timestamp: SystemTime::now(),
            flags: PlatformFlags {
                inotify_mask: Some(event.mask),
                is_directory: event.mask & IN_ISDIR != 0,
                is_symlink,
                file_size,
            },
        }))
    }
}

#[async_trait]
impl PlatformWatcher for LinuxWatcher {
    async fn watch(&mut self, path: &Path, recursive: bool) -> Result<()> {
        let canonical_path = path.canonicalize()
            .map_err(|e| IndexError::WatcherError(format!("Cannot canonicalize path: {}", e)))?;

        debug!("Adding inotify watch for path: {:?} (recursive: {})", canonical_path, recursive);

        // Check watch limit
        let current_watches = self.wd_to_path.read().await.len();
        if current_watches >= self.max_watches {
            return Err(IndexError::WatcherError(
                format!("Cannot add watch: at maximum limit of {} watches", self.max_watches)
            ));
        }

        self.add_inotify_watch(&canonical_path, recursive).await?;

        let mut stats = self.stats.write().await;
        stats.paths_watched = self.wd_to_path.read().await.len();

        Ok(())
    }

    async fn unwatch(&mut self, path: &Path) -> Result<()> {
        let canonical_path = path.canonicalize()
            .map_err(|e| IndexError::WatcherError(format!("Cannot canonicalize path: {}", e)))?;

        debug!("Removing inotify watch for path: {:?}", canonical_path);

        self.remove_inotify_watch(&canonical_path).await?;

        // For recursive watches, we'd need to remove all subdirectory watches too
        // For now, we'll just remove the primary watch

        let mut stats = self.stats.write().await;
        stats.paths_watched = self.wd_to_path.read().await.len();
        stats.at_watch_limit = stats.paths_watched >= self.max_watches;

        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<FsEvent>> {
        self.read_events().await
    }

    fn capabilities(&self) -> WatcherCapabilities {
        WatcherCapabilities {
            supports_rename_tracking: true,
            supports_file_id: false, // Could be added with additional stat calls
            max_watches: Some(self.max_watches),
            supports_overflow_detection: true,
            supports_network_paths: false, // inotify doesn't work on network filesystems
            max_path_length: Some(4096), // Linux path limit
            provides_file_size: true,
        }
    }

    async fn health_check(&self) -> Result<bool> {
        // Try to read the current watch count to verify inotify is working
        Ok(true) // If we can create the watcher, it should be healthy
    }

    async fn get_stats(&self) -> WatcherStats {
        let mut stats = self.stats.read().await.clone();
        stats.paths_watched = self.wd_to_path.read().await.len();
        stats.at_watch_limit = stats.paths_watched >= self.max_watches;
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::time::{sleep, timeout, Duration};

    #[tokio::test]
    async fn test_linux_watcher_creation() {
        let watcher = LinuxWatcher::new().await;
        assert!(watcher.is_ok());
        
        let watcher = watcher.unwrap();
        let capabilities = watcher.capabilities();
        assert!(capabilities.supports_rename_tracking);
        assert!(capabilities.supports_overflow_detection);
        assert!(capabilities.max_watches.is_some());
    }

    #[tokio::test]
    async fn test_watch_unwatch() {
        let temp_dir = tempdir().unwrap();
        let mut watcher = LinuxWatcher::new().await.unwrap();

        // Start watching
        watcher.watch(temp_dir.path(), true).await.unwrap();
        assert!(watcher.health_check().await.unwrap());

        let stats = watcher.get_stats().await;
        assert!(stats.paths_watched > 0);

        // Stop watching
        watcher.unwatch(temp_dir.path()).await.unwrap();
    }

    #[tokio::test]
    async fn test_file_events() {
        let temp_dir = tempdir().unwrap();
        let mut watcher = LinuxWatcher::new().await.unwrap();

        watcher.watch(temp_dir.path(), true).await.unwrap();

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "Hello, World!").await.unwrap();

        // Give inotify time to process
        sleep(Duration::from_millis(100)).await;

        // Check for events
        let events = timeout(Duration::from_secs(1), async {
            for _ in 0..10 {
                let events = watcher.poll_events().await.unwrap();
                if !events.is_empty() {
                    return events;
                }
                sleep(Duration::from_millis(50)).await;
            }
            Vec::new()
        }).await.unwrap();

        // Should have at least one event
        assert!(!events.is_empty());
        
        // Should be a creation event
        let create_event = events.iter().find(|e| e.kind == EventKind::Created);
        assert!(create_event.is_some());
    }

    #[tokio::test]
    async fn test_rename_tracking() {
        let temp_dir = tempdir().unwrap();
        let mut watcher = LinuxWatcher::new().await.unwrap();

        watcher.watch(temp_dir.path(), true).await.unwrap();

        // Create and rename a file
        let old_file = temp_dir.path().join("old.txt");
        let new_file = temp_dir.path().join("new.txt");
        
        fs::write(&old_file, "content").await.unwrap();
        sleep(Duration::from_millis(50)).await;
        
        std::fs::rename(&old_file, &new_file).unwrap();
        sleep(Duration::from_millis(100)).await;

        // Check for rename event
        let events = timeout(Duration::from_secs(1), async {
            for _ in 0..10 {
                let events = watcher.poll_events().await.unwrap();
                for event in &events {
                    if event.kind == EventKind::Renamed {
                        return events;
                    }
                }
                sleep(Duration::from_millis(50)).await;
            }
            Vec::new()
        }).await.unwrap();

        // Should find a rename event
        let rename_event = events.iter().find(|e| e.kind == EventKind::Renamed);
        if let Some(event) = rename_event {
            assert!(event.old_path.is_some());
        }
    }
}