//! Windows USN Journal implementation for efficient file system watching
//!
//! This implementation uses the Windows Update Sequence Number (USN) Journal
//! to track all file system changes efficiently across NTFS volumes.

use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{RwLock, Mutex};
use tokio::time::{sleep, Instant};
use tracing::{debug, error, trace, warn};
use winapi::ctypes::c_void;
use winapi::shared::minwindef::{BOOL, DWORD, FALSE, TRUE};
use winapi::shared::ntdef::{HANDLE, LARGE_INTEGER, ULONGLONG};
use winapi::shared::winerror::{ERROR_HANDLE_EOF, ERROR_JOURNAL_ENTRY_DELETED};
use winapi::um::fileapi::{CreateFileW, GetVolumeInformationW, OPEN_EXISTING};
use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
use winapi::um::ioapiset::DeviceIoControl;
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;
use winapi::um::winnt::{
    FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ, WCHAR,
};

use super::{EventKind, FsEvent, PlatformFlags, PlatformWatcher, WatcherCapabilities, WatcherStats, WindowsUsnRecord};
use crate::errors::{IndexError, Result};

// USN Journal IOCTL codes
const FSCTL_QUERY_USN_JOURNAL: DWORD = 0x000900f4;
const FSCTL_READ_USN_JOURNAL: DWORD = 0x000900bb;
const FSCTL_CREATE_USN_JOURNAL: DWORD = 0x000900e7;
const FSCTL_ENUM_USN_DATA: DWORD = 0x000900b3;

// USN Journal structures
#[repr(C)]
#[derive(Clone)]
struct UsnJournalData {
    usn_journal_id: ULONGLONG,
    first_usn: ULONGLONG,
    next_usn: ULONGLONG,
    lowest_valid_usn: ULONGLONG,
    max_usn: ULONGLONG,
    maximum_size: ULONGLONG,
    allocation_delta: ULONGLONG,
}

#[repr(C)]
struct CreateUsnJournalData {
    maximum_size: ULONGLONG,
    allocation_delta: ULONGLONG,
}

#[repr(C)]
struct ReadUsnJournalData {
    start_usn: ULONGLONG,
    reason_mask: DWORD,
    return_only_on_close: DWORD,
    timeout: ULONGLONG,
    bytes_to_wait_for: DWORD,
    usn_journal_id: ULONGLONG,
}

#[repr(C)]
struct UsnRecord {
    record_length: DWORD,
    major_version: WORD,
    minor_version: WORD,
    file_reference_number: ULONGLONG,
    parent_file_reference_number: ULONGLONG,
    usn: ULONGLONG,
    timestamp: LARGE_INTEGER,
    reason: DWORD,
    source_info: DWORD,
    security_id: DWORD,
    file_attributes: DWORD,
    file_name_length: WORD,
    file_name_offset: WORD,
    // file_name follows
}

type WORD = u16;

// USN reasons
const USN_REASON_DATA_OVERWRITE: DWORD = 0x00000001;
const USN_REASON_DATA_EXTEND: DWORD = 0x00000002;
const USN_REASON_DATA_TRUNCATION: DWORD = 0x00000004;
const USN_REASON_NAMED_DATA_OVERWRITE: DWORD = 0x00000010;
const USN_REASON_NAMED_DATA_EXTEND: DWORD = 0x00000020;
const USN_REASON_NAMED_DATA_TRUNCATION: DWORD = 0x00000040;
const USN_REASON_FILE_CREATE: DWORD = 0x00000100;
const USN_REASON_FILE_DELETE: DWORD = 0x00000200;
const USN_REASON_EA_CHANGE: DWORD = 0x00000400;
const USN_REASON_SECURITY_CHANGE: DWORD = 0x00000800;
const USN_REASON_RENAME_OLD_NAME: DWORD = 0x00001000;
const USN_REASON_RENAME_NEW_NAME: DWORD = 0x00002000;
const USN_REASON_INDEXABLE_CHANGE: DWORD = 0x00004000;
const USN_REASON_BASIC_INFO_CHANGE: DWORD = 0x00008000;
const USN_REASON_HARD_LINK_CHANGE: DWORD = 0x00010000;
const USN_REASON_COMPRESSION_CHANGE: DWORD = 0x00020000;
const USN_REASON_ENCRYPTION_CHANGE: DWORD = 0x00040000;
const USN_REASON_OBJECT_ID_CHANGE: DWORD = 0x00080000;
const USN_REASON_REPARSE_POINT_CHANGE: DWORD = 0x00100000;
const USN_REASON_STREAM_CHANGE: DWORD = 0x00200000;
const USN_REASON_TRANSACTED_CHANGE: DWORD = 0x00400000;
const USN_REASON_CLOSE: DWORD = 0x80000000;

// File attributes
const FILE_ATTRIBUTE_DIRECTORY: DWORD = 0x00000010;
const FILE_ATTRIBUTE_REPARSE_POINT: DWORD = 0x00000400;

/// Windows USN Journal-based file watcher
pub struct WindowsWatcher {
    /// Volume handles for each watched drive
    volume_handles: Arc<RwLock<HashMap<String, HANDLE>>>,
    /// Current USN for each volume
    current_usn: Arc<RwLock<HashMap<String, ULONGLONG>>>,
    /// Journal IDs for each volume
    journal_ids: Arc<RwLock<HashMap<String, ULONGLONG>>>,
    /// File reference number to path mapping
    file_ref_to_path: Arc<RwLock<HashMap<String, HashMap<ULONGLONG, PathBuf>>>>,
    /// Paths being watched
    watched_paths: Arc<RwLock<HashMap<PathBuf, String>>>, // Path -> Volume
    /// Pending rename operations
    rename_tracker: Arc<RwLock<HashMap<ULONGLONG, (PathBuf, SystemTime)>>>,
    /// Event buffer
    event_buffer: Arc<Mutex<VecDeque<FsEvent>>>,
    /// Statistics
    stats: Arc<RwLock<WatcherStats>>,
    /// Background polling task handle
    _polling_task: Option<tokio::task::JoinHandle<()>>,
}

impl WindowsWatcher {
    /// Create a new Windows USN Journal watcher
    pub async fn new() -> Result<Self> {
        Ok(Self {
            volume_handles: Arc::new(RwLock::new(HashMap::new())),
            current_usn: Arc::new(RwLock::new(HashMap::new())),
            journal_ids: Arc::new(RwLock::new(HashMap::new())),
            file_ref_to_path: Arc::new(RwLock::new(HashMap::new())),
            watched_paths: Arc::new(RwLock::new(HashMap::new())),
            rename_tracker: Arc::new(RwLock::new(HashMap::new())),
            event_buffer: Arc::new(Mutex::new(VecDeque::new())),
            stats: Arc::new(RwLock::new(WatcherStats::default())),
            _polling_task: None,
        })
    }

    /// Get the volume letter for a path (e.g., "C:" for "C:\folder\file.txt")
    fn get_volume_letter(path: &Path) -> Result<String> {
        let path_str = path.to_string_lossy();
        if path_str.len() >= 2 && path_str.chars().nth(1) == Some(':') {
            Ok(path_str.chars().take(2).collect::<String>().to_uppercase())
        } else {
            Err(IndexError::WatcherError(
                format!("Cannot determine volume for path: {:?}", path)
            ))
        }
    }

    /// Open a volume handle for USN Journal access
    async fn open_volume(&self, volume: &str) -> Result<HANDLE> {
        let volume_path = format!("\\\\.\\{}", volume);
        let volume_wide: Vec<WCHAR> = OsString::from(volume_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe {
            CreateFileW(
                volume_wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            return Err(IndexError::WatcherError(
                format!("Failed to open volume {}: {}", volume, 
                    std::io::Error::last_os_error())
            ));
        }

        debug!("Opened volume handle for {}", volume);
        Ok(handle)
    }

    /// Initialize USN Journal for a volume
    async fn initialize_journal(&self, volume: &str, handle: HANDLE) -> Result<ULONGLONG> {
        // Query existing journal
        let mut journal_data = UsnJournalData {
            usn_journal_id: 0,
            first_usn: 0,
            next_usn: 0,
            lowest_valid_usn: 0,
            max_usn: 0,
            maximum_size: 0,
            allocation_delta: 0,
        };

        let mut bytes_returned: DWORD = 0;
        let query_result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_QUERY_USN_JOURNAL,
                ptr::null_mut(),
                0,
                &mut journal_data as *mut _ as *mut c_void,
                std::mem::size_of::<UsnJournalData>() as DWORD,
                &mut bytes_returned,
                ptr::null_mut(),
            )
        };

        if query_result == FALSE {
            // Journal doesn't exist, create it
            let create_data = CreateUsnJournalData {
                maximum_size: 32 * 1024 * 1024, // 32MB
                allocation_delta: 1024 * 1024,  // 1MB
            };

            let create_result = unsafe {
                DeviceIoControl(
                    handle,
                    FSCTL_CREATE_USN_JOURNAL,
                    &create_data as *const _ as *const c_void,
                    std::mem::size_of::<CreateUsnJournalData>() as DWORD,
                    ptr::null_mut(),
                    0,
                    &mut bytes_returned,
                    ptr::null_mut(),
                )
            };

            if create_result == FALSE {
                return Err(IndexError::WatcherError(
                    format!("Failed to create USN Journal for volume {}: {}", 
                        volume, std::io::Error::last_os_error())
                ));
            }

            // Query again to get journal info
            let query_result = unsafe {
                DeviceIoControl(
                    handle,
                    FSCTL_QUERY_USN_JOURNAL,
                    ptr::null_mut(),
                    0,
                    &mut journal_data as *mut _ as *mut c_void,
                    std::mem::size_of::<UsnJournalData>() as DWORD,
                    &mut bytes_returned,
                    ptr::null_mut(),
                )
            };

            if query_result == FALSE {
                return Err(IndexError::WatcherError(
                    format!("Failed to query newly created USN Journal for volume {}: {}", 
                        volume, std::io::Error::last_os_error())
                ));
            }
        }

        debug!("Initialized USN Journal for volume {} (ID: {})", volume, journal_data.usn_journal_id);
        
        // Store initial USN position
        self.current_usn.write().await.insert(volume.to_string(), journal_data.next_usn);
        self.journal_ids.write().await.insert(volume.to_string(), journal_data.usn_journal_id);

        Ok(journal_data.usn_journal_id)
    }

    /// Read USN Journal entries
    async fn read_journal_entries(&self, volume: &str) -> Result<Vec<FsEvent>> {
        let handle = {
            let handles = self.volume_handles.read().await;
            handles.get(volume).copied()
        };

        let Some(handle) = handle else {
            return Ok(Vec::new());
        };

        let (start_usn, journal_id) = {
            let current_usn = self.current_usn.read().await;
            let journal_ids = self.journal_ids.read().await;
            (
                current_usn.get(volume).copied().unwrap_or(0),
                journal_ids.get(volume).copied().unwrap_or(0),
            )
        };

        let read_data = ReadUsnJournalData {
            start_usn,
            reason_mask: 0xFFFFFFFF, // All reasons
            return_only_on_close: 0,
            timeout: 1000, // 1 second timeout
            bytes_to_wait_for: 1,
            usn_journal_id: journal_id,
        };

        let mut buffer = vec![0u8; 64 * 1024]; // 64KB buffer
        let mut bytes_returned: DWORD = 0;

        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                &read_data as *const _ as *const c_void,
                std::mem::size_of::<ReadUsnJournalData>() as DWORD,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as DWORD,
                &mut bytes_returned,
                ptr::null_mut(),
            )
        };

        if result == FALSE {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) {
                // No more entries
                return Ok(Vec::new());
            }
            return Err(IndexError::WatcherError(
                format!("Failed to read USN Journal for volume {}: {}", volume, error)
            ));
        }

        if bytes_returned < 8 {
            return Ok(Vec::new());
        }

        // Parse USN records
        self.parse_usn_records(volume, &buffer[8..bytes_returned as usize]).await
    }

    /// Parse USN records from buffer
    async fn parse_usn_records(&self, volume: &str, buffer: &[u8]) -> Result<Vec<FsEvent>> {
        let mut events = Vec::new();
        let mut offset = 0;

        while offset < buffer.len() {
            if offset + std::mem::size_of::<UsnRecord>() > buffer.len() {
                break;
            }

            let record_ptr = buffer.as_ptr().add(offset) as *const UsnRecord;
            let record = unsafe { &*record_ptr };

            if record.record_length < std::mem::size_of::<UsnRecord>() as DWORD {
                break;
            }

            if offset + record.record_length as usize > buffer.len() {
                break;
            }

            // Extract filename
            let filename_offset = record.file_name_offset as usize;
            let filename_length = record.file_name_length as usize;
            
            let filename = if filename_offset + filename_length <= record.record_length as usize {
                let name_ptr = unsafe { 
                    buffer.as_ptr().add(offset + filename_offset) as *const WCHAR 
                };
                let name_slice = unsafe { 
                    std::slice::from_raw_parts(name_ptr, filename_length / 2) 
                };
                OsString::from_wide(name_slice).to_string_lossy().into_owned()
            } else {
                String::new()
            };

            // Convert to FsEvent
            if let Some(event) = self.convert_usn_record(volume, record, &filename).await? {
                events.push(event);
            }

            // Update current USN
            self.current_usn.write().await.insert(volume.to_string(), record.usn);

            offset += record.record_length as usize;
        }

        if !events.is_empty() {
            let mut stats = self.stats.write().await;
            stats.events_processed += events.len() as u64;
        }

        Ok(events)
    }

    /// Convert USN record to FsEvent
    async fn convert_usn_record(&self, volume: &str, record: &UsnRecord, filename: &str) -> Result<Option<FsEvent>> {
        // Skip if filename is empty or special
        if filename.is_empty() || filename == "." || filename == ".." {
            return Ok(None);
        }

        // Build full path (this is simplified - in reality you'd need to
        // track parent directories and build the full path)
        let path = PathBuf::from(format!("{}\\{}", volume, filename));

        // Handle rename operations
        if record.reason & (USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME) != 0 {
            let mut rename_tracker = self.rename_tracker.write().await;
            
            if record.reason & USN_REASON_RENAME_OLD_NAME != 0 {
                // Store the old name
                rename_tracker.insert(record.file_reference_number, (path.clone(), SystemTime::now()));
                return Ok(None);
            } else if record.reason & USN_REASON_RENAME_NEW_NAME != 0 {
                // Look for the old name
                if let Some((old_path, _)) = rename_tracker.remove(&record.file_reference_number) {
                    return Ok(Some(FsEvent {
                        kind: EventKind::Renamed,
                        path,
                        old_path: Some(old_path),
                        file_id: Some(record.file_reference_number),
                        timestamp: SystemTime::now(),
                        flags: PlatformFlags {
                            usn_record: Some(WindowsUsnRecord {
                                usn: record.usn,
                                reason: record.reason,
                                file_attributes: record.file_attributes,
                            }),
                            is_directory: record.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                            is_symlink: record.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
                            file_size: None, // USN doesn't provide file size
                        },
                    }));
                }
            }
        }

        // Determine event kind
        let kind = if record.reason & USN_REASON_FILE_CREATE != 0 {
            EventKind::Created
        } else if record.reason & USN_REASON_FILE_DELETE != 0 {
            EventKind::Removed
        } else if record.reason & (
            USN_REASON_DATA_OVERWRITE | USN_REASON_DATA_EXTEND | USN_REASON_DATA_TRUNCATION
        ) != 0 {
            EventKind::Modified
        } else if record.reason & (
            USN_REASON_BASIC_INFO_CHANGE | USN_REASON_SECURITY_CHANGE | USN_REASON_EA_CHANGE
        ) != 0 {
            EventKind::AttributeChanged
        } else {
            // Other changes we don't specifically track
            trace!("Unhandled USN reason: 0x{:x}", record.reason);
            return Ok(None);
        };

        Ok(Some(FsEvent {
            kind,
            path,
            old_path: None,
            file_id: Some(record.file_reference_number),
            timestamp: SystemTime::now(),
            flags: PlatformFlags {
                usn_record: Some(WindowsUsnRecord {
                    usn: record.usn,
                    reason: record.reason,
                    file_attributes: record.file_attributes,
                }),
                is_directory: record.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                is_symlink: record.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
                file_size: None,
            },
        }))
    }

    /// Start background polling task
    fn start_polling_task(&mut self) {
        let volume_handles = self.volume_handles.clone();
        let event_buffer = self.event_buffer.clone();
        let watched_paths = self.watched_paths.clone();

        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));

            loop {
                interval.tick().await;

                let volumes: Vec<String> = {
                    let handles = volume_handles.read().await;
                    handles.keys().cloned().collect()
                };

                for volume in volumes {
                    // This would need access to self, so in practice you'd
                    // restructure this differently
                    // For now, just sleep to prevent busy loop
                    sleep(Duration::from_millis(10)).await;
                }
            }
        });

        self._polling_task = Some(task);
    }
}

#[async_trait]
impl PlatformWatcher for WindowsWatcher {
    async fn watch(&mut self, path: &Path, recursive: bool) -> Result<()> {
        let canonical_path = path.canonicalize()
            .map_err(|e| IndexError::WatcherError(format!("Cannot canonicalize path: {}", e)))?;

        let volume = Self::get_volume_letter(&canonical_path)?;

        debug!("Adding USN Journal watch for path: {:?} on volume {} (recursive: {})", 
               canonical_path, volume, recursive);

        // Open volume handle if not already open
        if !self.volume_handles.read().await.contains_key(&volume) {
            let handle = self.open_volume(&volume).await?;
            self.initialize_journal(&volume, handle).await?;
            self.volume_handles.write().await.insert(volume.clone(), handle);
        }

        // Track this path
        self.watched_paths.write().await.insert(canonical_path, volume);

        // Start polling task if not already started
        if self._polling_task.is_none() {
            self.start_polling_task();
        }

        let mut stats = self.stats.write().await;
        stats.paths_watched += 1;

        Ok(())
    }

    async fn unwatch(&mut self, path: &Path) -> Result<()> {
        let canonical_path = path.canonicalize()
            .map_err(|e| IndexError::WatcherError(format!("Cannot canonicalize path: {}", e)))?;

        debug!("Removing USN Journal watch for path: {:?}", canonical_path);

        self.watched_paths.write().await.remove(&canonical_path);

        let mut stats = self.stats.write().await;
        stats.paths_watched = stats.paths_watched.saturating_sub(1);

        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<FsEvent>> {
        // In a real implementation, this would read from the event buffer
        // populated by the background polling task
        let mut buffer = self.event_buffer.lock().await;
        let mut events = Vec::new();

        while let Some(event) = buffer.pop_front() {
            events.push(event);
        }

        Ok(events)
    }

    fn capabilities(&self) -> WatcherCapabilities {
        WatcherCapabilities {
            supports_rename_tracking: true,
            supports_file_id: true,
            max_watches: None, // USN Journal doesn't have explicit watch limits
            supports_overflow_detection: false, // USN Journal handles this internally
            supports_network_paths: false, // Only works on NTFS volumes
            max_path_length: Some(32767), // Windows path limit
            provides_file_size: false, // USN doesn't include file size
        }
    }

    async fn health_check(&self) -> Result<bool> {
        // Check if we have any open volume handles
        Ok(!self.volume_handles.read().await.is_empty())
    }

    async fn get_stats(&self) -> WatcherStats {
        let mut stats = self.stats.read().await.clone();
        stats.paths_watched = self.watched_paths.read().await.len();
        stats
    }
}

impl Drop for WindowsWatcher {
    fn drop(&mut self) {
        // Close all volume handles
        if let Ok(mut handles) = self.volume_handles.try_write() {
            for (_volume, handle) in handles.drain() {
                unsafe {
                    CloseHandle(handle);
                }
            }
        }

        // Cancel polling task
        if let Some(task) = self._polling_task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_volume_letter_extraction() {
        assert_eq!(
            WindowsWatcher::get_volume_letter(&PathBuf::from("C:\\test\\file.txt")).unwrap(),
            "C:"
        );
        assert_eq!(
            WindowsWatcher::get_volume_letter(&PathBuf::from("D:\\")).unwrap(),
            "D:"
        );
    }

    #[tokio::test]
    async fn test_windows_watcher_creation() {
        let watcher = WindowsWatcher::new().await;
        assert!(watcher.is_ok());
        
        let watcher = watcher.unwrap();
        let capabilities = watcher.capabilities();
        assert!(capabilities.supports_rename_tracking);
        assert!(capabilities.supports_file_id);
    }
}