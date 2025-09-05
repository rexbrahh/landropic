/// Database schema version
pub const SCHEMA_VERSION: u32 = 1;

/// SQL schema for the index database
pub const SCHEMA: &str = r#"
-- Enable WAL mode for better concurrency
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA cache_size = -64000; -- 64MB cache
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456; -- 256MB mmap
PRAGMA foreign_keys = ON;

-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Files table with optimized structure
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    modified_at TIMESTAMP NOT NULL,
    content_hash TEXT NOT NULL,
    permissions INTEGER, -- Renamed from 'mode' to be clearer
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Optimized indexes for files table
CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
CREATE INDEX IF NOT EXISTS idx_files_content_hash ON files(content_hash);
CREATE INDEX IF NOT EXISTS idx_files_modified_at ON files(modified_at); -- For time-based queries
CREATE INDEX IF NOT EXISTS idx_files_size ON files(size); -- For size-based queries

-- Chunks table with reference counting for garbage collection
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    hash TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    ref_count INTEGER DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_chunks_hash ON chunks(hash);
CREATE INDEX IF NOT EXISTS idx_chunks_ref_count ON chunks(ref_count); -- For garbage collection
CREATE INDEX IF NOT EXISTS idx_chunks_size ON chunks(size); -- For size analysis

-- File-chunk mapping table with chunk metadata
CREATE TABLE IF NOT EXISTS file_chunks (
    file_id INTEGER NOT NULL,
    chunk_id INTEGER NOT NULL,
    chunk_order INTEGER NOT NULL,
    chunk_offset INTEGER NOT NULL, -- Offset within the file
    chunk_length INTEGER NOT NULL, -- Length of this chunk in the file
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE,
    FOREIGN KEY (chunk_id) REFERENCES chunks(id),
    PRIMARY KEY (file_id, chunk_order)
);

CREATE INDEX IF NOT EXISTS idx_file_chunks_file_id ON file_chunks(file_id);
CREATE INDEX IF NOT EXISTS idx_file_chunks_chunk_id ON file_chunks(chunk_id);

-- Manifests table with enhanced metadata
CREATE TABLE IF NOT EXISTS manifests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id TEXT NOT NULL, -- Device that created this manifest
    folder_path TEXT NOT NULL, -- Local folder path being synced
    version INTEGER NOT NULL,
    manifest_hash TEXT NOT NULL,
    file_count INTEGER NOT NULL,
    total_size INTEGER NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(device_id, folder_path, version)
);

-- Indexes for efficient manifest operations
CREATE INDEX IF NOT EXISTS idx_manifests_device_id ON manifests(device_id);
CREATE INDEX IF NOT EXISTS idx_manifests_folder_path ON manifests(folder_path);
CREATE INDEX IF NOT EXISTS idx_manifests_version ON manifests(device_id, folder_path, version);
CREATE INDEX IF NOT EXISTS idx_manifests_manifest_hash ON manifests(manifest_hash);
CREATE INDEX IF NOT EXISTS idx_manifests_created_at ON manifests(created_at);

-- Manifest files table with chunk list storage
CREATE TABLE IF NOT EXISTS manifest_files (
    manifest_id INTEGER NOT NULL,
    file_id INTEGER NOT NULL,
    chunk_list TEXT NOT NULL, -- JSON array of chunk hashes in order
    FOREIGN KEY (manifest_id) REFERENCES manifests(id) ON DELETE CASCADE,
    FOREIGN KEY (file_id) REFERENCES files(id),
    PRIMARY KEY (manifest_id, file_id)
);

-- Peers table with network addresses and enhanced metadata
CREATE TABLE IF NOT EXISTS peers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL, -- Renamed from device_name for consistency
    addresses TEXT NOT NULL, -- JSON array of network addresses
    last_seen TIMESTAMP,
    trusted BOOLEAN DEFAULT 0,
    public_key TEXT, -- Ed25519 public key for verification
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_peers_device_id ON peers(device_id);
CREATE INDEX IF NOT EXISTS idx_peers_last_seen ON peers(last_seen); -- For cleanup/status
CREATE INDEX IF NOT EXISTS idx_peers_trusted ON peers(trusted); -- For trusted peer filtering

-- Sync state tracking table for folder synchronization
CREATE TABLE IF NOT EXISTS sync_state (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_path TEXT NOT NULL UNIQUE,
    device_id TEXT NOT NULL, -- Our device ID
    local_version INTEGER NOT NULL DEFAULT 0,
    last_sync_at TIMESTAMP,
    sync_in_progress BOOLEAN DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (device_id) REFERENCES peers(device_id)
);

CREATE INDEX IF NOT EXISTS idx_sync_state_folder_path ON sync_state(folder_path);
CREATE INDEX IF NOT EXISTS idx_sync_state_device_id ON sync_state(device_id);
CREATE INDEX IF NOT EXISTS idx_sync_state_sync_in_progress ON sync_state(sync_in_progress);
"#;
