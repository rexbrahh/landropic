use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, info};

use crate::errors::{IndexError, Result};
use crate::manifest::Manifest;
use crate::schema::{SCHEMA, SCHEMA_VERSION};

/// File entry in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: Option<i64>,
    pub path: String,
    pub size: u64,
    pub modified_at: DateTime<Utc>,
    pub content_hash: String,
    pub mode: Option<u32>,
}

/// Chunk entry in the database
#[derive(Debug, Clone)]
pub struct ChunkEntry {
    pub id: Option<i64>,
    pub hash: String,
    pub size: u64,
    pub ref_count: u64,
}

/// Index database for metadata storage
pub struct IndexDatabase {
    conn: Connection,
}

impl IndexDatabase {
    /// Open or create a database
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Enable foreign keys
        conn.execute("PRAGMA foreign_keys = ON", [])?;

        let mut db = Self { conn };
        db.initialize()?;

        Ok(db)
    }

    /// Create an in-memory database (for testing)
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute("PRAGMA foreign_keys = ON", [])?;

        let mut db = Self { conn };
        db.initialize()?;

        Ok(db)
    }

    /// Initialize the database schema
    fn initialize(&mut self) -> Result<()> {
        // Check schema version
        let version: Option<u32> = self
            .conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();

        match version {
            None => {
                // Fresh database, create schema
                info!("Initializing new database schema");
                self.conn.execute_batch(SCHEMA)?;
                self.conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
            }
            Some(v) if v < SCHEMA_VERSION => {
                // TODO: Implement migrations
                return Err(IndexError::SchemaVersionMismatch {
                    expected: SCHEMA_VERSION,
                    actual: v,
                });
            }
            Some(v) if v > SCHEMA_VERSION => {
                return Err(IndexError::SchemaVersionMismatch {
                    expected: SCHEMA_VERSION,
                    actual: v,
                });
            }
            _ => {
                debug!("Database schema up to date (version {})", SCHEMA_VERSION);
            }
        }

        Ok(())
    }

    /// Begin a database transaction for atomic operations.
    ///
    /// # Returns
    ///
    /// A transaction handle that can be used for multiple operations
    pub fn begin_transaction(&mut self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self.conn.transaction()?)
    }

    /// Insert or update a file
    pub fn upsert_file(&mut self, file: &FileEntry) -> Result<i64> {
        let tx = self.begin_transaction()?;

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
                file.mode,
            ],
            |row| row.get(0),
        )?;

        tx.commit()?;
        Ok(id)
    }

    /// Get a file by path
    pub fn get_file_by_path(&self, path: &str) -> Result<Option<FileEntry>> {
        let result = self.conn.query_row(
            "SELECT id, path, size, modified_at, content_hash, permissions FROM files WHERE path = ?1",
            params![path],
            |row| {
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
                        .with_timezone(&Utc),
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
    }

    /// Insert or update a chunk
    pub fn upsert_chunk(&mut self, chunk: &ChunkEntry) -> Result<i64> {
        let tx = self.begin_transaction()?;

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

        tx.commit()?;
        Ok(id)
    }

    /// Get the latest manifest version for a folder
    pub fn get_latest_manifest_version(&self, folder_id: &str) -> Option<u64> {
        self.conn
            .query_row(
                "SELECT MAX(version) FROM manifests WHERE folder_id = ?1",
                params![folder_id],
                |row| row.get(0),
            )
            .ok()
            .flatten()
    }

    /// Save a manifest
    pub fn save_manifest(&mut self, manifest: &Manifest) -> Result<i64> {
        let tx = self.begin_transaction()?;

        let manifest_id = tx.query_row(
            r#"
            INSERT INTO manifests (folder_id, version, manifest_hash, file_count, total_size)
            VALUES (?1, ?2, ?3, ?4, ?5)
            RETURNING id
            "#,
            params![
                manifest.folder_id,
                manifest.version as i64,
                manifest.manifest_hash.as_ref().unwrap_or(&String::new()),
                manifest.file_count() as i64,
                manifest.total_size() as i64,
            ],
            |row| row.get(0),
        )?;

        // TODO: Also insert manifest_files entries

        tx.commit()?;
        Ok(manifest_id)
    }

    /// Get the latest manifest for a folder.
    ///
    /// # Arguments
    ///
    /// * `folder_id` - The folder identifier
    ///
    /// # Returns
    ///
    /// The latest manifest if it exists, None otherwise
    pub fn get_latest_manifest(&self, _folder_id: &str) -> Result<Option<Manifest>> {
        // TODO: Implement manifest retrieval with proper joins
        Ok(None)
    }

    /// Vacuum the database to reclaim space
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute("VACUUM", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_database() {
        let db = IndexDatabase::open_in_memory().unwrap();
        assert!(db.get_file_by_path("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_file_upsert() {
        let mut db = IndexDatabase::open_in_memory().unwrap();

        let file = FileEntry {
            id: None,
            path: "/test/file.txt".to_string(),
            size: 1024,
            modified_at: Utc::now(),
            content_hash: "abc123".to_string(),
            mode: Some(0o644),
        };

        let id = db.upsert_file(&file).unwrap();
        assert!(id > 0);

        let retrieved = db.get_file_by_path("/test/file.txt").unwrap().unwrap();
        assert_eq!(retrieved.path, file.path);
        assert_eq!(retrieved.size, file.size);
    }
}
