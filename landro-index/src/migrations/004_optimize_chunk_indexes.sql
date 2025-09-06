-- Migration: Optimize chunk indexes for better query performance
-- Version: 004
-- Description: Add optimized indexes for chunk queries and CAS storage

-- Add composite indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_chunks_hash_size ON chunks(hash, size);
CREATE INDEX IF NOT EXISTS idx_chunks_ref_count_desc ON chunks(ref_count DESC);
CREATE INDEX IF NOT EXISTS idx_chunks_created_at ON chunks(created_at);

-- Add covering index for file-chunk mappings
CREATE INDEX IF NOT EXISTS idx_file_chunks_covering 
    ON file_chunks(file_id, chunk_id, offset, size);

-- Add partial indexes for garbage collection
CREATE INDEX IF NOT EXISTS idx_chunks_orphaned 
    ON chunks(hash) 
    WHERE ref_count = 0;

-- Add index for hot chunks (frequently accessed)
CREATE INDEX IF NOT EXISTS idx_chunks_hot 
    ON chunks(hash, last_accessed) 
    WHERE ref_count > 5;

-- Statistics table for chunk access patterns
CREATE TABLE IF NOT EXISTS chunk_stats (
    chunk_hash TEXT PRIMARY KEY,
    access_count INTEGER DEFAULT 0,
    last_accessed INTEGER,
    avg_latency_ms REAL,
    cache_hits INTEGER DEFAULT 0,
    cache_misses INTEGER DEFAULT 0,
    FOREIGN KEY (chunk_hash) REFERENCES chunks(hash) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_chunk_stats_access_count 
    ON chunk_stats(access_count DESC);
CREATE INDEX IF NOT EXISTS idx_chunk_stats_last_accessed 
    ON chunk_stats(last_accessed DESC);

-- Batch operation tracking
CREATE TABLE IF NOT EXISTS batch_operations (
    batch_id TEXT PRIMARY KEY,
    operation_type TEXT NOT NULL, -- 'write', 'read', 'verify'
    chunk_count INTEGER NOT NULL,
    total_size INTEGER NOT NULL,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    status TEXT DEFAULT 'pending', -- 'pending', 'processing', 'completed', 'failed'
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_batch_operations_status 
    ON batch_operations(status, started_at DESC);

-- Connection pool stats
CREATE TABLE IF NOT EXISTS connection_stats (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    active_connections INTEGER,
    idle_connections INTEGER,
    total_queries INTEGER,
    avg_query_time_ms REAL
);

CREATE INDEX IF NOT EXISTS idx_connection_stats_timestamp 
    ON connection_stats(timestamp DESC);

-- Update trigger for chunk access tracking
CREATE TRIGGER IF NOT EXISTS update_chunk_stats_on_access
AFTER UPDATE ON chunks
WHEN NEW.last_accessed != OLD.last_accessed
BEGIN
    INSERT INTO chunk_stats (chunk_hash, access_count, last_accessed)
    VALUES (NEW.hash, 1, NEW.last_accessed)
    ON CONFLICT(chunk_hash) DO UPDATE SET
        access_count = access_count + 1,
        last_accessed = NEW.last_accessed;
END;

-- Analyze tables for query optimizer
ANALYZE chunks;
ANALYZE file_chunks;
ANALYZE chunk_stats;