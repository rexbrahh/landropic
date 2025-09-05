//! Manifest diff computation and synchronization protocol
//!
//! This module implements efficient manifest comparison algorithms for
//! determining what content needs to be synchronized between peers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use landro_index::manifest::{Manifest, ManifestEntry};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace};

use crate::errors::{Result, SyncError};

/// Type of change in a diff operation
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChangeType {
    /// File was added (exists in target but not source)
    Added,
    /// File was modified (different content hash)
    Modified,
    /// File was deleted (exists in source but not target)
    Deleted,
    /// File metadata changed but content is the same
    MetadataOnly,
}

/// Represents a single file change in a diff
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// Path of the file
    pub path: String,
    /// Type of change
    pub change_type: ChangeType,
    /// File entry from source manifest (for deleted/modified)
    pub source_entry: Option<ManifestEntry>,
    /// File entry from target manifest (for added/modified)
    pub target_entry: Option<ManifestEntry>,
    /// Chunks that need to be transferred (for added/modified)
    pub required_chunks: Vec<String>,
    /// Size of data to transfer
    pub transfer_size: u64,
}

impl FileChange {
    /// Check if this change requires data transfer
    pub fn requires_transfer(&self) -> bool {
        !self.required_chunks.is_empty()
    }

    /// Get priority for this change (higher = more important)
    pub fn priority(&self) -> u32 {
        match self.change_type {
            // Modifications are highest priority (user actively working)
            ChangeType::Modified => 100,
            // Small new files are high priority
            ChangeType::Added if self.transfer_size < 1_000_000 => 80,
            // Regular additions
            ChangeType::Added => 50,
            // Metadata changes are low priority
            ChangeType::MetadataOnly => 20,
            // Deletions don't require transfer
            ChangeType::Deleted => 10,
        }
    }
}

/// Result of a manifest diff operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    /// All file changes
    pub changes: Vec<FileChange>,
    /// Total bytes that need to be transferred
    pub total_transfer_size: u64,
    /// Unique chunks that need to be transferred
    pub required_chunks: HashSet<String>,
    /// Statistics about the diff
    pub stats: DiffStats,
    /// Merkle tree proof for partial sync (if applicable)
    pub merkle_proof: Option<MerkleProof>,
}

impl DiffResult {
    /// Check if there are any changes
    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Get changes of a specific type
    pub fn changes_by_type(&self, change_type: ChangeType) -> Vec<&FileChange> {
        self.changes
            .iter()
            .filter(|c| c.change_type == change_type)
            .collect()
    }

    /// Sort changes by priority for optimal transfer ordering
    pub fn prioritize_changes(&mut self) {
        self.changes.sort_by(|a, b| {
            b.priority().cmp(&a.priority())
                .then_with(|| a.transfer_size.cmp(&b.transfer_size))
        });
    }
}

/// Statistics about a diff operation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiffStats {
    pub files_added: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub files_metadata_only: usize,
    pub bytes_to_add: u64,
    pub bytes_to_modify: u64,
    pub bytes_to_delete: u64,
    pub total_chunks: usize,
    pub unique_chunks: usize,
}

/// Merkle tree proof for efficient manifest comparison
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    /// Root hash of the merkle tree
    pub root: Vec<u8>,
    /// Subtree hashes for partial verification
    pub subtree_hashes: Vec<Vec<u8>>,
    /// Depth of the tree
    pub depth: u32,
}

/// Computes diff between manifests with optimizations
pub struct DiffComputer {
    /// Cache of chunk availability (which chunks we already have)
    chunk_cache: Arc<HashSet<String>>,
    /// Whether to compute merkle proofs
    enable_merkle_proofs: bool,
    /// Maximum depth for merkle tree subdivision
    max_merkle_depth: u32,
}

impl DiffComputer {
    /// Create a new diff computer
    pub fn new(chunk_cache: Arc<HashSet<String>>) -> Self {
        Self {
            chunk_cache,
            enable_merkle_proofs: true,
            max_merkle_depth: 10,
        }
    }

    /// Compute diff between source and target manifests
    ///
    /// The diff shows what changes are needed to transform `source` into `target`:
    /// - Added: Files in target but not in source
    /// - Modified: Files in both with different content
    /// - Deleted: Files in source but not in target
    pub fn compute_diff(
        &self,
        source: &Manifest,
        target: &Manifest,
    ) -> Result<DiffResult> {
        info!(
            "Computing diff: {} -> {} (v{} -> v{})",
            source.folder_id, target.folder_id, source.version, target.version
        );

        let mut changes = Vec::new();
        let mut required_chunks = HashSet::new();
        let mut stats = DiffStats::default();

        // Create lookup maps for efficient comparison
        let source_map: HashMap<_, _> = source
            .files
            .iter()
            .map(|f| (f.path.clone(), f))
            .collect();

        let target_map: HashMap<_, _> = target
            .files
            .iter()
            .map(|f| (f.path.clone(), f))
            .collect();

        // Find added and modified files
        for (path, target_entry) in &target_map {
            match source_map.get(path) {
                None => {
                    // File added
                    let required = self.find_required_chunks(target_entry);
                    let transfer_size = self.calculate_transfer_size(&required);
                    
                    stats.files_added += 1;
                    stats.bytes_to_add += target_entry.size;
                    required_chunks.extend(required.iter().cloned());

                    changes.push(FileChange {
                        path: path.clone(),
                        change_type: ChangeType::Added,
                        source_entry: None,
                        target_entry: Some((*target_entry).clone()),
                        required_chunks: required,
                        transfer_size,
                    });
                }
                Some(source_entry) => {
                    if source_entry.content_hash != target_entry.content_hash {
                        // File modified - compute chunk-level diff
                        let required = self.compute_chunk_diff(source_entry, target_entry);
                        let transfer_size = self.calculate_transfer_size(&required);

                        if required.is_empty() && source_entry.size == target_entry.size {
                            // Only metadata changed
                            stats.files_metadata_only += 1;
                            changes.push(FileChange {
                                path: path.clone(),
                                change_type: ChangeType::MetadataOnly,
                                source_entry: Some((*source_entry).clone()),
                                target_entry: Some((*target_entry).clone()),
                                required_chunks: vec![],
                                transfer_size: 0,
                            });
                        } else {
                            // Content changed
                            stats.files_modified += 1;
                            stats.bytes_to_modify += target_entry.size;
                            required_chunks.extend(required.iter().cloned());

                            changes.push(FileChange {
                                path: path.clone(),
                                change_type: ChangeType::Modified,
                                source_entry: Some((*source_entry).clone()),
                                target_entry: Some((*target_entry).clone()),
                                required_chunks: required,
                                transfer_size,
                            });
                        }
                    }
                    // else: Files are identical, no change needed
                }
            }
        }

        // Find deleted files
        for (path, source_entry) in &source_map {
            if !target_map.contains_key(path) {
                stats.files_deleted += 1;
                stats.bytes_to_delete += source_entry.size;

                changes.push(FileChange {
                    path: path.clone(),
                    change_type: ChangeType::Deleted,
                    source_entry: Some((*source_entry).clone()),
                    target_entry: None,
                    required_chunks: vec![],
                    transfer_size: 0,
                });
            }
        }

        // Calculate total statistics
        stats.unique_chunks = required_chunks.len();
        stats.total_chunks = changes
            .iter()
            .map(|c| c.required_chunks.len())
            .sum();

        let total_transfer_size = changes
            .iter()
            .map(|c| c.transfer_size)
            .sum();

        // Compute merkle proof if enabled
        let merkle_proof = if self.enable_merkle_proofs {
            self.compute_merkle_proof(source, target)?
        } else {
            None
        };

        debug!(
            "Diff complete: {} changes, {} unique chunks, {} bytes to transfer",
            changes.len(),
            required_chunks.len(),
            total_transfer_size
        );

        let mut result = DiffResult {
            changes,
            total_transfer_size,
            required_chunks,
            stats,
            merkle_proof,
        };

        // Sort by priority
        result.prioritize_changes();

        Ok(result)
    }

    /// Find chunks that need to be transferred for a file
    fn find_required_chunks(&self, entry: &ManifestEntry) -> Vec<String> {
        entry
            .chunk_hashes
            .iter()
            .filter(|hash| !self.chunk_cache.contains(*hash))
            .cloned()
            .collect()
    }

    /// Compute chunk-level diff between two file versions
    fn compute_chunk_diff(
        &self,
        source: &ManifestEntry,
        target: &ManifestEntry,
    ) -> Vec<String> {
        // Create set of source chunks for fast lookup
        let source_chunks: HashSet<_> = source.chunk_hashes.iter().collect();

        // Find chunks in target that aren't in source or cache
        target
            .chunk_hashes
            .iter()
            .filter(|hash| {
                !source_chunks.contains(hash) && !self.chunk_cache.contains(*hash)
            })
            .cloned()
            .collect()
    }

    /// Calculate approximate transfer size for chunks
    fn calculate_transfer_size(&self, chunk_hashes: &[String]) -> u64 {
        // Average chunk size is ~512KB with FastCDC
        const AVERAGE_CHUNK_SIZE: u64 = 512 * 1024;
        chunk_hashes.len() as u64 * AVERAGE_CHUNK_SIZE
    }

    /// Compute merkle proof for efficient manifest comparison
    fn compute_merkle_proof(
        &self,
        source: &Manifest,
        target: &Manifest,
    ) -> Result<Option<MerkleProof>> {
        // Check if manifests are small enough to not need merkle proofs
        if source.files.len() < 100 && target.files.len() < 100 {
            return Ok(None);
        }

        trace!("Computing merkle proof for large manifest diff");

        // Get the merkle tree root hashes
        let source_root = source.calculate_hash();
        let target_root = target.calculate_hash();

        // If roots match, manifests are identical
        if source_root == target_root {
            return Ok(None);
        }

        // Build subtree hashes for efficient comparison
        let subtree_hashes = self.compute_subtree_hashes(target)?;

        Ok(Some(MerkleProof {
            root: target_root.as_bytes().to_vec(),
            subtree_hashes,
            depth: self.calculate_tree_depth(target.files.len()),
        }))
    }

    /// Compute subtree hashes for merkle proof
    fn compute_subtree_hashes(&self, manifest: &Manifest) -> Result<Vec<Vec<u8>>> {
        // Divide files into subtrees based on path prefixes
        let mut subtrees = HashMap::new();
        
        for file in &manifest.files {
            // Get first path component as subtree key
            let subtree_key = file
                .path
                .split('/')
                .next()
                .unwrap_or("")
                .to_string();
            
            subtrees
                .entry(subtree_key)
                .or_insert_with(Vec::new)
                .push(file);
        }

        // Compute hash for each subtree
        let mut hashes = Vec::new();
        for (_key, files) in subtrees {
            let mut hasher = blake3::Hasher::new();
            for file in files {
                hasher.update(file.path.as_bytes());
                hasher.update(&file.size.to_le_bytes());
                hasher.update(file.content_hash.as_bytes());
            }
            hashes.push(hasher.finalize().as_bytes().to_vec());
        }

        Ok(hashes)
    }

    /// Calculate optimal tree depth based on file count
    fn calculate_tree_depth(&self, file_count: usize) -> u32 {
        let depth = (file_count as f64).log2().ceil() as u32;
        depth.min(self.max_merkle_depth)
    }
}

/// Incremental diff tracker for continuous sync
pub struct IncrementalDiff {
    /// Last known manifest version for each peer
    last_versions: HashMap<String, u64>,
    /// Pending changes that haven't been acknowledged
    pending_changes: HashMap<String, Vec<FileChange>>,
    /// Diff computer instance
    diff_computer: DiffComputer,
}

impl IncrementalDiff {
    /// Create a new incremental diff tracker
    pub fn new(chunk_cache: Arc<HashSet<String>>) -> Self {
        Self {
            last_versions: HashMap::new(),
            pending_changes: HashMap::new(),
            diff_computer: DiffComputer::new(chunk_cache),
        }
    }

    /// Compute incremental diff since last sync
    pub fn compute_incremental(
        &mut self,
        peer_id: &str,
        current_manifest: &Manifest,
        peer_manifest: &Manifest,
    ) -> Result<DiffResult> {
        // Check if we have a previous version
        let last_version = self.last_versions.get(peer_id).copied();

        debug!(
            "Computing incremental diff for peer {} (last v{:?}, current v{})",
            peer_id, last_version, current_manifest.version
        );

        // Compute the diff
        let diff = self.diff_computer.compute_diff(peer_manifest, current_manifest)?;

        // Track pending changes
        if diff.has_changes() {
            self.pending_changes.insert(peer_id.to_string(), diff.changes.clone());
        }

        // Update last known version
        self.last_versions.insert(peer_id.to_string(), current_manifest.version);

        Ok(diff)
    }

    /// Mark changes as acknowledged by peer
    pub fn acknowledge_changes(&mut self, peer_id: &str, chunk_hashes: &[String]) {
        if let Some(pending) = self.pending_changes.get_mut(peer_id) {
            // Remove acknowledged chunks from pending changes
            for change in pending.iter_mut() {
                change.required_chunks.retain(|h| !chunk_hashes.contains(h));
            }

            // Remove fully transferred changes
            pending.retain(|c| !c.required_chunks.is_empty());

            if pending.is_empty() {
                self.pending_changes.remove(peer_id);
                info!("All changes acknowledged by peer {}", peer_id);
            }
        }
    }

    /// Get pending changes for a peer
    pub fn get_pending(&self, peer_id: &str) -> Option<&Vec<FileChange>> {
        self.pending_changes.get(peer_id)
    }

    /// Reset tracking for a peer
    pub fn reset_peer(&mut self, peer_id: &str) {
        self.last_versions.remove(peer_id);
        self.pending_changes.remove(peer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_entry(path: &str, content_hash: &str, chunks: Vec<&str>) -> ManifestEntry {
        ManifestEntry {
            path: path.to_string(),
            size: 1024,
            modified_at: Utc::now(),
            content_hash: content_hash.to_string(),
            chunk_hashes: chunks.iter().map(|s| s.to_string()).collect(),
            mode: Some(0o644),
        }
    }

    fn create_test_manifest(folder_id: &str, version: u64) -> Manifest {
        Manifest {
            folder_id: folder_id.to_string(),
            version,
            files: vec![],
            created_at: Utc::now(),
            manifest_hash: None,
        }
    }

    #[test]
    fn test_diff_added_files() {
        let chunk_cache = Arc::new(HashSet::new());
        let computer = DiffComputer::new(chunk_cache);

        let source = create_test_manifest("test", 1);
        let mut target = create_test_manifest("test", 2);
        target.files.push(create_test_entry(
            "new.txt",
            "hash1",
            vec!["chunk1", "chunk2"],
        ));

        let diff = computer.compute_diff(&source, &target).unwrap();

        assert_eq!(diff.stats.files_added, 1);
        assert_eq!(diff.stats.files_modified, 0);
        assert_eq!(diff.stats.files_deleted, 0);
        assert_eq!(diff.required_chunks.len(), 2);
    }

    #[test]
    fn test_diff_modified_files() {
        let chunk_cache = Arc::new(HashSet::new());
        let computer = DiffComputer::new(chunk_cache);

        let mut source = create_test_manifest("test", 1);
        source.files.push(create_test_entry(
            "file.txt",
            "hash1",
            vec!["chunk1", "chunk2"],
        ));

        let mut target = create_test_manifest("test", 2);
        target.files.push(create_test_entry(
            "file.txt",
            "hash2",
            vec!["chunk2", "chunk3", "chunk4"],
        ));

        let diff = computer.compute_diff(&source, &target).unwrap();

        assert_eq!(diff.stats.files_added, 0);
        assert_eq!(diff.stats.files_modified, 1);
        assert_eq!(diff.stats.files_deleted, 0);
        // Only chunk3 and chunk4 are new
        assert_eq!(diff.required_chunks.len(), 2);
        assert!(diff.required_chunks.contains("chunk3"));
        assert!(diff.required_chunks.contains("chunk4"));
    }

    #[test]
    fn test_diff_deleted_files() {
        let chunk_cache = Arc::new(HashSet::new());
        let computer = DiffComputer::new(chunk_cache);

        let mut source = create_test_manifest("test", 1);
        source.files.push(create_test_entry(
            "old.txt",
            "hash1",
            vec!["chunk1"],
        ));

        let target = create_test_manifest("test", 2);

        let diff = computer.compute_diff(&source, &target).unwrap();

        assert_eq!(diff.stats.files_added, 0);
        assert_eq!(diff.stats.files_modified, 0);
        assert_eq!(diff.stats.files_deleted, 1);
        assert_eq!(diff.required_chunks.len(), 0);
    }

    #[test]
    fn test_diff_with_chunk_cache() {
        let mut cache = HashSet::new();
        cache.insert("chunk1".to_string());
        cache.insert("chunk2".to_string());
        
        let chunk_cache = Arc::new(cache);
        let computer = DiffComputer::new(chunk_cache);

        let source = create_test_manifest("test", 1);
        let mut target = create_test_manifest("test", 2);
        target.files.push(create_test_entry(
            "file.txt",
            "hash1",
            vec!["chunk1", "chunk2", "chunk3"],
        ));

        let diff = computer.compute_diff(&source, &target).unwrap();

        // Only chunk3 should be required (chunk1 and chunk2 are in cache)
        assert_eq!(diff.required_chunks.len(), 1);
        assert!(diff.required_chunks.contains("chunk3"));
    }

    #[test]
    fn test_change_priority() {
        let mut changes = vec![
            FileChange {
                path: "large.bin".to_string(),
                change_type: ChangeType::Added,
                source_entry: None,
                target_entry: None,
                required_chunks: vec!["chunk1".to_string()],
                transfer_size: 10_000_000,
            },
            FileChange {
                path: "small.txt".to_string(),
                change_type: ChangeType::Added,
                source_entry: None,
                target_entry: None,
                required_chunks: vec!["chunk2".to_string()],
                transfer_size: 100,
            },
            FileChange {
                path: "modified.rs".to_string(),
                change_type: ChangeType::Modified,
                source_entry: None,
                target_entry: None,
                required_chunks: vec!["chunk3".to_string()],
                transfer_size: 5000,
            },
        ];

        let mut diff = DiffResult {
            changes: changes.clone(),
            total_transfer_size: 0,
            required_chunks: HashSet::new(),
            stats: DiffStats::default(),
            merkle_proof: None,
        };

        diff.prioritize_changes();

        // Modified files should be first
        assert_eq!(diff.changes[0].path, "modified.rs");
        // Small added files should be second
        assert_eq!(diff.changes[1].path, "small.txt");
        // Large added files should be last
        assert_eq!(diff.changes[2].path, "large.bin");
    }

    #[test]
    fn test_incremental_diff() {
        let chunk_cache = Arc::new(HashSet::new());
        let mut tracker = IncrementalDiff::new(chunk_cache);

        let mut manifest_v1 = create_test_manifest("test", 1);
        manifest_v1.files.push(create_test_entry(
            "file1.txt",
            "hash1",
            vec!["chunk1"],
        ));

        let mut manifest_v2 = create_test_manifest("test", 2);
        manifest_v2.files.push(create_test_entry(
            "file1.txt",
            "hash1",
            vec!["chunk1"],
        ));
        manifest_v2.files.push(create_test_entry(
            "file2.txt",
            "hash2",
            vec!["chunk2"],
        ));

        let peer_manifest = create_test_manifest("test", 0);

        // First sync
        let diff1 = tracker
            .compute_incremental("peer1", &manifest_v1, &peer_manifest)
            .unwrap();
        assert_eq!(diff1.stats.files_added, 1);

        // Second sync with new file
        let diff2 = tracker
            .compute_incremental("peer1", &manifest_v2, &manifest_v1)
            .unwrap();
        assert_eq!(diff2.stats.files_added, 1);
        
        // Check pending changes
        assert!(tracker.get_pending("peer1").is_some());
    }
}