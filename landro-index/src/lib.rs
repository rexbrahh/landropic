pub mod async_indexer;
pub mod database;
pub mod database_pool;
pub mod errors;
pub mod indexer;
pub mod manifest;
pub mod schema;
pub mod watcher;

pub use async_indexer::{AsyncIndexer, AsyncIndexerBuilder};
pub use database::{ChunkEntry, FileEntry, IndexDatabase};
pub use database_pool::{DatabasePool, DatabasePoolBuilder};
pub use errors::{IndexError, Result};
pub use indexer::{FileIndexer, IndexerConfig};
pub use manifest::{Manifest, ManifestDiff, ManifestEntry};
pub use watcher::{
    FolderWatcher, WatcherConfig,
    // Re-export enhanced types
    EventKind, FsEvent, PlatformFlags, PlatformWatcher, WatcherCapabilities, WatcherStats,
    create_platform_watcher, EventDebouncer, should_ignore_path,
};
