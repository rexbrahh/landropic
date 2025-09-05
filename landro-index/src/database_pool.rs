//! Thread-safe database pool wrapper for rusqlite::Connection
//!
//! This module provides a Send + Sync wrapper around rusqlite::Connection
//! using tokio::task::spawn_blocking to ensure database operations happen
//! on a dedicated thread pool, avoiding the !Send constraint of Connection.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::database::{ChunkEntry, FileEntry, IndexDatabase};
use crate::errors::{IndexError, Result};
use crate::manifest::Manifest;

/// Thread-safe wrapper for database operations
///
/// This wrapper ensures all database operations are executed on a blocking
/// thread pool, making it safe to use across await points in async code.
#[derive(Clone)]
pub struct DatabasePool {
    /// Path to the database file for reopening connections
    _db_path: Option<PathBuf>,
    /// Shared mutex-protected database instance
    /// We use Arc<Mutex<>> here to ensure only one operation at a time
    db: Arc<Mutex<IndexDatabase>>,
}

impl DatabasePool {
    /// Create a new database pool for a file-based database
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let db = IndexDatabase::open(&path)?;

        Ok(Self {
            _db_path: Some(path),
            db: Arc::new(Mutex::new(db)),
        })
    }

    /// Create a new in-memory database pool (for testing)
    pub fn new_in_memory() -> Result<Self> {
        let db = IndexDatabase::open_in_memory()?;

        Ok(Self {
            _db_path: None,
            db: Arc::new(Mutex::new(db)),
        })
    }

    /// Execute a database operation in a blocking context
    ///
    /// This is the core method that ensures all database operations
    /// happen on the blocking thread pool.
    async fn execute_blocking<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut IndexDatabase) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            // We need to block here to acquire the mutex in the blocking context
            let mut db_guard = futures::executor::block_on(async { db.lock().await });
            f(&mut *db_guard)
        })
        .await
        .map_err(|e| IndexError::DatabaseError(format!("Task join error: {}", e)))?
    }

    /// Upsert a file entry
    pub async fn upsert_file(&self, file: FileEntry) -> Result<i64> {
        self.execute_blocking(move |db| db.upsert_file(&file)).await
    }

    /// Get a file by path
    pub async fn get_file_by_path(&self, path: String) -> Result<Option<FileEntry>> {
        self.execute_blocking(move |db| db.get_file_by_path(&path))
            .await
    }

    /// Upsert a chunk entry
    pub async fn upsert_chunk(&self, chunk: ChunkEntry) -> Result<i64> {
        self.execute_blocking(move |db| db.upsert_chunk(&chunk))
            .await
    }

    /// Get the latest manifest version for a folder
    pub async fn get_latest_manifest_version(&self, folder_id: String) -> Result<Option<u64>> {
        self.execute_blocking(move |db| Ok(db.get_latest_manifest_version(&folder_id)))
            .await
    }

    /// Save a manifest
    pub async fn save_manifest(&self, manifest: Manifest) -> Result<i64> {
        self.execute_blocking(move |db| db.save_manifest(&manifest))
            .await
    }

    /// Get the latest manifest for a folder
    pub async fn get_latest_manifest(&self, folder_id: String) -> Result<Option<Manifest>> {
        self.execute_blocking(move |db| db.get_latest_manifest(&folder_id))
            .await
    }

    /// Vacuum the database to reclaim space
    pub async fn vacuum(&self) -> Result<()> {
        self.execute_blocking(|db| db.vacuum()).await
    }
}

/// Builder for DatabasePool with configuration options
pub struct DatabasePoolBuilder {
    path: Option<PathBuf>,
}

impl DatabasePoolBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self { path: None }
    }

    /// Set the database file path
    pub fn path(mut self, path: impl AsRef<Path>) -> Self {
        self.path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Build the database pool
    pub fn build(self) -> Result<DatabasePool> {
        match self.path {
            Some(path) => DatabasePool::new(path),
            None => DatabasePool::new_in_memory(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_database_pool_operations() {
        let pool = DatabasePool::new_in_memory().unwrap();

        let file = FileEntry {
            id: None,
            path: "/test/file.txt".to_string(),
            size: 1024,
            modified_at: Utc::now(),
            content_hash: "abc123".to_string(),
            mode: Some(0o644),
        };

        // Test upsert
        let id = pool.upsert_file(file.clone()).await.unwrap();
        assert!(id > 0);

        // Test retrieval
        let retrieved = pool
            .get_file_by_path("/test/file.txt".to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.path, file.path);
        assert_eq!(retrieved.size, file.size);
    }

    #[tokio::test]
    async fn test_concurrent_operations() {
        let pool = DatabasePool::new_in_memory().unwrap();

        // Test multiple concurrent operations
        let mut handles = vec![];

        for i in 0..10 {
            let pool_clone = pool.clone();
            let handle = tokio::spawn(async move {
                let file = FileEntry {
                    id: None,
                    path: format!("/test/file{}.txt", i),
                    size: 1024 * (i as u64 + 1),
                    modified_at: Utc::now(),
                    content_hash: format!("hash{}", i),
                    mode: Some(0o644),
                };

                pool_clone.upsert_file(file).await
            });
            handles.push(handle);
        }

        // Wait for all operations to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
        }
    }
}
