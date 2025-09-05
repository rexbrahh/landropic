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
pub use diff::{DiffComputer, DiffResult, FileChange, ChangeType, IncrementalDiff};
pub use errors::{SyncError, Result};
pub use orchestrator::{SyncOrchestrator, SyncConfig};
pub use progress::{SyncProgress, TransferProgress};
pub use protocol::{SyncSession, SessionState, TransferStats};
pub use recovery::{RecoveryManager, Operation, OperationType, OperationStatus, RecoveryStats, generate_operation_id};
pub use scheduler::{TransferScheduler, TransferPriority};
pub use state::{SyncState, PeerSyncState, AsyncSyncDatabase};
