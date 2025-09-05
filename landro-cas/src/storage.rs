use blake3::Hasher;
use bytes::Bytes;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, trace};

use crate::errors::{CasError, Result};
use landro_chunker::ContentHash;

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
    /// A new ContentStore instance
    pub async fn new(root_path: impl AsRef<Path>) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();

        // Ensure root directory exists
        fs::create_dir_all(&root_path).await?;

        // Create objects directory
        let objects_dir = root_path.join("objects");
        fs::create_dir_all(&objects_dir).await?;

        debug!("Content store initialized at {:?}", root_path);

        Ok(Self { root_path })
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
    ///
    /// # Arguments
    ///
    /// * `data` - The data to store
    ///
    /// # Returns
    ///
    /// An ObjectRef containing the hash and size of the stored data
    pub async fn write(&self, data: &[u8]) -> Result<ObjectRef> {
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

        // Write atomically using temp file
        let temp_dir = object_path
            .parent()
            .ok_or_else(|| CasError::StoragePath("Invalid object path".to_string()))?;

        let temp_file = NamedTempFile::new_in(temp_dir)
            .map_err(|e| CasError::AtomicWriteFailed(e.to_string()))?;

        let temp_path = temp_file.path().to_path_buf();

        // Write data to temp file
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(data).await?;
        file.sync_all().await?;

        // Atomic rename
        fs::rename(&temp_path, &object_path)
            .await
            .map_err(|e| CasError::AtomicWriteFailed(e.to_string()))?;

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
}
