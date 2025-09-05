use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use landro_chunker::ContentHash;

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
    
    /// Calculate manifest hash
    pub fn calculate_hash(&self) -> ContentHash {
        // TODO: Implement proper manifest hashing
        // For now, return a placeholder
        ContentHash::from_bytes([0u8; 32])
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
    pub fn diff(&self, other: &Manifest) -> ManifestDiff {
        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut deleted = Vec::new();
        
        // Create maps for efficient lookup
        let self_map: std::collections::HashMap<_, _> = self.files.iter()
            .map(|f| (f.path.clone(), f))
            .collect();
        
        let other_map: std::collections::HashMap<_, _> = other.files.iter()
            .map(|f| (f.path.clone(), f))
            .collect();
        
        // Find added and modified files
        for (path, file) in &other_map {
            match self_map.get(path) {
                None => deleted.push((*file).clone()),
                Some(self_file) => {
                    if self_file.content_hash != file.content_hash {
                        modified.push((*file).clone());
                    }
                }
            }
        }
        
        // Find deleted files
        for (path, file) in &self_map {
            if !other_map.contains_key(path) {
                added.push((*file).clone());
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