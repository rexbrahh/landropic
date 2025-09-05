use chrono::DateTime;
use landro_cas::ContentStore;
use landro_chunker::{Chunker, ChunkerConfig};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{info, trace, warn};

use crate::database::{ChunkEntry, FileEntry};
use crate::database_pool::DatabasePool;
use crate::errors::{IndexError, Result};
use crate::manifest::{Manifest, ManifestEntry};

/// Configuration for the file indexer
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    /// Chunker configuration
    pub chunker_config: ChunkerConfig,
    /// Whether to follow symbolic links
    pub follow_symlinks: bool,
    /// File patterns to ignore (basic glob patterns)
    pub ignore_patterns: Vec<String>,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            chunker_config: ChunkerConfig::default(),
            follow_symlinks: false,
            ignore_patterns: vec![
                ".git".to_string(),
                ".DS_Store".to_string(),
                "Thumbs.db".to_string(),
                "*.tmp".to_string(),
            ],
        }
    }
}

/// File indexer that coordinates chunking, storage, and database operations
///
/// This struct is now Send + Sync safe thanks to DatabasePool
pub struct FileIndexer {
    chunker: Chunker,
    cas: ContentStore,
    database: DatabasePool,
    config: IndexerConfig,
}

impl FileIndexer {
    /// Create a new file indexer
    pub async fn new(
        cas_path: impl AsRef<Path>,
        database_path: impl AsRef<Path>,
        config: IndexerConfig,
    ) -> Result<Self> {
        let chunker = Chunker::new(config.chunker_config.clone())
            .map_err(|e| IndexError::ChunkerError(e.to_string()))?;

        let cas = ContentStore::new(cas_path)
            .await
            .map_err(|e| IndexError::StorageError(e.to_string()))?;

        let database = DatabasePool::new(database_path)?;

        Ok(Self {
            chunker,
            cas,
            database,
            config,
        })
    }

    /// Index a folder and create a manifest
    pub async fn index_folder(&mut self, folder_path: impl AsRef<Path>) -> Result<Manifest> {
        let folder_path = folder_path.as_ref();
        info!("Starting folder indexing: {:?}", folder_path);

        // Generate folder ID from canonical path
        let canonical_path = folder_path.canonicalize().map_err(|e| IndexError::Io(e))?;

        let folder_id = Self::path_to_folder_id(&canonical_path);

        // Get the next version number (check for existing manifests)
        let next_version = self
            .database
            .get_latest_manifest_version(folder_id.clone())
            .await?
            .unwrap_or(0)
            + 1;

        // Create new manifest with incremented version
        let mut manifest = Manifest::new(folder_id, next_version);

        // Walk directory tree
        let files = self.walk_directory(folder_path).await?;
        info!("Found {} files to index", files.len());

        // Process each file
        for file_path in files {
            match self.index_file(&file_path).await {
                Ok(manifest_entry) => {
                    manifest.add_file(manifest_entry);
                }
                Err(e) => {
                    warn!("Failed to index file {:?}: {}", file_path, e);
                    // Continue with other files
                }
            }
        }

        // Finalize manifest with hash
        manifest.finalize();

        // Save manifest to database
        self.database.save_manifest(manifest.clone()).await?;

        info!(
            "Folder indexing completed: {} files, {} total bytes",
            manifest.file_count(),
            manifest.total_size()
        );

        Ok(manifest)
    }

    /// Index a single file
    async fn index_file(&mut self, file_path: &Path) -> Result<ManifestEntry> {
        trace!("Indexing file: {:?}", file_path);

        // Get file metadata
        let metadata = fs::metadata(file_path)
            .await
            .map_err(|e| IndexError::Io(e))?;

        if !metadata.is_file() {
            return Err(IndexError::InvalidFileType(file_path.to_path_buf()));
        }

        let size = metadata.len();
        let modified_at = DateTime::from(metadata.modified().map_err(|e| IndexError::Io(e))?);

        // Read and chunk the file
        let mut file = fs::File::open(file_path)
            .await
            .map_err(|e| IndexError::Io(e))?;

        let chunks = self
            .chunker
            .chunk_stream(&mut file)
            .await
            .map_err(|e| IndexError::ChunkerError(e.to_string()))?;

        // Store chunks in CAS and collect references
        let mut chunk_hashes = Vec::new();
        let mut chunk_refs = Vec::new();

        for chunk in chunks {
            // Store chunk in CAS
            let obj_ref = self
                .cas
                .write(&chunk.data)
                .await
                .map_err(|e| IndexError::StorageError(e.to_string()))?;

            chunk_hashes.push(chunk.hash.to_hex());
            chunk_refs.push(obj_ref);

            // Store chunk metadata in database
            let chunk_entry = ChunkEntry {
                id: None,
                hash: chunk.hash.to_hex(),
                size: chunk.data.len() as u64,
                ref_count: 1,
            };
            self.database.upsert_chunk(chunk_entry).await?;
        }

        // Calculate file content hash from chunk hashes
        let content_hash = Self::calculate_file_hash(&chunk_hashes);

        // Get file mode (Unix permissions)
        let mode = self.get_file_mode(&metadata);

        // Create file entry
        let file_entry = FileEntry {
            id: None,
            path: file_path.to_string_lossy().to_string(),
            size,
            modified_at,
            content_hash: content_hash.clone(),
            mode,
        };

        // Store file entry in database
        let _file_id = self.database.upsert_file(file_entry).await?;

        // TODO: Store file-chunk mappings in database
        // This will be implemented when we add the file-chunk mapping functionality

        // Create manifest entry
        Ok(ManifestEntry {
            path: file_path.to_string_lossy().to_string(),
            size,
            modified_at,
            content_hash,
            chunk_hashes,
            mode,
        })
    }

    /// Walk directory tree and collect file paths
    async fn walk_directory(&self, dir_path: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut dirs_to_process = vec![dir_path.to_path_buf()];

        while let Some(current_dir) = dirs_to_process.pop() {
            let mut entries = fs::read_dir(&current_dir)
                .await
                .map_err(|e| IndexError::Io(e))?;

            while let Some(entry) = entries.next_entry().await.map_err(|e| IndexError::Io(e))? {
                let path = entry.path();

                // Skip if matches ignore patterns
                if self.should_ignore(&path) {
                    trace!("Ignoring path: {:?}", path);
                    continue;
                }

                let file_type = entry.file_type().await.map_err(|e| IndexError::Io(e))?;

                if file_type.is_file() {
                    files.push(path);
                } else if file_type.is_dir() {
                    dirs_to_process.push(path);
                } else if file_type.is_symlink() && self.config.follow_symlinks {
                    // Handle symlinks if configured to follow them
                    let target = fs::read_link(&path).await.map_err(|e| IndexError::Io(e))?;

                    if target.is_file() {
                        files.push(path);
                    } else if target.is_dir() {
                        dirs_to_process.push(path);
                    }
                }
            }
        }

        files.sort(); // Ensure consistent ordering
        Ok(files)
    }

    /// Check if path should be ignored based on patterns
    fn should_ignore(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        for pattern in &self.config.ignore_patterns {
            if pattern.contains('*') {
                // Simple glob matching
                if Self::glob_match(pattern, name) {
                    return true;
                }
            } else if name == pattern {
                return true;
            }
        }

        false
    }

    /// Simple glob pattern matching for ignore patterns
    fn glob_match(pattern: &str, name: &str) -> bool {
        if pattern.starts_with('*') {
            let suffix = &pattern[1..];
            return name.ends_with(suffix);
        }
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            return name.starts_with(prefix);
        }
        pattern == name
    }

    /// Calculate file content hash from chunk hashes
    fn calculate_file_hash(chunk_hashes: &[String]) -> String {
        use blake3::Hasher;

        let mut hasher = Hasher::new();
        for chunk_hash in chunk_hashes {
            hasher.update(chunk_hash.as_bytes());
        }
        hex::encode(hasher.finalize().as_bytes())
    }

    /// Generate folder ID from canonical path
    fn path_to_folder_id(path: &Path) -> String {
        use blake3::Hasher;

        let mut hasher = Hasher::new();
        hasher.update(path.to_string_lossy().as_bytes());
        hex::encode(hasher.finalize().as_bytes())
    }

    /// Get file mode (Unix permissions) in a cross-platform way
    fn get_file_mode(&self, metadata: &std::fs::Metadata) -> Option<u32> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(metadata.mode())
        }

        #[cfg(not(unix))]
        {
            None // Windows doesn't have Unix-style permissions
        }
    }

    /// Get storage statistics
    pub async fn storage_stats(&self) -> Result<StorageStatistics> {
        let cas_stats = self
            .cas
            .stats()
            .await
            .map_err(|e| IndexError::StorageError(e.to_string()))?;

        // TODO: Add database statistics

        Ok(StorageStatistics {
            total_objects: cas_stats.object_count,
            total_storage_bytes: cas_stats.total_size,
            total_files: 0,  // Will be populated from database
            total_chunks: 0, // Will be populated from database
        })
    }

    /// Verify integrity of stored objects
    pub async fn verify_integrity(&self) -> Result<IntegrityReport> {
        let cas_report = self
            .cas
            .verify_integrity()
            .await
            .map_err(|e| IndexError::StorageError(e.to_string()))?;

        Ok(IntegrityReport {
            cas_report,
            database_inconsistencies: Vec::new(), // TODO: Add database verification
        })
    }
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStatistics {
    pub total_objects: u64,
    pub total_storage_bytes: u64,
    pub total_files: u64,
    pub total_chunks: u64,
}

/// Integrity verification report
#[derive(Debug, Clone)]
pub struct IntegrityReport {
    pub cas_report: landro_cas::VerificationReport,
    pub database_inconsistencies: Vec<String>,
}

impl IntegrityReport {
    pub fn is_healthy(&self) -> bool {
        self.cas_report.is_healthy() && self.database_inconsistencies.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_index_single_file() {
        let temp_dir = tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let db_path = temp_dir.path().join("index.db");

        let config = IndexerConfig::default();
        let mut indexer = FileIndexer::new(&cas_dir, &db_path, config).await.unwrap();

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        let test_data = b"Hello, Landropic indexing!";
        let mut file = fs::File::create(&test_file).await.unwrap();
        file.write_all(test_data).await.unwrap();
        file.sync_all().await.unwrap();

        // Index the file
        let manifest_entry = indexer.index_file(&test_file).await.unwrap();

        assert_eq!(manifest_entry.size, test_data.len() as u64);
        assert!(!manifest_entry.chunk_hashes.is_empty());
        assert!(!manifest_entry.content_hash.is_empty());
    }

    #[tokio::test]
    async fn test_directory_walking() {
        let temp_dir = tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let db_path = temp_dir.path().join("index.db");

        // Create test directory structure
        let test_root = temp_dir.path().join("test_folder");
        fs::create_dir_all(&test_root).await.unwrap();

        let file1 = test_root.join("file1.txt");
        let file2 = test_root.join("subdir").join("file2.txt");
        fs::create_dir_all(file2.parent().unwrap()).await.unwrap();

        fs::write(&file1, b"Content 1").await.unwrap();
        fs::write(&file2, b"Content 2").await.unwrap();

        let config = IndexerConfig::default();
        let mut indexer = FileIndexer::new(&cas_dir, &db_path, config).await.unwrap();

        let files = indexer.walk_directory(&test_root).await.unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_glob_matching() {
        assert!(FileIndexer::glob_match("*.txt", "test.txt"));
        assert!(FileIndexer::glob_match("test*", "test.txt"));
        assert!(!FileIndexer::glob_match("*.txt", "test.doc"));
        assert!(!FileIndexer::glob_match("test*", "other.txt"));
    }
}
