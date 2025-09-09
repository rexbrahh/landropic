//! Basic conflict detection for Day 3 alpha release
//!
//! Provides simple conflict detection for file synchronization

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};

use crate::errors::{Result, SyncError};
use landro_index::manifest::{Manifest, ManifestEntry};

/// Types of conflicts that can occur during sync
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    /// File modified on both sides
    BothModified,
    /// File deleted on one side, modified on other
    DeletedModified,
    /// File created on both sides with same name but different content
    CreateCreate,
}

/// Information about a detected conflict
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub file_path: String,
    pub conflict_type: ConflictType,
    pub local_entry: Option<ManifestEntry>,
    pub remote_entry: Option<ManifestEntry>,
    pub detected_at: DateTime<Utc>,
    pub resolved: bool,
}

/// Resolution strategies for conflicts (simple for alpha)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Keep the local version
    KeepLocal,
    /// Keep the remote version  
    KeepRemote,
    /// Keep the newer version based on modification time
    KeepNewer,
    /// Manual resolution required
    Manual,
}

/// Configuration for conflict detection
#[derive(Debug, Clone)]
pub struct ConflictDetectionConfig {
    /// Resolution strategy to use
    pub default_resolution: ConflictResolution,
    /// Grace period for considering files as "simultaneous" changes
    pub simultaneous_change_window: Duration,
    /// Whether to auto-resolve conflicts
    pub auto_resolve: bool,
}

impl Default for ConflictDetectionConfig {
    fn default() -> Self {
        Self {
            default_resolution: ConflictResolution::KeepNewer,
            simultaneous_change_window: Duration::from_secs(10),
            auto_resolve: true,
        }
    }
}

/// Basic conflict detector for alpha release
pub struct ConflictDetector {
    config: ConflictDetectionConfig,
    detected_conflicts: HashMap<String, Conflict>,
}

impl ConflictDetector {
    /// Create new conflict detector
    pub fn new(config: ConflictDetectionConfig) -> Self {
        Self {
            config,
            detected_conflicts: HashMap::new(),
        }
    }

    /// Detect conflicts between local and remote manifests
    pub fn detect_conflicts(
        &mut self,
        local_manifest: &Manifest,
        remote_manifest: &Manifest,
        last_sync_time: Option<DateTime<Utc>>,
    ) -> Result<Vec<Conflict>> {
        info!("Detecting conflicts between manifests");

        let mut conflicts = Vec::new();

        // Build lookup maps for efficiency
        let local_files: HashMap<String, &ManifestEntry> = local_manifest
            .files
            .iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect();

        let remote_files: HashMap<String, &ManifestEntry> = remote_manifest
            .files
            .iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect();

        // Check all unique file paths
        let mut all_paths = std::collections::HashSet::new();
        all_paths.extend(local_files.keys());
        all_paths.extend(remote_files.keys());

        for path in all_paths {
            if let Some(conflict) = self.check_file_conflict(
                path,
                local_files.get(path).copied(),
                remote_files.get(path).copied(),
                last_sync_time,
            )? {
                conflicts.push(conflict.clone());
                self.detected_conflicts.insert(path.clone(), conflict);
            }
        }

        info!("Detected {} conflicts", conflicts.len());
        Ok(conflicts)
    }

    /// Check for conflict on a specific file
    fn check_file_conflict(
        &self,
        path: &str,
        local_entry: Option<&ManifestEntry>,
        remote_entry: Option<&ManifestEntry>,
        last_sync_time: Option<DateTime<Utc>>,
    ) -> Result<Option<Conflict>> {
        match (local_entry, remote_entry) {
            // File exists on both sides
            (Some(local), Some(remote)) => {
                // Check if content is different
                if local.content_hash != remote.content_hash {
                    // Both were modified since last sync
                    if let Some(last_sync) = last_sync_time {
                        let local_modified_since = local.modified_at > last_sync;
                        let remote_modified_since = remote.modified_at > last_sync;

                        if local_modified_since && remote_modified_since {
                            // Both modified - conflict!
                            return Ok(Some(Conflict {
                                file_path: path.to_string(),
                                conflict_type: ConflictType::BothModified,
                                local_entry: Some(local.clone()),
                                remote_entry: Some(remote.clone()),
                                detected_at: Utc::now(),
                                resolved: false,
                            }));
                        }
                    } else {
                        // No last sync time - assume both modified
                        return Ok(Some(Conflict {
                            file_path: path.to_string(),
                            conflict_type: ConflictType::BothModified,
                            local_entry: Some(local.clone()),
                            remote_entry: Some(remote.clone()),
                            detected_at: Utc::now(),
                            resolved: false,
                        }));
                    }
                }
            }

            // File exists only locally (remote deleted or never existed)
            (Some(local), None) => {
                if let Some(last_sync) = last_sync_time {
                    // If local was modified since last sync, this might be a delete-modify conflict
                    if local.modified_at > last_sync {
                        return Ok(Some(Conflict {
                            file_path: path.to_string(),
                            conflict_type: ConflictType::DeletedModified,
                            local_entry: Some(local.clone()),
                            remote_entry: None,
                            detected_at: Utc::now(),
                            resolved: false,
                        }));
                    }
                }
            }

            // File exists only remotely (local deleted or never existed)
            (None, Some(remote)) => {
                if let Some(last_sync) = last_sync_time {
                    // If remote was modified since last sync, this might be a delete-modify conflict
                    if remote.modified_at > last_sync {
                        return Ok(Some(Conflict {
                            file_path: path.to_string(),
                            conflict_type: ConflictType::DeletedModified,
                            local_entry: None,
                            remote_entry: Some(remote.clone()),
                            detected_at: Utc::now(),
                            resolved: false,
                        }));
                    }
                }
            }

            // File doesn't exist on either side
            (None, None) => {
                // No conflict
            }
        }

        Ok(None)
    }

    /// Attempt to auto-resolve a conflict
    pub fn auto_resolve_conflict(&mut self, conflict: &mut Conflict) -> Result<ConflictResolution> {
        if !self.config.auto_resolve {
            return Ok(ConflictResolution::Manual);
        }

        let resolution = match conflict.conflict_type {
            ConflictType::BothModified => {
                match (&conflict.local_entry, &conflict.remote_entry) {
                    (Some(local), Some(remote)) => {
                        match &self.config.default_resolution {
                            ConflictResolution::KeepNewer => {
                                if local.modified_at > remote.modified_at {
                                    ConflictResolution::KeepLocal
                                } else if remote.modified_at > local.modified_at {
                                    ConflictResolution::KeepRemote
                                } else {
                                    // Same modification time - fall back to keep local
                                    ConflictResolution::KeepLocal
                                }
                            }
                            other => other.clone(),
                        }
                    }
                    _ => ConflictResolution::Manual,
                }
            }
            ConflictType::DeletedModified => {
                // For alpha, always keep the existing file over deletion
                if conflict.local_entry.is_some() {
                    ConflictResolution::KeepLocal
                } else {
                    ConflictResolution::KeepRemote
                }
            }
            ConflictType::CreateCreate => {
                // Use default resolution strategy
                self.config.default_resolution.clone()
            }
        };

        conflict.resolved = matches!(resolution, ConflictResolution::Manual) == false;

        info!(
            "Auto-resolved conflict for {} using strategy {:?}",
            conflict.file_path, resolution
        );

        Ok(resolution)
    }

    /// Get all detected conflicts
    pub fn get_conflicts(&self) -> Vec<&Conflict> {
        self.detected_conflicts.values().collect()
    }

    /// Get unresolved conflicts
    pub fn get_unresolved_conflicts(&self) -> Vec<&Conflict> {
        self.detected_conflicts
            .values()
            .filter(|c| !c.resolved)
            .collect()
    }

    /// Mark a conflict as resolved
    pub fn mark_resolved(&mut self, file_path: &str) {
        if let Some(conflict) = self.detected_conflicts.get_mut(file_path) {
            conflict.resolved = true;
            info!("Marked conflict for {} as resolved", file_path);
        }
    }

    /// Clear all resolved conflicts
    pub fn clear_resolved_conflicts(&mut self) {
        let before_count = self.detected_conflicts.len();
        self.detected_conflicts
            .retain(|_, conflict| !conflict.resolved);
        let after_count = self.detected_conflicts.len();

        if before_count != after_count {
            info!(
                "Cleared {} resolved conflicts, {} remaining",
                before_count - after_count,
                after_count
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn create_test_entry(path: &str, hash: &str, modified_mins_ago: i64) -> ManifestEntry {
        let modified_at = Utc::now() - chrono::Duration::minutes(modified_mins_ago);
        ManifestEntry {
            path: path.to_string(),
            size: 1024,
            modified_at,
            content_hash: hash.to_string(),
            chunk_hashes: vec![hash.to_string()],
            mode: Some(0o644),
        }
    }

    #[test]
    fn test_both_modified_conflict() {
        let mut detector = ConflictDetector::new(ConflictDetectionConfig::default());

        let local_entry = create_test_entry("test.txt", "hash1", 5);
        let remote_entry = create_test_entry("test.txt", "hash2", 3);

        let last_sync = Utc::now() - chrono::Duration::minutes(10);

        let conflict = detector
            .check_file_conflict(
                "test.txt",
                Some(&local_entry),
                Some(&remote_entry),
                Some(last_sync),
            )
            .unwrap();

        assert!(conflict.is_some());
        let conflict = conflict.unwrap();
        assert_eq!(conflict.conflict_type, ConflictType::BothModified);
        assert_eq!(conflict.file_path, "test.txt");
        assert!(!conflict.resolved);
    }

    #[test]
    fn test_auto_resolve_keep_newer() {
        let mut detector = ConflictDetector::new(ConflictDetectionConfig {
            default_resolution: ConflictResolution::KeepNewer,
            auto_resolve: true,
            ..Default::default()
        });

        let mut conflict = Conflict {
            file_path: "test.txt".to_string(),
            conflict_type: ConflictType::BothModified,
            local_entry: Some(create_test_entry("test.txt", "hash1", 5)), // 5 mins ago
            remote_entry: Some(create_test_entry("test.txt", "hash2", 3)), // 3 mins ago (newer)
            detected_at: Utc::now(),
            resolved: false,
        };

        let resolution = detector.auto_resolve_conflict(&mut conflict).unwrap();
        assert_eq!(resolution, ConflictResolution::KeepRemote);
        assert!(conflict.resolved);
    }

    #[test]
    fn test_no_conflict_same_content() {
        let detector = ConflictDetector::new(ConflictDetectionConfig::default());

        let local_entry = create_test_entry("test.txt", "same_hash", 5);
        let remote_entry = create_test_entry("test.txt", "same_hash", 3);

        let conflict = detector
            .check_file_conflict("test.txt", Some(&local_entry), Some(&remote_entry), None)
            .unwrap();

        assert!(conflict.is_none());
    }
}
