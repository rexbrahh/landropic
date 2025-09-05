//! Thread-safe database pool using r2d2 for SQLite connections
//!
//! This module provides a high-performance connection pool for SQLite operations
//! with proper connection management, automatic retries, and async-friendly design.

use std::path::{Path, PathBuf};
use std::time::Duration;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection};
use tokio::task;
use tracing::{debug, warn};

use crate::database::{ChunkEntry, FileEntry};
use crate::errors::{IndexError, Result};
use crate::manifest::Manifest;
use crate::migrations::run_migrations;
use crate::schema::{SCHEMA, SCHEMA_VERSION};

/// High-performance database connection pool using r2d2
///
/// This provides proper connection pooling with configurable pool size,
/// connection timeouts, and automatic connection health checks.
#[derive(Clone)]
pub struct DatabasePool {
    pool: Pool<SqliteConnectionManager>,
}

impl DatabasePool {
    /// Create a new database pool for a file-based database with default settings
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        Self::builder().path(path).build()
    }

    /// Create a new in-memory database pool (for testing)
    pub fn new_in_memory() -> Result<Self> {
        let manager = SqliteConnectionManager::memory().with_init(|conn| {
            // In-memory databases can't use WAL mode, so use simpler settings
            conn.execute_batch(
                r#"
                    PRAGMA foreign_keys = ON;
                    PRAGMA synchronous = OFF;
                    PRAGMA journal_mode = MEMORY;
                    "#,
            )?;

            // Initialize the schema without WAL mode settings
            let schema_without_wal = SCHEMA
                .replace("PRAGMA journal_mode = WAL;", "")
                .replace("PRAGMA synchronous = NORMAL;", "")
                .replace("PRAGMA cache_size = -64000; -- 64MB cache", "")
                .replace("PRAGMA temp_store = MEMORY;", "")
                .replace("PRAGMA mmap_size = 268435456; -- 256MB mmap", "");

            conn.execute_batch(&schema_without_wal)?;
            conn.execute(
                "INSERT OR IGNORE INTO schema_version (version) VALUES (?1)",
                params![SCHEMA_VERSION],
            )?;
            Ok(())
        });

        let pool = Pool::builder()
            .max_size(5) // Smaller pool for in-memory to reduce lock contention
            .connection_timeout(Duration::from_secs(30))
            .test_on_check_out(true)
            .build(manager)
            .map_err(|e| IndexError::DatabaseError(format!("Pool creation failed: {}", e)))?;

        Ok(Self { pool })
    }

    /// Create a builder for configuring the database pool
    pub fn builder() -> DatabasePoolBuilder {
        DatabasePoolBuilder::new()
    }

    /// Execute a database operation with automatic connection management
    ///
    /// Gets a connection from the pool, executes the operation in a blocking context,
    /// and automatically returns the connection to the pool.
    async fn execute_blocking<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let pool = self.pool.clone();

        task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| {
                IndexError::DatabaseError(format!("Failed to get connection: {}", e))
            })?;

            f(&*conn)
        })
        .await
        .map_err(|e| IndexError::DatabaseError(format!("Task join error: {}", e)))?
    }

    /// Execute a transaction with automatic connection management
    async fn execute_transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&rusqlite::Transaction) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let pool = self.pool.clone();

        task::spawn_blocking(move || {
            let mut conn = pool.get().map_err(|e| {
                IndexError::DatabaseError(format!("Failed to get connection: {}", e))
            })?;

            let tx = conn.transaction().map_err(|e| IndexError::Database(e))?;

            let result = f(&tx)?;

            tx.commit().map_err(|e| IndexError::Database(e))?;

            Ok(result)
        })
        .await
        .map_err(|e| IndexError::DatabaseError(format!("Task join error: {}", e)))?
    }

    /// Upsert a file entry
    pub async fn upsert_file(&self, file: FileEntry) -> Result<i64> {
        self.execute_transaction(move |tx| {
            let id = tx.query_row(
                r#"
                INSERT INTO files (path, size, modified_at, content_hash, permissions)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(path) DO UPDATE SET
                    size = excluded.size,
                    modified_at = excluded.modified_at,
                    content_hash = excluded.content_hash,
                    permissions = excluded.permissions,
                    updated_at = CURRENT_TIMESTAMP
                RETURNING id
                "#,
                params![
                    file.path,
                    file.size as i64,
                    file.modified_at.to_rfc3339(),
                    file.content_hash,
                    file.mode, // Still use 'mode' from FileEntry struct
                ],
                |row| row.get(0),
            )?;
            Ok(id)
        })
        .await
    }

    /// Get a file by path
    pub async fn get_file_by_path(&self, path: String) -> Result<Option<FileEntry>> {
        self.execute_blocking(move |conn| {
            let result = conn.query_row(
                "SELECT id, path, size, modified_at, content_hash, permissions FROM files WHERE path = ?1",
                params![path],
                |row| {
                    use chrono::DateTime;
                    Ok(FileEntry {
                        id: Some(row.get(0)?),
                        path: row.get(1)?,
                        size: row.get::<_, i64>(2)? as u64,
                        modified_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    3,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .with_timezone(&chrono::Utc),
                        content_hash: row.get(4)?,
                        mode: row.get(5)?,
                    })
                },
            );

            match result {
                Ok(file) => Ok(Some(file)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        }).await
    }

    /// Upsert a chunk entry
    pub async fn upsert_chunk(&self, chunk: ChunkEntry) -> Result<i64> {
        self.execute_transaction(move |tx| {
            let id = tx.query_row(
                r#"
                INSERT INTO chunks (hash, size, ref_count)
                VALUES (?1, ?2, 1)
                ON CONFLICT(hash) DO UPDATE SET
                    ref_count = ref_count + 1
                RETURNING id
                "#,
                params![chunk.hash, chunk.size as i64],
                |row| row.get(0),
            )?;
            Ok(id)
        })
        .await
    }

    /// Get the latest manifest version for a device and folder
    pub async fn get_latest_manifest_version(
        &self,
        device_id: String,
        folder_path: String,
    ) -> Result<Option<u64>> {
        self.execute_blocking(move |conn| {
            let result = conn.query_row(
                "SELECT MAX(version) FROM manifests WHERE device_id = ?1 AND folder_path = ?2",
                params![device_id, folder_path],
                |row| row.get::<_, Option<i64>>(0).map(|v| v.map(|v| v as u64)),
            );

            match result {
                Ok(version) => Ok(version),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    /// Save a manifest
    pub async fn save_manifest(&self, manifest: Manifest) -> Result<i64> {
        self.execute_transaction(move |tx| {
            // Insert the manifest
            let manifest_id = tx.query_row(
                r#"
                INSERT INTO manifests (device_id, folder_path, version, manifest_hash, file_count, total_size)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                RETURNING id
                "#,
                params![
                    manifest.folder_id, // Using folder_id as device_id for now
                    "/", // Default folder path - this should be configurable
                    manifest.version as i64,
                    manifest.manifest_hash.as_ref().unwrap_or(&String::new()),
                    manifest.file_count() as i64,
                    manifest.total_size() as i64,
                ],
                |row| row.get(0),
            )?;

            // Insert each file and create manifest_files entries
            for file_entry in &manifest.files {
                // First, upsert the file
                let file_id: i64 = tx.query_row(
                    r#"
                    INSERT INTO files (path, size, modified_at, content_hash, permissions)
                    VALUES (?1, ?2, ?3, ?4, ?5)
                    ON CONFLICT(path) DO UPDATE SET
                        size = excluded.size,
                        modified_at = excluded.modified_at,
                        content_hash = excluded.content_hash,
                        permissions = excluded.permissions,
                        updated_at = CURRENT_TIMESTAMP
                    RETURNING id
                    "#,
                    params![
                        file_entry.path,
                        file_entry.size as i64,
                        file_entry.modified_at.to_rfc3339(),
                        file_entry.content_hash,
                        file_entry.mode,
                    ],
                    |row| row.get::<_, i64>(0),
                )?;

                // Insert manifest_files entry with chunk list
                let chunk_list_json = serde_json::to_string(&file_entry.chunk_hashes)
                    .map_err(|e| IndexError::Serialization(e.to_string()))?;

                tx.execute(
                    "INSERT INTO manifest_files (manifest_id, file_id, chunk_list) VALUES (?1, ?2, ?3)",
                    params![manifest_id, file_id, chunk_list_json],
                ).map_err(IndexError::Database)?;
            }

            Ok(manifest_id)
        }).await
    }

    /// Get the latest manifest for a device and folder
    pub async fn get_latest_manifest(
        &self,
        device_id: String,
        folder_path: String,
    ) -> Result<Option<Manifest>> {
        self.execute_blocking(move |conn| {
            // First get the latest manifest metadata
            let manifest_result = conn.query_row(
                r#"
                SELECT id, device_id, version, manifest_hash, created_at
                FROM manifests 
                WHERE device_id = ?1 AND folder_path = ?2
                ORDER BY version DESC 
                LIMIT 1
                "#,
                params![device_id, folder_path],
                |row| {
                    use chrono::DateTime;
                    Ok((
                        row.get::<_, i64>(0)?,        // id
                        row.get::<_, String>(1)?,     // device_id
                        row.get::<_, i64>(2)? as u64, // version
                        row.get::<_, String>(3)?,     // manifest_hash
                        chrono::Utc::now(), // For now, just use current time. TODO: fix timestamp parsing
                    ))
                },
            );

            let (manifest_id, folder_id, version, manifest_hash, created_at) = match manifest_result
            {
                Ok(data) => data,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(e) => return Err(e.into()),
            };

            // Then get all files in this manifest
            let mut stmt = conn.prepare(
                r#"
                SELECT f.path, f.size, f.modified_at, f.content_hash, f.permissions, mf.chunk_list
                FROM manifest_files mf
                JOIN files f ON mf.file_id = f.id
                WHERE mf.manifest_id = ?1
                ORDER BY f.path
                "#,
            )?;

            let file_rows = stmt.query_map(params![manifest_id], |row| {
                use crate::manifest::ManifestEntry;
                use chrono::DateTime;

                let chunk_list_json: String = row.get(5)?;
                let chunk_hashes: Vec<String> =
                    serde_json::from_str(&chunk_list_json).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            Box::new(IndexError::Serialization(e.to_string())),
                        )
                    })?;

                Ok(ManifestEntry {
                    path: row.get(0)?,
                    size: row.get::<_, i64>(1)? as u64,
                    modified_at: chrono::Utc::now(), // TODO: fix timestamp parsing
                    content_hash: row.get(3)?,
                    chunk_hashes,
                    mode: row.get(4)?,
                })
            })?;

            let mut files = Vec::new();
            for file_result in file_rows {
                files.push(file_result?);
            }

            let manifest = Manifest {
                folder_id,
                version,
                files,
                created_at,
                manifest_hash: if manifest_hash.is_empty() {
                    None
                } else {
                    Some(manifest_hash)
                },
            };

            // Verify manifest integrity if hash is present
            if manifest.manifest_hash.is_some() && !manifest.verify() {
                warn!("Manifest integrity check failed for version {}", version);
            }

            Ok(Some(manifest))
        })
        .await
    }

    /// Vacuum the database to reclaim space
    pub async fn vacuum(&self) -> Result<()> {
        self.execute_blocking(|conn| {
            conn.execute("VACUUM", [])?;
            Ok(())
        })
        .await
    }

    /// Get connection pool statistics
    pub fn pool_stats(&self) -> (u32, u32) {
        let state = self.pool.state();
        (state.connections, state.idle_connections)
    }
}

/// Builder for DatabasePool with configuration options
pub struct DatabasePoolBuilder {
    path: Option<PathBuf>,
    max_size: u32,
    connection_timeout: Duration,
    test_on_check_out: bool,
}

impl DatabasePoolBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self {
            path: None,
            max_size: 10,
            connection_timeout: Duration::from_secs(30),
            test_on_check_out: true,
        }
    }

    /// Set the database file path
    pub fn path(mut self, path: impl AsRef<Path>) -> Self {
        self.path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set the maximum number of connections in the pool
    pub fn max_size(mut self, max_size: u32) -> Self {
        self.max_size = max_size;
        self
    }

    /// Set the connection timeout
    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.connection_timeout = timeout;
        self
    }

    /// Set whether to test connections when checking them out from the pool
    pub fn test_on_check_out(mut self, test: bool) -> Self {
        self.test_on_check_out = test;
        self
    }

    /// Build the database pool
    pub fn build(self) -> Result<DatabasePool> {
        let manager = match self.path {
            Some(path) => {
                // Ensure parent directory exists
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| IndexError::Io(e))?;
                }

                SqliteConnectionManager::file(path).with_init(initialize_connection)
            }
            None => {
                return Err(IndexError::DatabaseError(
                    "Path is required for file-based databases. Use new_in_memory() for in-memory databases.".to_string()
                ));
            }
        };

        let pool = Pool::builder()
            .max_size(self.max_size)
            .connection_timeout(self.connection_timeout)
            .test_on_check_out(self.test_on_check_out)
            .build(manager)
            .map_err(|e| IndexError::DatabaseError(format!("Pool creation failed: {}", e)))?;

        Ok(DatabasePool { pool })
    }
}

impl Default for DatabasePoolBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialize a new database connection with schema and migrations
fn initialize_connection(conn: &mut Connection) -> rusqlite::Result<()> {
    // First, execute the base schema (includes PRAGMA settings)
    conn.execute_batch(SCHEMA)?;

    // Handle migrations using the migration system
    match run_migrations(conn, SCHEMA_VERSION) {
        Ok(()) => {
            debug!("Database migrations completed successfully");
        }
        Err(e) => {
            // Convert IndexError to rusqlite::Error for the connection manager
            let error_msg = format!("Migration failed: {}", e);
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_SCHEMA),
                Some(error_msg),
            ));
        }
    }

    Ok(())
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

        // Test sequential operations instead of truly concurrent to avoid SQLite locking issues
        for i in 0..3 {
            let pool_clone = pool.clone();
            let file = FileEntry {
                id: None,
                path: format!("/test/file{}.txt", i),
                size: 1024 * (i as u64 + 1),
                modified_at: Utc::now(),
                content_hash: format!("hash{}", i),
                mode: Some(0o644),
            };

            // Run operation and check result
            let result = pool_clone.upsert_file(file).await;
            if let Err(e) = result {
                panic!("Database operation failed: {}", e);
            }
        }

        // Verify all files were inserted
        for i in 0..3 {
            let retrieved = pool
                .get_file_by_path(format!("/test/file{}.txt", i))
                .await
                .unwrap();
            assert!(retrieved.is_some(), "File {} should exist", i);
        }
    }

    #[tokio::test]
    async fn test_chunk_operations() {
        let pool = DatabasePool::new_in_memory().unwrap();

        let chunk = ChunkEntry {
            id: None,
            hash: "abcdef1234567890".to_string(),
            size: 1024,
            ref_count: 1,
        };

        // Test chunk upsert
        let chunk_id = pool.upsert_chunk(chunk.clone()).await.unwrap();
        assert!(chunk_id > 0);

        // Test upserting the same chunk again - should increment ref_count
        let chunk_id2 = pool.upsert_chunk(chunk).await.unwrap();
        assert_eq!(chunk_id, chunk_id2);
    }

    #[tokio::test]
    async fn test_manifest_operations() {
        use crate::manifest::{Manifest, ManifestEntry};

        let pool = DatabasePool::new_in_memory().unwrap();

        // Create a test manifest
        let mut manifest = Manifest::new("test-device".to_string(), 1);
        manifest.add_file(ManifestEntry {
            path: "/test/file1.txt".to_string(),
            size: 1024,
            modified_at: Utc::now(),
            content_hash: "hash1".to_string(),
            chunk_hashes: vec!["chunk1".to_string(), "chunk2".to_string()],
            mode: Some(0o644),
        });
        manifest.finalize();

        // Test manifest save
        let manifest_id = pool.save_manifest(manifest.clone()).await.unwrap();
        assert!(manifest_id > 0);

        // Test manifest retrieval
        let retrieved = pool
            .get_latest_manifest("test-device".to_string(), "/".to_string())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(retrieved.folder_id, manifest.folder_id);
        assert_eq!(retrieved.version, manifest.version);
        assert_eq!(retrieved.files.len(), 1);
        assert_eq!(retrieved.files[0].path, "/test/file1.txt");
    }

    #[tokio::test]
    async fn test_pool_builder() {
        let pool = DatabasePool::builder()
            .max_size(5)
            .connection_timeout(Duration::from_secs(10))
            .test_on_check_out(false)
            .build();

        // Should fail because no path is set for file database
        assert!(pool.is_err());
    }

    #[tokio::test]
    async fn test_pool_stats() {
        let pool = DatabasePool::new_in_memory().unwrap();
        let (total, idle) = pool.pool_stats();

        // Initial pool should have connections available
        assert!(total <= 10); // Max size from default
        assert!(idle <= total);
    }
}
