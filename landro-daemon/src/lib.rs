pub mod daemon;
pub mod discovery;
pub mod orchestrator;
pub mod sync_engine;
pub mod watcher;
pub mod bloom_diff;
pub mod bloom_sync_integration;
pub mod resume_manager;
pub mod cli_progress_api;
pub mod end_to_end_sync;
pub mod security;
// pub mod network; // TODO: Implement ConnectionPool in landro-quic first

pub use daemon::{Daemon, DaemonConfig, DaemonStatus};
pub use orchestrator::{
    OrchestratorConfig, OrchestratorMessage, OrchestratorStatus, SyncOrchestrator,
};
pub use sync_engine::{EnhancedSyncEngine, SyncConnection};
