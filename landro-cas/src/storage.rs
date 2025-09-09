use blake3::Hasher;
use bytes::Bytes;
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::{self, File};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tracing::{debug, error, trace, warn};

use crate::errors::{CasError, Result};
use crate::packfile::{PackfileConfig, PackfileManager};
use crate::validation;
use landro_chunker::ContentHash;

/// Partial transfer state for resumable operations
#[derive(Debug, Clone)]
pub struct PartialTransfer {
    /// Temporary file path
    pub temp_path: PathBuf,
    /// Expected hash if known
    pub expected_hash: Option<ContentHash>,
    /// Expected total size if known
    pub total_size: Option<u64>,
    /// Bytes already received
    pub bytes_received: u64,
    /// Serialized hash state for resumption
    pub hash_state: Option<String>,
    /// When this partial transfer expires
    pub expires_at: std::time::SystemTime,
}

impl PartialTransfer {
    /// Create a new partial transfer
    pub fn new(
        temp_path: PathBuf,
        expected_hash: Option<ContentHash>,
        total_size: Option<u64>,
    ) -> Self {
        let expires_at = std::time::SystemTime::now() + std::time::Duration::from_secs(24 * 3600); // 24 hours
        Self {
            temp_path,
            expected_hash,
            total_size,
            bytes_received: 0,
            hash_state: None,
            expires_at,
        }
    }

    /// Check if this partial transfer has expired
    pub fn is_expired(&self) -> bool {
        std::time::SystemTime::now() > self.expires_at
    }
}

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
#[derive(Debug, Clone, Copy)]
pub enum CompressionType {
    /// No compression
    None,
    /// LZ4 compression (fast)
    Lz4,
    /// Zstd compression (balanced)
    Zstd { level: i32 },
    /// Snappy compression (very fast)
    Snappy,
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
    /// Configuration for packfile behavior
    pub packfile_config: PackfileConfig,
    /// Enable packfile storage for small objects
    pub enable_packfiles: bool,
    /// Cache configuration
    pub cache_config: CacheConfig,
    /// Enable batch operations
    pub enable_batch_ops: bool,
    /// Maximum concurrent operations
    pub max_concurrent_ops: usize,
    /// Enable background integrity scanning
    pub enable_background_scan: bool,
    /// Background scan interval in seconds
    pub scan_interval_secs: u64,
}

/// Cache configuration for the content store
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Maximum number of cached chunks
    pub max_chunks: usize,
    /// Maximum total size of cached data (bytes)
    pub max_size_bytes: u64,
    /// Cache TTL in seconds
    pub ttl_seconds: u64,
    /// Enable adaptive caching based on access patterns
    pub adaptive_caching: bool,
    /// Prefetch related chunks
    pub prefetch_enabled: bool,
}

impl Default for ContentStoreConfig {
    fn default() -> Self {
        Self {
            fsync_policy: FsyncPolicy::Always,
            enable_recovery: true,
            compression: CompressionType::None,
            packfile_config: PackfileConfig::default(),
            enable_packfiles: false, // Disabled by default for v1.0 - focusing on individual file storage
            cache_config: CacheConfig::default(),
            enable_batch_ops: true,
            max_concurrent_ops: 32,
            enable_background_scan: false,
            scan_interval_secs: 3600, // 1 hour
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_chunks: 10000,
            max_size_bytes: 512 * 1024 * 1024, // 512MB
            ttl_seconds: 300,                  // 5 minutes
            adaptive_caching: true,
            prefetch_enabled: true,
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
/// Cached chunk data with metadata
#[derive(Clone)]
struct CachedChunk {
    data: Bytes,
    size: u64,
    last_accessed: Instant,
    access_count: u32,
}

/// Chunk cache for hot data
struct ChunkCache {
    chunks: LruCache<ContentHash, CachedChunk>,
    total_size: u64,
    hit_count: AtomicU64,
    miss_count: AtomicU64,
    eviction_count: AtomicU64,
}

impl ChunkCache {
    fn new(max_chunks: usize) -> Self {
        Self {
            chunks: LruCache::new(NonZeroUsize::new(max_chunks).unwrap()),
            total_size: 0,
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
            eviction_count: AtomicU64::new(0),
        }
    }

    fn hit_rate(&self) -> f64 {
        let hits = self.hit_count.load(Ordering::Relaxed) as f64;
        let misses = self.miss_count.load(Ordering::Relaxed) as f64;
        if hits + misses == 0.0 {
            0.0
        } else {
            hits / (hits + misses)
        }
    }
}

/// Batch write accumulator
struct BatchWriter {
    pending: Vec<(ContentHash, Bytes)>,
    total_size: usize,
    last_flush: Instant,
}

impl BatchWriter {
    fn new() -> Self {
        Self {
            pending: Vec::with_capacity(100),
            total_size: 0,
            last_flush: Instant::now(),
        }
    }

    fn should_flush(&self) -> bool {
        self.pending.len() >= 100 ||
        self.total_size >= 10 * 1024 * 1024 || // 10MB
        self.last_flush.elapsed() >= Duration::from_millis(100)
    }
}

pub struct ContentStore {
    pub(crate) root_path: PathBuf,
    config: ContentStoreConfig,
    pending_syncs: Arc<AtomicU32>,
    packfile_manager: Option<Arc<Mutex<PackfileManager>>>,
    cache: Arc<RwLock<ChunkCache>>,
    batch_writer: Arc<Mutex<BatchWriter>>,
    write_semaphore: Arc<Semaphore>,
    dedup_index: Arc<RwLock<HashMap<ContentHash, ObjectRef>>>,
    stats: ContentStoreRuntimeStats,
}

impl std::fmt::Debug for ContentStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Security-conscious debug implementation
        // Only expose safe, non-sensitive operational information
        f.debug_struct("ContentStore")
            .field(
                "storage_root",
                &format!(
                    "{}/**",
                    self.root_path
                        .file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or("unknown".into())
                ),
            )
            .field("fsync_policy", &self.config.fsync_policy)
            .field("compression", &self.config.compression)
            .field("packfiles_enabled", &self.config.enable_packfiles)
            .field("batch_ops_enabled", &self.config.enable_batch_ops)
            .field("max_concurrent_ops", &self.config.max_concurrent_ops)
            .field("pending_syncs", &self.pending_syncs.load(Ordering::Relaxed))
            .field(
                "background_scan_enabled",
                &self.config.enable_background_scan,
            )
            // Expose safe runtime statistics without sensitive data
            .field("total_writes", &self.stats.writes.load(Ordering::Relaxed))
            .field("total_reads", &self.stats.reads.load(Ordering::Relaxed))
            .field("cache_hits", &self.stats.cache_hits.load(Ordering::Relaxed))
            .field(
                "cache_misses",
                &self.stats.cache_misses.load(Ordering::Relaxed),
            )
            .field("dedup_hits", &self.stats.dedup_hits.load(Ordering::Relaxed))
            .field(
                "batch_flushes",
                &self.stats.batch_flushes.load(Ordering::Relaxed),
            )
            .finish()
    }
}

/// Runtime statistics for the content store
#[derive(Debug)]
pub struct ContentStoreRuntimeStats {
    writes: AtomicU64,
    reads: AtomicU64,
    bytes_written: AtomicU64,
    bytes_read: AtomicU64,
    objects_packed: AtomicU64,
    verifications: AtomicU64,
    corruptions_found: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    batch_flushes: AtomicU64,
    dedup_hits: AtomicU64,
}

impl Default for ContentStoreRuntimeStats {
    fn default() -> Self {
        Self {
            writes: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            objects_packed: AtomicU64::new(0),
            verifications: AtomicU64::new(0),
            corruptions_found: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            batch_flushes: AtomicU64::new(0),
            dedup_hits: AtomicU64::new(0),
        }
    }
}

impl Clone for ContentStoreRuntimeStats {
    fn clone(&self) -> Self {
        Self {
            writes: AtomicU64::new(self.writes.load(Ordering::Relaxed)),
            reads: AtomicU64::new(self.reads.load(Ordering::Relaxed)),
            bytes_written: AtomicU64::new(self.bytes_written.load(Ordering::Relaxed)),
            bytes_read: AtomicU64::new(self.bytes_read.load(Ordering::Relaxed)),
            objects_packed: AtomicU64::new(self.objects_packed.load(Ordering::Relaxed)),
            verifications: AtomicU64::new(self.verifications.load(Ordering::Relaxed)),
            corruptions_found: AtomicU64::new(self.corruptions_found.load(Ordering::Relaxed)),
            cache_hits: AtomicU64::new(self.cache_hits.load(Ordering::Relaxed)),
            cache_misses: AtomicU64::new(self.cache_misses.load(Ordering::Relaxed)),
            batch_flushes: AtomicU64::new(self.batch_flushes.load(Ordering::Relaxed)),
            dedup_hits: AtomicU64::new(self.dedup_hits.load(Ordering::Relaxed)),
        }
    }
}

impl Clone for ContentStore {
    fn clone(&self) -> Self {
        Self {
            root_path: self.root_path.clone(),
            config: self.config.clone(),
            pending_syncs: Arc::clone(&self.pending_syncs),
            packfile_manager: self.packfile_manager.clone(),
            cache: Arc::clone(&self.cache),
            batch_writer: Arc::clone(&self.batch_writer),
            write_semaphore: Arc::clone(&self.write_semaphore),
            dedup_index: Arc::clone(&self.dedup_index),
            stats: self.stats.clone(),
        }
    }
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
    pub async fn new_with_config(
        root_path: impl AsRef<Path>,
        config: ContentStoreConfig,
    ) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();

        // Ensure root directory exists
        fs::create_dir_all(&root_path).await?;

        // Create objects directory
        let objects_dir = root_path.join("objects");
        fs::create_dir_all(&objects_dir).await?;

        debug!(
            "Content store initialized at {:?} with config {:?}",
            root_path, config
        );

        // Initialize packfile manager if enabled
        let packfile_manager = if config.enable_packfiles {
            Some(Arc::new(Mutex::new(
                PackfileManager::new_with_config(&root_path, config.packfile_config.clone())
                    .await?,
            )))
        } else {
            None
        };

        let store = Self {
            root_path,
            config: config.clone(),
            pending_syncs: Arc::new(AtomicU32::new(0)),
            packfile_manager,
            cache: Arc::new(RwLock::new(ChunkCache::new(config.cache_config.max_chunks))),
            batch_writer: Arc::new(Mutex::new(BatchWriter::new())),
            write_semaphore: Arc::new(Semaphore::new(config.max_concurrent_ops)),
            dedup_index: Arc::new(RwLock::new(HashMap::new())),
            stats: ContentStoreRuntimeStats::default(),
        };

        // Perform recovery if enabled
        if config.enable_recovery {
            let stats = store.recover().await?;
            if stats.recovered > 0 || stats.cleaned > 0 {
                debug!(
                    "Recovery completed: recovered {}, cleaned {}, errors: {:?}",
                    stats.recovered, stats.cleaned, stats.errors
                );
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

    /// Write an object to the content store using Bytes for zero-copy optimization.
    ///
    /// The object is written atomically using a temporary file and rename for large objects,
    /// or stored in packfiles for small objects. If an object with the same hash already
    /// exists, this is a no-op. Fsync behavior is controlled by the store's configuration.
    ///
    /// # Arguments
    ///
    /// * `data` - The data to store as Bytes
    ///
    /// # Returns
    ///
    /// An ObjectRef containing the hash and size of the stored data
    pub async fn write_bytes(&self, data: Bytes) -> Result<ObjectRef> {
        self.write(&data[..]).await
    }

    /// Write multiple objects in a batch for improved performance
    pub async fn write_batch(
        &self,
        chunks: Vec<(&[u8], Option<ContentHash>)>,
    ) -> Result<Vec<ObjectRef>> {
        let mut results = Vec::with_capacity(chunks.len());
        let mut batch = self.batch_writer.lock().await;

        for (data, expected_hash) in chunks {
            // Validate object size
            validation::validate_object_size(data.len()).map_err(|e| {
                CasError::InvalidOperation(format!("Object validation failed: {}", e))
            })?;

            // Calculate hash or use provided one
            let hash = if let Some(h) = expected_hash {
                h
            } else {
                let mut hasher = Hasher::new();
                hasher.update(data);
                ContentHash::from_blake3(hasher.finalize())
            };

            // Check dedup index first
            {
                let dedup = self.dedup_index.read().await;
                if let Some(obj_ref) = dedup.get(&hash) {
                    self.stats.dedup_hits.fetch_add(1, Ordering::Relaxed);
                    results.push(obj_ref.clone());
                    continue;
                }
            }

            // Add to batch
            batch.pending.push((hash, Bytes::from(data.to_vec())));
            batch.total_size += data.len();

            results.push(ObjectRef {
                hash,
                size: data.len() as u64,
            });
        }

        // Flush if needed
        if batch.should_flush() {
            self.flush_batch_internal(&mut batch).await?;
        }

        Ok(results)
    }

    /// Flush pending batch writes
    pub async fn flush_batch(&self) -> Result<()> {
        let mut batch = self.batch_writer.lock().await;
        if !batch.pending.is_empty() {
            self.flush_batch_internal(&mut batch).await?;
        }
        Ok(())
    }

    async fn flush_batch_internal(&self, batch: &mut BatchWriter) -> Result<()> {
        if batch.pending.is_empty() {
            return Ok(());
        }

        debug!(
            "Flushing batch of {} chunks ({} bytes)",
            batch.pending.len(),
            batch.total_size
        );
        self.stats.batch_flushes.fetch_add(1, Ordering::Relaxed);

        // Process all pending writes concurrently with semaphore limiting
        let mut handles = Vec::new();
        for (hash, data) in batch.pending.drain(..) {
            let store = self.clone();
            let permit = self.write_semaphore.clone().acquire_owned().await;

            let handle = tokio::spawn(async move {
                let _permit = permit;
                store.write_individual_optimized(&data, hash).await
            });
            handles.push(handle);
        }

        // Wait for all writes to complete
        for handle in handles {
            handle
                .await
                .map_err(|e| CasError::Internal(format!("Batch write failed: {}", e)))??;
        }

        batch.total_size = 0;
        batch.last_flush = Instant::now();

        Ok(())
    }

    /// Optimized individual write with compression support
    async fn write_individual_optimized(
        &self,
        data: &[u8],
        hash: ContentHash,
    ) -> Result<ObjectRef> {
        let object_path = self.object_path(&hash);

        // Check if already exists
        if object_path.exists() {
            return Ok(ObjectRef {
                hash,
                size: data.len() as u64,
            });
        }

        // Compress if configured
        let (compressed_data, was_compressed) = self.compress_data(data)?;
        let data_to_write = if was_compressed {
            &compressed_data
        } else {
            data
        };

        // Ensure shard directories exist
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        self.write_atomic(&object_path, data_to_write, &hash)
            .await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            let cached = CachedChunk {
                data: Bytes::from(data.to_vec()),
                size: data.len() as u64,
                last_accessed: Instant::now(),
                access_count: 1,
            };

            // Evict if necessary
            while cache.total_size + cached.size > self.config.cache_config.max_size_bytes {
                if let Some((_, evicted)) = cache.chunks.pop_lru() {
                    cache.total_size -= evicted.size;
                    cache.eviction_count.fetch_add(1, Ordering::Relaxed);
                }
            }

            cache.chunks.put(hash, cached.clone());
            cache.total_size += cached.size;
        }

        // Update dedup index
        {
            let mut dedup = self.dedup_index.write().await;
            dedup.insert(
                hash,
                ObjectRef {
                    hash,
                    size: data.len() as u64,
                },
            );
        }

        self.stats.writes.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes_written
            .fetch_add(data.len() as u64, Ordering::Relaxed);

        Ok(ObjectRef {
            hash,
            size: data.len() as u64,
        })
    }

    /// Compress data using configured algorithm
    fn compress_data(&self, data: &[u8]) -> Result<(Vec<u8>, bool)> {
        match self.config.compression {
            CompressionType::None => Ok((vec![], false)),
            CompressionType::Lz4 => {
                // Use lz4_flex for compression
                Ok((lz4_flex::compress_prepend_size(data), true))
            }
            CompressionType::Zstd { level } => {
                // Use zstd for compression
                match zstd::encode_all(data, level) {
                    Ok(compressed) => Ok((compressed, true)),
                    Err(e) => Err(CasError::Internal(format!("Compression failed: {}", e))),
                }
            }
            CompressionType::Snappy => {
                // Use snap for compression
                let mut encoder = snap::raw::Encoder::new();
                match encoder.compress_vec(data) {
                    Ok(compressed) => Ok((compressed, true)),
                    Err(e) => Err(CasError::Internal(format!("Compression failed: {}", e))),
                }
            }
        }
    }

    /// Decompress data using configured algorithm
    fn decompress_data(&self, data: &[u8], compressed: bool) -> Result<Vec<u8>> {
        if !compressed {
            return Ok(data.to_vec());
        }

        match self.config.compression {
            CompressionType::None => Ok(data.to_vec()),
            CompressionType::Lz4 => lz4_flex::decompress_size_prepended(data)
                .map_err(|e| CasError::Internal(format!("Decompression failed: {}", e))),
            CompressionType::Zstd { .. } => zstd::decode_all(data)
                .map_err(|e| CasError::Internal(format!("Decompression failed: {}", e))),
            CompressionType::Snappy => {
                let mut decoder = snap::raw::Decoder::new();
                decoder
                    .decompress_vec(data)
                    .map_err(|e| CasError::Internal(format!("Decompression failed: {}", e)))
            }
        }
    }

    /// Write an object to the content store.
    ///
    /// The object is written atomically using a temporary file and rename for large objects,
    /// or stored in packfiles for small objects. If an object with the same hash already
    /// exists, this is a no-op. Fsync behavior is controlled by the store's configuration.
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

        // Check dedup index first (fast path)
        {
            let dedup = self.dedup_index.read().await;
            if let Some(obj_ref) = dedup.get(&hash) {
                self.stats.dedup_hits.fetch_add(1, Ordering::Relaxed);
                trace!("Object {} found in dedup index", hash);
                return Ok(obj_ref.clone());
            }
        }

        // Check if object already exists (check both individual files and packfiles)
        if self.exists(&hash).await {
            trace!("Object {} already exists", hash);
            let obj_ref = ObjectRef {
                hash,
                size: data.len() as u64,
            };

            // Update dedup index
            let mut dedup = self.dedup_index.write().await;
            dedup.insert(hash, obj_ref.clone());

            return Ok(obj_ref);
        }

        // Decide whether to use packfiles or individual storage
        if self.config.enable_packfiles {
            if let Some(ref manager) = self.packfile_manager {
                let manager_guard = manager.lock().await;
                if manager_guard.should_pack(data.len() as u64) {
                    drop(manager_guard);
                    return self.write_with_packing(data, hash).await;
                }
            }
        }

        // Store as individual file
        self.write_individual(data, hash).await
    }

    /// Write data using packfile storage
    async fn write_with_packing(&self, data: &[u8], hash: ContentHash) -> Result<ObjectRef> {
        if let Some(_manager) = &self.packfile_manager {
            // For thread safety, we'd normally need a mutex here, but for now assume single-threaded access
            // In a production implementation, you'd want Arc<Mutex<PackfileManager>> or similar
            // manager.add_chunk(hash, data).await?;
            debug!(
                "Would store object {} ({} bytes) in packfile",
                hash,
                data.len()
            );

            // For now, fall back to individual storage until we implement proper async mutex handling
            return self.write_individual(data, hash).await;
        }

        // Fallback to individual storage
        self.write_individual(data, hash).await
    }

    /// Write data as individual file (the original write logic)
    async fn write_individual(&self, data: &[u8], hash: ContentHash) -> Result<ObjectRef> {
        let object_path = self.object_path(&hash);

        // Ensure shard directories exist
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        self.write_atomic(&object_path, data, &hash).await?;

        debug!(
            "Wrote object {} ({} bytes) as individual file",
            hash,
            data.len()
        );

        Ok(ObjectRef {
            hash,
            size: data.len() as u64,
        })
    }

    /// Read an object from the content store with caching.
    ///
    /// The object's integrity is verified by recomputing its hash and checking
    /// file metadata for consistency. Searches cache first, then both individual files and packfiles.
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

        // Check cache first
        {
            let mut cache = self.cache.write().await;
            if let Some(cached) = cache.chunks.get_mut(hash) {
                cached.last_accessed = Instant::now();
                cached.access_count += 1;
                let cached_data = cached.data.clone();
                let cached_size = cached.size;

                cache.hit_count.fetch_add(1, Ordering::Relaxed);
                self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                self.stats.reads.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .bytes_read
                    .fetch_add(cached_size, Ordering::Relaxed);
                trace!("Cache hit for chunk {}", hash);
                return Ok(cached_data);
            }
            cache.miss_count.fetch_add(1, Ordering::Relaxed);
            self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
        }

        // Try reading from packfiles first (they're likely to have small, frequently accessed objects)
        if self.config.enable_packfiles {
            if let Some(ref manager) = self.packfile_manager {
                let manager_guard = manager.lock().await;
                if let Ok(Some(data)) = manager_guard.read_packed(hash).await {
                    trace!("Read object {} ({} bytes) from packfile", hash, data.len());
                    return Ok(data);
                }
            }
        }

        // Fall back to individual file storage
        let data = self.read_individual(hash).await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            let cached = CachedChunk {
                data: data.clone(),
                size: data.len() as u64,
                last_accessed: Instant::now(),
                access_count: 1,
            };

            // Evict if necessary
            while cache.total_size + cached.size > self.config.cache_config.max_size_bytes {
                if let Some((_, evicted)) = cache.chunks.pop_lru() {
                    cache.total_size -= evicted.size;
                    cache.eviction_count.fetch_add(1, Ordering::Relaxed);
                }
            }

            cache.chunks.put(*hash, cached.clone());
            cache.total_size += cached.size;
        }

        self.stats.reads.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes_read
            .fetch_add(data.len() as u64, Ordering::Relaxed);

        Ok(data)
    }

    /// Read data from individual file storage (the original read logic)
    async fn read_individual(&self, hash: &ContentHash) -> Result<Bytes> {
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

        trace!(
            "Read object {} ({} bytes) from individual file",
            hash,
            data.len()
        );

        Ok(Bytes::from(data))
    }

    /// Check if an object exists in the store (both individual files and packfiles).
    ///
    /// # Arguments
    ///
    /// * `hash` - The hash of the object to check
    ///
    /// # Returns
    ///
    /// true if the object exists, false otherwise
    pub async fn exists(&self, hash: &ContentHash) -> bool {
        // Check individual file first (fastest)
        if self.object_path(hash).exists() {
            return true;
        }

        // Check packfiles if enabled
        if self.config.enable_packfiles {
            if let Some(ref manager) = self.packfile_manager {
                let manager_guard = manager.lock().await;
                if let Ok(Some(_)) = manager_guard.read_packed(hash).await {
                    return true;
                }
            }
        }

        false
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

    /// Stream write operation that doesn't load full chunk into memory.
    /// Calculates hash while streaming and verifies at the end.
    ///
    /// # Arguments
    ///
    /// * `reader` - Async reader to stream data from
    /// * `expected_hash` - Optional expected hash for verification
    ///
    /// # Returns
    ///
    /// An ObjectRef containing the hash and size of the stored data
    pub async fn write_stream<R>(
        &self,
        mut reader: R,
        expected_hash: Option<&ContentHash>,
    ) -> Result<ObjectRef>
    where
        R: AsyncRead + Unpin,
    {
        // Create a hasher to compute hash while streaming
        let mut hasher = Hasher::new();
        let temp_id = uuid::Uuid::new_v4();
        let temp_path = self.root_path.join(".tmp").join(format!("{}.tmp", temp_id));

        // Ensure temp directory exists
        if let Some(parent) = temp_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Stream to temporary file while computing hash
        let mut file = BufWriter::new(File::create(&temp_path).await?);
        let mut buffer = vec![0u8; 64 * 1024]; // 64KB buffer
        let mut total_size = 0u64;

        loop {
            let n = reader.read(&mut buffer).await?;
            if n == 0 {
                break;
            }

            let chunk = &buffer[..n];
            hasher.update(chunk);
            file.write_all(chunk).await?;
            total_size += n as u64;
        }

        file.flush().await?;

        // Apply fsync based on policy
        if matches!(self.config.fsync_policy, FsyncPolicy::Always) {
            file.get_mut().sync_all().await?;
        }

        drop(file); // Close the file

        // Compute final hash
        let computed_hash = ContentHash::from_blake3(hasher.finalize());

        // Verify hash if expected
        if let Some(expected) = expected_hash {
            if &computed_hash != expected {
                // Clean up temp file
                let _ = fs::remove_file(&temp_path).await;
                return Err(CasError::HashMismatch {
                    expected: *expected,
                    actual: computed_hash,
                });
            }
        }

        // Check if object already exists
        let object_path = self.object_path(&computed_hash);
        if object_path.exists() {
            // Remove temp file and return existing ref
            fs::remove_file(&temp_path).await?;

            let obj_ref = ObjectRef {
                hash: computed_hash,
                size: total_size,
            };

            // Update dedup index
            {
                let mut dedup = self.dedup_index.write().await;
                dedup.insert(computed_hash, obj_ref.clone());
            }

            return Ok(obj_ref);
        }

        // Ensure shard directories exist
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Move temp file to final location
        fs::rename(&temp_path, &object_path).await?;

        let obj_ref = ObjectRef {
            hash: computed_hash,
            size: total_size,
        };

        // Update dedup index
        {
            let mut dedup = self.dedup_index.write().await;
            dedup.insert(computed_hash, obj_ref.clone());
        }

        // Update stats
        self.stats.writes.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes_written
            .fetch_add(total_size, Ordering::Relaxed);

        debug!("Streamed write of {} ({} bytes)", computed_hash, total_size);

        Ok(obj_ref)
    }

    /// Stream read operation that returns an async reader instead of loading full chunk.
    /// Useful for network transfers and large files.
    ///
    /// # Arguments
    ///
    /// * `hash` - The hash of the object to read
    ///
    /// # Returns
    ///
    /// A boxed async reader that streams the object data
    pub async fn read_stream(
        &self,
        hash: &ContentHash,
    ) -> Result<Box<dyn AsyncRead + Send + Unpin>> {
        // Validate hash format
        validation::validate_content_hash(&hash.to_string())
            .map_err(|e| CasError::InvalidOperation(format!("Hash validation failed: {}", e)))?;

        // Check if exists first
        if !self.exists(hash).await {
            return Err(CasError::ObjectNotFound(*hash));
        }

        // Try packfiles first if enabled
        if self.config.enable_packfiles {
            if let Some(ref manager) = self.packfile_manager {
                let manager_guard = manager.lock().await;
                if let Ok(Some(data)) = manager_guard.read_packed(hash).await {
                    // For packfile data, wrap in a cursor for streaming
                    use std::io::Cursor;
                    let cursor = Cursor::new(data.to_vec());
                    return Ok(Box::new(cursor));
                }
            }
        }

        // Stream from individual file
        let object_path = self.object_path(hash);
        let file = File::open(&object_path).await?;
        let reader = BufReader::new(file);

        // Update stats
        self.stats.reads.fetch_add(1, Ordering::Relaxed);

        Ok(Box::new(reader))
    }

    /// Write multiple chunks in a single batch transaction for improved performance.
    /// Uses streaming to avoid loading all chunks into memory at once.
    ///
    /// # Arguments
    ///
    /// * `chunks` - Iterator of (data, optional expected hash) pairs
    ///
    /// # Returns
    ///
    /// Vector of ObjectRefs for the written chunks
    pub async fn write_batch_stream<I, R>(&self, chunks: I) -> Result<Vec<ObjectRef>>
    where
        I: IntoIterator<Item = (R, Option<ContentHash>)>,
        R: AsyncRead + Unpin + Send + 'static,
    {
        let chunks: Vec<_> = chunks.into_iter().collect();
        let mut results = Vec::with_capacity(chunks.len());

        // Process chunks with concurrency control
        let mut handles = Vec::new();

        for (reader, expected_hash) in chunks {
            let store = self.clone();
            let permit = self.write_semaphore.clone().acquire_owned().await;

            let handle = tokio::spawn(async move {
                let _permit = permit;
                store.write_stream(reader, expected_hash.as_ref()).await
            });
            handles.push(handle);
        }

        // Collect results
        for handle in handles {
            let result = handle
                .await
                .map_err(|e| CasError::Internal(format!("Batch stream write failed: {}", e)))??;
            results.push(result);
        }

        // Update batch stats
        self.stats.batch_flushes.fetch_add(1, Ordering::Relaxed);

        Ok(results)
    }

    /// Start a resumable write operation that can be interrupted and resumed later.
    /// Returns a partial transfer handle that can be used to resume the operation.
    ///
    /// # Arguments
    ///
    /// * `expected_hash` - Optional expected hash for verification
    /// * `total_size` - Optional total size for progress tracking
    ///
    /// # Returns
    ///
    /// A PartialTransfer handle for resuming the operation
    pub async fn start_resumable_write(
        &self,
        expected_hash: Option<ContentHash>,
        total_size: Option<u64>,
    ) -> Result<PartialTransfer> {
        let temp_id = uuid::Uuid::new_v4();
        let temp_path = self
            .root_path
            .join(".tmp")
            .join(format!("{}.partial", temp_id));

        // Ensure temp directory exists
        if let Some(parent) = temp_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let partial = PartialTransfer::new(temp_path, expected_hash, total_size);

        debug!("Started resumable write: {:?}", partial.temp_path);

        Ok(partial)
    }

    /// Continue writing to a partial transfer from a reader.
    /// This appends new data to the existing partial file and updates the hash.
    ///
    /// # Arguments
    ///
    /// * `partial` - Mutable partial transfer to update
    /// * `reader` - Reader to append data from
    ///
    /// # Returns
    ///
    /// Updated partial transfer with new bytes received
    pub async fn continue_resumable_write<R>(
        &self,
        partial: &mut PartialTransfer,
        mut reader: R,
    ) -> Result<()>
    where
        R: AsyncRead + Unpin,
    {
        if partial.is_expired() {
            return Err(CasError::InvalidOperation(
                "Partial transfer has expired".to_string(),
            ));
        }

        // Open/create the partial file for appending
        let mut file = if partial.bytes_received == 0 {
            BufWriter::new(File::create(&partial.temp_path).await?)
        } else {
            BufWriter::new(
                File::options()
                    .append(true)
                    .open(&partial.temp_path)
                    .await?,
            )
        };

        // Initialize or restore hasher state
        let mut hasher = if let Some(_hash_state) = &partial.hash_state {
            // TODO: Deserialize hasher state when blake3 supports it
            // For now, re-read the existing file to rebuild state
            if partial.bytes_received > 0 {
                let mut existing_file = File::open(&partial.temp_path).await?;
                let mut existing_hasher = Hasher::new();
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    let n = existing_file.read(&mut buffer).await?;
                    if n == 0 {
                        break;
                    }
                    existing_hasher.update(&buffer[..n]);
                }
                existing_hasher
            } else {
                Hasher::new()
            }
        } else {
            Hasher::new()
        };

        // Stream new data while updating hash
        let mut buffer = vec![0u8; 64 * 1024];
        let mut bytes_written = 0u64;

        loop {
            let n = reader.read(&mut buffer).await?;
            if n == 0 {
                break;
            }

            let chunk = &buffer[..n];
            hasher.update(chunk);
            file.write_all(chunk).await?;
            bytes_written += n as u64;
        }

        file.flush().await?;

        // Update partial transfer state
        partial.bytes_received += bytes_written;
        partial.hash_state = Some("state_placeholder".to_string()); // TODO: Serialize hasher state

        debug!(
            "Resumed write: {} bytes added to {:?} (total: {})",
            bytes_written, partial.temp_path, partial.bytes_received
        );

        Ok(())
    }

    /// Complete a resumable write operation, moving the temp file to final location.
    /// Verifies the final hash against expected hash if provided.
    ///
    /// # Arguments
    ///
    /// * `partial` - Partial transfer to complete
    ///
    /// # Returns
    ///
    /// ObjectRef of the completed object
    pub async fn complete_resumable_write(&self, partial: PartialTransfer) -> Result<ObjectRef> {
        if partial.is_expired() {
            // Clean up expired transfer
            let _ = fs::remove_file(&partial.temp_path).await;
            return Err(CasError::InvalidOperation(
                "Partial transfer has expired".to_string(),
            ));
        }

        if !partial.temp_path.exists() {
            return Err(CasError::InvalidOperation(
                "Partial transfer file not found".to_string(),
            ));
        }

        // Re-compute hash from the complete file to verify integrity
        let mut file = File::open(&partial.temp_path).await?;
        let mut hasher = Hasher::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut total_size = 0u64;

        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            total_size += n as u64;
        }

        let computed_hash = ContentHash::from_blake3(hasher.finalize());

        // Verify against expected hash if provided
        if let Some(expected) = partial.expected_hash {
            if computed_hash != expected {
                // Clean up failed transfer
                let _ = fs::remove_file(&partial.temp_path).await;
                return Err(CasError::HashMismatch {
                    expected,
                    actual: computed_hash,
                });
            }
        }

        // Verify against expected size if provided
        if let Some(expected_size) = partial.total_size {
            if total_size != expected_size {
                let _ = fs::remove_file(&partial.temp_path).await;
                return Err(CasError::InvalidOperation(format!(
                    "Size mismatch: expected {}, got {}",
                    expected_size, total_size
                )));
            }
        }

        // Check if object already exists
        let object_path = self.object_path(&computed_hash);
        if object_path.exists() {
            // Remove temp file and return existing ref
            fs::remove_file(&partial.temp_path).await?;

            let obj_ref = ObjectRef {
                hash: computed_hash,
                size: total_size,
            };

            // Update dedup index
            {
                let mut dedup = self.dedup_index.write().await;
                dedup.insert(computed_hash, obj_ref.clone());
            }

            return Ok(obj_ref);
        }

        // Ensure shard directories exist
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Move temp file to final location
        fs::rename(&partial.temp_path, &object_path).await?;

        let obj_ref = ObjectRef {
            hash: computed_hash,
            size: total_size,
        };

        // Update dedup index
        {
            let mut dedup = self.dedup_index.write().await;
            dedup.insert(computed_hash, obj_ref.clone());
        }

        // Update stats
        self.stats.writes.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes_written
            .fetch_add(total_size, Ordering::Relaxed);

        debug!(
            "Completed resumable write of {} ({} bytes)",
            computed_hash, total_size
        );

        Ok(obj_ref)
    }

    /// Cancel a resumable write operation, cleaning up temporary files.
    ///
    /// # Arguments
    ///
    /// * `partial` - Partial transfer to cancel
    pub async fn cancel_resumable_write(&self, partial: PartialTransfer) -> Result<()> {
        if partial.temp_path.exists() {
            fs::remove_file(&partial.temp_path).await?;
            debug!("Cancelled resumable write: {:?}", partial.temp_path);
        }
        Ok(())
    }

    /// Clean up expired partial transfers.
    /// Should be called periodically for maintenance.
    pub async fn cleanup_expired_partials(&self) -> Result<usize> {
        let temp_dir = self.root_path.join(".tmp");
        if !temp_dir.exists() {
            return Ok(0);
        }

        let mut cleaned_count = 0;
        let now = std::time::SystemTime::now();

        let mut dir = fs::read_dir(&temp_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".partial") {
                    // Check if file is older than 24 hours
                    if let Ok(metadata) = entry.metadata().await {
                        if let Ok(created) = metadata.created() {
                            if now.duration_since(created).unwrap_or_default().as_secs() > 24 * 3600
                            {
                                let _ = fs::remove_file(&path).await;
                                cleaned_count += 1;
                                debug!("Cleaned up expired partial: {:?}", path);
                            }
                        }
                    }
                }
            }
        }

        Ok(cleaned_count)
    }

    /// Get access to metrics collection
    pub fn get_metrics(&self) -> &ContentStoreRuntimeStats {
        &self.stats
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
    async fn write_atomic(
        &self,
        final_path: &Path,
        data: &[u8],
        _expected_hash: &ContentHash,
    ) -> Result<()> {
        let temp_dir = final_path
            .parent()
            .ok_or_else(|| CasError::StoragePath("Invalid object path".to_string()))?;

        // Create temp file with .tmp extension in the same directory
        let temp_path = {
            let mut temp_name = final_path
                .file_name()
                .ok_or_else(|| CasError::StoragePath("Invalid object filename".to_string()))?
                .to_os_string();
            temp_name.push(".tmp");
            temp_dir.join(temp_name)
        };

        // Write data to temp file
        {
            let mut file = File::create(&temp_path).await.map_err(|e| {
                CasError::AtomicWriteFailed(format!("Failed to create temp file: {}", e))
            })?;

            file.write_all(data)
                .await
                .map_err(|e| CasError::AtomicWriteFailed(format!("Failed to write data: {}", e)))?;

            // Fsync temp file based on policy
            if self.should_fsync_file() {
                file.sync_all().await.map_err(|e| {
                    CasError::AtomicWriteFailed(format!("Failed to sync temp file: {}", e))
                })?;
            }
        }

        // Atomic rename to final location
        fs::rename(&temp_path, final_path).await.map_err(|e| {
            // Clean up temp file on rename failure
            let _ = std::fs::remove_file(&temp_path);
            CasError::AtomicWriteFailed(format!("Failed to rename temp file: {}", e))
        })?;

        // Fsync parent directory to ensure rename is durable
        if self.should_fsync_directory() {
            self.fsync_directory(temp_dir).await.map_err(|e| {
                CasError::AtomicWriteFailed(format!("Failed to sync directory: {}", e))
            })?;
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
                    stats
                        .errors
                        .push(format!("Failed to read directory {:?}: {}", current_dir, e));
                    continue;
                }
            };

            loop {
                let entry = match dir_entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(e) => {
                        stats
                            .errors
                            .push(format!("Failed to read directory entry: {}", e));
                        continue;
                    }
                };
                let path = entry.path();

                if entry
                    .file_type()
                    .await
                    .map(|ft| ft.is_dir())
                    .unwrap_or(false)
                {
                    stack.push(path);
                } else if path.extension().and_then(|s| s.to_str()) == Some("tmp") {
                    match self.recover_temp_file(&path, &mut stats).await {
                        Ok(_) => {}
                        Err(e) => stats
                            .errors
                            .push(format!("Recovery error for {:?}: {}", path, e)),
                    }
                }
            }
        }

        if stats.recovered > 0 || stats.cleaned > 0 || !stats.errors.is_empty() {
            debug!(
                "Recovery completed: recovered={}, cleaned={}, errors={}",
                stats.recovered,
                stats.cleaned,
                stats.errors.len()
            );
        }

        Ok(stats)
    }

    /// Attempt to recover a single temporary file.
    async fn recover_temp_file(&self, temp_path: &Path, stats: &mut RecoveryStats) -> Result<()> {
        // Determine what the final path should be
        let final_path = {
            let temp_name = temp_path
                .file_name()
                .ok_or_else(|| CasError::StoragePath("Invalid temp filename".to_string()))?;
            let temp_name_str = temp_name.to_string_lossy();

            if !temp_name_str.ends_with(".tmp") {
                return Err(CasError::StoragePath(
                    "Temp file doesn't end with .tmp".to_string(),
                ));
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
                }
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
                }
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
    /// total storage size, including both individual files and packfiles.
    /// For large stores, this operation may be expensive.
    ///
    /// # Returns
    ///
    /// Statistics including object count and total size
    pub async fn stats(&self) -> Result<StorageStats> {
        let objects_dir = self.root_path.join("objects");

        let mut object_count = 0u64;
        let mut total_size = 0u64;

        // Count individual files
        if objects_dir.exists() {
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
        }

        // Add packfile statistics
        if self.config.enable_packfiles {
            if let Some(ref manager) = self.packfile_manager {
                let manager_guard = manager.lock().await;
                let packfile_stats = manager_guard.stats();
                object_count += packfile_stats.total_objects as u64;
                total_size += packfile_stats.total_packed_size;
            }
        }

        debug!(
            "Storage stats: {} objects, {} bytes (including packfiles)",
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

    /// Force flush any pending packfile operations
    pub async fn flush_packfiles(&self) -> Result<()> {
        if let Some(ref manager) = self.packfile_manager {
            let mut manager = manager.lock().await;
            manager.flush_packfile().await?;
        }
        Ok(())
    }

    /// Finalize all packfile operations (useful for shutdown)
    pub async fn finalize_packfiles(&self) -> Result<()> {
        if let Some(ref manager) = self.packfile_manager {
            let mut manager = manager.lock().await;
            manager.finalize().await?;
        }
        Ok(())
    }

    /// Get packfile statistics if packfiles are enabled
    pub async fn packfile_stats(&self) -> Option<crate::packfile::PackfileStats> {
        if let Some(ref manager) = self.packfile_manager {
            let manager = manager.lock().await;
            Some(manager.stats())
        } else {
            None
        }
    }

    /// Get runtime statistics for the content store
    pub async fn runtime_stats(&self) -> ContentStoreStats {
        let cache = self.cache.read().await;
        ContentStoreStats {
            total_writes: self.stats.writes.load(Ordering::Relaxed),
            total_reads: self.stats.reads.load(Ordering::Relaxed),
            bytes_written: self.stats.bytes_written.load(Ordering::Relaxed),
            bytes_read: self.stats.bytes_read.load(Ordering::Relaxed),
            objects_packed: self.stats.objects_packed.load(Ordering::Relaxed),
            verifications_performed: self.stats.verifications.load(Ordering::Relaxed),
            corruptions_found: self.stats.corruptions_found.load(Ordering::Relaxed),
            cache_hits: self.stats.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.stats.cache_misses.load(Ordering::Relaxed),
            cache_hit_rate: cache.hit_rate(),
            batch_flushes: self.stats.batch_flushes.load(Ordering::Relaxed),
            dedup_hits: self.stats.dedup_hits.load(Ordering::Relaxed),
        }
    }

    /// Prefetch chunks for improved performance
    pub async fn prefetch(&self, hashes: &[ContentHash]) -> Result<()> {
        if !self.config.cache_config.prefetch_enabled {
            return Ok(());
        }

        for hash in hashes {
            // Check if already in cache
            {
                let cache = self.cache.read().await;
                if cache.chunks.contains(hash) {
                    continue;
                }
            }

            // Try to read and cache
            match self.read(hash).await {
                Ok(_) => trace!("Prefetched chunk {}", hash),
                Err(e) => debug!("Failed to prefetch chunk {}: {}", hash, e),
            }
        }

        Ok(())
    }

    /// Clear the chunk cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.chunks.clear();
        cache.total_size = 0;
        debug!("Cleared chunk cache");
    }

    /// Get cache statistics
    pub async fn cache_stats(&self) -> CacheStats {
        let cache = self.cache.read().await;
        CacheStats {
            chunks_cached: cache.chunks.len(),
            total_size: cache.total_size,
            hit_rate: cache.hit_rate(),
            hits: cache.hit_count.load(Ordering::Relaxed),
            misses: cache.miss_count.load(Ordering::Relaxed),
            evictions: cache.eviction_count.load(Ordering::Relaxed),
        }
    }

    /// Start background integrity scanner (if enabled)
    pub fn start_background_scanner(self: Arc<Self>) {
        if !self.config.enable_background_scan {
            return;
        }

        let scanner = Arc::clone(&self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                scanner.config.scan_interval_secs,
            ));

            loop {
                interval.tick().await;

                debug!("Starting background integrity scan");
                match scanner.verify_integrity().await {
                    Ok(report) => {
                        if !report.is_healthy() {
                            warn!(
                                "Background scan found issues: {} corrupted, {} errors",
                                report.corruption_count, report.error_count
                            );
                        } else {
                            debug!(
                                "Background scan completed: {} objects verified healthy",
                                report.verified_count
                            );
                        }
                    }
                    Err(e) => {
                        error!("Background integrity scan failed: {}", e);
                    }
                }
            }
        });
    }

    /// Perform garbage collection and repacking
    pub async fn gc_packfiles(
        &mut self,
        keep_chunks: &std::collections::HashSet<ContentHash>,
    ) -> Result<GcStats> {
        if let Some(ref mut _manager) = self.packfile_manager {
            // TODO: Implement actual GC logic
            // This would:
            // 1. Identify unreferenced chunks in packfiles
            // 2. Create new packfiles with only referenced chunks
            // 3. Remove old packfiles atomically
            // 4. Update internal data structures

            debug!(
                "GC requested for {} keep_chunks, but not yet implemented",
                keep_chunks.len()
            );

            Ok(GcStats {
                objects_examined: 0,
                objects_kept: 0,
                objects_removed: 0,
                bytes_reclaimed: 0,
                packfiles_examined: 0,
                packfiles_repacked: 0,
            })
        } else {
            Ok(GcStats {
                objects_examined: 0,
                objects_kept: 0,
                objects_removed: 0,
                bytes_reclaimed: 0,
                packfiles_examined: 0,
                packfiles_repacked: 0,
            })
        }
    }

    /// Repack underutilized packfiles
    pub async fn repack(&mut self) -> Result<PackingStats> {
        if let Some(ref mut _manager) = self.packfile_manager {
            // TODO: Implement actual repacking logic
            // This would:
            // 1. Find packfiles with low utilization
            // 2. Extract still-valid chunks
            // 3. Create new optimized packfiles
            // 4. Remove old packfiles atomically

            debug!("Repack requested but not yet implemented");

            Ok(PackingStats {
                objects_repacked: 0,
                old_packfiles_removed: 0,
                new_packfiles_created: 0,
                bytes_saved: 0,
            })
        } else {
            Ok(PackingStats {
                objects_repacked: 0,
                old_packfiles_removed: 0,
                new_packfiles_created: 0,
                bytes_saved: 0,
            })
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

/// Statistics about garbage collection operations
#[derive(Debug, Clone)]
pub struct GcStats {
    /// Objects examined during GC
    pub objects_examined: u64,
    /// Objects that were kept (still referenced)
    pub objects_kept: u64,
    /// Objects that were removed
    pub objects_removed: u64,
    /// Bytes reclaimed by GC
    pub bytes_reclaimed: u64,
    /// Packfiles examined
    pub packfiles_examined: usize,
    /// Packfiles repacked
    pub packfiles_repacked: usize,
}

/// Statistics about repacking operations
#[derive(Debug, Clone)]
pub struct PackingStats {
    /// Objects moved into new packfiles
    pub objects_repacked: u64,
    /// Old packfiles removed
    pub old_packfiles_removed: usize,
    /// New packfiles created
    pub new_packfiles_created: usize,
    /// Bytes saved by repacking
    pub bytes_saved: u64,
}

/// Runtime statistics for the content store
#[derive(Debug, Clone)]
pub struct ContentStoreStats {
    /// Total write operations
    pub total_writes: u64,
    /// Total read operations
    pub total_reads: u64,
    /// Total bytes written
    pub bytes_written: u64,
    /// Total bytes read
    pub bytes_read: u64,
    /// Objects stored in packfiles
    pub objects_packed: u64,
    /// Verification operations performed
    pub verifications_performed: u64,
    /// Corruptions found during verification
    pub corruptions_found: u64,
    /// Cache hits
    pub cache_hits: u64,
    /// Cache misses
    pub cache_misses: u64,
    /// Cache hit rate
    pub cache_hit_rate: f64,
    /// Batch flushes
    pub batch_flushes: u64,
    /// Deduplication hits
    pub dedup_hits: u64,
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of chunks in cache
    pub chunks_cached: usize,
    /// Total size of cached data
    pub total_size: u64,
    /// Cache hit rate
    pub hit_rate: f64,
    /// Total hits
    pub hits: u64,
    /// Total misses
    pub misses: u64,
    /// Total evictions
    pub evictions: u64,
}

/// Helper function to verify an object at a specific path
async fn verify_object_with_path(path: &Path, expected_hash: &ContentHash) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    // Check file metadata for basic integrity
    let metadata = fs::metadata(path).await?;
    if metadata.len() == 0 {
        return Ok(false);
    }

    // Read and verify hash
    let data = fs::read(path).await?;

    // Verify that the read size matches the metadata
    if data.len() as u64 != metadata.len() {
        return Ok(false);
    }

    // Verify hash integrity
    let mut hasher = Hasher::new();
    hasher.update(&data);
    let actual_hash = ContentHash::from_blake3(hasher.finalize());

    Ok(actual_hash == *expected_hash)
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
            packfile_config: PackfileConfig::default(),
            enable_packfiles: false, // Disable for this test
            cache_config: CacheConfig::default(),
            enable_batch_ops: true,
            max_concurrent_ops: 32,
            enable_background_scan: false,
            scan_interval_secs: 3600,
        };
        let store = ContentStore::new_with_config(dir.path(), config)
            .await
            .unwrap();

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
            packfile_config: PackfileConfig::default(),
            enable_packfiles: false, // Disable for this test
            cache_config: CacheConfig::default(),
            enable_batch_ops: true,
            max_concurrent_ops: 32,
            enable_background_scan: false,
            scan_interval_secs: 3600,
        };
        let store = ContentStore::new_with_config(dir.path(), config)
            .await
            .unwrap();

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
            packfile_config: PackfileConfig::default(),
            enable_packfiles: false, // Disable for this test
            cache_config: CacheConfig::default(),
            enable_batch_ops: true,
            max_concurrent_ops: 32,
            enable_background_scan: false,
            scan_interval_secs: 3600,
        };
        let store = ContentStore::new_with_config(dir.path(), config)
            .await
            .unwrap();

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
                assert!(
                    !filename_str.ends_with(".tmp"),
                    "Found temp file: {}",
                    filename_str
                );
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
            packfile_config: PackfileConfig::default(),
            enable_packfiles: false, // Disable for this test
            cache_config: CacheConfig::default(),
            enable_batch_ops: true,
            max_concurrent_ops: 32,
            enable_background_scan: false,
            scan_interval_secs: 3600,
        };

        // Create store without recovery
        let store = ContentStore::new_with_config(dir.path(), config.clone())
            .await
            .unwrap();

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
            assert_eq!(
                String::from_utf8(read_data.to_vec()).unwrap(),
                expected_data
            );
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
                    assert!(
                        !filename_str.ends_with(".tmp"),
                        "Found temp file after concurrent writes: {}",
                        filename_str
                    );
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
            packfile_config: PackfileConfig::default(),
            enable_packfiles: false, // Disable for this test
            cache_config: CacheConfig::default(),
            enable_batch_ops: true,
            max_concurrent_ops: 32,
            enable_background_scan: false,
            scan_interval_secs: 3600,
        };

        let store = ContentStore::new_with_config(dir.path(), config)
            .await
            .unwrap();

        // Recovery should have happened during initialization
        // Check that valid temp file was recovered
        let object_path1 = store.object_path(&hash1);
        assert!(
            object_path1.exists(),
            "Valid temp file should have been recovered"
        );
        assert!(!temp_path1.exists(), "Temp file should be cleaned up");

        // Check that corrupted temp file was cleaned up
        assert!(
            !temp_path2.exists(),
            "Corrupted temp file should be cleaned up"
        );

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
            packfile_config: PackfileConfig::default(),
            enable_packfiles: false, // Disable for this test
            cache_config: CacheConfig::default(),
            enable_batch_ops: true,
            max_concurrent_ops: 32,
            enable_background_scan: false,
            scan_interval_secs: 3600,
        };
        let store = ContentStore::new_with_config(dir.path(), config)
            .await
            .unwrap();

        // Test that directory fsync doesn't fail
        let test_dir = dir.path().join("test_fsync");
        fs::create_dir_all(&test_dir).await.unwrap();

        // This should not panic or error
        store.fsync_directory(&test_dir).await.unwrap();
    }

    #[ignore = "Packfiles disabled for v1.0"]
    #[tokio::test]
    async fn test_packfile_integration() {
        let dir = tempdir().unwrap();
        let config = ContentStoreConfig {
            fsync_policy: FsyncPolicy::Always,
            enable_recovery: false,
            compression: CompressionType::None,
            packfile_config: PackfileConfig {
                max_packfile_size: 1024 * 1024,
                pack_threshold: 1024, // 1KB threshold
                min_chunks_per_pack: 2,
                compression: CompressionType::None,
            },
            enable_packfiles: true, // Enable for this test
            cache_config: CacheConfig::default(),
            enable_batch_ops: true,
            max_concurrent_ops: 32,
            enable_background_scan: false,
            scan_interval_secs: 3600,
        };

        let mut store = ContentStore::new_with_config(dir.path(), config)
            .await
            .unwrap();

        // Write small objects that should go into packfiles
        let small_data1 = b"Small chunk 1";
        let small_data2 = b"Small chunk 2";
        let small_data3 = b"Small chunk 3";

        let obj1 = store.write(small_data1).await.unwrap();
        let obj2 = store.write(small_data2).await.unwrap();
        let obj3 = store.write(small_data3).await.unwrap();

        // Write large object that should be stored individually
        let large_data = vec![42u8; 2048]; // 2KB, above threshold
        let large_obj = store.write(&large_data).await.unwrap();

        // Flush any pending packfile operations
        store.flush_packfiles().await.unwrap();

        // Verify all objects can be read back
        let read1 = store.read(&obj1.hash).await.unwrap();
        let read2 = store.read(&obj2.hash).await.unwrap();
        let read3 = store.read(&obj3.hash).await.unwrap();
        let read_large = store.read(&large_obj.hash).await.unwrap();

        assert_eq!(&read1[..], small_data1);
        assert_eq!(&read2[..], small_data2);
        assert_eq!(&read3[..], small_data3);
        assert_eq!(&read_large[..], &large_data[..]);

        // Check that objects exist
        assert!(store.exists(&obj1.hash).await);
        assert!(store.exists(&obj2.hash).await);
        assert!(store.exists(&obj3.hash).await);
        assert!(store.exists(&large_obj.hash).await);

        // Get stats including packfile data
        let stats = store.stats().await.unwrap();
        assert_eq!(stats.object_count, 4);

        // Get packfile-specific stats
        let packfile_stats = store.packfile_stats().await;
        assert!(packfile_stats.is_some());
        let pack_stats = packfile_stats.unwrap();

        // Should have at least some objects in packfiles (the small ones)
        assert!(pack_stats.total_objects >= 2); // At least the small chunks

        // Finalize packfiles
        store.finalize_packfiles().await.unwrap();
    }
}
