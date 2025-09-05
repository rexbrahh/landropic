//! Async-safe wrapper for FileIndexer operations
//!
//! This module provides a fully async-compatible interface to FileIndexer
//! that properly handles the Send + Sync requirements for use across await points.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::errors::Result;
use crate::indexer::{FileIndexer, IndexerConfig, IntegrityReport, StorageStatistics};
use crate::manifest::Manifest;

/// Async-safe wrapper around FileIndexer
///
/// This wrapper ensures FileIndexer can be safely shared across async tasks
/// and await points, handling all the necessary thread-safety requirements.
#[derive(Clone)]
pub struct AsyncIndexer {
    inner: Arc<RwLock<FileIndexer>>,
}

impl AsyncIndexer {
    /// Create a new async indexer
    pub async fn new(
        cas_path: impl AsRef<Path>,
        database_path: impl AsRef<Path>,
        config: IndexerConfig,
    ) -> Result<Self> {
        let indexer = FileIndexer::new(cas_path, database_path, config).await?;
        Ok(Self {
            inner: Arc::new(RwLock::new(indexer)),
        })
    }

    /// Index a folder asynchronously
    pub async fn index_folder(&self, folder_path: impl AsRef<Path>) -> Result<Manifest> {
        let folder_path = folder_path.as_ref().to_path_buf();
        let mut indexer = self.inner.write().await;
        indexer.index_folder(folder_path).await
    }

    /// Get storage statistics
    pub async fn storage_stats(&self) -> Result<StorageStatistics> {
        let indexer = self.inner.read().await;
        indexer.storage_stats().await
    }

    /// Verify integrity of stored objects
    pub async fn verify_integrity(&self) -> Result<IntegrityReport> {
        let indexer = self.inner.read().await;
        indexer.verify_integrity().await
    }

    /// Get the inner Arc<RwLock<FileIndexer>> for advanced use cases
    pub fn inner(&self) -> Arc<RwLock<FileIndexer>> {
        self.inner.clone()
    }
}

/// Builder pattern for AsyncIndexer with configuration options
pub struct AsyncIndexerBuilder {
    cas_path: Option<PathBuf>,
    database_path: Option<PathBuf>,
    config: IndexerConfig,
}

impl AsyncIndexerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            cas_path: None,
            database_path: None,
            config: IndexerConfig::default(),
        }
    }

    /// Set the CAS storage path
    pub fn cas_path(mut self, path: impl AsRef<Path>) -> Self {
        self.cas_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set the database path
    pub fn database_path(mut self, path: impl AsRef<Path>) -> Self {
        self.database_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set the indexer configuration
    pub fn config(mut self, config: IndexerConfig) -> Self {
        self.config = config;
        self
    }

    /// Build the async indexer
    pub async fn build(self) -> Result<AsyncIndexer> {
        let cas_path = self.cas_path.expect("CAS path is required");
        let database_path = self.database_path.expect("Database path is required");
        AsyncIndexer::new(cas_path, database_path, self.config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn test_async_indexer_operations() {
        let temp_dir = tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let db_path = temp_dir.path().join("index.db");

        // Create test directory structure
        let test_folder = temp_dir.path().join("test_folder");
        fs::create_dir_all(&test_folder).await.unwrap();

        let test_file = test_folder.join("test.txt");
        fs::write(&test_file, b"Hello, async world!").await.unwrap();

        // Create async indexer
        let indexer = AsyncIndexer::new(&cas_dir, &db_path, IndexerConfig::default())
            .await
            .unwrap();

        // Test indexing
        let manifest = indexer.index_folder(&test_folder).await.unwrap();
        assert_eq!(manifest.file_count(), 1);

        // Test concurrent access (should not deadlock)
        let indexer_clone = indexer.clone();
        let handle = tokio::spawn(async move { indexer_clone.storage_stats().await });

        let stats = handle.await.unwrap().unwrap();
        assert!(stats.total_objects > 0);
    }

    #[tokio::test]
    async fn test_async_indexer_builder() {
        let temp_dir = tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let db_path = temp_dir.path().join("index.db");

        let indexer = AsyncIndexerBuilder::new()
            .cas_path(&cas_dir)
            .database_path(&db_path)
            .config(IndexerConfig::default())
            .build()
            .await
            .unwrap();

        // Verify it works
        let stats = indexer.storage_stats().await.unwrap();
        assert_eq!(stats.total_objects, 0);
    }
}
