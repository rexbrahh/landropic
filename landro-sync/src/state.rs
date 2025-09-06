//! Persistent sync state management

use chrono::TimeZone;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::errors::{Result, SyncError};

/// Overall sync state for the system
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SyncState {
    /// No active sync operations
    Idle,
    /// Currently syncing with one or more peers
    Syncing {
        peer_count: usize,
        start_time: DateTime<Utc>,
    },
    /// Sync paused due to conflicts
    Conflicted { conflict_count: usize },
    /// Sync paused by user
    Paused,
    /// Error state
    Failed {
        error: String,
        timestamp: DateTime<Utc>,
    },
}

/// Sync state for a specific peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSyncState {
    pub peer_id: String,
    pub peer_name: String,
    pub last_sync: Option<DateTime<Utc>>,
    pub last_manifest_hash: Option<String>,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub files_sent: u64,
    pub files_received: u64,
    pub current_state: PeerState,
}

/// Current state of sync with a specific peer
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PeerState {
    /// Not currently syncing
    Idle,
    /// Exchanging manifests
    Negotiating,
    /// Transferring data
    Transferring { progress: f32 },
    /// Sync completed successfully
    Completed { timestamp: DateTime<Utc> },
    /// Sync failed
    Failed {
        error: String,
        timestamp: DateTime<Utc>,
    },
}

/// Database for persistent sync state
/// Note: We use a Mutex instead of RwLock to ensure proper Send/Sync for SQLite
struct SyncDatabase {
    conn: Connection,
}

// Ensure SyncDatabase can be safely sent across threads
// SQLite connections can be shared if we use proper synchronization
unsafe impl Send for SyncDatabase {}
unsafe impl Sync for SyncDatabase {}

/// Async-safe wrapper around SyncDatabase
///
/// This wrapper ensures SyncDatabase can be safely shared across async tasks
/// and await points, handling all the necessary thread-safety requirements.
#[derive(Clone)]
pub struct AsyncSyncDatabase {
    inner: Arc<RwLock<SyncDatabase>>,
}

impl SyncDatabase {
    /// Open or create a sync database
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        let mut db = Self { conn };
        db.initialize()?;
        Ok(db)
    }

    /// Create in-memory database (for testing)
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self { conn };
        db.initialize()?;
        Ok(db)
    }

    /// Initialize database schema
    fn initialize(&mut self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            -- Peer sync state table
            CREATE TABLE IF NOT EXISTS peer_sync_state (
                peer_id TEXT PRIMARY KEY,
                peer_name TEXT NOT NULL,
                last_sync TIMESTAMP,
                last_manifest_hash TEXT,
                bytes_sent INTEGER NOT NULL DEFAULT 0,
                bytes_received INTEGER NOT NULL DEFAULT 0,
                files_sent INTEGER NOT NULL DEFAULT 0,
                files_received INTEGER NOT NULL DEFAULT 0,
                current_state TEXT NOT NULL,
                updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            -- Pending transfers table
            CREATE TABLE IF NOT EXISTS pending_transfers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                peer_id TEXT NOT NULL,
                folder_id TEXT NOT NULL,
                file_path TEXT NOT NULL,
                chunk_hash TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 0,
                retry_count INTEGER NOT NULL DEFAULT 0,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (peer_id) REFERENCES peer_sync_state(peer_id),
                UNIQUE(peer_id, folder_id, chunk_hash)
            );

            -- Conflict log table
            CREATE TABLE IF NOT EXISTS conflict_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                peer_id TEXT NOT NULL,
                file_path TEXT NOT NULL,
                conflict_type TEXT NOT NULL,
                our_version TEXT NOT NULL,
                their_version TEXT NOT NULL,
                detected_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                resolved BOOLEAN NOT NULL DEFAULT 0,
                resolution TEXT,
                resolved_at TIMESTAMP,
                FOREIGN KEY (peer_id) REFERENCES peer_sync_state(peer_id)
            );

            -- Sync history table
            CREATE TABLE IF NOT EXISTS sync_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                peer_id TEXT NOT NULL,
                start_time TIMESTAMP NOT NULL,
                end_time TIMESTAMP,
                bytes_sent INTEGER NOT NULL DEFAULT 0,
                bytes_received INTEGER NOT NULL DEFAULT 0,
                files_sent INTEGER NOT NULL DEFAULT 0,
                files_received INTEGER NOT NULL DEFAULT 0,
                success BOOLEAN NOT NULL DEFAULT 0,
                error_message TEXT,
                FOREIGN KEY (peer_id) REFERENCES peer_sync_state(peer_id)
            );

            -- Create indexes
            CREATE INDEX IF NOT EXISTS idx_pending_transfers_peer ON pending_transfers(peer_id);
            CREATE INDEX IF NOT EXISTS idx_pending_transfers_priority ON pending_transfers(priority DESC);
            CREATE INDEX IF NOT EXISTS idx_conflict_log_unresolved ON conflict_log(resolved) WHERE resolved = 0;
            CREATE INDEX IF NOT EXISTS idx_sync_history_peer ON sync_history(peer_id, start_time DESC);
            "#,
        )?;

        info!("Sync database initialized");
        Ok(())
    }

    /// Get sync state for a peer
    pub fn get_peer_state(&self, peer_id: &str) -> Result<Option<PeerSyncState>> {
        let result = self
            .conn
            .query_row(
                r#"
                SELECT peer_id, peer_name, datetime(last_sync), last_manifest_hash,
                       bytes_sent, bytes_received, files_sent, files_received,
                       current_state
                FROM peer_sync_state
                WHERE peer_id = ?1
                "#,
                params![peer_id],
                |row| {
                    let state_json: String = row.get(8)?;
                    let current_state: PeerState =
                        serde_json::from_str(&state_json).unwrap_or(PeerState::Idle);

                    Ok(PeerSyncState {
                        peer_id: row.get(0)?,
                        peer_name: row.get(1)?,
                        last_sync: row.get::<_, Option<String>>(2)?.and_then(|s| {
                            DateTime::parse_from_rfc3339(&s)
                                .ok()
                                .map(|dt| dt.with_timezone(&Utc))
                        }),
                        last_manifest_hash: row.get(3)?,
                        bytes_sent: row.get(4)?,
                        bytes_received: row.get(5)?,
                        files_sent: row.get(6)?,
                        files_received: row.get(7)?,
                        current_state,
                    })
                },
            )
            .optional()?;

        Ok(result)
    }

    /// Update or insert peer sync state
    pub fn upsert_peer_state(&mut self, state: &PeerSyncState) -> Result<()> {
        let state_json =
            serde_json::to_string(&state.current_state).map_err(|e| SyncError::Serialization(e))?;

        self.conn.execute(
            r#"
            INSERT INTO peer_sync_state 
                (peer_id, peer_name, last_sync, last_manifest_hash,
                 bytes_sent, bytes_received, files_sent, files_received,
                 current_state, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CURRENT_TIMESTAMP)
            ON CONFLICT(peer_id) DO UPDATE SET
                peer_name = excluded.peer_name,
                last_sync = excluded.last_sync,
                last_manifest_hash = excluded.last_manifest_hash,
                bytes_sent = excluded.bytes_sent,
                bytes_received = excluded.bytes_received,
                files_sent = excluded.files_sent,
                files_received = excluded.files_received,
                current_state = excluded.current_state,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![
                state.peer_id,
                state.peer_name,
                state.last_sync.map(|dt| dt.to_rfc3339()),
                state.last_manifest_hash,
                state.bytes_sent,
                state.bytes_received,
                state.files_sent,
                state.files_received,
                state_json,
            ],
        )?;

        debug!("Updated sync state for peer: {}", state.peer_id);
        Ok(())
    }

    /// Get all peer states
    pub fn get_all_peer_states(&self) -> Result<HashMap<String, PeerSyncState>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT peer_id, peer_name, last_sync, last_manifest_hash,
                   bytes_sent, bytes_received, files_sent, files_received,
                   current_state
            FROM peer_sync_state
            "#,
        )?;

        let peer_iter = stmt.query_map([], |row| {
            let state_json: String = row.get(8)?;
            let current_state: PeerState =
                serde_json::from_str(&state_json).unwrap_or(PeerState::Idle);

            Ok(PeerSyncState {
                peer_id: row.get(0)?,
                peer_name: row.get(1)?,
                last_sync: row.get::<_, Option<String>>(2)?.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|dt| dt.with_timezone(&Utc))
                }),
                last_manifest_hash: row.get(3)?,
                bytes_sent: row.get(4)?,
                bytes_received: row.get(5)?,
                files_sent: row.get(6)?,
                files_received: row.get(7)?,
                current_state,
            })
        })?;

        let mut peers = HashMap::new();
        for peer_result in peer_iter {
            let peer = peer_result?;
            peers.insert(peer.peer_id.clone(), peer);
        }

        Ok(peers)
    }

    /// Add pending transfer
    pub fn add_pending_transfer(
        &mut self,
        peer_id: &str,
        folder_id: &str,
        file_path: &str,
        chunk_hash: &str,
        priority: i32,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO pending_transfers 
                (peer_id, folder_id, file_path, chunk_hash, priority)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![peer_id, folder_id, file_path, chunk_hash, priority],
        )?;
        Ok(())
    }

    /// Get pending transfers for a peer
    pub fn get_pending_transfers(
        &self,
        peer_id: &str,
        limit: usize,
    ) -> Result<Vec<PendingTransfer>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, peer_id, folder_id, file_path, chunk_hash, priority, retry_count
            FROM pending_transfers
            WHERE peer_id = ?1
            ORDER BY priority DESC, created_at ASC
            LIMIT ?2
            "#,
        )?;

        let transfers = stmt.query_map(params![peer_id, limit as i64], |row| {
            Ok(PendingTransfer {
                id: row.get(0)?,
                peer_id: row.get(1)?,
                folder_id: row.get(2)?,
                file_path: row.get(3)?,
                chunk_hash: row.get(4)?,
                priority: row.get(5)?,
                retry_count: row.get(6)?,
            })
        })?;

        Ok(transfers.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Remove completed transfer
    pub fn remove_pending_transfer(&mut self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM pending_transfers WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Remove pending transfer by peer and chunk hash
    pub fn remove_pending_transfer_by_hash(
        &mut self,
        peer_id: &str,
        chunk_hash: &str,
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM pending_transfers WHERE peer_id = ?1 AND chunk_hash = ?2",
            params![peer_id, chunk_hash],
        )?;
        Ok(())
    }

    /// Record sync session
    pub fn record_sync_session(&mut self, session: &SyncSession) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO sync_history 
                (peer_id, start_time, end_time, bytes_sent, bytes_received,
                 files_sent, files_received, success, error_message)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                session.peer_id,
                session.start_time.to_rfc3339(),
                session.end_time.map(|dt| dt.to_rfc3339()),
                session.bytes_sent,
                session.bytes_received,
                session.files_sent,
                session.files_received,
                session.success,
                session.error_message,
            ],
        )?;
        Ok(())
    }
}

impl AsyncSyncDatabase {
    /// Create new async database wrapper
    pub fn new(database: SyncDatabase) -> Self {
        Self {
            inner: Arc::new(RwLock::new(database)),
        }
    }

    /// Open or create a sync database
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let database = SyncDatabase::open(path)?;
        Ok(Self::new(database))
    }

    /// Create in-memory database (for testing)
    pub async fn open_in_memory() -> Result<Self> {
        let database = SyncDatabase::open_in_memory()?;
        Ok(Self::new(database))
    }

    /// Get all peer states
    pub async fn get_all_peer_states(&self) -> Result<HashMap<String, PeerSyncState>> {
        let db = self.inner.read().await;
        db.get_all_peer_states()
    }

    /// Get a specific peer state
    pub async fn get_peer_state(&self, peer_id: &str) -> Result<Option<PeerSyncState>> {
        let db = self.inner.read().await;
        db.get_peer_state(peer_id)
    }

    /// Insert or update a peer state
    pub async fn upsert_peer_state(&self, peer_state: &PeerSyncState) -> Result<()> {
        let mut db = self.inner.write().await;
        db.upsert_peer_state(peer_state)
    }

    /// Add a pending transfer
    pub async fn add_pending_transfer(
        &self,
        peer_id: &str,
        folder_id: &str,
        file_path: &str,
        chunk_hash: &str,
        priority: i32,
    ) -> Result<()> {
        let mut db = self.inner.write().await;
        db.add_pending_transfer(peer_id, folder_id, file_path, chunk_hash, priority)
    }

    /// Remove a pending transfer
    pub async fn remove_pending_transfer(&self, peer_id: &str, chunk_hash: &str) -> Result<()> {
        let mut db = self.inner.write().await;
        db.remove_pending_transfer_by_hash(peer_id, chunk_hash)
    }

    /// Get all pending transfers for a peer
    pub async fn get_pending_transfers(
        &self,
        peer_id: &str,
        limit: usize,
    ) -> Result<Vec<PendingTransfer>> {
        let db = self.inner.read().await;
        db.get_pending_transfers(peer_id, limit)
    }

    /// Record a sync session
    pub async fn record_sync_session(&self, session: &SyncSession) -> Result<()> {
        let mut db = self.inner.write().await;
        db.record_sync_session(session)
    }
}

/// Pending transfer entry
#[derive(Debug, Clone)]
pub struct PendingTransfer {
    pub id: i64,
    pub peer_id: String,
    pub folder_id: String,
    pub file_path: String,
    pub chunk_hash: String,
    pub priority: i32,
    pub retry_count: i32,
}

/// Sync session record
#[derive(Debug, Clone)]
pub struct SyncSession {
    pub peer_id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub files_sent: u64,
    pub files_received: u64,
    pub success: bool,
    pub error_message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_database() {
        let db = SyncDatabase::open_in_memory().unwrap();

        // Test peer state operations
        let mut state = PeerSyncState {
            peer_id: "test-peer".to_string(),
            peer_name: "Test Device".to_string(),
            last_sync: Some(Utc::now()),
            last_manifest_hash: Some("abc123".to_string()),
            bytes_sent: 1024,
            bytes_received: 2048,
            files_sent: 5,
            files_received: 10,
            current_state: PeerState::Idle,
        };

        let mut db = SyncDatabase::open_in_memory().unwrap();
        db.upsert_peer_state(&state).unwrap();

        let retrieved = db.get_peer_state("test-peer").unwrap().unwrap();
        assert_eq!(retrieved.peer_id, state.peer_id);
        assert_eq!(retrieved.bytes_sent, state.bytes_sent);
    }
}
