//! Storage utilities for debugging, maintenance, and administration

use crate::errors::{CasError, Result};
use crate::metrics::StorageHealth;
use crate::storage::ContentStore;
use landro_chunker::ContentHash;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs;
use tracing::info;

/// Storage diagnostics and repair utilities
pub struct StorageUtilities {
    store: ContentStore,
}

/// Comprehensive storage report for debugging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageReport {
    pub health: StorageHealth,
    pub filesystem: FilesystemReport,
    pub corruption: CorruptionReport,
    pub performance: PerformanceReport,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemReport {
    pub total_objects: u64,
    pub total_size_bytes: u64,
    pub temp_files: u64,
    pub orphaned_files: u64,
    pub shard_distribution: HashMap<String, u64>,
    pub largest_objects: Vec<ObjectInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorruptionReport {
    pub corrupt_objects: Vec<CorruptObject>,
    pub missing_objects: Vec<ContentHash>,
    pub hash_mismatches: Vec<HashMismatch>,
    pub integrity_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceReport {
    pub read_latency_ms: f64,
    pub write_latency_ms: f64,
    pub throughput_mbps: f64,
    pub cache_efficiency: f64,
    pub disk_utilization: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub hash: String,
    pub size: u64,
    pub path: PathBuf,
    pub modified: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorruptObject {
    pub hash: String,
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashMismatch {
    pub expected: String,
    pub actual: String,
    pub path: PathBuf,
}

/// Maintenance operations
#[derive(Debug, Clone)]
pub struct MaintenanceOptions {
    pub verify_hashes: bool,
    pub cleanup_temp_files: bool,
    pub repair_corruption: bool,
    pub optimize_storage: bool,
    pub update_indexes: bool,
}

impl Default for MaintenanceOptions {
    fn default() -> Self {
        Self {
            verify_hashes: true,
            cleanup_temp_files: true,
            repair_corruption: false, // Conservative default
            optimize_storage: true,
            update_indexes: true,
        }
    }
}

impl StorageUtilities {
    pub fn new(store: ContentStore) -> Self {
        Self { store }
    }

    /// Generate comprehensive storage report
    pub async fn generate_report(&self) -> Result<StorageReport> {
        info!("Generating comprehensive storage report");

        // For now, create a simple health placeholder since we need to restructure metrics
        let health = StorageHealth {
            status: crate::metrics::HealthStatus::Healthy,
            score: 0.9,
            issues: Vec::new(),
            recommendations: Vec::new(),
            last_check: SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        // Analyze filesystem
        let filesystem = self.analyze_filesystem().await?;

        // Check for corruption
        let corruption = self.check_corruption().await?;

        // Performance analysis
        let performance = self.analyze_performance().await?;

        // Generate recommendations
        let recommendations =
            self.generate_recommendations(&health, &filesystem, &corruption, &performance);

        Ok(StorageReport {
            health,
            filesystem,
            corruption,
            performance,
            recommendations,
        })
    }

    /// Analyze filesystem structure and usage
    async fn analyze_filesystem(&self) -> Result<FilesystemReport> {
        let root_path = self.store.root_path();
        let mut total_objects = 0u64;
        let mut total_size = 0u64;
        let mut temp_files = 0u64;
        let mut orphaned_files = 0u64;
        let mut shard_distribution = HashMap::new();
        let mut largest_objects = Vec::new();

        // Walk through object storage structure
        if root_path.exists() {
            let mut dir = fs::read_dir(root_path).await?;
            while let Some(entry) = dir.next_entry().await? {
                let path = entry.path();

                if path.is_dir() {
                    let shard_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    if shard_name == ".tmp" {
                        // Count temp files
                        temp_files += self.count_temp_files(&path).await?;
                    } else if shard_name.len() == 2
                        && shard_name.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        // This is a shard directory
                        let shard_stats = self.analyze_shard(&path).await?;
                        total_objects += shard_stats.0;
                        total_size += shard_stats.1;
                        shard_distribution.insert(shard_name, shard_stats.0);

                        // Collect large objects
                        for obj in shard_stats.2 {
                            if obj.size > 10 * 1024 * 1024 {
                                // > 10MB
                                largest_objects.push(obj);
                            }
                        }
                    }
                } else {
                    // Check for orphaned files in root
                    orphaned_files += 1;
                }
            }
        }

        // Sort largest objects by size
        largest_objects.sort_by(|a, b| b.size.cmp(&a.size));
        largest_objects.truncate(20); // Keep top 20

        Ok(FilesystemReport {
            total_objects,
            total_size_bytes: total_size,
            temp_files,
            orphaned_files,
            shard_distribution,
            largest_objects,
        })
    }

    /// Check for data corruption
    async fn check_corruption(&self) -> Result<CorruptionReport> {
        let mut corrupt_objects = Vec::new();
        let missing_objects = Vec::new();
        let mut hash_mismatches = Vec::new();
        let mut checked = 0u64;
        let mut corrupt_count = 0u64;

        let root_path = self.store.root_path();

        // Walk through all objects and verify integrity
        if root_path.exists() {
            let mut dir = fs::read_dir(root_path).await?;
            while let Some(entry) = dir.next_entry().await? {
                let path = entry.path();

                if path.is_dir() && !path.file_name().unwrap().to_str().unwrap().starts_with('.') {
                    // Process shard directory
                    let results = self.verify_shard_integrity(&path).await?;
                    checked += results.0;
                    corrupt_count += results.1;
                    corrupt_objects.extend(results.2);
                    hash_mismatches.extend(results.3);
                }
            }
        }

        let integrity_score = if checked > 0 {
            1.0 - (corrupt_count as f64 / checked as f64)
        } else {
            1.0
        };

        Ok(CorruptionReport {
            corrupt_objects,
            missing_objects,
            hash_mismatches,
            integrity_score,
        })
    }

    /// Analyze storage performance
    async fn analyze_performance(&self) -> Result<PerformanceReport> {
        // For now, return placeholder performance metrics
        // In a full implementation, we would collect these from the runtime stats

        Ok(PerformanceReport {
            read_latency_ms: 0.0,  // TODO: Get from runtime stats
            write_latency_ms: 0.0, // TODO: Get from runtime stats
            throughput_mbps: 0.0,  // TODO: Calculate throughput
            cache_efficiency: 0.0, // TODO: Get cache stats
            disk_utilization: 0.0, // TODO: Calculate disk utilization
        })
    }

    /// Generate maintenance recommendations
    fn generate_recommendations(
        &self,
        health: &StorageHealth,
        filesystem: &FilesystemReport,
        corruption: &CorruptionReport,
        performance: &PerformanceReport,
    ) -> Vec<String> {
        let mut recommendations = Vec::new();

        // Health-based recommendations
        if health.score < 0.8 {
            recommendations.push("Storage health is degraded - run maintenance".to_string());
        }

        // Filesystem recommendations
        if filesystem.orphaned_files > 0 {
            recommendations.push(format!(
                "Clean up {} orphaned files",
                filesystem.orphaned_files
            ));
        }

        if filesystem.temp_files > 100 {
            recommendations
                .push("High number of temp files - run cleanup_expired_partials()".to_string());
        }

        // Check shard balance
        if !filesystem.shard_distribution.is_empty() {
            let values: Vec<_> = filesystem.shard_distribution.values().collect();
            let max = **values.iter().max().unwrap();
            let min = **values.iter().min().unwrap();
            if max > min * 3 {
                recommendations
                    .push("Shard distribution is unbalanced - consider rehashing".to_string());
            }
        }

        // Corruption recommendations
        if corruption.integrity_score < 0.95 {
            recommendations.push("Data corruption detected - run repair immediately".to_string());
        }

        if !corruption.corrupt_objects.is_empty() {
            recommendations.push(format!(
                "Remove {} corrupt objects",
                corruption.corrupt_objects.len()
            ));
        }

        // Performance recommendations
        if performance.cache_efficiency < 0.7 {
            recommendations.push("Low cache hit rate - consider increasing cache size".to_string());
        }

        if performance.throughput_mbps < 50.0 {
            recommendations
                .push("Low throughput - check disk performance and consider SSD".to_string());
        }

        if performance.read_latency_ms > 10.0 {
            recommendations.push("High read latency - optimize storage backend".to_string());
        }

        recommendations
    }

    /// Perform maintenance operations
    pub async fn perform_maintenance(
        &self,
        options: &MaintenanceOptions,
    ) -> Result<MaintenanceReport> {
        info!("Starting storage maintenance with options: {:?}", options);
        let start_time = std::time::Instant::now();

        let mut report = MaintenanceReport::default();

        if options.cleanup_temp_files {
            info!("Cleaning up temp files");
            report.temp_files_cleaned = self.store.cleanup_expired_partials().await?;
        }

        if options.verify_hashes {
            info!("Verifying object hashes");
            let verification = self.verify_all_objects().await?;
            report.objects_verified = verification.0;
            report.corruption_found = verification.1;
        }

        if options.repair_corruption && report.corruption_found > 0 {
            info!("Repairing corrupted objects");
            report.objects_repaired = self.repair_corruption().await?;
        }

        if options.optimize_storage {
            info!("Optimizing storage");
            report.optimization_savings = self.optimize_storage().await?;
        }

        if options.update_indexes {
            info!("Updating indexes");
            self.update_indexes().await?;
            report.indexes_updated = true;
        }

        report.duration = start_time.elapsed();
        info!("Maintenance completed in {:?}", report.duration);

        Ok(report)
    }

    /// Export storage statistics as JSON
    pub async fn export_statistics(&self) -> Result<String> {
        let report = self.generate_report().await?;
        serde_json::to_string_pretty(&report)
            .map_err(|e| CasError::Internal(format!("JSON serialization error: {}", e)))
    }

    // Helper methods
    async fn count_temp_files(&self, temp_dir: &Path) -> Result<u64> {
        let mut count = 0;
        if temp_dir.exists() {
            let mut dir = fs::read_dir(temp_dir).await?;
            while let Some(_) = dir.next_entry().await? {
                count += 1;
            }
        }
        Ok(count)
    }

    async fn analyze_shard(&self, shard_path: &Path) -> Result<(u64, u64, Vec<ObjectInfo>)> {
        let mut object_count = 0u64;
        let mut total_size = 0u64;
        let mut objects = Vec::new();

        // Walk through second-level directories
        let mut dir = fs::read_dir(shard_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let sub_stats = self.analyze_sub_shard(&path).await?;
                object_count += sub_stats.0;
                total_size += sub_stats.1;
                objects.extend(sub_stats.2);
            }
        }

        Ok((object_count, total_size, objects))
    }

    async fn analyze_sub_shard(
        &self,
        sub_shard_path: &Path,
    ) -> Result<(u64, u64, Vec<ObjectInfo>)> {
        let mut object_count = 0u64;
        let mut total_size = 0u64;
        let mut objects = Vec::new();

        let mut dir = fs::read_dir(sub_shard_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_file() {
                if let Ok(metadata) = entry.metadata().await {
                    let size = metadata.len();
                    total_size += size;
                    object_count += 1;

                    if let Some(file_name) = path.file_name() {
                        if let Some(hash_str) = file_name.to_str() {
                            objects.push(ObjectInfo {
                                hash: hash_str.to_string(),
                                size,
                                path: path.clone(),
                                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                            });
                        }
                    }
                }
            }
        }

        Ok((object_count, total_size, objects))
    }

    async fn verify_shard_integrity(
        &self,
        shard_path: &Path,
    ) -> Result<(u64, u64, Vec<CorruptObject>, Vec<HashMismatch>)> {
        let mut checked = 0u64;
        let mut corrupt = 0u64;
        let mut corrupt_objects = Vec::new();
        let mut hash_mismatches = Vec::new();

        // Walk through objects and verify each one
        let mut dir = fs::read_dir(shard_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let results = self.verify_sub_shard_integrity(&path).await?;
                checked += results.0;
                corrupt += results.1;
                corrupt_objects.extend(results.2);
                hash_mismatches.extend(results.3);
            }
        }

        Ok((checked, corrupt, corrupt_objects, hash_mismatches))
    }

    async fn verify_sub_shard_integrity(
        &self,
        sub_shard_path: &Path,
    ) -> Result<(u64, u64, Vec<CorruptObject>, Vec<HashMismatch>)> {
        let mut checked = 0u64;
        let mut corrupt = 0u64;
        let mut corrupt_objects = Vec::new();
        let mut hash_mismatches = Vec::new();

        let mut dir = fs::read_dir(sub_shard_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_file() {
                checked += 1;

                if let Some(file_name) = path.file_name() {
                    if let Some(hash_str) = file_name.to_str() {
                        // Try to parse hash and verify object
                        match self.verify_object(&path, hash_str).await {
                            Ok(true) => {} // Object is valid
                            Ok(false) => {
                                corrupt += 1;
                                hash_mismatches.push(HashMismatch {
                                    expected: hash_str.to_string(),
                                    actual: "mismatch".to_string(),
                                    path: path.clone(),
                                });
                            }
                            Err(e) => {
                                corrupt += 1;
                                corrupt_objects.push(CorruptObject {
                                    hash: hash_str.to_string(),
                                    path: path.clone(),
                                    error: e.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok((checked, corrupt, corrupt_objects, hash_mismatches))
    }

    async fn verify_object(&self, path: &Path, expected_hash: &str) -> Result<bool> {
        // Read file and compute hash
        let data = fs::read(path).await?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(&data);
        let computed = hasher.finalize();

        // Compare with expected hash
        let computed_hex = computed.to_hex().to_string();
        Ok(computed_hex == expected_hash)
    }

    async fn verify_all_objects(&self) -> Result<(u64, u64)> {
        // This would be implemented to verify all objects
        // For now, return placeholder values
        Ok((0, 0))
    }

    async fn repair_corruption(&self) -> Result<u64> {
        // This would implement corruption repair logic
        // For now, return placeholder
        Ok(0)
    }

    async fn optimize_storage(&self) -> Result<u64> {
        // This would implement storage optimization
        // For now, return placeholder
        Ok(0)
    }

    async fn update_indexes(&self) -> Result<()> {
        // This would update storage indexes
        // For now, no-op
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct MaintenanceReport {
    pub temp_files_cleaned: usize,
    pub objects_verified: u64,
    pub corruption_found: u64,
    pub objects_repaired: u64,
    pub optimization_savings: u64,
    pub indexes_updated: bool,
    pub duration: std::time::Duration,
}

impl ContentStore {
    /// Get storage utilities for this store
    pub fn utilities(&self) -> StorageUtilities {
        StorageUtilities::new(self.clone())
    }

    /// Get the root path for this store (helper for utilities)
    pub fn root_path(&self) -> &std::path::Path {
        &self.root_path
    }
}
