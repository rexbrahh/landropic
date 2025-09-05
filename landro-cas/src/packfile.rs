use blake3::Hasher;
use bytes::Bytes;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tracing::{debug, trace};

use crate::errors::{CasError, Result};
use landro_chunker::ContentHash;

/// Minimum size threshold for objects to be stored in packfiles
/// Objects smaller than this will be packed together
const PACKFILE_THRESHOLD: u64 = 4096; // 4KB

/// Target size for each packfile
const PACKFILE_TARGET_SIZE: u64 = 32 * 1024 * 1024; // 32MB

/// Header for packfile format
const PACKFILE_MAGIC: &[u8] = b"LANDRO_PACK_V1";

/// Entry in a packfile representing a stored object
#[derive(Debug, Clone)]
pub struct PackEntry {
    /// Hash of the object
    pub hash: ContentHash,
    /// Offset within the packfile
    pub offset: u64,
    /// Size of the object data
    pub size: u64,
}

/// Index for a packfile containing metadata about stored objects
#[derive(Debug)]
pub struct PackIndex {
    /// Path to the packfile
    pub packfile_path: PathBuf,
    /// Map from content hash to pack entry
    pub entries: HashMap<ContentHash, PackEntry>,
    /// Total size of data in the packfile
    pub data_size: u64,
}

impl PackIndex {
    /// Create a new empty pack index
    pub fn new(packfile_path: PathBuf) -> Self {
        Self {
            packfile_path,
            entries: HashMap::new(),
            data_size: 0,
        }
    }

    /// Add an entry to the index
    pub fn add_entry(&mut self, entry: PackEntry) {
        self.data_size += entry.size;
        self.entries.insert(entry.hash, entry);
    }

    /// Check if the packfile has reached the target size
    pub fn is_full(&self) -> bool {
        self.data_size >= PACKFILE_TARGET_SIZE
    }

    /// Get the number of objects in this packfile
    pub fn object_count(&self) -> usize {
        self.entries.len()
    }
}

/// Manager for packfile-based storage of small objects
#[derive(Debug)]
pub struct PackfileManager {
    /// Root directory for packfiles
    pack_dir: PathBuf,
    /// Currently active packfile being written to
    current_pack: Option<PackIndex>,
    /// Map of packfile names to their indexes
    pack_indexes: HashMap<String, PackIndex>,
}

impl PackfileManager {
    /// Create a new packfile manager
    pub async fn new(root_path: impl AsRef<Path>) -> Result<Self> {
        let pack_dir = root_path.as_ref().join("packs");
        fs::create_dir_all(&pack_dir).await?;

        let mut manager = Self {
            pack_dir,
            current_pack: None,
            pack_indexes: HashMap::new(),
        };

        // Load existing packfiles
        manager.load_existing_packs().await?;

        debug!("Packfile manager initialized with {} packs", manager.pack_indexes.len());
        Ok(manager)
    }

    /// Load existing packfiles from disk
    async fn load_existing_packs(&mut self) -> Result<()> {
        let mut entries = fs::read_dir(&self.pack_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension() == Some(std::ffi::OsStr::new("pack")) {
                if let Some(pack_name) = path.file_stem() {
                    let index_path = self.pack_dir.join(format!("{}.idx", pack_name.to_string_lossy()));
                    
                    if index_path.exists() {
                        match self.load_pack_index(&path, &index_path).await {
                            Ok(index) => {
                                let pack_name = pack_name.to_string_lossy().to_string();
                                self.pack_indexes.insert(pack_name, index);
                            }
                            Err(e) => {
                                trace!("Failed to load pack index {:?}: {}", index_path, e);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Load a pack index from disk
    async fn load_pack_index(&self, _pack_path: &Path, _index_path: &Path) -> Result<PackIndex> {
        // TODO: Implement actual index loading from disk
        // For now, return an error to indicate this needs implementation
        Err(CasError::InconsistentState(
            "Pack index loading not yet implemented".to_string(),
        ))
    }

    /// Determine if an object should be stored in a packfile
    pub fn should_pack(&self, size: u64) -> bool {
        size <= PACKFILE_THRESHOLD
    }

    /// Store an object in a packfile
    pub async fn store_packed(&mut self, hash: ContentHash, data: &[u8]) -> Result<()> {
        trace!("Storing {} bytes in packfile for hash {}", data.len(), hash);

        // Get or create current packfile
        let pack_index = self.get_or_create_current_pack().await?;
        let packfile_path = pack_index.packfile_path.clone();

        // Open packfile for appending
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&packfile_path)
            .await?;

        // Get current position (this will be our offset)
        let offset = file.seek(SeekFrom::End(0)).await?;

        // Write the data
        file.write_all(data).await?;
        file.sync_all().await?;

        // Add entry to index
        let entry = PackEntry {
            hash,
            offset,
            size: data.len() as u64,
        };

        pack_index.add_entry(entry);

        // Check if we need to finalize this packfile
        if pack_index.is_full() {
            self.finalize_current_pack().await?;
        }

        debug!("Stored object {} in packfile", hash);
        Ok(())
    }

    /// Read an object from a packfile
    pub async fn read_packed(&self, hash: &ContentHash) -> Result<Option<Bytes>> {
        // Find which packfile contains this object
        for pack_index in self.pack_indexes.values() {
            if let Some(entry) = pack_index.entries.get(hash) {
                return Ok(Some(self.read_from_pack(pack_index, entry).await?));
            }
        }

        // Check current pack
        if let Some(current_pack) = &self.current_pack {
            if let Some(entry) = current_pack.entries.get(hash) {
                return Ok(Some(self.read_from_pack(current_pack, entry).await?));
            }
        }

        Ok(None)
    }

    /// Read data from a specific packfile entry
    async fn read_from_pack(&self, pack_index: &PackIndex, entry: &PackEntry) -> Result<Bytes> {
        let mut file = fs::File::open(&pack_index.packfile_path).await?;
        
        // Seek to the object's position
        file.seek(SeekFrom::Start(entry.offset)).await?;

        // Read the object data
        let mut buffer = vec![0u8; entry.size as usize];
        file.read_exact(&mut buffer).await?;

        // Verify the hash
        let mut hasher = Hasher::new();
        hasher.update(&buffer);
        let actual_hash = ContentHash::from_blake3(hasher.finalize());

        if actual_hash != entry.hash {
            return Err(CasError::HashMismatch {
                expected: entry.hash,
                actual: actual_hash,
            });
        }

        Ok(Bytes::from(buffer))
    }

    /// Get or create the current packfile
    async fn get_or_create_current_pack(&mut self) -> Result<&mut PackIndex> {
        if self.current_pack.is_none() || self.current_pack.as_ref().unwrap().is_full() {
            self.create_new_pack().await?;
        }

        Ok(self.current_pack.as_mut().unwrap())
    }

    /// Create a new packfile
    async fn create_new_pack(&mut self) -> Result<()> {
        let pack_id = chrono::Utc::now().timestamp();
        let pack_name = format!("pack-{:016x}", pack_id);
        let packfile_path = self.pack_dir.join(format!("{}.pack", pack_name));

        // Create the packfile with header
        let mut file = fs::File::create(&packfile_path).await?;
        file.write_all(PACKFILE_MAGIC).await?;
        file.sync_all().await?;

        let pack_index = PackIndex::new(packfile_path);
        self.current_pack = Some(pack_index);

        debug!("Created new packfile: {}", pack_name);
        Ok(())
    }

    /// Finalize the current packfile and create its index
    async fn finalize_current_pack(&mut self) -> Result<()> {
        if let Some(pack_index) = self.current_pack.take() {
            let pack_name = pack_index
                .packfile_path
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .to_string();

            debug!(
                "Finalizing packfile {} with {} objects",
                pack_name,
                pack_index.object_count()
            );

            // TODO: Write index to disk
            // For now, just move it to the permanent collection
            self.pack_indexes.insert(pack_name, pack_index);
        }

        Ok(())
    }

    /// Get statistics about packfile usage
    pub fn stats(&self) -> PackfileStats {
        let mut stats = PackfileStats {
            total_packs: self.pack_indexes.len(),
            total_objects: 0,
            total_packed_size: 0,
        };

        for pack_index in self.pack_indexes.values() {
            stats.total_objects += pack_index.object_count();
            stats.total_packed_size += pack_index.data_size;
        }

        // Include current pack
        if let Some(current_pack) = &self.current_pack {
            stats.total_objects += current_pack.object_count();
            stats.total_packed_size += current_pack.data_size;
            if current_pack.object_count() > 0 {
                stats.total_packs += 1;
            }
        }

        stats
    }
}

/// Statistics about packfile storage
#[derive(Debug, Clone)]
pub struct PackfileStats {
    /// Total number of packfiles
    pub total_packs: usize,
    /// Total objects stored in packfiles
    pub total_objects: usize,
    /// Total size of data stored in packfiles
    pub total_packed_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_packfile_creation() {
        let dir = tempdir().unwrap();
        let manager = PackfileManager::new(dir.path()).await.unwrap();

        // Test that small objects should be packed
        assert!(manager.should_pack(1024)); // 1KB
        assert!(!manager.should_pack(8192)); // 8KB

        let stats = manager.stats();
        assert_eq!(stats.total_packs, 0);
        assert_eq!(stats.total_objects, 0);
    }

    #[tokio::test]
    async fn test_pack_and_read() {
        let dir = tempdir().unwrap();
        let mut manager = PackfileManager::new(dir.path()).await.unwrap();

        // Store some small objects
        let data1 = b"Small test data 1";
        let data2 = b"Small test data 2";

        let mut hasher1 = Hasher::new();
        hasher1.update(data1);
        let hash1 = ContentHash::from_blake3(hasher1.finalize());

        let mut hasher2 = Hasher::new();
        hasher2.update(data2);
        let hash2 = ContentHash::from_blake3(hasher2.finalize());

        // Store the objects
        manager.store_packed(hash1, data1).await.unwrap();
        manager.store_packed(hash2, data2).await.unwrap();

        // Read them back
        let read_data1 = manager.read_packed(&hash1).await.unwrap();
        let read_data2 = manager.read_packed(&hash2).await.unwrap();

        assert!(read_data1.is_some());
        assert!(read_data2.is_some());
        assert_eq!(&read_data1.unwrap()[..], data1);
        assert_eq!(&read_data2.unwrap()[..], data2);

        // Check stats
        let stats = manager.stats();
        assert_eq!(stats.total_objects, 2);
        assert!(stats.total_packed_size > 0);
    }
}