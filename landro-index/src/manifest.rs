use chrono::{DateTime, Utc};
use landro_chunker::ContentHash;
use serde::{Deserialize, Serialize};

/// File entry in a manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub modified_at: DateTime<Utc>,
    pub content_hash: String,
    pub chunk_hashes: Vec<String>,
    pub mode: Option<u32>,
}

/// A folder manifest representing a snapshot in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub folder_id: String,
    pub version: u64,
    pub files: Vec<ManifestEntry>,
    pub created_at: DateTime<Utc>,
    pub manifest_hash: Option<String>,
}

impl Manifest {
    /// Create a new manifest
    pub fn new(folder_id: String, version: u64) -> Self {
        Self {
            folder_id,
            version,
            files: Vec::new(),
            created_at: Utc::now(),
            manifest_hash: None,
        }
    }

    /// Add a file entry
    pub fn add_file(&mut self, entry: ManifestEntry) {
        self.files.push(entry);
    }

    /// Finalize the manifest by calculating its hash
    pub fn finalize(&mut self) {
        let hash = self.calculate_hash();
        self.manifest_hash = Some(hash.to_hex());
    }

    /// Verify manifest integrity by recalculating root hash
    pub fn verify(&self) -> bool {
        if let Some(stored_hash) = &self.manifest_hash {
            let calculated = self.calculate_hash();
            calculated.to_hex() == *stored_hash
        } else {
            false
        }
    }

    /// Get merkle proof for a specific file (future enhancement)
    pub fn get_proof(&self, _file_path: &str) -> Option<Vec<Vec<u8>>> {
        // TODO: Implement merkle proof generation
        None
    }

    /// Calculate merkle tree root hash for manifest integrity
    pub fn calculate_hash(&self) -> ContentHash {
        // Sort files by path for deterministic ordering
        let mut sorted_files = self.files.clone();
        sorted_files.sort_by(|a, b| a.path.cmp(&b.path));

        if sorted_files.is_empty() {
            // Empty manifest gets hash of empty data
            let hasher = blake3::Hasher::new();
            return ContentHash::from_blake3(hasher.finalize());
        }

        // Calculate leaf hashes for each file
        let mut hashes: Vec<Vec<u8>> = sorted_files
            .iter()
            .map(|file| self.hash_file_entry(file))
            .collect();

        // Build merkle tree
        while hashes.len() > 1 {
            hashes = self.hash_pairs(hashes);
        }

        // Convert root to ContentHash
        let mut root_bytes = [0u8; 32];
        root_bytes.copy_from_slice(&hashes[0]);
        ContentHash::from_bytes(root_bytes)
    }

    /// Hash a single file entry for merkle tree leaf
    fn hash_file_entry(&self, file: &ManifestEntry) -> Vec<u8> {
        let mut hasher = blake3::Hasher::new();

        // Include all metadata in deterministic order
        hasher.update(file.path.as_bytes());
        hasher.update(&file.size.to_le_bytes());

        // Content hash as bytes (decode from hex)
        let content_bytes = hex::decode(&file.content_hash).unwrap_or_else(|_| vec![0; 32]);
        hasher.update(&content_bytes);

        // Optional mode
        if let Some(mode) = file.mode {
            hasher.update(&mode.to_le_bytes());
        } else {
            hasher.update(&[0u8; 4]); // Placeholder for missing mode
        }

        // Timestamp
        hasher.update(&file.modified_at.timestamp().to_le_bytes());

        // Hash chunk hashes in order
        for chunk_hash in &file.chunk_hashes {
            let chunk_bytes = hex::decode(chunk_hash).unwrap_or_else(|_| vec![0; 32]);
            hasher.update(&chunk_bytes);
        }

        hasher.finalize().as_bytes().to_vec()
    }

    /// Hash pairs of hashes to build next level of merkle tree
    fn hash_pairs(&self, hashes: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        let mut next_level = Vec::new();
        let mut i = 0;

        while i < hashes.len() {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&hashes[i]);

            if i + 1 < hashes.len() {
                hasher.update(&hashes[i + 1]);
            } else {
                // Duplicate last hash for odd count
                hasher.update(&hashes[i]);
            }

            next_level.push(hasher.finalize().as_bytes().to_vec());
            i += 2;
        }

        next_level
    }

    /// Get total size of all files
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Get file count
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Compare with another manifest to find differences
    ///
    /// Returns a diff showing what changes would be needed to transform `self` into `other`:
    /// - `added`: Files that exist in `other` but not in `self` (new files)
    /// - `modified`: Files that exist in both but with different content hashes
    /// - `deleted`: Files that exist in `self` but not in `other` (removed files)
    pub fn diff(&self, other: &Manifest) -> ManifestDiff {
        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut deleted = Vec::new();

        // Create maps for efficient lookup
        let self_map: std::collections::HashMap<_, _> =
            self.files.iter().map(|f| (f.path.clone(), f)).collect();

        let other_map: std::collections::HashMap<_, _> =
            other.files.iter().map(|f| (f.path.clone(), f)).collect();

        // Find added and modified files (files in other)
        for (path, file) in &other_map {
            match self_map.get(path) {
                None => added.push((*file).clone()), // File in other but not in self = added
                Some(self_file) => {
                    if self_file.content_hash != file.content_hash {
                        modified.push((*file).clone()); // File exists in both but different = modified
                    }
                }
            }
        }

        // Find deleted files (files in self but not in other)
        for (path, file) in &self_map {
            if !other_map.contains_key(path) {
                deleted.push((*file).clone()); // File in self but not in other = deleted
            }
        }

        ManifestDiff {
            added,
            modified,
            deleted,
        }
    }
}

/// Difference between two manifests
#[derive(Debug, Clone)]
pub struct ManifestDiff {
    pub added: Vec<ManifestEntry>,
    pub modified: Vec<ManifestEntry>,
    pub deleted: Vec<ManifestEntry>,
}

impl ManifestDiff {
    /// Check if there are any changes
    pub fn has_changes(&self) -> bool {
        !self.added.is_empty() || !self.modified.is_empty() || !self.deleted.is_empty()
    }

    /// Get total number of changes
    pub fn change_count(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_entry(path: &str, content_hash: &str) -> ManifestEntry {
        ManifestEntry {
            path: path.to_string(),
            size: 100,
            modified_at: Utc::now(),
            content_hash: content_hash.to_string(),
            chunk_hashes: vec![
                "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            ],
            mode: Some(0o644),
        }
    }

    mod merkle_tests {
        use super::*;

        #[test]
        fn test_empty_manifest_hash() {
            let mut manifest = Manifest::new("test".to_string(), 1);
            manifest.finalize();
            assert!(manifest.manifest_hash.is_some());
            // Empty manifest should have consistent hash
            let hash1 = manifest.manifest_hash.clone();
            manifest.finalize();
            assert_eq!(hash1, manifest.manifest_hash);
        }

        #[test]
        fn test_deterministic_hashing() {
            // Same files should always produce same hash regardless of insertion order
            let mut manifest1 = Manifest::new("test".to_string(), 1);
            let mut manifest2 = Manifest::new("test".to_string(), 1);

            let hash1 = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
            let hash2 = "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321";

            // Add files in different order
            manifest1.add_file(create_test_entry("file1.txt", hash1));
            manifest1.add_file(create_test_entry("file2.txt", hash2));

            manifest2.add_file(create_test_entry("file2.txt", hash2));
            manifest2.add_file(create_test_entry("file1.txt", hash1));

            manifest1.finalize();
            manifest2.finalize();

            assert_eq!(manifest1.manifest_hash, manifest2.manifest_hash);
        }

        #[test]
        fn test_single_file_manifest() {
            let mut manifest = Manifest::new("test".to_string(), 1);
            let hash1 = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
            manifest.add_file(create_test_entry("file1.txt", hash1));
            manifest.finalize();

            assert!(manifest.manifest_hash.is_some());
            assert!(manifest.verify());
        }

        #[test]
        fn test_merkle_tree_construction() {
            // Test with 1, 2, 3, 4, 5 files to verify tree construction
            let base_hashes = [
                "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
                "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321",
                "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
                "654321fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987",
                "567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234",
            ];

            for file_count in 1..=5 {
                let mut manifest = Manifest::new("test".to_string(), 1);

                for i in 0..file_count {
                    manifest.add_file(create_test_entry(&format!("file{}.txt", i), base_hashes[i]));
                }

                manifest.finalize();
                assert!(manifest.manifest_hash.is_some());
                assert!(manifest.verify());
            }
        }

        #[test]
        fn test_file_order_independence() {
            // Add files in different orders, should produce same hash after sorting
            let files = vec![
                (
                    "zebra.txt",
                    "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
                ),
                (
                    "alpha.txt",
                    "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321",
                ),
                (
                    "beta.txt",
                    "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
                ),
            ];

            // Forward order
            let mut manifest1 = Manifest::new("test".to_string(), 1);
            for (path, hash) in &files {
                manifest1.add_file(create_test_entry(path, hash));
            }

            // Reverse order
            let mut manifest2 = Manifest::new("test".to_string(), 1);
            for (path, hash) in files.iter().rev() {
                manifest2.add_file(create_test_entry(path, hash));
            }

            manifest1.finalize();
            manifest2.finalize();

            assert_eq!(manifest1.manifest_hash, manifest2.manifest_hash);
        }

        #[test]
        fn test_metadata_sensitivity() {
            // Use valid 32-byte hex strings for testing
            let hash1 = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
            let hash2 = "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321";

            // Changing any metadata should change hash
            let mut base_manifest = Manifest::new("test".to_string(), 1);
            base_manifest.add_file(create_test_entry("file.txt", hash1));
            base_manifest.finalize();
            let base_hash = base_manifest.manifest_hash.clone();

            // Different content hash
            let mut manifest_diff_content = Manifest::new("test".to_string(), 1);
            manifest_diff_content.add_file(create_test_entry("file.txt", hash2));
            manifest_diff_content.finalize();
            assert_ne!(base_hash, manifest_diff_content.manifest_hash);

            // Different file size
            let mut manifest_diff_size = Manifest::new("test".to_string(), 1);
            let mut entry_diff_size = create_test_entry("file.txt", hash1);
            entry_diff_size.size = 200;
            manifest_diff_size.add_file(entry_diff_size);
            manifest_diff_size.finalize();
            assert_ne!(base_hash, manifest_diff_size.manifest_hash);

            // Different path
            let mut manifest_diff_path = Manifest::new("test".to_string(), 1);
            manifest_diff_path.add_file(create_test_entry("different.txt", hash1));
            manifest_diff_path.finalize();
            assert_ne!(base_hash, manifest_diff_path.manifest_hash);
        }

        #[test]
        fn test_verify_integrity() {
            let mut manifest = Manifest::new("test".to_string(), 1);
            let hash1 = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
            manifest.add_file(create_test_entry("file.txt", hash1));
            manifest.finalize();

            // Should verify correctly
            assert!(manifest.verify());

            // Corrupt the stored hash
            manifest.manifest_hash = Some("corrupted_hash".to_string());
            assert!(!manifest.verify());

            // Missing hash should fail verification
            manifest.manifest_hash = None;
            assert!(!manifest.verify());
        }
    }

    #[test]
    fn test_diff_empty_manifests() {
        let manifest1 = Manifest::new("test".to_string(), 1);
        let manifest2 = Manifest::new("test".to_string(), 2);

        let diff = manifest1.diff(&manifest2);

        assert!(!diff.has_changes());
        assert_eq!(diff.change_count(), 0);
        assert!(diff.added.is_empty());
        assert!(diff.modified.is_empty());
        assert!(diff.deleted.is_empty());
    }

    #[test]
    fn test_diff_added_files() {
        let mut manifest1 = Manifest::new("test".to_string(), 1);
        let mut manifest2 = Manifest::new("test".to_string(), 2);

        // manifest2 has files that manifest1 doesn't have
        manifest2.add_file(create_test_entry("file1.txt", "hash1"));
        manifest2.add_file(create_test_entry("file2.txt", "hash2"));

        let diff = manifest1.diff(&manifest2);

        assert!(diff.has_changes());
        assert_eq!(diff.added.len(), 2);
        assert_eq!(diff.modified.len(), 0);
        assert_eq!(diff.deleted.len(), 0);
        assert!(diff.added.iter().any(|f| f.path == "file1.txt"));
        assert!(diff.added.iter().any(|f| f.path == "file2.txt"));
    }

    #[test]
    fn test_diff_deleted_files() {
        let mut manifest1 = Manifest::new("test".to_string(), 1);
        let mut manifest2 = Manifest::new("test".to_string(), 2);

        // manifest1 has files that manifest2 doesn't have
        manifest1.add_file(create_test_entry("file1.txt", "hash1"));
        manifest1.add_file(create_test_entry("file2.txt", "hash2"));

        let diff = manifest1.diff(&manifest2);

        assert!(diff.has_changes());
        assert_eq!(diff.added.len(), 0);
        assert_eq!(diff.modified.len(), 0);
        assert_eq!(diff.deleted.len(), 2);
        assert!(diff.deleted.iter().any(|f| f.path == "file1.txt"));
        assert!(diff.deleted.iter().any(|f| f.path == "file2.txt"));
    }

    #[test]
    fn test_diff_modified_files() {
        let mut manifest1 = Manifest::new("test".to_string(), 1);
        let mut manifest2 = Manifest::new("test".to_string(), 2);

        // Same files but different content hashes
        manifest1.add_file(create_test_entry("file1.txt", "hash1"));
        manifest1.add_file(create_test_entry("file2.txt", "hash2"));

        manifest2.add_file(create_test_entry("file1.txt", "hash1_modified"));
        manifest2.add_file(create_test_entry("file2.txt", "hash2_modified"));

        let diff = manifest1.diff(&manifest2);

        assert!(diff.has_changes());
        assert_eq!(diff.added.len(), 0);
        assert_eq!(diff.modified.len(), 2);
        assert_eq!(diff.deleted.len(), 0);
        assert!(diff
            .modified
            .iter()
            .any(|f| f.path == "file1.txt" && f.content_hash == "hash1_modified"));
        assert!(diff
            .modified
            .iter()
            .any(|f| f.path == "file2.txt" && f.content_hash == "hash2_modified"));
    }

    #[test]
    fn test_diff_mixed_changes() {
        let mut manifest1 = Manifest::new("test".to_string(), 1);
        let mut manifest2 = Manifest::new("test".to_string(), 2);

        // manifest1: file1, file2, file3
        manifest1.add_file(create_test_entry("file1.txt", "hash1"));
        manifest1.add_file(create_test_entry("file2.txt", "hash2"));
        manifest1.add_file(create_test_entry("file3.txt", "hash3"));

        // manifest2: file1 (modified), file4 (new)
        // missing: file2, file3 (deleted)
        manifest2.add_file(create_test_entry("file1.txt", "hash1_modified"));
        manifest2.add_file(create_test_entry("file4.txt", "hash4"));

        let diff = manifest1.diff(&manifest2);

        assert!(diff.has_changes());
        assert_eq!(diff.change_count(), 4);

        // Check added files
        assert_eq!(diff.added.len(), 1);
        assert!(diff.added.iter().any(|f| f.path == "file4.txt"));

        // Check modified files
        assert_eq!(diff.modified.len(), 1);
        assert!(diff
            .modified
            .iter()
            .any(|f| f.path == "file1.txt" && f.content_hash == "hash1_modified"));

        // Check deleted files
        assert_eq!(diff.deleted.len(), 2);
        assert!(diff.deleted.iter().any(|f| f.path == "file2.txt"));
        assert!(diff.deleted.iter().any(|f| f.path == "file3.txt"));
    }

    #[test]
    fn test_diff_identical_manifests() {
        let mut manifest1 = Manifest::new("test".to_string(), 1);
        let mut manifest2 = Manifest::new("test".to_string(), 2);

        // Same files with same content hashes
        manifest1.add_file(create_test_entry("file1.txt", "hash1"));
        manifest1.add_file(create_test_entry("file2.txt", "hash2"));

        manifest2.add_file(create_test_entry("file1.txt", "hash1"));
        manifest2.add_file(create_test_entry("file2.txt", "hash2"));

        let diff = manifest1.diff(&manifest2);

        assert!(!diff.has_changes());
        assert_eq!(diff.change_count(), 0);
        assert!(diff.added.is_empty());
        assert!(diff.modified.is_empty());
        assert!(diff.deleted.is_empty());
    }

    #[test]
    fn test_diff_direction_semantics() {
        let mut old_manifest = Manifest::new("test".to_string(), 1);
        let mut new_manifest = Manifest::new("test".to_string(), 2);

        // Old state: file1, file2
        old_manifest.add_file(create_test_entry("file1.txt", "hash1"));
        old_manifest.add_file(create_test_entry("file2.txt", "hash2"));

        // New state: file1 (modified), file3 (new)
        new_manifest.add_file(create_test_entry("file1.txt", "hash1_new"));
        new_manifest.add_file(create_test_entry("file3.txt", "hash3"));

        // Forward diff: old -> new
        let forward_diff = old_manifest.diff(&new_manifest);

        assert_eq!(forward_diff.added.len(), 1);
        assert!(forward_diff.added.iter().any(|f| f.path == "file3.txt"));

        assert_eq!(forward_diff.modified.len(), 1);
        assert!(forward_diff.modified.iter().any(|f| f.path == "file1.txt"));

        assert_eq!(forward_diff.deleted.len(), 1);
        assert!(forward_diff.deleted.iter().any(|f| f.path == "file2.txt"));

        // Reverse diff: new -> old
        let reverse_diff = new_manifest.diff(&old_manifest);

        assert_eq!(reverse_diff.added.len(), 1);
        assert!(reverse_diff.added.iter().any(|f| f.path == "file2.txt"));

        assert_eq!(reverse_diff.modified.len(), 1);
        assert!(reverse_diff.modified.iter().any(|f| f.path == "file1.txt"));

        assert_eq!(reverse_diff.deleted.len(), 1);
        assert!(reverse_diff.deleted.iter().any(|f| f.path == "file3.txt"));
    }
}
