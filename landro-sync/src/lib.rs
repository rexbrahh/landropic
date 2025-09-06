//! Synchronization orchestration for landropic
//!
//! This crate provides the central synchronization engine that manages:
//! - Sync state persistence and recovery
//! - Conflict detection and resolution
//! - Transfer scheduling and prioritization
//! - Progress tracking and reporting
//! - Manifest diff computation
//! - Protocol state machine

pub mod conflict;
pub mod diff;
pub mod errors;
pub mod orchestrator;
pub mod progress;
pub mod protocol;
pub mod recovery;
pub mod scheduler;
pub mod state;

pub use conflict::{ConflictResolution, ConflictResolver, ConflictType};
pub use diff::{ChangeType, DiffComputer, DiffResult, FileChange, IncrementalDiff};
pub use errors::{Result, SyncError};
pub use orchestrator::{SyncConfig, SyncOrchestrator};
pub use progress::{SyncProgress, TransferProgress};
pub use protocol::{SessionState, SyncSession, TransferStats};
pub use recovery::{
    generate_operation_id, Operation, OperationStatus, OperationType, RecoveryManager,
    RecoveryStats,
};
pub use scheduler::{TransferPriority, TransferScheduler};
pub use state::{AsyncSyncDatabase, PeerSyncState, SyncState};
