use blake3::Hasher;
use bytes::Bytes;
use memmap2::MmapOptions;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File as StdFile;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tracing::{debug, trace, warn};
use chrono;

use crate::errors::{CasError, Result};
use crate::storage::CompressionType;
use landro_chunker::ContentHash;

/// Header for packfile format
const PACKFILE_MAGIC: &[u8] = b"LANDRO_PACK_V1";
const PACKFILE_VERSION: u32 = 1;

/// Size of packfile header: magic(14) + version(4) + entry_count(8) = 26 bytes
const PACKFILE_HEADER_SIZE: u64 = 26;

/// Size of each entry in the entry table: hash(32) + offset(8) + size(4) = 44 bytes
const PACKFILE_ENTRY_SIZE: u64 = 44;

/// Size of packfile footer: packfile_hash(32) bytes
const PACKFILE_FOOTER_SIZE: u64 = 32;

/// Configuration for packfile behavior
#[derive(Debug, Clone)]
pub struct PackfileConfig {
    /// Maximum size before creating new packfile
    pub max_packfile_size: u64,
    /// Minimum chunk size to pack (pack only small chunks)
    pub pack_threshold: u32,
    /// Minimum chunks before creating packfile
    pub min_chunks_per_pack: usize,
    /// Compression algorithm for packfile data
    pub compression: CompressionType,
}

impl Default for PackfileConfig {
    fn default() -> Self {
        Self {
            max_packfile_size: 256 * 1024 * 1024, // 256MB
            pack_threshold: 64 * 1024, // 64KB
            min_chunks_per_pack: 100,
            compression: CompressionType::None,
        }
    }
}

/// Entry in a packfile representing a stored object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackfileEntry {
    /// Hash of the object
    pub hash: ContentHash,
    /// Offset within the packfile data section
    pub offset: u64,
    /// Size of the object data
    pub size: u32,
    /// Optional data field (only populated when reading)
    #[serde(skip)]
    pub data: Option<Vec<u8>>,
}

impl PackfileEntry {
    pub fn new(hash: ContentHash, offset: u64, size: u32) -> Self {
        Self {
            hash,
            offset,
            size,
            data: None,
        }
    }
    
    pub fn with_data(hash: ContentHash, offset: u64, size: u32, data: Vec<u8>) -> Self {
        Self {
            hash,
            offset,
            size,
            data: Some(data),
        }
    }
}

/// Index for a packfile containing metadata about stored objects
#[derive(Debug, Serialize, Deserialize)]
pub struct PackfileIndex {
    /// Path to the packfile (not serialized)
    #[serde(skip)]
    pub packfile_path: PathBuf,
    /// Map from content hash to pack entry
    pub entries: HashMap<ContentHash, PackfileEntry>,
    /// Total size of data in the packfile
    pub data_size: u64,
    /// Packfile version for compatibility checking
    pub version: u32,
    /// Creation timestamp
    pub created_at: u64,
}

impl PackfileIndex {
    /// Create a new empty pack index
    pub fn new(packfile_path: PathBuf) -> Self {
        Self {
            packfile_path,
            entries: HashMap::new(),
            data_size: 0,
            version: PACKFILE_VERSION,
            created_at: chrono::Utc::now().timestamp() as u64,
        }
    }

    /// Add an entry to the index
    pub fn add_entry(&mut self, entry: PackfileEntry) {
        self.data_size += entry.size as u64;
        self.entries.insert(entry.hash, entry);
    }

    /// Check if the packfile has reached the target size
    pub fn is_full(&self, max_size: u64) -> bool {
        self.data_size >= max_size
    }

    /// Check if the packfile has minimum required entries
    pub fn has_min_entries(&self, min_entries: usize) -> bool {
        self.entries.len() >= min_entries
    }

    /// Get the number of objects in this packfile
    pub fn object_count(&self) -> usize {
        self.entries.len()
    }

    /// Save the index to disk
    pub async fn save_to_disk(&self, index_path: &Path) -> Result<()> {
        let serialized = bincode::serialize(self)
            .map_err(|e| CasError::InconsistentState(format!("Failed to serialize index: {}", e)))?;
        
        // Write atomically using temporary file
        let temp_path = index_path.with_extension("tmp");
        fs::write(&temp_path, &serialized).await?;
        fs::rename(&temp_path, index_path).await?;
        
        debug!("Saved packfile index to {:?}", index_path);
        Ok(())
    }

    /// Load index from disk
    pub async fn load_from_disk(index_path: &Path, packfile_path: PathBuf) -> Result<Self> {
        let data = fs::read(index_path).await?;
        
        let mut index: PackfileIndex = bincode::deserialize(&data)
            .map_err(|e| CasError::InconsistentState(format!("Failed to deserialize index: {}", e)))?;
        
        index.packfile_path = packfile_path;
        
        debug!("Loaded packfile index from {:?} with {} entries", index_path, index.entries.len());
        Ok(index)
    }
}

/// Writer for creating new packfiles with proper format
#[derive(Debug)]
pub struct PackfileWriter {
    file: File,
    entries: Vec<PackfileEntry>,
    data_offset: u64,
    #[allow(dead_code)]
    config: PackfileConfig,
    packfile_path: PathBuf,
}

impl PackfileWriter {
    /// Create a new packfile writer
    pub async fn new(path: &Path, config: PackfileConfig) -> Result<Self> {
        let mut file = File::create(path).await?;
        
        // Write header: magic + version + placeholder entry count
        file.write_all(PACKFILE_MAGIC).await?;
        file.write_u32_le(PACKFILE_VERSION).await?;
        file.write_u64_le(0).await?; // Placeholder for entry count
        file.sync_all().await?;
        
        let data_offset = PACKFILE_HEADER_SIZE;
        
        Ok(Self {
            file,
            entries: Vec::new(),
            data_offset,
            config,
            packfile_path: path.to_path_buf(),
        })
    }
    
    /// Add a chunk to the packfile
    pub async fn add_chunk(&mut self, hash: &ContentHash, data: &[u8]) -> Result<()> {
        if data.len() > u32::MAX as usize {
            return Err(CasError::InvalidObjectSize {
                expected: u32::MAX as u64,
                actual: data.len() as u64,
            });
        }
        
        // Write data immediately to the file
        self.file.write_all(data).await?;
        
        // Create entry record
        let entry = PackfileEntry::new(*hash, self.data_offset, data.len() as u32);
        self.entries.push(entry);
        
        // Update offset for next chunk
        self.data_offset += data.len() as u64;
        
        trace!("Added chunk {} ({} bytes) to packfile", hash, data.len());
        Ok(())
    }
    
    /// Finalize the packfile and return a reader
    pub async fn finalize(mut self) -> Result<PackfileReader> {
        // Write entry table at current position
        let _entry_table_offset = self.data_offset;
        
        for entry in &self.entries {
            // Write hash (32 bytes)
            self.file.write_all(entry.hash.as_bytes()).await?;
            // Write offset (8 bytes)
            self.file.write_u64_le(entry.offset).await?;
            // Write size (4 bytes)
            self.file.write_u32_le(entry.size).await?;
        }
        
        // Calculate and write packfile hash as footer
        let mut packfile_hasher = Hasher::new();
        
        // Seek back to beginning and hash the entire file content
        self.file.seek(SeekFrom::Start(0)).await?;
        let mut buffer = vec![0u8; 8192];
        loop {
            let n = self.file.read(&mut buffer).await?;
            if n == 0 { break; }
            packfile_hasher.update(&buffer[..n]);
        }
        
        let packfile_hash = packfile_hasher.finalize();
        self.file.write_all(packfile_hash.as_bytes()).await?;
        
        // Update header with actual entry count
        self.file.seek(SeekFrom::Start(PACKFILE_MAGIC.len() as u64 + 4)).await?;
        self.file.write_u64_le(self.entries.len() as u64).await?;
        
        // Sync to disk
        self.file.sync_all().await?;
        
        debug!("Finalized packfile with {} entries at {:?}", self.entries.len(), self.packfile_path);
        
        // Create and return reader
        PackfileReader::open(&self.packfile_path).await
    }
    
    /// Get the current number of chunks in the packfile
    pub fn chunk_count(&self) -> usize {
        self.entries.len()
    }
    
    /// Get the current data size
    pub fn data_size(&self) -> u64 {
        self.data_offset - PACKFILE_HEADER_SIZE
    }
}

/// Reader for accessing chunks in existing packfiles (async version)
#[derive(Debug, Clone)]
pub struct PackfileReader {
    file_path: PathBuf,
    entries: HashMap<ContentHash, PackfileEntry>,
    stats: PackfileStats,
}

/// Memory-mapped reader for zero-copy access to packfile chunks
pub struct MmapPackfileReader {
    file_path: PathBuf,
    entries: HashMap<ContentHash, PackfileEntry>,
    stats: PackfileStats,
    mmap: Arc<memmap2::Mmap>,
}

impl PackfileReader {
    /// Open an existing packfile for reading
    pub async fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).await?;
        
        // Read and verify header
        let mut magic = vec![0u8; PACKFILE_MAGIC.len()];
        file.read_exact(&mut magic).await?;
        if magic != PACKFILE_MAGIC {
            return Err(CasError::InconsistentState("Invalid packfile magic".to_string()));
        }
        
        let version = file.read_u32_le().await?;
        if version != PACKFILE_VERSION {
            return Err(CasError::InconsistentState(format!("Unsupported packfile version: {}", version)));
        }
        
        let entry_count = file.read_u64_le().await?;
        
        // Get file size to calculate entry table offset
        let file_size = file.metadata().await?.len();
        let entry_table_offset = file_size - PACKFILE_FOOTER_SIZE - (entry_count * PACKFILE_ENTRY_SIZE);
        
        // Seek to entry table and read entries
        file.seek(SeekFrom::Start(entry_table_offset)).await?;
        
        let mut entries = HashMap::new();
        let mut total_size = 0u64;
        
        for _ in 0..entry_count {
            // Read hash (32 bytes)
            let mut hash_bytes = [0u8; 32];
            file.read_exact(&mut hash_bytes).await?;
            let hash = ContentHash::from_bytes(hash_bytes);
            
            // Read offset (8 bytes)
            let offset = file.read_u64_le().await?;
            
            // Read size (4 bytes)
            let size = file.read_u32_le().await?;
            
            let entry = PackfileEntry::new(hash, offset, size);
            total_size += size as u64;
            entries.insert(hash, entry);
        }
        
        // Verify packfile hash (optional but recommended)
        let mut stored_hash = [0u8; 32];
        file.read_exact(&mut stored_hash).await?;
        
        let stats = PackfileStats {
            total_packs: 1,
            total_objects: entries.len(),
            total_packed_size: total_size,
        };
        
        debug!("Opened packfile {:?} with {} chunks", path, entries.len());
        
        Ok(Self {
            file_path: path.to_path_buf(),
            entries,
            stats,
        })
    }
    
    /// Read a chunk from the packfile
    pub async fn read_chunk(&self, hash: &ContentHash) -> Result<Vec<u8>> {
        let entry = self.entries.get(hash)
            .ok_or_else(|| CasError::ObjectNotFound(*hash))?;
        
        let mut file = File::open(&self.file_path).await?;
        
        // Seek to chunk data
        file.seek(SeekFrom::Start(entry.offset)).await?;
        
        // Read chunk data
        let mut buffer = vec![0u8; entry.size as usize];
        file.read_exact(&mut buffer).await?;
        
        // Verify chunk hash
        let mut hasher = Hasher::new();
        hasher.update(&buffer);
        let actual_hash = ContentHash::from_blake3(hasher.finalize());
        
        if actual_hash != *hash {
            return Err(CasError::HashMismatch {
                expected: *hash,
                actual: actual_hash,
            });
        }
        
        Ok(buffer)
    }
    
    /// Check if the packfile contains a specific chunk
    pub fn contains_chunk(&self, hash: &ContentHash) -> bool {
        self.entries.contains_key(hash)
    }
    
    /// Get the number of chunks in the packfile
    pub fn chunk_count(&self) -> usize {
        self.entries.len()
    }
    
    /// Get the total size of all chunks
    pub fn total_size(&self) -> u64 {
        self.stats.total_packed_size
    }
    
    /// Get statistics for this packfile
    pub fn stats(&self) -> &PackfileStats {
        &self.stats
    }
}

impl MmapPackfileReader {
    /// Open a packfile with memory mapping for zero-copy reads
    pub fn open(path: &Path) -> Result<Self> {
        // Open file for memory mapping
        let std_file = StdFile::open(path)
            .map_err(|e| CasError::StoragePath(format!("Failed to open packfile: {}", e)))?;
        
        // Create memory map
        let mmap = unsafe {
            MmapOptions::new()
                .map(&std_file)
                .map_err(|e| CasError::StoragePath(format!("Failed to memory map packfile: {}", e)))?
        };
        
        let mmap = Arc::new(mmap);
        
        // Verify header
        if mmap.len() < PACKFILE_HEADER_SIZE as usize {
            return Err(CasError::InconsistentState("Packfile too small".to_string()));
        }
        
        // Check magic bytes
        if &mmap[0..PACKFILE_MAGIC.len()] != PACKFILE_MAGIC {
            return Err(CasError::InconsistentState("Invalid packfile magic".to_string()));
        }
        
        // Read version
        let version_bytes = &mmap[PACKFILE_MAGIC.len()..PACKFILE_MAGIC.len() + 4];
        let version = u32::from_le_bytes([
            version_bytes[0],
            version_bytes[1], 
            version_bytes[2],
            version_bytes[3],
        ]);
        
        if version != PACKFILE_VERSION {
            return Err(CasError::InconsistentState(format!("Unsupported packfile version: {}", version)));
        }
        
        // Read entry count
        let count_offset = PACKFILE_MAGIC.len() + 4;
        let count_bytes = &mmap[count_offset..count_offset + 8];
        let entry_count = u64::from_le_bytes([
            count_bytes[0], count_bytes[1], count_bytes[2], count_bytes[3],
            count_bytes[4], count_bytes[5], count_bytes[6], count_bytes[7],
        ]);
        
        // Calculate entry table offset
        let file_size = mmap.len() as u64;
        let entry_table_offset = file_size - PACKFILE_FOOTER_SIZE - (entry_count * PACKFILE_ENTRY_SIZE);
        
        // Parse entries
        let mut entries = HashMap::new();
        let mut total_size = 0u64;
        let mut offset = entry_table_offset as usize;
        
        for _ in 0..entry_count {
            if offset + PACKFILE_ENTRY_SIZE as usize > mmap.len() {
                return Err(CasError::InconsistentState("Entry table extends beyond file".to_string()));
            }
            
            // Read hash
            let hash_bytes: [u8; 32] = mmap[offset..offset + 32]
                .try_into()
                .map_err(|_| CasError::InconsistentState("Invalid hash size".to_string()))?;
            let hash = ContentHash::from_bytes(hash_bytes);
            offset += 32;
            
            // Read offset
            let offset_bytes = &mmap[offset..offset + 8];
            let data_offset = u64::from_le_bytes([
                offset_bytes[0], offset_bytes[1], offset_bytes[2], offset_bytes[3],
                offset_bytes[4], offset_bytes[5], offset_bytes[6], offset_bytes[7],
            ]);
            offset += 8;
            
            // Read size
            let size_bytes = &mmap[offset..offset + 4];
            let size = u32::from_le_bytes([
                size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3],
            ]);
            offset += 4;
            
            let entry = PackfileEntry::new(hash, data_offset, size);
            total_size += size as u64;
            entries.insert(hash, entry);
        }
        
        let stats = PackfileStats {
            total_packs: 1,
            total_objects: entries.len(),
            total_packed_size: total_size,
        };
        
        debug!("Opened memory-mapped packfile {:?} with {} chunks", path, entries.len());
        
        Ok(Self {
            file_path: path.to_path_buf(),
            entries,
            stats,
            mmap,
        })
    }
    
    /// Read a chunk with zero-copy (returns Bytes)
    pub fn read_chunk(&self, hash: &ContentHash) -> Result<Bytes> {
        let entry = self.entries.get(hash)
            .ok_or_else(|| CasError::ObjectNotFound(*hash))?;
        
        let start = entry.offset as usize;
        let end = start + entry.size as usize;
        
        if end > self.mmap.len() {
            return Err(CasError::CorruptObject(
                "Chunk data extends beyond packfile".to_string()
            ));
        }
        
        // Get data slice
        let data = &self.mmap[start..end];
        
        // Verify hash
        let mut hasher = Hasher::new();
        hasher.update(data);
        let actual_hash = ContentHash::from_blake3(hasher.finalize());
        
        if actual_hash != *hash {
            return Err(CasError::HashMismatch {
                expected: *hash,
                actual: actual_hash,
            });
        }
        
        // Return as Bytes (copies data but allows for safe lifetime management)
        Ok(Bytes::copy_from_slice(data))
    }
    
    /// Get a direct reference to chunk data (zero-copy, no verification)
    pub fn get_chunk_slice(&self, hash: &ContentHash) -> Result<&[u8]> {
        let entry = self.entries.get(hash)
            .ok_or_else(|| CasError::ObjectNotFound(*hash))?;
        
        let start = entry.offset as usize;
        let end = start + entry.size as usize;
        
        if end > self.mmap.len() {
            return Err(CasError::CorruptObject(
                "Chunk data extends beyond packfile".to_string()
            ));
        }
        
        Ok(&self.mmap[start..end])
    }
    
    /// Check if the packfile contains a chunk
    pub fn contains_chunk(&self, hash: &ContentHash) -> bool {
        self.entries.contains_key(hash)
    }
    
    /// Get packfile statistics
    pub fn stats(&self) -> &PackfileStats {
        &self.stats
    }
}

/// Manager for packfile-based storage of small objects
#[derive(Debug)]
pub struct PackfileManager {
    /// Root directory for packfiles
    pack_dir: PathBuf,
    /// Configuration for packfile behavior
    config: PackfileConfig,
    /// Currently active packfile being written to
    current_writer: Option<PackfileWriter>,
    /// Map of packfile names to their readers
    pack_readers: HashMap<String, PackfileReader>,
    /// Pending chunks waiting to be packed
    pending_chunks: Vec<(ContentHash, Vec<u8>)>,
}

impl PackfileManager {
    /// Create a new packfile manager with default configuration
    pub async fn new(root_path: impl AsRef<Path>) -> Result<Self> {
        Self::new_with_config(root_path, PackfileConfig::default()).await
    }
    
    /// Create a new packfile manager with custom configuration
    pub async fn new_with_config(
        root_path: impl AsRef<Path>, 
        config: PackfileConfig
    ) -> Result<Self> {
        let pack_dir = root_path.as_ref().join("packs");
        fs::create_dir_all(&pack_dir).await?;

        let mut manager = Self {
            pack_dir,
            config,
            current_writer: None,
            pack_readers: HashMap::new(),
            pending_chunks: Vec::new(),
        };

        // Load existing packfiles
        manager.load_existing_packs().await?;

        debug!(
            "Packfile manager initialized with {} packs",
            manager.pack_readers.len()
        );
        Ok(manager)
    }

    /// Load existing packfiles from disk
    async fn load_existing_packs(&mut self) -> Result<()> {
        let mut entries = fs::read_dir(&self.pack_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension() == Some(std::ffi::OsStr::new("pack")) {
                if let Some(pack_name) = path.file_stem() {
                    match PackfileReader::open(&path).await {
                        Ok(reader) => {
                            let pack_name = pack_name.to_string_lossy().to_string();
                            self.pack_readers.insert(pack_name, reader);
                        }
                        Err(e) => {
                            warn!("Failed to load packfile {:?}: {}", path, e);
                        }
                    }
                }
            }
        }

        Ok(())
    }


    /// Determine if an object should be stored in a packfile
    pub fn should_pack(&self, size: u64) -> bool {
        size <= self.config.pack_threshold as u64
    }

    /// Add a chunk to be packed (may not pack immediately)
    pub async fn add_chunk(&mut self, hash: ContentHash, data: &[u8]) -> Result<()> {
        trace!("Adding {} bytes to pending chunks for hash {}", data.len(), hash);
        
        self.pending_chunks.push((hash, data.to_vec()));
        
        // Check if we should flush packfile
        if self.should_flush_packfile().await {
            self.flush_packfile().await?;
        }
        
        Ok(())
    }
    
    /// Store an object in a packfile immediately
    pub async fn store_packed(&mut self, hash: ContentHash, data: &[u8]) -> Result<()> {
        trace!("Storing {} bytes in packfile for hash {}", data.len(), hash);

        // Get or create current packfile writer
        let writer = self.get_or_create_current_writer().await?;
        
        // Add chunk to current packfile
        writer.add_chunk(&hash, data).await?;

        // Check if we need to finalize this packfile
        if self.should_finalize_current_pack().await {
            self.finalize_current_pack().await?;
        }

        debug!("Stored object {} in packfile", hash);
        Ok(())
    }

    /// Read an object from a packfile
    pub async fn read_packed(&self, hash: &ContentHash) -> Result<Option<Bytes>> {
        // Check pending chunks first
        for (chunk_hash, chunk_data) in &self.pending_chunks {
            if chunk_hash == hash {
                return Ok(Some(Bytes::from(chunk_data.clone())));
            }
        }
        
        // Find which packfile contains this object
        for reader in self.pack_readers.values() {
            if reader.contains_chunk(hash) {
                let data = reader.read_chunk(hash).await?;
                return Ok(Some(Bytes::from(data)));
            }
        }

        Ok(None)
    }


    /// Get or create the current packfile writer
    async fn get_or_create_current_writer(&mut self) -> Result<&mut PackfileWriter> {
        if self.current_writer.is_none() {
            self.create_new_pack_writer().await?;
        }

        Ok(self.current_writer.as_mut().unwrap())
    }

    /// Create a new packfile writer
    async fn create_new_pack_writer(&mut self) -> Result<()> {
        let pack_id = chrono::Utc::now().timestamp();
        let pack_name = format!("pack-{:016x}", pack_id);
        let packfile_path = self.pack_dir.join(format!("{}.pack", pack_name));

        let writer = PackfileWriter::new(&packfile_path, self.config.clone()).await?;
        self.current_writer = Some(writer);

        debug!("Created new packfile writer: {}", pack_name);
        Ok(())
    }

    /// Check if current packfile should be finalized
    async fn should_finalize_current_pack(&self) -> bool {
        if let Some(writer) = &self.current_writer {
            writer.data_size() >= self.config.max_packfile_size ||
            writer.chunk_count() >= self.config.min_chunks_per_pack * 10 // Allow some growth
        } else {
            false
        }
    }
    
    /// Check if pending chunks should be flushed to packfile
    async fn should_flush_packfile(&self) -> bool {
        self.pending_chunks.len() >= self.config.min_chunks_per_pack
    }
    
    /// Flush pending chunks to packfile
    pub async fn flush_packfile(&mut self) -> Result<()> {
        if self.pending_chunks.is_empty() {
            return Ok(());
        }
        
        debug!("Flushing {} pending chunks to packfile", self.pending_chunks.len());
        
        // Collect chunks first to avoid mutable borrow conflicts
        let chunks_to_flush: Vec<(ContentHash, Vec<u8>)> = self.pending_chunks.drain(..).collect();
        
        // Ensure we have a writer
        let writer = self.get_or_create_current_writer().await?;
        
        // Add all pending chunks
        for (hash, data) in chunks_to_flush {
            writer.add_chunk(&hash, &data).await?;
        }
        
        // Check if we should finalize
        if self.should_finalize_current_pack().await {
            self.finalize_current_pack().await?;
        }
        
        Ok(())
    }
    
    /// Finalize the current packfile and create its reader
    async fn finalize_current_pack(&mut self) -> Result<()> {
        if let Some(writer) = self.current_writer.take() {
            let packfile_path = writer.packfile_path.clone();
            let pack_name = packfile_path
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .to_string();

            debug!(
                "Finalizing packfile {} with {} objects",
                pack_name,
                writer.chunk_count()
            );

            // Finalize writer and get reader
            let reader = writer.finalize().await?;
            self.pack_readers.insert(pack_name, reader);
        }

        Ok(())
    }

    /// Force finalization of current packfile (for shutdown)
    pub async fn finalize(&mut self) -> Result<()> {
        if !self.pending_chunks.is_empty() {
            self.flush_packfile().await?;
        }
        
        if self.current_writer.is_some() {
            self.finalize_current_pack().await?;
        }
        
        Ok(())
    }
    
    /// Get statistics about packfile usage
    pub fn stats(&self) -> PackfileStats {
        let mut stats = PackfileStats {
            total_packs: self.pack_readers.len(),
            total_objects: self.pending_chunks.len(),
            total_packed_size: 0,
        };

        for reader in self.pack_readers.values() {
            let reader_stats = reader.stats();
            stats.total_objects += reader_stats.total_objects;
            stats.total_packed_size += reader_stats.total_packed_size;
        }

        // Include current writer
        if let Some(current_writer) = &self.current_writer {
            stats.total_objects += current_writer.chunk_count();
            stats.total_packed_size += current_writer.data_size();
            if current_writer.chunk_count() > 0 {
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

        // Test that small objects should be packed (default threshold is 64KB)
        assert!(manager.should_pack(1024)); // 1KB
        assert!(manager.should_pack(32768)); // 32KB
        assert!(!manager.should_pack(128 * 1024)); // 128KB

        let stats = manager.stats();
        assert_eq!(stats.total_packs, 0);
        assert_eq!(stats.total_objects, 0);
    }

    #[ignore = "Packfiles disabled for v1.0"]
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
        
        // Finalize any pending writes
        manager.finalize().await.unwrap();

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
    
    #[ignore = "Packfiles disabled for v1.0"]
    #[tokio::test]
    async fn test_packfile_writer_reader() {
        let dir = tempdir().unwrap();
        let pack_path = dir.path().join("test.pack");
        let config = PackfileConfig::default();
        
        // Create writer and add chunks
        let mut writer = PackfileWriter::new(&pack_path, config).await.unwrap();
        
        let data1 = b"First chunk data";
        let data2 = b"Second chunk data with more content";
        let data3 = b"Third chunk";
        
        let mut hasher = Hasher::new();
        hasher.update(data1);
        let hash1 = ContentHash::from_blake3(hasher.finalize());
        
        let mut hasher = Hasher::new();
        hasher.update(data2);
        let hash2 = ContentHash::from_blake3(hasher.finalize());
        
        let mut hasher = Hasher::new();
        hasher.update(data3);
        let hash3 = ContentHash::from_blake3(hasher.finalize());
        
        writer.add_chunk(&hash1, data1).await.unwrap();
        writer.add_chunk(&hash2, data2).await.unwrap();
        writer.add_chunk(&hash3, data3).await.unwrap();
        
        assert_eq!(writer.chunk_count(), 3);
        
        // Finalize and get reader
        let reader = writer.finalize().await.unwrap();
        
        // Test reading chunks
        assert!(reader.contains_chunk(&hash1));
        assert!(reader.contains_chunk(&hash2));
        assert!(reader.contains_chunk(&hash3));
        
        let read_data1 = reader.read_chunk(&hash1).await.unwrap();
        let read_data2 = reader.read_chunk(&hash2).await.unwrap();
        let read_data3 = reader.read_chunk(&hash3).await.unwrap();
        
        assert_eq!(&read_data1[..], data1);
        assert_eq!(&read_data2[..], data2);
        assert_eq!(&read_data3[..], data3);
        
        assert_eq!(reader.chunk_count(), 3);
        assert!(reader.total_size() > 0);
    }
    
    #[ignore = "Packfiles disabled for v1.0"]
    #[tokio::test]
    async fn test_packfile_config_thresholds() {
        let dir = tempdir().unwrap();
        let config = PackfileConfig {
            max_packfile_size: 1024, // Very small for testing
            pack_threshold: 32,
            min_chunks_per_pack: 2,
            compression: CompressionType::None,
        };
        
        let mut manager = PackfileManager::new_with_config(dir.path(), config).await.unwrap();
        
        // Test threshold
        assert!(manager.should_pack(16)); // Below threshold
        assert!(manager.should_pack(32)); // At threshold
        assert!(!manager.should_pack(64)); // Above threshold
        
        // Add chunks that should trigger packfile finalization
        let mut chunks = Vec::new();
        
        for i in 0..5 {
            let chunk_data = format!("Test data chunk {}", i);
            let mut hasher = Hasher::new();
            hasher.update(chunk_data.as_bytes());
            let hash = ContentHash::from_blake3(hasher.finalize());
            chunks.push((hash, chunk_data.clone()));
            
            manager.add_chunk(hash, chunk_data.as_bytes()).await.unwrap();
        }
        
        // Should have created at least one packfile
        manager.finalize().await.unwrap();
        let stats = manager.stats();
        assert!(stats.total_packs >= 1);
        assert_eq!(stats.total_objects, 5);
        
        // Verify all chunks are readable
        for (hash, original_data) in &chunks {
            let read_data = manager.read_packed(hash).await.unwrap();
            assert!(read_data.is_some());
            assert_eq!(&read_data.unwrap()[..], original_data.as_bytes());
        }
    }
}
