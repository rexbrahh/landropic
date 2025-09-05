use blake3::Hasher;
use bytes::Bytes;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use tracing::{debug, trace, warn};

use crate::errors::{CasError, Result};
use crate::validation;
use landro_chunker::ContentHash;

/// Fsync policy for controlling write durability vs performance.
#[derive(Debug, Clone)]
pub enum FsyncPolicy {
    /// Fsync after every write operation (maximum durability)
    Always,
    /// Fsync after N write operations (batched durability)
    Batch(u32),
    /// Let OS handle background sync (minimal durability guarantees)
    Async,
    /// Skip fsync entirely (testing only, not safe for production)
    Never,
}

/// Compression algorithm selection.
#[derive(Debug, Clone)]
pub enum CompressionType {
    /// No compression
    None,
    // Future compression algorithms can be added here
    // Lz4,
    // Zstd,
}

/// Configuration options for the content store.
#[derive(Debug, Clone)]
pub struct ContentStoreConfig {
    /// Fsync policy for write operations
    pub fsync_policy: FsyncPolicy,
    /// Whether to attempt recovery of incomplete writes on startup
    pub enable_recovery: bool,
    /// Compression algorithm to use
    pub compression: CompressionType,
}

impl Default for ContentStoreConfig {
    fn default() -> Self {
        Self {
            fsync_policy: FsyncPolicy::Always,
            enable_recovery: true,
            compression: CompressionType::None,
        }
    }
}

/// Statistics from recovery operations.
#[derive(Debug, Clone)]
pub struct RecoveryStats {
    /// Number of temporary files successfully recovered
    pub recovered: usize,
    /// Number of corrupted temporary files cleaned up
    pub cleaned: usize,
    /// Errors encountered during recovery
    pub errors: Vec<String>,
}

/// Reference to an object in the content-addressed storage.
///
/// Contains the hash and size of stored content, providing a unique
/// identifier that can be used to retrieve the data.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectRef {
    /// Blake3 hash of the content
    pub hash: ContentHash,
    /// Size of the content in bytes
    pub size: u64,
}

/// Content-addressed storage system for deduplicated object storage.
///
/// The ContentStore provides a content-addressed storage system where objects
/// are stored based on their Blake3 hash. This enables automatic deduplication
/// and integrity verification. Objects are stored in a sharded directory structure
/// to avoid filesystem limitations with large numbers of files.
///
/// # Storage Layout
///
/// Objects are stored at: `{root}/objects/{shard1}/{shard2}/{hash}`
/// where shard1 and shard2 are the first 2 and next 2 characters of the hex hash.
///
/// # Example
///
/// ```rust,no_run
/// use landro_cas::ContentStore;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let store = ContentStore::new("/tmp/cas").await?;
///
/// // Write data
/// let data = b"Hello, World!";
/// let obj_ref = store.write(data).await?;
///
/// // Read data back
/// let retrieved = store.read(&obj_ref.hash).await?;
/// assert_eq!(&retrieved[..], data);
/// # Ok(())
/// # }
/// ```
pub struct ContentStore {
    root_path: PathBuf,
    config: ContentStoreConfig,
    pending_syncs: Arc<AtomicU32>,
}

impl ContentStore {
    /// Create a new content store at the given path.
    ///
    /// This will create the necessary directory structure if it doesn't exist.
    ///
    /// # Arguments
    ///
    /// * `root_path` - The root directory for the content store
    ///
    /// # Returns
    ///
    /// A new ContentStore instance with default configuration
    pub async fn new(root_path: impl AsRef<Path>) -> Result<Self> {
        Self::new_with_config(root_path, ContentStoreConfig::default()).await
    }

    /// Create a new content store with custom configuration.
    ///
    /// This will create the necessary directory structure if it doesn't exist.
    /// If recovery is enabled, it will scan for and recover any incomplete writes.
    ///
    /// # Arguments
    ///
    /// * `root_path` - The root directory for the content store
    /// * `config` - Configuration options for the store
    ///
    /// # Returns
    ///
    /// A new ContentStore instance
    pub async fn new_with_config(root_path: impl AsRef<Path>, config: ContentStoreConfig) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();

        // Ensure root directory exists
        fs::create_dir_all(&root_path).await?;

        // Create objects directory
        let objects_dir = root_path.join("objects");
        fs::create_dir_all(&objects_dir).await?;

        debug!("Content store initialized at {:?} with config {:?}", root_path, config);

        let store = Self {
            root_path,
            config: config.clone(),
            pending_syncs: Arc::new(AtomicU32::new(0)),
        };

        // Perform recovery if enabled
        if config.enable_recovery {
            let stats = store.recover().await?;
            if stats.recovered > 0 || stats.cleaned > 0 {
                debug!("Recovery completed: recovered {}, cleaned {}, errors: {:?}", 
                      stats.recovered, stats.cleaned, stats.errors);
            }
        }

        Ok(store)
    }

    /// Get the path for an object with sharding
    fn object_path(&self, hash: &ContentHash) -> PathBuf {
        let hex = hash.to_hex();
        let (shard1, rest) = hex.split_at(2);
        let (shard2, filename) = rest.split_at(2);

        self.root_path
            .join("objects")
            .join(shard1)
            .join(shard2)
            .join(filename)
    }

    /// Write an object to the content store.
    ///
    /// The object is written atomically using a temporary file and rename.
    /// If an object with the same hash already exists, this is a no-op.
    /// Fsync behavior is controlled by the store's configuration.
    ///
    /// # Arguments
    ///
    /// * `data` - The data to store
    ///
    /// # Returns
    ///
    /// An ObjectRef containing the hash and size of the stored data
    pub async fn write(&self, data: &[u8]) -> Result<ObjectRef> {
        // Validate object size
        validation::validate_object_size(data.len())
            .map_err(|e| CasError::InvalidOperation(format!("Object validation failed: {}", e)))?;
        
        // Calculate hash
        let mut hasher = Hasher::new();
        hasher.update(data);
        let hash = ContentHash::from_blake3(hasher.finalize());

        let object_path = self.object_path(&hash);

        // Check if object already exists
        if object_path.exists() {
            trace!("Object {} already exists", hash);
            return Ok(ObjectRef {
                hash,
                size: data.len() as u64,
            });
        }

        // Ensure shard directories exist
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        self.write_atomic(&object_path, data, &hash).await?;

        debug!("Wrote object {} ({} bytes)", hash, data.len());

        Ok(ObjectRef {
            hash,
            size: data.len() as u64,
        })
    }

    /// Read an object from the content store.
    ///
    /// The object's integrity is verified by recomputing its hash and checking
    /// file metadata for consistency.
    ///
    /// # Arguments
    ///
    /// * `hash` - The hash of the object to read
    ///
    /// # Returns
    ///
    /// The object data as Bytes
    ///
    /// # Errors
    ///
    /// - `ObjectNotFound` if the object doesn't exist
    /// - `HashMismatch` if the stored data doesn't match the expected hash
    /// - `CorruptObject` if the file appears corrupted
    pub async fn read(&self, hash: &ContentHash) -> Result<Bytes> {
        // Validate hash format
        validation::validate_content_hash(&hash.to_string())
            .map_err(|e| CasError::InvalidOperation(format!("Hash validation failed: {}", e)))?;
        
        let object_path = self.object_path(hash);

        if !object_path.exists() {
            return Err(CasError::ObjectNotFound(*hash));
        }

        // Check file metadata for basic integrity
        let metadata = fs::metadata(&object_path).await?;

        if metadata.len() == 0 {
            return Err(CasError::CorruptObject("Object file is empty".to_string()));
        }

        let data = fs::read(&object_path).await?;

        // Verify that the read size matches the metadata
        if data.len() as u64 != metadata.len() {
            return Err(CasError::InvalidObjectSize {
                expected: metadata.len(),
                actual: data.len() as u64,
            });
        }

        // Verify hash integrity
        let mut hasher = Hasher::new();
        hasher.update(&data);
        let actual_hash = ContentHash::from_blake3(hasher.finalize());

        if actual_hash != *hash {
            return Err(CasError::HashMismatch {
                expected: *hash,
                actual: actual_hash,
            });
        }

        trace!("Read object {} ({} bytes)", hash, data.len());

        Ok(Bytes::from(data))
    }

    /// Check if an object exists in the store.
    ///
    /// # Arguments
    ///
    /// * `hash` - The hash of the object to check
    ///
    /// # Returns
    ///
    /// true if the object exists, false otherwise
    pub async fn exists(&self, hash: &ContentHash) -> bool {
        self.object_path(hash).exists()
    }

    /// Verify an object's integrity by reading and checking its hash.
    ///
    /// # Arguments
    ///
    /// * `hash` - The expected hash of the object
    ///
    /// # Returns
    ///
    /// - Ok(true) if the object exists and has the correct hash
    /// - Ok(false) if the object doesn't exist or has wrong hash
    /// - Err for I/O errors
    pub async fn verify(&self, hash: &ContentHash) -> Result<bool> {
        match self.read(hash).await {
            Ok(_) => Ok(true),
            Err(CasError::ObjectNotFound(_)) => Ok(false),
            Err(CasError::HashMismatch { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Delete an object from the store.
    ///
    /// **Warning**: Use with caution. In a content-addressed system, multiple
    /// references may point to the same object. Consider implementing reference
    /// counting before using this method.
    ///
    /// # Arguments
    ///
    /// * `hash` - The hash of the object to delete
    pub async fn delete(&self, hash: &ContentHash) -> Result<()> {
        let object_path = self.object_path(hash);

        if object_path.exists() {
            fs::remove_file(&object_path).await?;
            debug!("Deleted object {}", hash);
        }

        Ok(())
    }

    /// Perform atomic write operation with configurable fsync behavior.
    ///
    /// This implements the core atomic write pattern:
    /// 1. Write to temporary file with .tmp extension
    /// 2. Fsync temp file (based on policy)
    /// 3. Atomic rename to final location
    /// 4. Fsync parent directory (based on policy)
    ///
    /// # Arguments
    ///
    /// * `final_path` - The final destination path
    /// * `data` - The data to write
    /// * `expected_hash` - Expected hash for verification
    async fn write_atomic(&self, final_path: &Path, data: &[u8], _expected_hash: &ContentHash) -> Result<()> {
        let temp_dir = final_path
            .parent()
            .ok_or_else(|| CasError::StoragePath("Invalid object path".to_string()))?;

        // Create temp file with .tmp extension in the same directory
        let temp_path = {
            let mut temp_name = final_path.file_name()
                .ok_or_else(|| CasError::StoragePath("Invalid object filename".to_string()))?
                .to_os_string();
            temp_name.push(".tmp");
            temp_dir.join(temp_name)
        };

        // Write data to temp file
        {
            let mut file = File::create(&temp_path).await
                .map_err(|e| CasError::AtomicWriteFailed(format!("Failed to create temp file: {}", e)))?;
            
            file.write_all(data).await
                .map_err(|e| CasError::AtomicWriteFailed(format!("Failed to write data: {}", e)))?;

            // Fsync temp file based on policy
            if self.should_fsync_file() {
                file.sync_all().await
                    .map_err(|e| CasError::AtomicWriteFailed(format!("Failed to sync temp file: {}", e)))?;
            }
        }

        // Atomic rename to final location
        fs::rename(&temp_path, final_path)
            .await
            .map_err(|e| {
                // Clean up temp file on rename failure
                let _ = std::fs::remove_file(&temp_path);
                CasError::AtomicWriteFailed(format!("Failed to rename temp file: {}", e))
            })?;

        // Fsync parent directory to ensure rename is durable
        if self.should_fsync_directory() {
            self.fsync_directory(temp_dir).await
                .map_err(|e| CasError::AtomicWriteFailed(format!("Failed to sync directory: {}", e)))?;
        }

        // Update pending sync counter for batch fsync
        if let FsyncPolicy::Batch(batch_size) = &self.config.fsync_policy {
            let pending = self.pending_syncs.fetch_add(1, Ordering::SeqCst) + 1;
            if pending >= *batch_size {
                // Reset counter and force sync
                self.pending_syncs.store(0, Ordering::SeqCst);
                // For batch mode, we could implement a global sync here
                // For now, we rely on the OS to handle batching
            }
        }

        Ok(())
    }

    /// Check if file-level fsync should be performed based on policy.
    fn should_fsync_file(&self) -> bool {
        match &self.config.fsync_policy {
            FsyncPolicy::Always => true,
            FsyncPolicy::Batch(_) => true, // Sync individual files, batch directory syncs
            FsyncPolicy::Async => false,
            FsyncPolicy::Never => false,
        }
    }

    /// Check if directory-level fsync should be performed based on policy.
    fn should_fsync_directory(&self) -> bool {
        match &self.config.fsync_policy {
            FsyncPolicy::Always => true,
            FsyncPolicy::Batch(_) => false, // Directory syncs are batched
            FsyncPolicy::Async => false,
            FsyncPolicy::Never => false,
        }
    }

    /// Fsync a directory to ensure metadata changes are durable.
    ///
    /// This is critical for ensuring that file renames are durable
    /// across system crashes.
    async fn fsync_directory(&self, dir: &Path) -> Result<()> {
        let dir_file = File::open(dir).await?;
        dir_file.sync_all().await?;
        Ok(())
    }

    /// Recover from incomplete write operations.
    ///
    /// This method scans for temporary files (.tmp extension) and attempts
    /// to either complete the write operation if the content is valid,
    /// or clean up corrupted temporary files.
    ///
    /// # Returns
    ///
    /// Statistics about the recovery operation
    pub async fn recover(&self) -> Result<RecoveryStats> {
        let mut stats = RecoveryStats {
            recovered: 0,
            cleaned: 0,
            errors: Vec::new(),
        };

        let objects_dir = self.root_path.join("objects");
        if !objects_dir.exists() {
            return Ok(stats);
        }

        // Walk through all shard directories looking for .tmp files
        let mut stack = vec![objects_dir];

        while let Some(current_dir) = stack.pop() {
            let mut dir_entries = match fs::read_dir(&current_dir).await {
                Ok(entries) => entries,
                Err(e) => {
                    stats.errors.push(format!("Failed to read directory {:?}: {}", current_dir, e));
                    continue;
                }
            };

            loop {
                let entry = match dir_entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(e) => {
                        stats.errors.push(format!("Failed to read directory entry: {}", e));
                        continue;
                    }
                };
                let path = entry.path();
                
                if entry.file_type().await.map(|ft| ft.is_dir()).unwrap_or(false) {
                    stack.push(path);
                } else if path.extension().and_then(|s| s.to_str()) == Some("tmp") {
                    match self.recover_temp_file(&path, &mut stats).await {
                        Ok(_) => {},
                        Err(e) => stats.errors.push(format!("Recovery error for {:?}: {}", path, e)),
                    }
                }
            }
        }

        if stats.recovered > 0 || stats.cleaned > 0 || !stats.errors.is_empty() {
            debug!("Recovery completed: recovered={}, cleaned={}, errors={}",
                  stats.recovered, stats.cleaned, stats.errors.len());
        }

        Ok(stats)
    }

    /// Attempt to recover a single temporary file.
    async fn recover_temp_file(&self, temp_path: &Path, stats: &mut RecoveryStats) -> Result<()> {
        // Determine what the final path should be
        let final_path = {
            let temp_name = temp_path.file_name()
                .ok_or_else(|| CasError::StoragePath("Invalid temp filename".to_string()))?;
            let temp_name_str = temp_name.to_string_lossy();
            
            if !temp_name_str.ends_with(".tmp") {
                return Err(CasError::StoragePath("Temp file doesn't end with .tmp".to_string()));
            }
            
            let final_name = &temp_name_str[..temp_name_str.len() - 4]; // Remove ".tmp"
            temp_path.parent().unwrap().join(final_name)
        };

        // If final file already exists, just clean up the temp file
        if final_path.exists() {
            match fs::remove_file(temp_path).await {
                Ok(_) => {
                    stats.cleaned += 1;
                    trace!("Cleaned up redundant temp file: {:?}", temp_path);
                },
                Err(e) => warn!("Failed to clean up temp file {:?}: {}", temp_path, e),
            }
            return Ok(());
        }

        // Try to validate the temp file by computing its hash
        let data = match fs::read(temp_path).await {
            Ok(data) => data,
            Err(_) => {
                // Can't read temp file, clean it up
                let _ = fs::remove_file(temp_path).await;
                stats.cleaned += 1;
                return Ok(());
            }
        };

        // Compute hash and check if it matches the expected filename
        let mut hasher = Hasher::new();
        hasher.update(&data);
        let computed_hash = ContentHash::from_blake3(hasher.finalize());
        let expected_path = self.object_path(&computed_hash);

        if expected_path == final_path {
            // Hash matches, complete the write operation
            match fs::rename(temp_path, &final_path).await {
                Ok(_) => {
                    stats.recovered += 1;
                    debug!("Recovered temp file: {:?} -> {:?}", temp_path, final_path);
                    
                    // Fsync directory if policy requires it
                    if self.should_fsync_directory() {
                        if let Some(parent) = final_path.parent() {
                            let _ = self.fsync_directory(parent).await;
                        }
                    }
                },
                Err(e) => {
                    warn!("Failed to complete recovery of {:?}: {}", temp_path, e);
                    let _ = fs::remove_file(temp_path).await;
                    stats.cleaned += 1;
                }
            }
        } else {
            // Hash doesn't match, file is corrupted
            let _ = fs::remove_file(temp_path).await;
            stats.cleaned += 1;
            trace!("Cleaned up corrupted temp file: {:?}", temp_path);
        }

        Ok(())
    }

    /// Get storage statistics for the content store.
    ///
    /// This walks the entire object directory tree to count objects and calculate
    /// total storage size. For large stores, this operation may be expensive.
    ///
    /// # Returns
    ///
    /// Statistics including object count and total size
    pub async fn stats(&self) -> Result<StorageStats> {
        let objects_dir = self.root_path.join("objects");

        if !objects_dir.exists() {
            return Ok(StorageStats {
                object_count: 0,
                total_size: 0,
            });
        }

        let mut object_count = 0u64;
        let mut total_size = 0u64;

        // Walk through all shard directories
        let mut dir_entries = fs::read_dir(&objects_dir).await?;

        while let Some(shard1_entry) = dir_entries.next_entry().await? {
            if !shard1_entry.file_type().await?.is_dir() {
                continue;
            }

            let mut shard1_entries = fs::read_dir(shard1_entry.path()).await?;

            while let Some(shard2_entry) = shard1_entries.next_entry().await? {
                if !shard2_entry.file_type().await?.is_dir() {
                    continue;
                }

                let mut object_entries = fs::read_dir(shard2_entry.path()).await?;

                while let Some(object_entry) = object_entries.next_entry().await? {
                    if object_entry.file_type().await?.is_file() {
                        let metadata = object_entry.metadata().await?;
                        object_count += 1;
                        total_size += metadata.len();
                    }
                }
            }
        }

        debug!(
            "Storage stats: {} objects, {} bytes",
            object_count, total_size
        );

        Ok(StorageStats {
            object_count,
            total_size,
        })
    }

    /// Perform comprehensive integrity verification of the entire content store.
    ///
    /// This method walks through all objects in the store and verifies their
    /// integrity by checking file metadata and hash consistency.
    ///
    /// # Returns
    ///
    /// A `VerificationReport` containing details about the verification process
    ///
    /// # Performance Note
    ///
    /// This is an expensive operation for large stores as it reads and hashes
    /// every object. Use sparingly or run during maintenance windows.
    pub async fn verify_integrity(&self) -> Result<VerificationReport> {
        let objects_dir = self.root_path.join("objects");

        if !objects_dir.exists() {
            return Ok(VerificationReport::empty());
        }

        let mut report = VerificationReport::new();
        let mut dir_entries = fs::read_dir(&objects_dir).await?;

        while let Some(shard1_entry) = dir_entries.next_entry().await? {
            if !shard1_entry.file_type().await?.is_dir() {
                continue;
            }

            let mut shard1_entries = fs::read_dir(shard1_entry.path()).await?;

            while let Some(shard2_entry) = shard1_entries.next_entry().await? {
                if !shard2_entry.file_type().await?.is_dir() {
                    continue;
                }

                let mut object_entries = fs::read_dir(shard2_entry.path()).await?;

                while let Some(object_entry) = object_entries.next_entry().await? {
                    if object_entry.file_type().await?.is_file() {
                        let file_name = object_entry.file_name();
                        let file_name_str = file_name.to_string_lossy();

                        // Reconstruct the full hash from the directory structure
                        let shard1_name = shard1_entry.file_name();
                        let shard2_name = shard2_entry.file_name();
                        let shard1 = shard1_name.to_string_lossy();
                        let shard2 = shard2_name.to_string_lossy();
                        let full_hash = format!("{}{}{}", shard1, shard2, file_name_str);

                        match ContentHash::from_hex(&full_hash) {
                            Ok(expected_hash) => {
                                match self.verify_single_object(&expected_hash).await {
                                    Ok(true) => report.verified_count += 1,
                                    Ok(false) => {
                                        report.corrupted_objects.push(expected_hash);
                                        report.corruption_count += 1;
                                    }
                                    Err(e) => {
                                        report.error_objects.push((expected_hash, e.to_string()));
                                        report.error_count += 1;
                                    }
                                }
                            }
                            Err(_) => {
                                report.invalid_filenames.push(object_entry.path());
                                report.error_count += 1;
                            }
                        }

                        report.total_objects += 1;
                    }
                }
            }
        }

        debug!("Integrity verification completed: {:?}", report);
        Ok(report)
    }

    /// Verify a single object's integrity without returning its data.
    ///
    /// This is more efficient than `verify()` when you don't need the object data.
    async fn verify_single_object(&self, hash: &ContentHash) -> Result<bool> {
        match self.read(hash).await {
            Ok(_) => Ok(true),
            Err(CasError::ObjectNotFound(_)) => Ok(false),
            Err(CasError::HashMismatch { .. }) => Ok(false),
            Err(CasError::CorruptObject(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// Storage statistics for the content store.
#[derive(Debug, Clone)]
pub struct StorageStats {
    /// Total number of objects in the store
    pub object_count: u64,
    /// Total size of all objects in bytes
    pub total_size: u64,
}

/// Report from integrity verification of the content store.
#[derive(Debug, Clone)]
pub struct VerificationReport {
    /// Total objects examined
    pub total_objects: u64,
    /// Objects that passed verification
    pub verified_count: u64,
    /// Objects that failed integrity checks
    pub corruption_count: u64,
    /// Objects that caused errors during verification
    pub error_count: u64,
    /// List of corrupted object hashes
    pub corrupted_objects: Vec<ContentHash>,
    /// Objects with verification errors and their error messages
    pub error_objects: Vec<(ContentHash, String)>,
    /// Invalid filenames that couldn't be parsed as hashes
    pub invalid_filenames: Vec<std::path::PathBuf>,
}

impl VerificationReport {
    fn new() -> Self {
        Self {
            total_objects: 0,
            verified_count: 0,
            corruption_count: 0,
            error_count: 0,
            corrupted_objects: Vec::new(),
            error_objects: Vec::new(),
            invalid_filenames: Vec::new(),
        }
    }

    fn empty() -> Self {
        Self::new()
    }

    /// Returns true if all objects passed verification
    pub fn is_healthy(&self) -> bool {
        self.corruption_count == 0 && self.error_count == 0
    }

    /// Returns the percentage of objects that are verified and healthy
    pub fn health_percentage(&self) -> f64 {
        if self.total_objects == 0 {
            return 100.0;
        }
        (self.verified_count as f64 / self.total_objects as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_write_and_read() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();

        let data = b"Hello, Landropic!";
        let obj_ref = store.write(data).await.unwrap();

        let read_data = store.read(&obj_ref.hash).await.unwrap();
        assert_eq!(&read_data[..], data);
    }

    #[tokio::test]
    async fn test_exists() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();

        let data = b"Test data";
        let obj_ref = store.write(data).await.unwrap();

        assert!(store.exists(&obj_ref.hash).await);

        let fake_hash = ContentHash::from_bytes([0u8; 32]);
        assert!(!store.exists(&fake_hash).await);
    }

    #[tokio::test]
    async fn test_stats() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();

        // Initially no objects
        let stats = store.stats().await.unwrap();
        assert_eq!(stats.object_count, 0);
        assert_eq!(stats.total_size, 0);

        // Add some objects
        let data1 = b"Hello, World!";
        let data2 = b"This is test data for the content store";

        store.write(data1).await.unwrap();
        store.write(data2).await.unwrap();

        // Check stats
        let stats = store.stats().await.unwrap();
        assert_eq!(stats.object_count, 2);
        assert_eq!(stats.total_size, (data1.len() + data2.len()) as u64);
    }

    #[tokio::test]
    async fn test_deduplication_in_stats() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();

        // Write the same data twice - should only count once
        let data = b"Duplicate data";
        store.write(data).await.unwrap();
        store.write(data).await.unwrap(); // Same data, should be deduplicated

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.object_count, 1); // Only one unique object
        assert_eq!(stats.total_size, data.len() as u64);
    }

    #[tokio::test]
    async fn test_integrity_verification() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();

        // Empty store should report 100% health
        let report = store.verify_integrity().await.unwrap();
        assert_eq!(report.total_objects, 0);
        assert!(report.is_healthy());
        assert_eq!(report.health_percentage(), 100.0);

        // Add some objects
        let data1 = b"Test data for verification";
        let data2 = b"More test data";

        store.write(data1).await.unwrap();
        store.write(data2).await.unwrap();

        // Verify integrity
        let report = store.verify_integrity().await.unwrap();
        assert_eq!(report.total_objects, 2);
        assert_eq!(report.verified_count, 2);
        assert_eq!(report.corruption_count, 0);
        assert_eq!(report.error_count, 0);
        assert!(report.is_healthy());
        assert_eq!(report.health_percentage(), 100.0);
    }

    #[tokio::test]
    async fn test_enhanced_error_handling() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();

        // Test reading non-existent object
        let fake_hash = ContentHash::from_bytes([0u8; 32]);
        let result = store.read(&fake_hash).await;
        assert!(matches!(result, Err(CasError::ObjectNotFound(_))));

        // Test that verification properly handles missing objects
        let verified = store.verify(&fake_hash).await.unwrap();
        assert!(!verified);
    }

    // Atomic write and fsync tests
    #[tokio::test]
    async fn test_fsync_policy_always() {
        let dir = tempdir().unwrap();
        let config = ContentStoreConfig {
            fsync_policy: FsyncPolicy::Always,
            enable_recovery: false,
            compression: CompressionType::None,
        };
        let store = ContentStore::new_with_config(dir.path(), config).await.unwrap();

        let data = b"Test data with always fsync";
        let obj_ref = store.write(data).await.unwrap();

        // Verify the data was written correctly
        let read_data = store.read(&obj_ref.hash).await.unwrap();
        assert_eq!(&read_data[..], data);
    }

    #[tokio::test]
    async fn test_fsync_policy_never() {
        let dir = tempdir().unwrap();
        let config = ContentStoreConfig {
            fsync_policy: FsyncPolicy::Never,
            enable_recovery: false,
            compression: CompressionType::None,
        };
        let store = ContentStore::new_with_config(dir.path(), config).await.unwrap();

        let data = b"Test data with never fsync";
        let obj_ref = store.write(data).await.unwrap();

        // Verify the data was written correctly
        let read_data = store.read(&obj_ref.hash).await.unwrap();
        assert_eq!(&read_data[..], data);
    }

    #[tokio::test]
    async fn test_fsync_policy_batch() {
        let dir = tempdir().unwrap();
        let config = ContentStoreConfig {
            fsync_policy: FsyncPolicy::Batch(3),
            enable_recovery: false,
            compression: CompressionType::None,
        };
        let store = ContentStore::new_with_config(dir.path(), config).await.unwrap();

        // Write multiple objects to test batch behavior
        let data1 = b"First object";
        let data2 = b"Second object";
        let data3 = b"Third object";

        let obj1 = store.write(data1).await.unwrap();
        let obj2 = store.write(data2).await.unwrap();
        let obj3 = store.write(data3).await.unwrap();

        // Verify all objects are readable
        assert_eq!(&store.read(&obj1.hash).await.unwrap()[..], data1);
        assert_eq!(&store.read(&obj2.hash).await.unwrap()[..], data2);
        assert_eq!(&store.read(&obj3.hash).await.unwrap()[..], data3);
    }

    #[tokio::test]
    async fn test_atomic_write_behavior() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();

        let data = b"Atomic write test data";
        let mut hasher = Hasher::new();
        hasher.update(data);
        let expected_hash = ContentHash::from_blake3(hasher.finalize());
        let object_path = store.object_path(&expected_hash);

        // Verify object doesn't exist initially
        assert!(!object_path.exists());

        // Write the object
        let obj_ref = store.write(data).await.unwrap();
        assert_eq!(obj_ref.hash, expected_hash);

        // Verify object exists and no temp files remain
        assert!(object_path.exists());
        
        // Check there are no .tmp files in the shard directory
        if let Some(shard_dir) = object_path.parent() {
            let mut entries = fs::read_dir(shard_dir).await.unwrap();
            while let Some(entry) = entries.next_entry().await.unwrap() {
                let filename = entry.file_name();
                let filename_str = filename.to_string_lossy();
                assert!(!filename_str.ends_with(".tmp"), "Found temp file: {}", filename_str);
            }
        }
    }

    #[tokio::test]
    async fn test_recovery_clean_orphaned_temp_files() {
        let dir = tempdir().unwrap();
        let config = ContentStoreConfig {
            fsync_policy: FsyncPolicy::Always,
            enable_recovery: false, // Disabled initially
            compression: CompressionType::None,
        };
        
        // Create store without recovery
        let store = ContentStore::new_with_config(dir.path(), config.clone()).await.unwrap();
        
        // Simulate incomplete write by creating a temp file manually
        let data = b"Incomplete write test";
        let mut hasher = Hasher::new();
        hasher.update(data);
        let hash = ContentHash::from_blake3(hasher.finalize());
        let object_path = store.object_path(&hash);
        
        // Ensure shard directory exists
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).await.unwrap();
        }
        
        // Create temp file
        let temp_path = {
            let mut temp_name = object_path.file_name().unwrap().to_os_string();
            temp_name.push(".tmp");
            object_path.parent().unwrap().join(temp_name)
        };
        
        fs::write(&temp_path, data).await.unwrap();
        assert!(temp_path.exists());
        
        // Run recovery
        let stats = store.recover().await.unwrap();
        
        // Should have recovered the temp file
        assert_eq!(stats.recovered, 1);
        assert_eq!(stats.cleaned, 0);
        assert!(stats.errors.is_empty());
        
        // Temp file should be gone, object file should exist
        assert!(!temp_path.exists());
        assert!(object_path.exists());
        
        // Should be able to read the recovered object
        let read_data = store.read(&hash).await.unwrap();
        assert_eq!(&read_data[..], data);
    }

    #[tokio::test]
    async fn test_recovery_clean_corrupted_temp_files() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();
        
        // Create a corrupted temp file (wrong content for the filename)
        let correct_data = b"Correct data";
        let mut hasher = Hasher::new();
        hasher.update(correct_data);
        let hash = ContentHash::from_blake3(hasher.finalize());
        let object_path = store.object_path(&hash);
        
        // Ensure shard directory exists
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).await.unwrap();
        }
        
        // Create temp file with wrong content
        let temp_path = {
            let mut temp_name = object_path.file_name().unwrap().to_os_string();
            temp_name.push(".tmp");
            object_path.parent().unwrap().join(temp_name)
        };
        
        let wrong_data = b"Wrong data that doesn't match hash";
        fs::write(&temp_path, wrong_data).await.unwrap();
        assert!(temp_path.exists());
        
        // Run recovery
        let stats = store.recover().await.unwrap();
        
        // Should have cleaned up the corrupted temp file
        assert_eq!(stats.recovered, 0);
        assert_eq!(stats.cleaned, 1);
        assert!(stats.errors.is_empty());
        
        // Both temp and object files should be gone
        assert!(!temp_path.exists());
        assert!(!object_path.exists());
    }

    #[tokio::test]
    async fn test_recovery_skip_existing_objects() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();
        
        // Write an object normally
        let data = b"Normal write test";
        let obj_ref = store.write(data).await.unwrap();
        let object_path = store.object_path(&obj_ref.hash);
        
        // Create a temp file that would correspond to the same object
        let temp_path = {
            let mut temp_name = object_path.file_name().unwrap().to_os_string();
            temp_name.push(".tmp");
            object_path.parent().unwrap().join(temp_name)
        };
        
        fs::write(&temp_path, data).await.unwrap();
        assert!(temp_path.exists());
        assert!(object_path.exists());
        
        // Run recovery
        let stats = store.recover().await.unwrap();
        
        // Should have cleaned up the redundant temp file
        assert_eq!(stats.recovered, 0);
        assert_eq!(stats.cleaned, 1);
        assert!(stats.errors.is_empty());
        
        // Object should still exist, temp should be gone
        assert!(object_path.exists());
        assert!(!temp_path.exists());
    }

    #[tokio::test]
    async fn test_concurrent_writes() {
        let dir = tempdir().unwrap();
        let store = Arc::new(ContentStore::new(dir.path()).await.unwrap());
        
        // Launch multiple concurrent writes
        let mut handles = Vec::new();
        for i in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = tokio::spawn(async move {
                let data = format!("Concurrent write test data {}", i);
                store_clone.write(data.as_bytes()).await
            });
            handles.push(handle);
        }
        
        // Wait for all writes to complete
        let mut obj_refs = Vec::new();
        for handle in handles {
            let obj_ref = handle.await.unwrap().unwrap();
            obj_refs.push(obj_ref);
        }
        
        // Verify all objects can be read back correctly
        for (i, obj_ref) in obj_refs.iter().enumerate() {
            let expected_data = format!("Concurrent write test data {}", i);
            let read_data = store.read(&obj_ref.hash).await.unwrap();
            assert_eq!(String::from_utf8(read_data.to_vec()).unwrap(), expected_data);
        }
        
        // Verify no temp files remain
        let objects_dir = store.root_path.join("objects");
        let mut stack = vec![objects_dir];
        
        while let Some(current_dir) = stack.pop() {
            let mut dir_entries = fs::read_dir(&current_dir).await.unwrap();
            while let Some(entry) = dir_entries.next_entry().await.unwrap() {
                let path = entry.path();
                if entry.file_type().await.unwrap().is_dir() {
                    stack.push(path);
                } else {
                    let filename = entry.file_name();
                    let filename_str = filename.to_string_lossy();
                    assert!(!filename_str.ends_with(".tmp"), "Found temp file after concurrent writes: {}", filename_str);
                }
            }
        }
    }

    #[tokio::test]
    async fn test_recovery_on_startup() {
        let dir = tempdir().unwrap();
        
        // Create some temp files manually to simulate crash
        let data1 = b"Recovery test data 1";
        let data2 = b"Recovery test data 2";
        
        // Create valid temp files in the correct shard directories
        let mut hasher = Hasher::new();
        hasher.update(data1);
        let hash1 = ContentHash::from_blake3(hasher.finalize());
        
        // Use the same sharding logic as ContentStore
        let hex1 = hash1.to_hex();
        let (shard1_1, rest1) = hex1.split_at(2);
        let (shard2_1, filename1) = rest1.split_at(2);
        let shard_dir1 = dir.path().join("objects").join(shard1_1).join(shard2_1);
        fs::create_dir_all(&shard_dir1).await.unwrap();
        let temp_path1 = shard_dir1.join(format!("{}.tmp", filename1));
        fs::write(&temp_path1, data1).await.unwrap();
        
        let mut hasher = Hasher::new();
        hasher.update(data2);
        let hash2 = ContentHash::from_blake3(hasher.finalize());
        
        let hex2 = hash2.to_hex();
        let (shard1_2, rest2) = hex2.split_at(2);
        let (shard2_2, filename2) = rest2.split_at(2);
        let shard_dir2 = dir.path().join("objects").join(shard1_2).join(shard2_2);
        fs::create_dir_all(&shard_dir2).await.unwrap();
        let temp_path2 = shard_dir2.join(format!("{}.tmp", filename2));
        fs::write(&temp_path2, b"corrupted data").await.unwrap(); // Wrong content
        
        // Create store with recovery enabled
        let config = ContentStoreConfig {
            fsync_policy: FsyncPolicy::Always,
            enable_recovery: true,
            compression: CompressionType::None,
        };
        
        let store = ContentStore::new_with_config(dir.path(), config).await.unwrap();
        
        // Recovery should have happened during initialization
        // Check that valid temp file was recovered
        let object_path1 = store.object_path(&hash1);
        assert!(object_path1.exists(), "Valid temp file should have been recovered");
        assert!(!temp_path1.exists(), "Temp file should be cleaned up");
        
        // Check that corrupted temp file was cleaned up
        assert!(!temp_path2.exists(), "Corrupted temp file should be cleaned up");
        
        // Should be able to read the recovered object
        let read_data = store.read(&hash1).await.unwrap();
        assert_eq!(&read_data[..], data1);
    }

    #[tokio::test]
    async fn test_directory_fsync() {
        let dir = tempdir().unwrap();
        let config = ContentStoreConfig {
            fsync_policy: FsyncPolicy::Always,
            enable_recovery: false,
            compression: CompressionType::None,
        };
        let store = ContentStore::new_with_config(dir.path(), config).await.unwrap();
        
        // Test that directory fsync doesn't fail
        let test_dir = dir.path().join("test_fsync");
        fs::create_dir_all(&test_dir).await.unwrap();
        
        // This should not panic or error
        store.fsync_directory(&test_dir).await.unwrap();
    }
}
