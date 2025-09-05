//! macOS FSEvents implementation for efficient file system watching
//!
//! This implementation uses FSEvents directly for maximum performance and
//! proper handling of file moves/renames with inode tracking.

use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, trace, warn};

use super::{EventKind, FsEvent, PlatformFlags, PlatformWatcher, WatcherCapabilities, WatcherStats};
use crate::errors::{IndexError, Result};

// FSEvents C API bindings
#[repr(C)]
#[derive(Debug)]
struct FSEventStreamContext {
    version: i64,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type FSEventStreamRef = *mut c_void;
type FSEventStreamCallback = extern "C" fn(
    stream_ref: FSEventStreamRef,
    client_callback_info: *mut c_void,
    num_events: usize,
    event_paths: *mut *mut c_char,
    event_flags: *const u32,
    event_ids: *const u64,
);

const FS_EVENT_STREAM_CREATE_FLAG_NO_DEFER: u32 = 0x00000002;
const FS_EVENT_STREAM_CREATE_FLAG_WATCH_ROOT: u32 = 0x00000004;
const FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS: u32 = 0x00000010;
const FS_EVENT_STREAM_CREATE_FLAG_MARK_SELF: u32 = 0x00000020;

// FSEvent flags
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_CREATED: u32 = 0x00000100;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_REMOVED: u32 = 0x00000200;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_INODE_META_MOD: u32 = 0x00000400;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_RENAMED: u32 = 0x00000800;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_MODIFIED: u32 = 0x00001000;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_FINDER_INFO_MOD: u32 = 0x00002000;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_CHANGE_OWNER: u32 = 0x00004000;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_XATTR_MOD: u32 = 0x00008000;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_IS_FILE: u32 = 0x00010000;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_IS_DIR: u32 = 0x00020000;
const K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_IS_SYMLINK: u32 = 0x00040000;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: FSEventStreamCallback,
        context: *mut FSEventStreamContext,
        paths: *const c_void,  // CFArrayRef
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;

    fn FSEventStreamStart(stream: FSEventStreamRef) -> bool;
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
    fn FSEventStreamScheduleWithRunLoop(
        stream: FSEventStreamRef,
        run_loop: *const c_void,  // CFRunLoopRef
        run_loop_mode: *const c_void,  // CFStringRef
    );

    // Core Foundation
    fn CFArrayCreateMutable(
        allocator: *const c_void,
        capacity: isize,
        callbacks: *const c_void,
    ) -> *const c_void;
    fn CFArrayAppendValue(array: *const c_void, value: *const c_void);
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFRunLoopGetCurrent() -> *const c_void;
    fn CFRunLoopGetMain() -> *const c_void;
    fn CFStringGetCStringPtr(string: *const c_void, encoding: u32) -> *const c_char;
    
    static kCFRunLoopDefaultMode: *const c_void;
}

const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

/// macOS FSEvents-based file watcher
pub struct MacosWatcher {
    /// FSEvents stream reference (wrapped in Arc for thread safety)
    stream: Arc<Mutex<Option<FSEventStreamRef>>>,
    /// Channel for receiving events from FSEvents callback
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<RawFSEvent>>>,
    event_tx: mpsc::UnboundedSender<RawFSEvent>,
    /// Paths currently being watched
    watched_paths: Arc<RwLock<HashSet<PathBuf>>>,
    /// Inode tracking for rename detection
    inode_tracker: Arc<RwLock<HashMap<u64, PathBuf>>>,
    /// Statistics
    stats: Arc<RwLock<WatcherStats>>,
}

// Mark as Send and Sync since we're handling the raw pointers carefully
unsafe impl Send for MacosWatcher {}
unsafe impl Sync for MacosWatcher {}

/// Raw FSEvent data from the callback
#[derive(Debug, Clone)]
struct RawFSEvent {
    path: PathBuf,
    flags: u32,
    event_id: u64,
}

impl MacosWatcher {
    /// Create a new macOS watcher
    pub async fn new() -> Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Ok(Self {
            stream: Arc::new(Mutex::new(None)),
            event_rx: Arc::new(Mutex::new(event_rx)),
            event_tx,
            watched_paths: Arc::new(RwLock::new(HashSet::new())),
            inode_tracker: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(WatcherStats::default())),
        })
    }

    /// Start FSEvents stream for all watched paths
    async fn start_stream(&mut self) -> Result<()> {
        let watched_paths = self.watched_paths.read().await;
        if watched_paths.is_empty() {
            return Ok(()); // Nothing to watch
        }

        // Stop existing stream
        {
            let mut stream_guard = self.stream.lock().await;
            if let Some(stream) = stream_guard.take() {
                unsafe {
                    FSEventStreamStop(stream);
                    FSEventStreamInvalidate(stream);
                    FSEventStreamRelease(stream);
                }
            }
        }

        // Create array of paths to watch
        let paths_array = unsafe {
            let array = CFArrayCreateMutable(std::ptr::null(), 0, std::ptr::null());
            
            for path in watched_paths.iter() {
                if let Some(path_str) = path.to_str() {
                    let c_path = CString::new(path_str)
                        .map_err(|e| IndexError::WatcherError(format!("Invalid path: {}", e)))?;
                    
                    let cf_string = CFStringCreateWithCString(
                        std::ptr::null(),
                        c_path.as_ptr(),
                        K_CF_STRING_ENCODING_UTF8,
                    );
                    
                    CFArrayAppendValue(array, cf_string);
                }
            }
            
            array
        };

        // Create callback context
        let event_tx = Box::into_raw(Box::new(self.event_tx.clone()));
        let mut context = FSEventStreamContext {
            version: 0,
            info: event_tx as *mut c_void,
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };

        // Create FSEvents stream
        let stream = unsafe {
            FSEventStreamCreate(
                std::ptr::null(), // Default allocator
                fsevents_callback,
                &mut context,
                paths_array,
                0, // Since when (0 = now)
                0.1, // Latency (100ms)
                FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS 
                    | FS_EVENT_STREAM_CREATE_FLAG_MARK_SELF
                    | FS_EVENT_STREAM_CREATE_FLAG_WATCH_ROOT,
            )
        };

        if stream.is_null() {
            return Err(IndexError::WatcherError("Failed to create FSEvents stream".to_string()));
        }

        // Schedule on run loop and start
        unsafe {
            FSEventStreamScheduleWithRunLoop(
                stream,
                CFRunLoopGetCurrent(),
                kCFRunLoopDefaultMode,
            );
            
            if !FSEventStreamStart(stream) {
                FSEventStreamRelease(stream);
                return Err(IndexError::WatcherError("Failed to start FSEvents stream".to_string()));
            }
        }

        *self.stream.lock().await = Some(stream);
        debug!("Started FSEvents stream for {} paths", watched_paths.len());

        Ok(())
    }

    /// Convert raw FSEvent to our FsEvent type
    async fn convert_raw_event(&self, raw: RawFSEvent) -> Result<Option<FsEvent>> {
        let path = raw.path;
        
        // Get file metadata for inode tracking
        let (file_id, is_directory, is_symlink, file_size) = match std::fs::metadata(&path) {
            Ok(metadata) => (
                Some(metadata.ino()),
                metadata.is_dir(),
                metadata.file_type().is_symlink(),
                Some(metadata.len()),
            ),
            Err(_) => {
                // File might have been deleted or moved
                (None, false, false, None)
            }
        };

        // Determine event kind and handle renames
        let (kind, old_path) = if raw.flags & K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_RENAMED != 0 {
            // Handle rename/move
            if let Some(inode) = file_id {
                let mut tracker = self.inode_tracker.write().await;
                if let Some(old_path) = tracker.remove(&inode) {
                    // This is the destination of a rename
                    (EventKind::Renamed, Some(old_path))
                } else {
                    // This is the source of a rename, or file was moved away
                    tracker.insert(inode, path.clone());
                    return Ok(None); // Wait for the destination event
                }
            } else {
                // File was removed/moved away without destination
                (EventKind::Removed, None)
            }
        } else if raw.flags & K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_CREATED != 0 {
            (EventKind::Created, None)
        } else if raw.flags & K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_REMOVED != 0 {
            (EventKind::Removed, None)
        } else if raw.flags & (K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_MODIFIED 
                            | K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_INODE_META_MOD
                            | K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_FINDER_INFO_MOD
                            | K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_CHANGE_OWNER
                            | K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_XATTR_MOD) != 0 {
            if raw.flags & (K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_INODE_META_MOD
                         | K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_CHANGE_OWNER
                         | K_FS_EVENT_STREAM_EVENT_FLAG_ITEM_XATTR_MOD) != 0 {
                (EventKind::AttributeChanged, None)
            } else {
                (EventKind::Modified, None)
            }
        } else {
            trace!("Unknown FSEvent flags: 0x{:x} for path: {:?}", raw.flags, path);
            return Ok(None);
        };

        let event = FsEvent {
            kind,
            path,
            old_path,
            file_id,
            timestamp: SystemTime::now(),
            flags: PlatformFlags {
                fsevent_flags: Some(raw.flags),
                is_directory,
                is_symlink,
                file_size,
            },
        };

        Ok(Some(event))
    }
}

#[async_trait]
impl PlatformWatcher for MacosWatcher {
    async fn watch(&mut self, path: &Path, recursive: bool) -> Result<()> {
        let canonical_path = path.canonicalize()
            .map_err(|e| IndexError::WatcherError(format!("Cannot canonicalize path: {}", e)))?;

        debug!("Adding watch for path: {:?} (recursive: {})", canonical_path, recursive);

        self.watched_paths.write().await.insert(canonical_path);
        self.start_stream().await?;

        let mut stats = self.stats.write().await;
        stats.paths_watched += 1;

        Ok(())
    }

    async fn unwatch(&mut self, path: &Path) -> Result<()> {
        let canonical_path = path.canonicalize()
            .map_err(|e| IndexError::WatcherError(format!("Cannot canonicalize path: {}", e)))?;

        debug!("Removing watch for path: {:?}", canonical_path);

        let removed = self.watched_paths.write().await.remove(&canonical_path);
        
        if removed {
            self.start_stream().await?;
            let mut stats = self.stats.write().await;
            stats.paths_watched = stats.paths_watched.saturating_sub(1);
        }

        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<FsEvent>> {
        let mut events = Vec::new();
        let mut rx = self.event_rx.lock().await;

        // Process all available events
        while let Ok(raw_event) = rx.try_recv() {
            if let Some(event) = self.convert_raw_event(raw_event).await? {
                events.push(event);
            }
        }

        if !events.is_empty() {
            let mut stats = self.stats.write().await;
            stats.events_processed += events.len() as u64;
            trace!("Processed {} FSEvents", events.len());
        }

        Ok(events)
    }

    fn capabilities(&self) -> WatcherCapabilities {
        WatcherCapabilities {
            supports_rename_tracking: true,
            supports_file_id: true,
            max_watches: None, // FSEvents doesn't have explicit limits
            supports_overflow_detection: false, // FSEvents handles this internally
            supports_network_paths: true,
            max_path_length: Some(1024), // macOS path limit
            provides_file_size: true,
        }
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(self.stream.lock().await.is_some())
    }

    async fn get_stats(&self) -> WatcherStats {
        self.stats.read().await.clone()
    }
}

impl Drop for MacosWatcher {
    fn drop(&mut self) {
        // We need to use try_lock here since Drop is sync
        if let Ok(mut stream_guard) = self.stream.try_lock() {
            if let Some(stream) = stream_guard.take() {
                unsafe {
                    FSEventStreamStop(stream);
                    FSEventStreamInvalidate(stream);
                    FSEventStreamRelease(stream);
                }
            }
        }
    }
}

/// FSEvents callback function
extern "C" fn fsevents_callback(
    _stream_ref: FSEventStreamRef,
    client_callback_info: *mut c_void,
    num_events: usize,
    event_paths: *mut *mut c_char,
    event_flags: *const u32,
    event_ids: *const u64,
) {
    if client_callback_info.is_null() {
        return;
    }

    let sender = unsafe { &*(client_callback_info as *const mpsc::UnboundedSender<RawFSEvent>) };

    for i in 0..num_events {
        let path_ptr = unsafe { *event_paths.add(i) };
        let flags = unsafe { *event_flags.add(i) };
        let event_id = unsafe { *event_ids.add(i) };

        if path_ptr.is_null() {
            continue;
        }

        let path_str = unsafe { CStr::from_ptr(path_ptr) };
        if let Ok(path_str) = path_str.to_str() {
            let raw_event = RawFSEvent {
                path: PathBuf::from(path_str),
                flags,
                event_id,
            };

            if sender.send(raw_event).is_err() {
                // Channel closed, stop processing
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::time::{sleep, timeout, Duration};

    #[tokio::test]
    async fn test_macos_watcher_creation() {
        let watcher = MacosWatcher::new().await;
        assert!(watcher.is_ok());
        
        let watcher = watcher.unwrap();
        let capabilities = watcher.capabilities();
        assert!(capabilities.supports_rename_tracking);
        assert!(capabilities.supports_file_id);
    }

    #[tokio::test]
    async fn test_watch_unwatch() {
        let temp_dir = tempdir().unwrap();
        let mut watcher = MacosWatcher::new().await.unwrap();

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
        let mut watcher = MacosWatcher::new().await.unwrap();

        watcher.watch(temp_dir.path(), true).await.unwrap();

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "Hello, World!").await.unwrap();

        // Give FSEvents time to process
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
        }).await;

        assert!(events.is_ok());
        let events = events.unwrap();
        
        // Should have at least one event
        assert!(!events.is_empty());
        
        // Should be a creation event
        let create_event = events.iter().find(|e| e.kind == EventKind::Created);
        assert!(create_event.is_some());
    }
}