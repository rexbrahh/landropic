/// Database schema version
pub const SCHEMA_VERSION: u32 = 1;

/// SQL schema for the index database
pub const SCHEMA: &str = r#"
-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Files table
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    modified_at TIMESTAMP NOT NULL,
    content_hash TEXT NOT NULL,
    mode INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
CREATE INDEX IF NOT EXISTS idx_files_content_hash ON files(content_hash);

-- Chunks table
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    hash TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    ref_count INTEGER DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_chunks_hash ON chunks(hash);

-- File-chunk mapping table
CREATE TABLE IF NOT EXISTS file_chunks (
    file_id INTEGER NOT NULL,
    chunk_id INTEGER NOT NULL,
    chunk_order INTEGER NOT NULL,
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE,
    FOREIGN KEY (chunk_id) REFERENCES chunks(id),
    PRIMARY KEY (file_id, chunk_order)
);

CREATE INDEX IF NOT EXISTS idx_file_chunks_file_id ON file_chunks(file_id);
CREATE INDEX IF NOT EXISTS idx_file_chunks_chunk_id ON file_chunks(chunk_id);

-- Manifests table
CREATE TABLE IF NOT EXISTS manifests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    manifest_hash TEXT NOT NULL,
    file_count INTEGER NOT NULL,
    total_size INTEGER NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(folder_id, version)
);

CREATE INDEX IF NOT EXISTS idx_manifests_folder_id ON manifests(folder_id);
CREATE INDEX IF NOT EXISTS idx_manifests_manifest_hash ON manifests(manifest_hash);

-- Manifest files table
CREATE TABLE IF NOT EXISTS manifest_files (
    manifest_id INTEGER NOT NULL,
    file_id INTEGER NOT NULL,
    FOREIGN KEY (manifest_id) REFERENCES manifests(id) ON DELETE CASCADE,
    FOREIGN KEY (file_id) REFERENCES files(id),
    PRIMARY KEY (manifest_id, file_id)
);

-- Peers table (for tracking known peers)
CREATE TABLE IF NOT EXISTS peers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id TEXT NOT NULL UNIQUE,
    device_name TEXT NOT NULL,
    last_seen TIMESTAMP,
    trusted BOOLEAN DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_peers_device_id ON peers(device_id);
"#;
