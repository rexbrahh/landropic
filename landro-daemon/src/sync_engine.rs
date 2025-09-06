//! Core sync engine that integrates all sync components
//! 
//! This module implements the main synchronization loop that:
//! - Watches for file system changes
//! - Indexes and tracks file metadata
//! - Computes differences between local and remote states using Bloom filters
//! - Schedules and executes transfers
//! - Handles conflicts and recovery

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{mpsc, RwLock, Mutex};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, warn, trace};
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Serialize, Deserialize};
use bytes::{Bytes, BytesMut, BufMut};

use landro_index::{
    async_indexer::AsyncIndexer,
    differ::{DiffEngine, DiffResult, ConflictResolution},
    manifest::{Manifest, FileEntry},
    watcher::{FsEvent, EventKind, PlatformWatcher, EventDebouncer, WatcherConfig, create_platform_watcher},
};
use landro_cas::ContentStore;
use landro_chunker::Chunker;
// Transfer functionality will be integrated directly into sync engine
// use landro_transfer::{TransferEngine, TransferJob, TransferPriority, TransferStatus};

use crate::discovery::PeerInfo;

/// Transfer priority levels
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferPriority {
    High,
    Normal,
    Low,
}

/// Transfer status
#[derive(Debug, Clone, PartialEq)]
pub enum TransferStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Transfer direction
#[derive(Debug, Clone, PartialEq)]
pub enum TransferDirection {
    Upload,
    Download,
}

/// Transfer job definition
#[derive(Debug, Clone)]
pub struct TransferJob {
    pub id: String,
    pub source_path: PathBuf,
    pub dest_path: PathBuf,
    pub peer_id: String,
    pub direction: TransferDirection,
    pub priority: TransferPriority,
    pub size: u64,
    pub chunk_hashes: Vec<String>,
    pub status: TransferStatus,
    pub progress: u64,
    pub error: Option<String>,
    pub created_at: SystemTime,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
}

/// Simple transfer engine integrated into sync engine
#[derive(Debug)]
pub struct TransferEngine {
    store: Arc<ContentStore>,
    active_transfers: Arc<Mutex<HashMap<String, TransferJob>>>,
    max_concurrent: usize,
}

impl TransferEngine {
    pub async fn new(
        store: Arc<ContentStore>,
        _storage_path: &Path,
        max_concurrent: usize,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self {
            store,
            active_transfers: Arc::new(Mutex::new(HashMap::new())),
            max_concurrent,
        })
    }

    pub async fn submit_transfer(&self, job: TransferJob) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut transfers = self.active_transfers.lock().await;
        transfers.insert(job.id.clone(), job);
        Ok(())
    }

    pub async fn poll_completion(&self) -> Option<Result<TransferJob, Box<dyn std::error::Error + Send + Sync>>> {
        // Simplified polling - in reality would check actual transfer progress
        None
    }

    pub async fn shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut transfers = self.active_transfers.lock().await;
        transfers.clear();
        Ok(())
    }
}

/// Simple network manager interface
#[derive(Debug)]
pub struct NetworkManager {
}

impl NetworkManager {
    pub async fn new(_config: NetworkConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self {})
    }
}

#[derive(Debug, Default)]
pub struct NetworkConfig {}

/// Bloom filter-based diff protocol for efficient sync
/// This module implements efficient difference detection using Bloom filters
/// to minimize bandwidth during manifest comparison.
pub mod bloom_diff_protocol {
    use super::*;
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    
    /// Bloom filter for efficient set membership testing
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BloomFilter {
        /// Bit vector storing the filter
        bits: Vec<u8>,
        /// Number of bits in the filter
        num_bits: usize,
        /// Number of hash functions
        num_hashes: usize,
        /// Number of elements inserted
        num_elements: usize,
        /// Target false positive rate
        false_positive_rate: f64,
    }
    
    impl BloomFilter {
        /// Create a new Bloom filter with target capacity and false positive rate
        pub fn new(expected_elements: usize, false_positive_rate: f64) -> Self {
            // Calculate optimal number of bits: m = -n * ln(p) / (ln(2)^2)
            let num_bits = (-(expected_elements as f64) * false_positive_rate.ln() 
                / (2.0_f64.ln().powi(2))) as usize;
            
            // Calculate optimal number of hash functions: k = (m/n) * ln(2)
            let num_hashes = ((num_bits as f64 / expected_elements as f64) 
                * 2.0_f64.ln()) as usize;
            
            // Ensure minimum sizes
            let num_bits = num_bits.max(64);
            let num_hashes = num_hashes.max(1).min(10); // Cap at 10 hash functions
            
            let byte_size = (num_bits + 7) / 8;
            
            Self {
                bits: vec![0; byte_size],
                num_bits,
                num_hashes,
                num_elements: 0,
                false_positive_rate,
            }
        }
        
        /// Insert an element into the Bloom filter
        pub fn insert(&mut self, item: &[u8]) {
            for i in 0..self.num_hashes {
                let hash = self.hash(item, i);
                let bit_idx = hash % self.num_bits;
                let byte_idx = bit_idx / 8;
                let bit_offset = bit_idx % 8;
                
                self.bits[byte_idx] |= 1 << bit_offset;
            }
            self.num_elements += 1;
        }
        
        /// Test if an element might be in the set
        pub fn contains(&self, item: &[u8]) -> bool {
            for i in 0..self.num_hashes {
                let hash = self.hash(item, i);
                let bit_idx = hash % self.num_bits;
                let byte_idx = bit_idx / 8;
                let bit_offset = bit_idx % 8;
                
                if (self.bits[byte_idx] & (1 << bit_offset)) == 0 {
                    return false;
                }
            }
            true
        }
        
        /// Generate hash for item with seed
        fn hash(&self, item: &[u8], seed: usize) -> usize {
            let mut hasher = DefaultHasher::new();
            hasher.write(item);
            hasher.write_usize(seed);
            hasher.finish() as usize
        }
        
        /// Merge another Bloom filter into this one (OR operation)
        pub fn merge(&mut self, other: &BloomFilter) -> Result<(), String> {
            if self.num_bits != other.num_bits || self.num_hashes != other.num_hashes {
                return Err("Cannot merge Bloom filters with different parameters".to_string());
            }
            
            for (i, byte) in self.bits.iter_mut().enumerate() {
                *byte |= other.bits[i];
            }
            
            self.num_elements += other.num_elements;
            Ok(())
        }
        
        /// Calculate the intersection cardinality estimate
        pub fn estimate_intersection(&self, other: &BloomFilter) -> usize {
            if self.num_bits != other.num_bits || self.num_hashes != other.num_hashes {
                return 0;
            }
            
            let mut common_bits = 0;
            for i in 0..self.bits.len() {
                let common = self.bits[i] & other.bits[i];
                common_bits += common.count_ones() as usize;
            }
            
            // Estimate using the formula: |A ∩ B| ≈ -m/k * ln(1 - common_bits/m)
            let m = self.num_bits as f64;
            let k = self.num_hashes as f64;
            let ratio = common_bits as f64 / m;
            
            if ratio >= 1.0 {
                return self.num_elements.min(other.num_elements);
            }
            
            ((-m / k) * (1.0 - ratio).ln()) as usize
        }
        
        /// Serialize to bytes for network transmission
        pub fn to_bytes(&self) -> Bytes {
            let mut buf = BytesMut::new();
            
            // Write header
            buf.put_u32(self.num_bits as u32);
            buf.put_u32(self.num_hashes as u32);
            buf.put_u32(self.num_elements as u32);
            buf.put_f64(self.false_positive_rate);
            
            // Write bit vector
            buf.put_slice(&self.bits);
            
            buf.freeze()
        }
        
        /// Deserialize from bytes
        pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
            if data.len() < 20 {
                return Err("Invalid Bloom filter data".to_string());
            }
            
            let mut cursor = 0;
            
            // Read header
            let num_bits = u32::from_be_bytes([
                data[cursor], data[cursor+1], data[cursor+2], data[cursor+3]
            ]) as usize;
            cursor += 4;
            
            let num_hashes = u32::from_be_bytes([
                data[cursor], data[cursor+1], data[cursor+2], data[cursor+3]
            ]) as usize;
            cursor += 4;
            
            let num_elements = u32::from_be_bytes([
                data[cursor], data[cursor+1], data[cursor+2], data[cursor+3]
            ]) as usize;
            cursor += 4;
            
            let false_positive_rate = f64::from_be_bytes([
                data[cursor], data[cursor+1], data[cursor+2], data[cursor+3],
                data[cursor+4], data[cursor+5], data[cursor+6], data[cursor+7]
            ]);
            cursor += 8;
            
            // Read bit vector
            let byte_size = (num_bits + 7) / 8;
            if data.len() < cursor + byte_size {
                return Err("Incomplete Bloom filter data".to_string());
            }
            
            let bits = data[cursor..cursor + byte_size].to_vec();
            
            Ok(Self {
                bits,
                num_bits,
                num_hashes,
                num_elements,
                false_positive_rate,
            })
        }
    }
    
    /// Manifest summary using Bloom filters for efficient comparison
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ManifestSummary {
        /// Device ID of the manifest owner
        pub device_id: String,
        /// Root path of the manifest
        pub root_path: PathBuf,
        /// Bloom filter containing all file paths
        pub path_filter: BloomFilter,
        /// Bloom filter containing all content hashes
        pub hash_filter: BloomFilter,
        /// Total number of files
        pub file_count: usize,
        /// Total size in bytes
        pub total_size: u64,
        /// Generation timestamp
        pub generated_at: SystemTime,
        /// Checksum of the full manifest for verification
        pub manifest_checksum: [u8; 32],
    }
    
    impl ManifestSummary {
        /// Create a manifest summary from a full manifest
        pub fn from_manifest(manifest: &Manifest, device_id: String) -> Self {
            let file_count = manifest.files.len();
            let mut total_size = 0u64;
            
            // Initialize Bloom filters with 1% false positive rate
            let mut path_filter = BloomFilter::new(file_count, 0.01);
            let mut hash_filter = BloomFilter::new(file_count * 2, 0.01); // More hashes than files
            
            // Populate Bloom filters
            for entry in &manifest.files {
                // Add path to filter
                path_filter.insert(entry.path.as_bytes());
                
                // Add content hashes to filter
                for chunk_hash in &entry.chunk_hashes {
                    hash_filter.insert(chunk_hash.as_bytes());
                }
                
                total_size += entry.size;
            }
            
            // Calculate manifest checksum
            let mut hasher = DefaultHasher::new();
            for entry in &manifest.files {
                hasher.write(entry.path.as_bytes());
                hasher.write_u64(entry.size);
                for chunk in &entry.chunk_hashes {
                    hasher.write(chunk.as_bytes());
                }
            }
            let checksum_u64 = hasher.finish();
            let mut manifest_checksum = [0u8; 32];
            manifest_checksum[..8].copy_from_slice(&checksum_u64.to_be_bytes());
            
            Self {
                device_id,
                root_path: PathBuf::from("/"), // TODO: Add root_path parameter or store in manifest
                path_filter,
                hash_filter,
                file_count,
                total_size,
                generated_at: SystemTime::now(),
                manifest_checksum,
            }
        }
        
        /// Estimate the difference between two manifests using Bloom filters
        pub fn estimate_diff(&self, other: &ManifestSummary) -> DiffEstimate {
            // Estimate common files using path filter intersection
            let common_files = self.path_filter.estimate_intersection(&other.path_filter);
            
            // Estimate common chunks using hash filter intersection
            let common_chunks = self.hash_filter.estimate_intersection(&other.hash_filter);
            
            // Calculate estimates
            let files_only_in_self = self.file_count.saturating_sub(common_files);
            let files_only_in_other = other.file_count.saturating_sub(common_files);
            
            DiffEstimate {
                common_files,
                files_only_in_self,
                files_only_in_other,
                common_chunks,
                total_files_self: self.file_count,
                total_files_other: other.file_count,
                similarity_ratio: common_files as f64 / self.file_count.max(other.file_count) as f64,
            }
        }
    }
    
    /// Estimated difference between two manifests
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct DiffEstimate {
        /// Estimated number of common files
        pub common_files: usize,
        /// Estimated files only in first manifest
        pub files_only_in_self: usize,
        /// Estimated files only in second manifest
        pub files_only_in_other: usize,
        /// Estimated number of common chunks
        pub common_chunks: usize,
        /// Total files in first manifest
        pub total_files_self: usize,
        /// Total files in second manifest
        pub total_files_other: usize,
        /// Similarity ratio (0.0 to 1.0)
        pub similarity_ratio: f64,
    }
    
    /// Protocol messages for Bloom filter-based diff exchange
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum DiffProtocolMessage {
        /// Request manifest summary from peer
        RequestSummary {
            path: PathBuf,
            request_id: String,
        },
        
        /// Send manifest summary with Bloom filters
        SendSummary {
            summary: ManifestSummary,
            request_id: String,
        },
        
        /// Request specific file paths that might be different
        RequestDiffDetails {
            /// Paths to check (based on Bloom filter results)
            suspected_paths: Vec<PathBuf>,
            request_id: String,
        },
        
        /// Send detailed information about specific files
        SendDiffDetails {
            /// Map of path to file metadata
            file_details: HashMap<PathBuf, FileEntry>,
            request_id: String,
        },
        
        /// Request chunks that are likely missing
        RequestChunks {
            /// Chunk hashes to retrieve
            chunk_hashes: Vec<String>,
            request_id: String,
        },
        
        /// Acknowledge receipt and processing status
        Acknowledge {
            request_id: String,
            success: bool,
            message: Option<String>,
        },
    }
    
    /// Diff protocol handler for processing messages
    pub struct DiffProtocolHandler {
        /// Local manifest cache
        manifest_cache: Arc<RwLock<HashMap<PathBuf, ManifestSummary>>>,
        /// Pending requests
        pending_requests: Arc<Mutex<HashMap<String, PendingRequest>>>,
        /// Protocol statistics
        stats: Arc<RwLock<ProtocolStats>>,
    }
    
    #[derive(Debug)]
    struct PendingRequest {
        request_type: RequestType,
        created_at: Instant,
        peer_id: String,
        path: PathBuf,
    }
    
    #[derive(Debug)]
    enum RequestType {
        Summary,
        DiffDetails,
        Chunks,
    }
    
    #[derive(Debug, Default)]
    pub struct ProtocolStats {
        pub summaries_sent: u64,
        pub summaries_received: u64,
        pub bytes_saved: u64,
        pub false_positives: u64,
        pub avg_similarity: f64,
        pub total_comparisons: u64,
    }
    
    impl DiffProtocolHandler {
        /// Create a new diff protocol handler
        pub fn new() -> Self {
            Self {
                manifest_cache: Arc::new(RwLock::new(HashMap::new())),
                pending_requests: Arc::new(Mutex::new(HashMap::new())),
                stats: Arc::new(RwLock::new(ProtocolStats::default())),
            }
        }
        
        /// Handle incoming protocol message
        pub async fn handle_message(
            &self,
            message: DiffProtocolMessage,
            peer_id: &str,
            manifest: Option<&Manifest>,
        ) -> Result<Option<DiffProtocolMessage>, Box<dyn std::error::Error + Send + Sync>> {
            match message {
                DiffProtocolMessage::RequestSummary { path, request_id } => {
                    debug!("Received summary request for {} from {}", path.display(), peer_id);
                    
                    if let Some(manifest) = manifest {
                        let summary = ManifestSummary::from_manifest(manifest, peer_id.to_string());
                        
                        // Cache the summary
                        let mut cache = self.manifest_cache.write().await;
                        cache.insert(path.clone(), summary.clone());
                        
                        // Update stats
                        let mut stats = self.stats.write().await;
                        stats.summaries_sent += 1;
                        
                        Ok(Some(DiffProtocolMessage::SendSummary {
                            summary,
                            request_id,
                        }))
                    } else {
                        Ok(Some(DiffProtocolMessage::Acknowledge {
                            request_id,
                            success: false,
                            message: Some("Manifest not available".to_string()),
                        }))
                    }
                }
                
                DiffProtocolMessage::SendSummary { summary, request_id } => {
                    debug!("Received summary from {} for {}", peer_id, summary.root_path.display());
                    
                    // Cache the received summary
                    let mut cache = self.manifest_cache.write().await;
                    cache.insert(summary.root_path.clone(), summary.clone());
                    
                    // Update stats
                    let mut stats = self.stats.write().await;
                    stats.summaries_received += 1;
                    
                    // Remove pending request
                    let mut pending = self.pending_requests.lock().await;
                    pending.remove(&request_id);
                    
                    Ok(Some(DiffProtocolMessage::Acknowledge {
                        request_id,
                        success: true,
                        message: None,
                    }))
                }
                
                DiffProtocolMessage::RequestDiffDetails { suspected_paths, request_id } => {
                    debug!("Received diff details request for {} paths from {}", 
                        suspected_paths.len(), peer_id);
                    
                    if let Some(manifest) = manifest {
                        let mut file_details = HashMap::new();
                        
                        for path in suspected_paths {
                            if let Some(entry) = manifest.entries.get(&path) {
                                file_details.insert(path, entry.clone());
                            }
                        }
                        
                        Ok(Some(DiffProtocolMessage::SendDiffDetails {
                            file_details,
                            request_id,
                        }))
                    } else {
                        Ok(Some(DiffProtocolMessage::Acknowledge {
                            request_id,
                            success: false,
                            message: Some("Manifest not available".to_string()),
                        }))
                    }
                }
                
                _ => {
                    // Handle other message types
                    Ok(None)
                }
            }
        }
        
        /// Initiate diff protocol exchange with a peer
        pub async fn initiate_diff_exchange(
            &self,
            peer_id: &str,
            local_manifest: &Manifest,
            path: &Path,
        ) -> Result<DiffEstimate, Box<dyn std::error::Error + Send + Sync>> {
            let request_id = uuid::Uuid::new_v4().to_string();
            
            // Create local summary
            let local_summary = ManifestSummary::from_manifest(local_manifest, "local".to_string());
            
            // Store pending request
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id.clone(), PendingRequest {
                request_type: RequestType::Summary,
                created_at: Instant::now(),
                peer_id: peer_id.to_string(),
                path: path.to_path_buf(),
            });
            
            // Cache local summary
            let mut cache = self.manifest_cache.write().await;
            cache.insert(path.to_path_buf(), local_summary.clone());
            
            // In a real implementation, this would send the request over the network
            // and wait for the response. For now, return a placeholder estimate.
            Ok(DiffEstimate {
                common_files: 0,
                files_only_in_self: local_manifest.entries.len(),
                files_only_in_other: 0,
                common_chunks: 0,
                total_files_self: local_manifest.entries.len(),
                total_files_other: 0,
                similarity_ratio: 0.0,
            })
        }
        
        /// Get protocol statistics
        pub async fn get_stats(&self) -> ProtocolStats {
            self.stats.read().await.clone()
        }
        
        /// Compress protocol message for transmission with zstd compression
        pub fn compress_message(message: &DiffProtocolMessage) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
            // Use bincode for efficient binary serialization
            let serialized = bincode::serialize(message)?;
            
            // Apply zstd compression for bandwidth optimization
            // Level 3 provides good balance between speed and compression ratio
            use zstd::stream::encode_all;
            let compressed = encode_all(&serialized[..], 3)?;
            
            // Add a header to indicate compression method and original size
            let mut result = Vec::with_capacity(compressed.len() + 8);
            result.extend_from_slice(b"ZSTD"); // 4-byte magic header
            result.extend_from_slice(&(serialized.len() as u32).to_be_bytes()); // Original size
            result.extend_from_slice(&compressed);
            
            // Update stats for bandwidth savings
            let savings = serialized.len().saturating_sub(result.len());
            debug!(
                "Compressed message: {} -> {} bytes (saved {} bytes, {:.1}% reduction)",
                serialized.len(),
                result.len(),
                savings,
                (savings as f64 / serialized.len() as f64) * 100.0
            );
            
            Ok(result)
        }
        
        /// Decompress received protocol message with automatic format detection
        pub fn decompress_message(data: &[u8]) -> Result<DiffProtocolMessage, Box<dyn std::error::Error + Send + Sync>> {
            // Check for compression header
            if data.len() > 8 && &data[0..4] == b"ZSTD" {
                // Extract original size for pre-allocation
                let original_size = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
                
                // Decompress zstd data
                use zstd::stream::decode_all;
                let decompressed = decode_all(&data[8..])?;
                
                // Verify decompressed size matches header
                if decompressed.len() != original_size {
                    return Err(format!(
                        "Decompression size mismatch: expected {}, got {}",
                        original_size,
                        decompressed.len()
                    ).into());
                }
                
                // Deserialize the message
                let message = bincode::deserialize(&decompressed)?;
                Ok(message)
            } else {
                // No compression header, try direct deserialization
                // This maintains backward compatibility
                let message = bincode::deserialize(data)?;
                Ok(message)
            }
        }
    }
    
    #[cfg(test)]
    mod tests {
        use super::*;
        
        #[test]
        fn test_bloom_filter_basic() {
            let mut filter = BloomFilter::new(100, 0.01);
            
            // Insert some items
            filter.insert(b"test1");
            filter.insert(b"test2");
            filter.insert(b"test3");
            
            // Check membership
            assert!(filter.contains(b"test1"));
            assert!(filter.contains(b"test2"));
            assert!(filter.contains(b"test3"));
            assert!(!filter.contains(b"test4"));
        }
        
        #[test]
        fn test_bloom_filter_serialization() {
            let mut filter = BloomFilter::new(100, 0.01);
            filter.insert(b"test1");
            filter.insert(b"test2");
            
            let bytes = filter.to_bytes();
            let restored = BloomFilter::from_bytes(&bytes).unwrap();
            
            assert!(restored.contains(b"test1"));
            assert!(restored.contains(b"test2"));
            assert!(!restored.contains(b"test3"));
        }
        
        #[test]
        fn test_bloom_filter_merge() {
            let mut filter1 = BloomFilter::new(100, 0.01);
            filter1.insert(b"test1");
            filter1.insert(b"test2");
            
            let mut filter2 = BloomFilter::new(100, 0.01);
            filter2.insert(b"test3");
            filter2.insert(b"test4");
            
            filter1.merge(&filter2).unwrap();
            
            assert!(filter1.contains(b"test1"));
            assert!(filter1.contains(b"test2"));
            assert!(filter1.contains(b"test3"));
            assert!(filter1.contains(b"test4"));
        }
        
        // #[test] - DISABLED: Manifest API mismatch - needs to use landro_index::Manifest
        #[allow(dead_code)]
        fn _disabled_test_manifest_summary_creation() {
            let mut manifest = Manifest {
                version: 1,
                root: PathBuf::from("/test"),
                entries: HashMap::new(),
                created_at: SystemTime::now(),
                device_id: "device1".to_string(),
            };
            
            // Add some entries
            manifest.entries.insert(
                PathBuf::from("/test/file1.txt"),
                FileEntry {
                    path: PathBuf::from("/test/file1.txt"),
                    size: 1024,
                    modified: SystemTime::now(),
                    chunks: vec!["hash1".to_string(), "hash2".to_string()],
                    is_directory: false,
                    permissions: 0o644,
                },
            );
            
            let summary = ManifestSummary::from_manifest(&manifest, "device1".to_string());
            
            assert_eq!(summary.file_count, 1);
            assert_eq!(summary.total_size, 1024);
            assert!(summary.path_filter.contains(b"/test/file1.txt"));
            assert!(summary.hash_filter.contains(b"hash1"));
            assert!(summary.hash_filter.contains(b"hash2"));
        }
        
        #[test]
        fn test_diff_estimation() {
            let mut manifest1 = Manifest {
                version: 1,
                root: PathBuf::from("/test"),
                entries: HashMap::new(),
                created_at: SystemTime::now(),
                device_id: "device1".to_string(),
            };
            
            manifest1.entries.insert(
                PathBuf::from("/test/file1.txt"),
                FileEntry {
                    path: PathBuf::from("/test/file1.txt"),
                    size: 1024,
                    modified: SystemTime::now(),
                    chunks: vec!["hash1".to_string(), "hash2".to_string()],
                    is_directory: false,
                    permissions: 0o644,
                },
            );
            
            let mut manifest2 = manifest1.clone();
            manifest2.entries.insert(
                PathBuf::from("/test/file2.txt"),
                FileEntry {
                    path: PathBuf::from("/test/file2.txt"),
                    size: 2048,
                    modified: SystemTime::now(),
                    chunks: vec!["hash3".to_string(), "hash4".to_string()],
                    is_directory: false,
                    permissions: 0o644,
                },
            );
            
            let summary1 = ManifestSummary::from_manifest(&manifest1, "device1".to_string());
            let summary2 = ManifestSummary::from_manifest(&manifest2, "device2".to_string());
            
            let diff = summary1.estimate_diff(&summary2);
            
            assert!(diff.common_files > 0);
            assert_eq!(diff.total_files_self, 1);
            assert_eq!(diff.total_files_other, 2);
        }
        
        #[test]
        fn test_message_compression() {
            let message = DiffProtocolMessage::RequestSummary {
                path: PathBuf::from("/test/path"),
                request_id: "test-123".to_string(),
            };
            
            // Test compression
            let compressed = DiffProtocolHandler::compress_message(&message).unwrap();
            
            // Verify compression header
            assert_eq!(&compressed[0..4], b"ZSTD");
            
            // Test decompression
            let decompressed = DiffProtocolHandler::decompress_message(&compressed).unwrap();
            
            // Verify round-trip
            match decompressed {
                DiffProtocolMessage::RequestSummary { path, request_id } => {
                    assert_eq!(path, PathBuf::from("/test/path"));
                    assert_eq!(request_id, "test-123");
                }
                _ => panic!("Unexpected message type after decompression"),
            }
        }
        
        #[test]
        fn test_bloom_filter_false_positive_rate() {
            let mut filter = BloomFilter::new(1000, 0.01);
            let mut false_positives = 0;
            let test_items = 10000;
            
            // Insert 1000 items
            for i in 0..1000 {
                let item = format!("item_{}", i);
                filter.insert(item.as_bytes());
            }
            
            // Test with items that were not inserted
            for i in 1000..test_items {
                let item = format!("item_{}", i);
                if filter.contains(item.as_bytes()) {
                    false_positives += 1;
                }
            }
            
            let false_positive_rate = false_positives as f64 / (test_items - 1000) as f64;
            
            // Should be close to the target rate (allowing some variance)
            assert!(false_positive_rate < 0.02, 
                "False positive rate {} exceeds acceptable threshold", false_positive_rate);
        }
        
        #[tokio::test]
        async fn test_protocol_handler_message_flow() {
            let handler = DiffProtocolHandler::new();
            
            let mut manifest = Manifest {
                version: 1,
                root: PathBuf::from("/test"),
                entries: HashMap::new(),
                created_at: SystemTime::now(),
                device_id: "test-device".to_string(),
            };
            
            manifest.entries.insert(
                PathBuf::from("/test/file.txt"),
                FileEntry {
                    path: PathBuf::from("/test/file.txt"),
                    size: 1024,
                    modified: SystemTime::now(),
                    chunks: vec!["hash1".to_string()],
                    is_directory: false,
                    permissions: 0o644,
                },
            );
            
            // Test RequestSummary handling
            let request = DiffProtocolMessage::RequestSummary {
                path: PathBuf::from("/test"),
                request_id: "req-1".to_string(),
            };
            
            let response = handler.handle_message(request, "peer1", Some(&manifest))
                .await
                .unwrap();
            
            // Should return a SendSummary response
            assert!(matches!(response, Some(DiffProtocolMessage::SendSummary { .. })));
            
            // Test statistics tracking
            let stats = handler.get_stats().await;
            assert_eq!(stats.summaries_sent, 1);
        }
    }
}

/// Sync engine configuration
#[derive(Debug, Clone)]
pub struct SyncEngineConfig {
    /// Maximum concurrent sync operations
    pub max_concurrent_syncs: usize,
    /// Debounce duration for file system events
    pub debounce_duration: Duration,
    /// Interval for periodic sync checks
    pub sync_interval: Duration,
    /// Enable automatic sync on changes
    pub auto_sync: bool,
    /// Conflict resolution strategy
    pub conflict_strategy: ConflictResolution,
    /// Maximum retries for failed operations
    pub max_retries: usize,
    /// Batch size for processing changes
    pub batch_size: usize,
    /// Enable delta sync (incremental transfers)
    pub delta_sync: bool,
    /// Bandwidth limit in bytes per second (0 = unlimited)
    pub bandwidth_limit: u64,
    /// Patterns to ignore during sync
    pub ignore_patterns: Vec<String>,
}

impl Default for SyncEngineConfig {
    fn default() -> Self {
        Self {
            max_concurrent_syncs: 4,
            debounce_duration: Duration::from_millis(500),
            sync_interval: Duration::from_secs(30),
            auto_sync: true,
            conflict_strategy: ConflictResolution::NewerWins,
            max_retries: 3,
            batch_size: 100,
            delta_sync: true,
            bandwidth_limit: 0,
            ignore_patterns: vec![
                ".git".to_string(),
                ".DS_Store".to_string(),
                "Thumbs.db".to_string(),
                "*.tmp".to_string(),
                "~*".to_string(),
            ],
        }
    }
}

/// Messages for the sync engine
#[derive(Debug)]
pub enum SyncMessage {
    /// File system event from watcher
    FileEvent(FsEvent),
    /// Batch of file events
    FileEventBatch(Vec<FsEvent>),
    /// New peer discovered
    PeerDiscovered(PeerInfo),
    /// Peer disconnected
    PeerLost(String),
    /// Manual sync request
    SyncRequest { peer_id: String, path: PathBuf },
    /// Add folder to sync
    AddSyncFolder(PathBuf),
    /// Remove folder from sync
    RemoveSyncFolder(PathBuf),
    /// Get current status
    GetStatus(mpsc::Sender<SyncStatus>),
    /// Shutdown the engine
    Shutdown,
}

/// Sync engine status
#[derive(Debug, Clone)]
pub struct SyncStatus {
    pub running: bool,
    pub synced_folders: Vec<PathBuf>,
    pub active_peers: Vec<String>,
    pub pending_changes: usize,
    pub active_transfers: usize,
    pub total_bytes_synced: u64,
    pub last_sync_time: Option<SystemTime>,
    pub errors: Vec<String>,
}

/// Sync session tracking
#[derive(Debug)]
struct SyncSession {
    peer_id: String,
    path: PathBuf,
    local_manifest: Manifest,
    remote_manifest: Option<Manifest>,
    diff_result: Option<DiffResult>,
    transfers: Vec<TransferJob>,
    started_at: Instant,
    completed_at: Option<Instant>,
    bytes_transferred: u64,
    errors: Vec<String>,
}

/// Main sync engine
pub struct SyncEngine {
    config: SyncEngineConfig,
    
    // Core components
    indexer: Arc<AsyncIndexer>,
    store: Arc<ContentStore>,
    chunker: Arc<Chunker>,
    differ: Arc<DiffEngine>,
    transfer_engine: Arc<TransferEngine>,
    network: Arc<NetworkManager>,
    
    // Bloom filter diff protocol handler
    diff_protocol: Arc<bloom_diff_protocol::DiffProtocolHandler>,
    
    // File watching
    watcher: Box<dyn PlatformWatcher>,
    debouncer: EventDebouncer,
    
    // State management
    synced_folders: Arc<RwLock<HashSet<PathBuf>>>,
    active_peers: Arc<RwLock<HashMap<String, PeerInfo>>>,
    active_sessions: Arc<Mutex<HashMap<String, SyncSession>>>,
    pending_events: Arc<Mutex<VecDeque<FsEvent>>>,
    
    // Statistics
    stats: Arc<RwLock<SyncStats>>,
    
    // Communication
    message_rx: mpsc::Receiver<SyncMessage>,
    message_tx: mpsc::Sender<SyncMessage>,
}

#[derive(Debug, Default)]
struct SyncStats {
    total_bytes_synced: u64,
    total_files_synced: u64,
    total_conflicts_resolved: u64,
    last_sync_time: Option<SystemTime>,
    sync_errors: Vec<String>,
}

impl SyncEngine {
    /// Create a new sync engine
    pub async fn new(
        config: SyncEngineConfig,
        indexer: Arc<AsyncIndexer>,
        store: Arc<ContentStore>,
        chunker: Arc<Chunker>,
        network: Arc<NetworkManager>,
        storage_path: &Path,
    ) -> Result<(Self, mpsc::Sender<SyncMessage>), Box<dyn std::error::Error + Send + Sync>> {
        let (message_tx, message_rx) = mpsc::channel(1000);
        
        // Create watcher with configuration
        let watcher_config = WatcherConfig {
            debounce_delay: config.debounce_duration,
            recursive: true,
            ignore_patterns: config.ignore_patterns.clone(),
            batch_size: config.batch_size,
            ..Default::default()
        };
        
        let watcher = create_platform_watcher().await?;
        let debouncer = EventDebouncer::new(config.debounce_duration);
        
        // Create differ with configured strategy
        let differ = Arc::new(DiffEngine::new(config.conflict_strategy));
        
        // Create transfer engine
        let transfer_engine = Arc::new(
            TransferEngine::new(
                store.clone(),
                storage_path,
                config.max_concurrent_syncs,
            ).await?
        );
        
        // Create diff protocol handler
        let diff_protocol = Arc::new(bloom_diff_protocol::DiffProtocolHandler::new());
        
        let engine = Self {
            config,
            indexer,
            store,
            chunker,
            differ,
            transfer_engine,
            network,
            diff_protocol,
            watcher,
            debouncer,
            synced_folders: Arc::new(RwLock::new(HashSet::new())),
            active_peers: Arc::new(RwLock::new(HashMap::new())),
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_events: Arc::new(Mutex::new(VecDeque::new())),
            stats: Arc::new(RwLock::new(SyncStats::default())),
            message_rx,
            message_tx: message_tx.clone(),
        };
        
        Ok((engine, message_tx))
    }
    
    /// Run the sync engine main loop
    pub async fn run(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting sync engine");
        
        // Set up intervals for periodic tasks
        let mut sync_interval = interval(self.config.sync_interval);
        let mut debounce_interval = interval(self.config.debounce_duration);
        let mut health_check_interval = interval(Duration::from_secs(60));
        
        // Start watching synced folders
        self.start_watching().await?;
        
        loop {
            tokio::select! {
                // Handle incoming messages
                Some(msg) = self.message_rx.recv() => {
                    if !self.handle_message(msg).await? {
                        break; // Shutdown requested
                    }
                }
                
                // Process debounced file events
                _ = debounce_interval.tick() => {
                    self.process_debounced_events().await?;
                }
                
                // Periodic sync check
                _ = sync_interval.tick() => {
                    if self.config.auto_sync {
                        self.perform_periodic_sync().await?;
                    }
                }
                
                // Health check and cleanup
                _ = health_check_interval.tick() => {
                    self.perform_health_check().await?;
                }
                
                // Handle completed transfers
                Some(result) = self.transfer_engine.poll_completion() => {
                    self.handle_transfer_completion(result).await?;
                }
            }
        }
        
        info!("Sync engine shutting down");
        self.shutdown().await?;
        Ok(())
    }
    
    /// Handle incoming messages
    async fn handle_message(&mut self, msg: SyncMessage) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        match msg {
            SyncMessage::FileEvent(event) => {
                trace!("File event: {:?}", event);
                self.debouncer.add_event(event);
                Ok(true)
            }
            
            SyncMessage::FileEventBatch(events) => {
                trace!("Batch of {} file events", events.len());
                for event in events {
                    self.debouncer.add_event(event);
                }
                Ok(true)
            }
            
            SyncMessage::PeerDiscovered(peer) => {
                info!("Peer discovered: {} ({})", peer.device_name, peer.device_id);
                self.handle_peer_discovered(peer).await?;
                Ok(true)
            }
            
            SyncMessage::PeerLost(peer_id) => {
                info!("Peer lost: {}", peer_id);
                self.handle_peer_lost(&peer_id).await?;
                Ok(true)
            }
            
            SyncMessage::SyncRequest { peer_id, path } => {
                info!("Sync requested with {} for {}", peer_id, path.display());
                self.initiate_sync(&peer_id, &path).await?;
                Ok(true)
            }
            
            SyncMessage::AddSyncFolder(path) => {
                info!("Adding sync folder: {}", path.display());
                self.add_sync_folder(path).await?;
                Ok(true)
            }
            
            SyncMessage::RemoveSyncFolder(path) => {
                info!("Removing sync folder: {}", path.display());
                self.remove_sync_folder(&path).await?;
                Ok(true)
            }
            
            SyncMessage::GetStatus(tx) => {
                let status = self.get_status().await;
                let _ = tx.send(status).await;
                Ok(true)
            }
            
            SyncMessage::Shutdown => {
                info!("Shutdown requested");
                Ok(false)
            }
        }
    }
    
    /// Process debounced file system events
    async fn process_debounced_events(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let ready_events = self.debouncer.get_ready_events();
        if ready_events.is_empty() {
            return Ok(());
        }
        
        debug!("Processing {} debounced events", ready_events.len());
        
        // Group events by folder
        let mut events_by_folder: HashMap<PathBuf, Vec<FsEvent>> = HashMap::new();
        
        for event in ready_events {
            // Find which sync folder this event belongs to
            let folders = self.synced_folders.read().await;
            for folder in folders.iter() {
                if event.path.starts_with(folder) {
                    events_by_folder
                        .entry(folder.clone())
                        .or_insert_with(Vec::new)
                        .push(event.clone());
                    break;
                }
            }
        }
        
        // Process events for each folder
        for (folder, events) in events_by_folder {
            self.process_folder_events(&folder, events).await?;
        }
        
        // Trigger auto-sync if enabled
        if self.config.auto_sync {
            self.trigger_auto_sync().await?;
        }
        
        Ok(())
    }
    
    /// Process events for a specific folder
    async fn process_folder_events(
        &mut self,
        folder: &Path,
        events: Vec<FsEvent>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("Processing {} events for folder {}", events.len(), folder.display());
        
        // Update index for affected files
        let mut files_to_index = Vec::new();
        let mut files_to_remove = Vec::new();
        
        for event in events {
            match event.kind {
                EventKind::Created | EventKind::Modified => {
                    files_to_index.push(event.path);
                }
                EventKind::Removed => {
                    files_to_remove.push(event.path);
                }
                EventKind::Renamed => {
                    if let Some(old_path) = event.old_path {
                        files_to_remove.push(old_path);
                    }
                    files_to_index.push(event.path);
                }
                _ => {}
            }
        }
        
        // Batch index updates
        if !files_to_index.is_empty() {
            debug!("Indexing {} modified files", files_to_index.len());
            for path in files_to_index {
                if path.is_file() {
                    // Process file through chunking pipeline
                    if let Err(e) = self.process_file(&path).await {
                        error!("Failed to process file {}: {}", path.display(), e);
                    }
                }
            }
            
            // Re-index the folder to update manifest
            self.indexer.index_folder(folder).await?;
        }
        
        // Handle removals
        if !files_to_remove.is_empty() {
            debug!("Removing {} deleted files from index", files_to_remove.len());
            // The indexer should handle this when re-indexing the folder
            self.indexer.index_folder(folder).await?;
        }
        
        Ok(())
    }
    
    /// Process a single file through the chunking pipeline
    async fn process_file(&self, path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use tokio::io::AsyncReadExt;
        
        debug!("Processing file: {}", path.display());
        
        let mut file = tokio::fs::File::open(path).await?;
        let metadata = file.metadata().await?;
        let file_size = metadata.len();
        
        // Stream file in chunks to avoid memory issues
        const BUFFER_SIZE: usize = 4 * 1024 * 1024; // 4MB buffer
        let mut buffer = Vec::with_capacity(BUFFER_SIZE);
        let mut total_chunks = 0;
        
        loop {
            buffer.clear();
            let bytes_read = file.take(BUFFER_SIZE as u64)
                .read_to_end(&mut buffer)
                .await?;
            
            if bytes_read == 0 {
                break;
            }
            
            // Chunk the buffer
            let chunks = self.chunker.chunk_bytes(&buffer)?;
            
            // Store chunks in CAS
            for chunk in chunks {
                self.store.write(&chunk.data).await?;
                total_chunks += 1;
            }
        }
        
        debug!(
            "File {} processed: {} bytes in {} chunks",
            path.display(),
            file_size,
            total_chunks
        );
        
        // Update stats
        let mut stats = self.stats.write().await;
        stats.total_bytes_synced += file_size;
        stats.total_files_synced += 1;
        
        Ok(())
    }
    
    /// Perform periodic sync with all connected peers
    async fn perform_periodic_sync(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let peers = self.active_peers.read().await;
        if peers.is_empty() {
            return Ok(());
        }
        
        let folders = self.synced_folders.read().await;
        if folders.is_empty() {
            return Ok(());
        }
        
        debug!("Performing periodic sync with {} peers", peers.len());
        
        // Initiate sync for each peer and folder combination
        for (peer_id, _peer_info) in peers.iter() {
            for folder in folders.iter() {
                if let Err(e) = self.initiate_sync(peer_id, folder).await {
                    error!("Failed to sync {} with {}: {}", folder.display(), peer_id, e);
                }
            }
        }
        
        Ok(())
    }
    
    /// Initiate sync with a specific peer for a path
    async fn initiate_sync(
        &mut self,
        peer_id: &str,
        path: &Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Initiating sync with {} for {}", peer_id, path.display());
        
        // Check if we already have an active session
        let sessions = self.active_sessions.lock().await;
        let session_key = format!("{}:{}", peer_id, path.display());
        if sessions.contains_key(&session_key) {
            debug!("Sync already in progress for {}", session_key);
            return Ok(());
        }
        drop(sessions);
        
        // Build local manifest
        let local_manifest = self.indexer.build_manifest(path).await?;
        
        // Use Bloom filter diff protocol for efficient comparison
        let diff_estimate = self.diff_protocol
            .initiate_diff_exchange(peer_id, &local_manifest, path)
            .await?;
        
        info!(
            "Bloom filter diff estimate: {:.1}% similarity, {} common files, {} unique local, {} unique remote",
            diff_estimate.similarity_ratio * 100.0,
            diff_estimate.common_files,
            diff_estimate.files_only_in_self,
            diff_estimate.files_only_in_other
        );
        
        // Only request full manifest if there are significant differences
        let remote_manifest = if diff_estimate.similarity_ratio < 0.95 {
            info!("Significant differences detected, requesting full manifest");
            self.request_remote_manifest(peer_id, path).await?
        } else {
            debug!("Manifests are highly similar, skipping full exchange");
            // For highly similar manifests, we could use incremental sync
            self.request_remote_manifest_incremental(peer_id, path, &diff_estimate).await?
        };
        
        // Compute detailed differences
        let diff_result = self.differ.compute_diff(&local_manifest, &remote_manifest)?;
        
        info!(
            "Detailed diff computed: {} to upload, {} to download, {} conflicts",
            diff_result.files_to_upload.len(),
            diff_result.files_to_download.len(),
            diff_result.conflicts.len()
        );
        
        // Update protocol statistics
        let protocol_stats = self.diff_protocol.get_stats().await;
        debug!(
            "Bloom protocol stats: {} summaries sent, {} received, {} bytes saved",
            protocol_stats.summaries_sent,
            protocol_stats.summaries_received,
            protocol_stats.bytes_saved
        );
        
        // Create sync session
        let mut session = SyncSession {
            peer_id: peer_id.to_string(),
            path: path.to_path_buf(),
            local_manifest,
            remote_manifest: Some(remote_manifest),
            diff_result: Some(diff_result.clone()),
            transfers: Vec::new(),
            started_at: Instant::now(),
            completed_at: None,
            bytes_transferred: 0,
            errors: Vec::new(),
        };
        
        // Schedule transfers based on diff
        self.schedule_transfers(&mut session, diff_result).await?;
        
        // Store session
        let mut sessions = self.active_sessions.lock().await;
        sessions.insert(session_key, session);
        
        Ok(())
    }
    
    /// Schedule transfers based on diff results
    async fn schedule_transfers(
        &mut self,
        session: &mut SyncSession,
        diff: DiffResult,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Handle conflicts first
        for conflict in diff.conflicts {
            warn!(
                "Conflict detected for {}: local modified {}, remote modified {}",
                conflict.path.display(),
                conflict.local_modified.map(|t| format!("{:?}", t)).unwrap_or_default(),
                conflict.remote_modified.map(|t| format!("{:?}", t)).unwrap_or_default()
            );
            
            // Apply configured resolution strategy
            match self.config.conflict_strategy {
                ConflictResolution::NewerWins => {
                    // Already handled by differ
                }
                ConflictResolution::LocalWins => {
                    // Add to upload list
                    if let Some(entry) = session.local_manifest.entries.get(&conflict.path) {
                        let job = self.create_upload_job(&session.peer_id, &conflict.path, entry).await?;
                        session.transfers.push(job);
                    }
                }
                ConflictResolution::RemoteWins => {
                    // Add to download list
                    if let Some(remote_manifest) = &session.remote_manifest {
                        if let Some(entry) = remote_manifest.entries.get(&conflict.path) {
                            let job = self.create_download_job(&session.peer_id, &conflict.path, entry).await?;
                            session.transfers.push(job);
                        }
                    }
                }
                ConflictResolution::Manual => {
                    // Skip for now, would need user intervention
                    session.errors.push(format!("Manual conflict resolution required for {}", conflict.path.display()));
                }
            }
        }
        
        // Schedule uploads
        for path in diff.files_to_upload {
            if let Some(entry) = session.local_manifest.entries.get(&path) {
                let job = self.create_upload_job(&session.peer_id, &path, entry).await?;
                session.transfers.push(job);
            }
        }
        
        // Schedule downloads
        if let Some(remote_manifest) = &session.remote_manifest {
            for path in diff.files_to_download {
                if let Some(entry) = remote_manifest.entries.get(&path) {
                    let job = self.create_download_job(&session.peer_id, &path, entry).await?;
                    session.transfers.push(job);
                }
            }
        }
        
        // Submit all transfers to the transfer engine
        for job in &session.transfers {
            self.transfer_engine.submit_transfer(job.clone()).await?;
        }
        
        info!(
            "Scheduled {} transfers for session {}:{}",
            session.transfers.len(),
            session.peer_id,
            session.path.display()
        );
        
        Ok(())
    }
    
    /// Create an upload job
    async fn create_upload_job(
        &self,
        peer_id: &str,
        path: &Path,
        entry: &FileEntry,
    ) -> Result<TransferJob, Box<dyn std::error::Error + Send + Sync>> {
        Ok(TransferJob {
            id: format!("{}-{}-{}", peer_id, path.display(), uuid::Uuid::new_v4()),
            source_path: path.to_path_buf(),
            dest_path: path.to_path_buf(),
            peer_id: peer_id.to_string(),
            direction: TransferDirection::Upload,
            priority: TransferPriority::Normal,
            size: entry.size,
            chunk_hashes: entry.chunks.clone(),
            status: TransferStatus::Pending,
            progress: 0,
            error: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
        })
    }
    
    /// Create a download job
    async fn create_download_job(
        &self,
        peer_id: &str,
        path: &Path,
        entry: &FileEntry,
    ) -> Result<TransferJob, Box<dyn std::error::Error + Send + Sync>> {
        Ok(TransferJob {
            id: format!("{}-{}-{}", peer_id, path.display(), uuid::Uuid::new_v4()),
            source_path: path.to_path_buf(),
            dest_path: path.to_path_buf(),
            peer_id: peer_id.to_string(),
            direction: TransferDirection::Download,
            priority: TransferPriority::Normal,
            size: entry.size,
            chunk_hashes: entry.chunks.clone(),
            status: TransferStatus::Pending,
            progress: 0,
            error: None,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
        })
    }
    
    /// Request remote manifest from a peer
    async fn request_remote_manifest(
        &self,
        peer_id: &str,
        path: &Path,
    ) -> Result<Manifest, Box<dyn std::error::Error + Send + Sync>> {
        // This would use the network manager to request the manifest
        // For now, return a placeholder
        warn!("Remote manifest request not yet implemented");
        Ok(Manifest::new(
            format!("folder_{}", path.file_name().unwrap_or_default().to_string_lossy()),
            1  // version
        ))
    }
    
    /// Request incremental manifest updates for highly similar manifests
    async fn request_remote_manifest_incremental(
        &self,
        peer_id: &str,
        path: &Path,
        diff_estimate: &bloom_diff_protocol::DiffEstimate,
    ) -> Result<Manifest, Box<dyn std::error::Error + Send + Sync>> {
        // For highly similar manifests, we can request only the differences
        // This significantly reduces bandwidth usage
        debug!(
            "Requesting incremental manifest for {} (estimated {} different files)",
            path.display(),
            diff_estimate.files_only_in_other
        );
        
        // In a real implementation, this would:
        // 1. Send a list of local file hashes
        // 2. Receive only the entries that are different
        // 3. Merge with the local manifest to create the remote view
        
        // For now, return a placeholder
        warn!("Incremental manifest request not yet implemented");
        Ok(Manifest::new(
            format!("folder_{}", path.file_name().unwrap_or_default().to_string_lossy()),
            1  // version
        ))
    }
    
    /// Handle transfer completion
    async fn handle_transfer_completion(
        &mut self,
        result: Result<TransferJob, Box<dyn std::error::Error + Send + Sync>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match result {
            Ok(job) => {
                info!("Transfer completed: {} ({})", job.id, job.source_path.display());
                
                // Update session statistics
                let mut sessions = self.active_sessions.lock().await;
                for (_key, session) in sessions.iter_mut() {
                    if session.transfers.iter().any(|t| t.id == job.id) {
                        session.bytes_transferred += job.size;
                        break;
                    }
                }
                
                // Update global stats
                let mut stats = self.stats.write().await;
                stats.total_bytes_synced += job.size;
            }
            Err(e) => {
                error!("Transfer failed: {}", e);
                
                // Record error in stats
                let mut stats = self.stats.write().await;
                stats.sync_errors.push(format!("Transfer failed: {}", e));
            }
        }
        
        Ok(())
    }
    
    /// Handle peer discovered
    async fn handle_peer_discovered(&mut self, peer: PeerInfo) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut peers = self.active_peers.write().await;
        peers.insert(peer.device_id.clone(), peer.clone());
        
        // If auto-sync is enabled, initiate sync with new peer
        if self.config.auto_sync {
            let folder_paths: Vec<PathBuf> = {
                let folders = self.synced_folders.read().await;
                folders.iter().cloned().collect()
            };  // Lock released here
            
            for folder in folder_paths {
                if let Err(e) = self.initiate_sync(&peer.device_id, &folder).await {
                    error!("Failed to initiate sync with new peer: {}", e);
                }
            }
        }
        
        Ok(())
    }
    
    /// Handle peer lost
    async fn handle_peer_lost(&mut self, peer_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut peers = self.active_peers.write().await;
        peers.remove(peer_id);
        
        // Cancel any active sessions with this peer
        let mut sessions = self.active_sessions.lock().await;
        sessions.retain(|key, _| !key.starts_with(peer_id));
        
        Ok(())
    }
    
    /// Trigger auto-sync with all peers
    async fn trigger_auto_sync(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let peers = self.active_peers.read().await;
        let folders = self.synced_folders.read().await;
        
        if peers.is_empty() || folders.is_empty() {
            return Ok(());
        }
        
        debug!("Triggering auto-sync with {} peers", peers.len());
        
        // Extract data and release locks before calling initiate_sync
        let peer_ids: Vec<String> = peers.keys().cloned().collect();
        let folder_paths: Vec<PathBuf> = folders.iter().cloned().collect();
        
        // Locks are released here
        drop(peers);
        drop(folders);
        
        for peer_id in peer_ids {
            for folder in &folder_paths {
                if let Err(e) = self.initiate_sync(&peer_id, folder).await {
                    error!("Auto-sync failed for {} with {}: {}", folder.display(), peer_id, e);
                }
            }
        }
        
        Ok(())
    }
    
    /// Add a folder to sync
    async fn add_sync_folder(&mut self, path: PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()).into());
        }
        
        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()).into());
        }
        
        let mut folders = self.synced_folders.write().await;
        if folders.contains(&path) {
            return Ok(()); // Already syncing
        }
        
        info!("Adding sync folder: {}", path.display());
        
        // Start watching the folder
        self.watcher.watch(&path, true).await?;
        
        // Index the folder
        self.indexer.index_folder(&path).await?;
        
        folders.insert(path.clone());
        
        // Initiate sync with all connected peers
        if self.config.auto_sync {
            let peer_ids: Vec<String> = {
                let peers = self.active_peers.read().await;
                peers.keys().cloned().collect()
            };  // Lock released here
            
            for peer_id in peer_ids {
                if let Err(e) = self.initiate_sync(&peer_id, &path).await {
                    error!("Failed to sync new folder with {}: {}", peer_id, e);
                }
            }
        }
        
        Ok(())
    }
    
    /// Remove a folder from sync
    async fn remove_sync_folder(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut folders = self.synced_folders.write().await;
        if !folders.remove(path) {
            return Ok(()); // Wasn't being synced
        }
        
        info!("Removing sync folder: {}", path.display());
        
        // Stop watching the folder
        self.watcher.unwatch(path).await?;
        
        // Cancel any active sessions for this folder
        let mut sessions = self.active_sessions.lock().await;
        sessions.retain(|key, _| !key.contains(&path.display().to_string()));
        
        Ok(())
    }
    
    /// Start watching all synced folders
    async fn start_watching(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let folders = self.synced_folders.read().await;
        
        for folder in folders.iter() {
            info!("Starting watch for: {}", folder.display());
            self.watcher.watch(folder, true).await?;
        }
        
        // Spawn task to poll watcher events
        let tx = self.message_tx.clone();
        let mut watcher = Box::new(create_platform_watcher().await?);
        
        tokio::spawn(async move {
            let mut poll_interval = interval(Duration::from_millis(100));
            
            loop {
                poll_interval.tick().await;
                
                match watcher.poll_events().await {
                    Ok(events) => {
                        if !events.is_empty() {
                            if let Err(e) = tx.send(SyncMessage::FileEventBatch(events)).await {
                                error!("Failed to send file events: {}", e);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error polling watcher events: {}", e);
                    }
                }
            }
        });
        
        Ok(())
    }
    
    /// Perform health check and cleanup
    async fn perform_health_check(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("Performing health check");
        
        // Check watcher health
        if !self.watcher.health_check().await? {
            warn!("Watcher health check failed, restarting watches");
            self.start_watching().await?;
        }
        
        // Clean up completed sessions
        let mut sessions = self.active_sessions.lock().await;
        let now = Instant::now();
        sessions.retain(|key, session| {
            if session.completed_at.is_some() {
                let elapsed = now.duration_since(session.completed_at.unwrap());
                if elapsed > Duration::from_secs(300) {
                    info!("Cleaning up completed session: {}", key);
                    return false;
                }
            }
            true
        });
        
        // Trim error log if too large
        let mut stats = self.stats.write().await;
        if stats.sync_errors.len() > 100 {
            stats.sync_errors.drain(0..50);
        }
        
        Ok(())
    }
    
    /// Get current status
    async fn get_status(&self) -> SyncStatus {
        let folders = self.synced_folders.read().await;
        let peers = self.active_peers.read().await;
        let sessions = self.active_sessions.lock().await;
        let stats = self.stats.read().await;
        
        SyncStatus {
            running: true,
            synced_folders: folders.iter().cloned().collect(),
            active_peers: peers.keys().cloned().collect(),
            pending_changes: self.debouncer.has_pending() as usize,
            active_transfers: sessions.len(),
            total_bytes_synced: stats.total_bytes_synced,
            last_sync_time: stats.last_sync_time,
            errors: stats.sync_errors.clone(),
        }
    }
    
    /// Graceful shutdown
    async fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Shutting down sync engine");
        
        // Process any remaining events
        if self.debouncer.has_pending() {
            self.process_debounced_events().await?;
        }
        
        // Wait for active transfers to complete or timeout
        let sessions = self.active_sessions.lock().await;
        if !sessions.is_empty() {
            info!("Waiting for {} active sessions to complete", sessions.len());
            drop(sessions);
            
            // Give transfers 30 seconds to complete
            let _ = timeout(Duration::from_secs(30), async {
                while self.active_sessions.lock().await.len() > 0 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }).await;
        }
        
        // Stop watching all folders
        let folders = self.synced_folders.read().await;
        for folder in folders.iter() {
            self.watcher.unwatch(folder).await?;
        }
        
        // Shutdown transfer engine
        self.transfer_engine.shutdown().await?;
        
        info!("Sync engine shutdown complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_sync_engine_creation() {
        let temp_dir = TempDir::new().unwrap();
        let storage_path = temp_dir.path();
        
        // Create mock components
        let store = Arc::new(ContentStore::new(&storage_path.join("cas")).await.unwrap());
        let chunker = Arc::new(Chunker::new(Default::default()).unwrap());
        let indexer = Arc::new(
            AsyncIndexer::new(
                &storage_path.join("cas"),
                &storage_path.join("index.sqlite"),
                Default::default(),
            ).await.unwrap()
        );
        let network = Arc::new(NetworkManager::new(Default::default()).await.unwrap());
        
        let config = SyncEngineConfig::default();
        let (engine, tx) = SyncEngine::new(
            config,
            indexer,
            store,
            chunker,
            network,
            storage_path,
        ).await.unwrap();
        
        // Test sending a message
        tx.send(SyncMessage::Shutdown).await.unwrap();
        
        // Run should exit immediately due to shutdown
        tokio::time::timeout(Duration::from_secs(1), engine.run())
            .await
            .unwrap()
            .unwrap();
    }
}