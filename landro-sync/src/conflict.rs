//! Conflict detection and resolution

use serde::{Deserialize, Serialize};

/// Types of conflicts that can occur
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConflictType {
    /// Both peers modified the same file
    ModifiedBoth,
    /// One peer deleted, other modified
    DeletedVsModified,
    /// Different file types (file vs directory)
    TypeMismatch,
}

/// Resolution strategy for conflicts
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConflictResolution {
    /// Keep our version
    KeepOurs,
    /// Take their version
    TakeTheirs,
    /// Keep both with rename
    KeepBoth,
    /// Merge if possible
    Merge,
    /// Ask user
    Manual,
}

/// Conflict resolver
pub struct ConflictResolver {
    default_strategy: ConflictResolution,
}

impl ConflictResolver {
    pub fn new(default_strategy: ConflictResolution) -> Self {
        Self { default_strategy }
    }

    pub fn resolve(&self, _conflict_type: ConflictType) -> ConflictResolution {
        // For now, use default strategy
        // TODO: Implement smart resolution based on conflict type
        self.default_strategy.clone()
    }
}