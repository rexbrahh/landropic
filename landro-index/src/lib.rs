pub mod async_indexer;
pub mod database;
pub mod database_pool;
pub mod errors;
pub mod indexer;
pub mod manifest;
pub mod migrations;
pub mod schema;
pub mod watcher;

pub use async_indexer::{AsyncIndexer, AsyncIndexerBuilder};
pub use database::{ChunkEntry, FileEntry, IndexDatabase};
pub use database_pool::{DatabasePool, DatabasePoolBuilder};
pub use errors::{IndexError, Result};
pub use indexer::{FileIndexer, IndexerConfig};
pub use manifest::{Manifest, ManifestDiff, ManifestEntry};
pub use migrations::{run_migrations, Migration, MigrationManager};
pub use watcher::{
    create_platform_watcher,
    should_ignore_path,
    EventDebouncer,
    // Re-export enhanced types
    EventKind,
    FolderWatcher,
    FsEvent,
    PlatformFlags,
    PlatformWatcher,
    WatcherCapabilities,
    WatcherConfig,
    WatcherStats,
};
