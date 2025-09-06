//! Integration tests for sync engine

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use landro_index::manifest::{Manifest, ManifestEntry};
use landro_sync::{ChangeType, DiffComputer, DiffResult, FileChange, IncrementalDiff};

fn create_test_manifest(folder_id: &str, version: u64) -> Manifest {
    Manifest {
        folder_id: folder_id.to_string(),
        version,
        files: vec![],
        created_at: Utc::now(),
        manifest_hash: None,
    }
}

fn create_test_entry(
    path: &str,
    size: u64,
    content_hash: &str,
    chunks: Vec<&str>,
) -> ManifestEntry {
    ManifestEntry {
        path: path.to_string(),
        size,
        modified_at: Utc::now(),
        content_hash: content_hash.to_string(),
        chunk_hashes: chunks.iter().map(|s| s.to_string()).collect(),
        mode: Some(0o644),
    }
}

#[test]
fn test_manifest_diff_comprehensive() {
    let chunk_cache = Arc::new(HashSet::new());
    let computer = DiffComputer::new(chunk_cache);

    // Create source manifest with some files
    let mut source = create_test_manifest("test-folder", 1);
    source.files.push(create_test_entry(
        "unchanged.txt",
        1024,
        "hash_unchanged",
        vec!["chunk_u1", "chunk_u2"],
    ));
    source.files.push(create_test_entry(
        "modified.rs",
        2048,
        "hash_old",
        vec!["chunk_m1", "chunk_m2"],
    ));
    source.files.push(create_test_entry(
        "deleted.md",
        512,
        "hash_deleted",
        vec!["chunk_d1"],
    ));

    // Create target manifest with changes
    let mut target = create_test_manifest("test-folder", 2);
    target.files.push(create_test_entry(
        "unchanged.txt",
        1024,
        "hash_unchanged",
        vec!["chunk_u1", "chunk_u2"],
    ));
    target.files.push(create_test_entry(
        "modified.rs",
        3072,
        "hash_new",
        vec!["chunk_m1", "chunk_m3", "chunk_m4"], // Reuses chunk_m1
    ));
    target.files.push(create_test_entry(
        "added.go",
        4096,
        "hash_added",
        vec!["chunk_a1", "chunk_a2", "chunk_a3"],
    ));

    // Compute diff
    let diff = computer.compute_diff(&source, &target).unwrap();

    // Verify statistics
    assert_eq!(diff.stats.files_added, 1, "Should have 1 added file");
    assert_eq!(diff.stats.files_modified, 1, "Should have 1 modified file");
    assert_eq!(diff.stats.files_deleted, 1, "Should have 1 deleted file");

    // Verify added file
    let added = diff.changes_by_type(ChangeType::Added);
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].path, "added.go");
    assert_eq!(added[0].required_chunks.len(), 3);

    // Verify modified file
    let modified = diff.changes_by_type(ChangeType::Modified);
    assert_eq!(modified.len(), 1);
    assert_eq!(modified[0].path, "modified.rs");
    // Should only need new chunks (chunk_m3, chunk_m4)
    assert_eq!(modified[0].required_chunks.len(), 2);
    assert!(modified[0]
        .required_chunks
        .contains(&"chunk_m3".to_string()));
    assert!(modified[0]
        .required_chunks
        .contains(&"chunk_m4".to_string()));

    // Verify deleted file
    let deleted = diff.changes_by_type(ChangeType::Deleted);
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].path, "deleted.md");
    assert_eq!(deleted[0].required_chunks.len(), 0);

    // Verify total unique chunks needed
    assert_eq!(diff.required_chunks.len(), 5); // chunk_m3, chunk_m4, chunk_a1, chunk_a2, chunk_a3
}

#[test]
fn test_diff_with_chunk_cache() {
    // Pre-populate cache with some chunks
    let mut cache = HashSet::new();
    cache.insert("chunk_existing1".to_string());
    cache.insert("chunk_existing2".to_string());

    let chunk_cache = Arc::new(cache);
    let computer = DiffComputer::new(chunk_cache);

    let source = create_test_manifest("test", 1);

    let mut target = create_test_manifest("test", 2);
    target.files.push(create_test_entry(
        "file.txt",
        5000,
        "hash_new",
        vec![
            "chunk_existing1",
            "chunk_existing2",
            "chunk_new1",
            "chunk_new2",
        ],
    ));

    let diff = computer.compute_diff(&source, &target).unwrap();

    // Should only request chunks not in cache
    assert_eq!(diff.required_chunks.len(), 2);
    assert!(diff.required_chunks.contains("chunk_new1"));
    assert!(diff.required_chunks.contains("chunk_new2"));
    assert!(!diff.required_chunks.contains("chunk_existing1"));
    assert!(!diff.required_chunks.contains("chunk_existing2"));
}

#[test]
fn test_incremental_diff_tracking() {
    let chunk_cache = Arc::new(HashSet::new());
    let mut tracker = IncrementalDiff::new(chunk_cache);

    // Initial state
    let mut manifest_v1 = create_test_manifest("shared", 1);
    manifest_v1.files.push(create_test_entry(
        "file1.txt",
        1000,
        "hash1",
        vec!["chunk1"],
    ));

    // Peer has nothing initially
    let peer_manifest_empty = create_test_manifest("shared", 0);

    // First sync - peer needs everything
    let diff1 = tracker
        .compute_incremental("peer1", &manifest_v1, &peer_manifest_empty)
        .unwrap();
    assert_eq!(diff1.stats.files_added, 1);
    assert!(tracker.get_pending("peer1").is_some());

    // Simulate acknowledging the transfer
    tracker.acknowledge_changes("peer1", &["chunk1".to_string()]);
    assert!(tracker.get_pending("peer1").is_none());

    // Add new file
    let mut manifest_v2 = create_test_manifest("shared", 2);
    manifest_v2.files.push(create_test_entry(
        "file1.txt",
        1000,
        "hash1",
        vec!["chunk1"],
    ));
    manifest_v2.files.push(create_test_entry(
        "file2.txt",
        2000,
        "hash2",
        vec!["chunk2", "chunk3"],
    ));

    // Peer now has v1
    let diff2 = tracker
        .compute_incremental("peer1", &manifest_v2, &manifest_v1)
        .unwrap();
    assert_eq!(diff2.stats.files_added, 1);
    assert_eq!(diff2.required_chunks.len(), 2);
}

#[test]
fn test_diff_priority_ordering() {
    let chunk_cache = Arc::new(HashSet::new());
    let computer = DiffComputer::new(chunk_cache);

    let source = create_test_manifest("test", 1);

    let mut target = create_test_manifest("test", 2);
    // Large new file
    target.files.push(create_test_entry(
        "large.bin",
        10_000_000,
        "hash_large",
        vec!["chunk_l1", "chunk_l2", "chunk_l3"],
    ));
    // Small new file
    target.files.push(create_test_entry(
        "small.txt",
        100,
        "hash_small",
        vec!["chunk_s1"],
    ));
    // Modified file (should have highest priority)
    target.files.push(create_test_entry(
        "modified.rs",
        5000,
        "hash_mod",
        vec!["chunk_m1"],
    ));

    let mut source_with_modified = source.clone();
    source_with_modified.files.push(create_test_entry(
        "modified.rs",
        4000,
        "hash_old",
        vec!["chunk_old"],
    ));

    let mut diff = computer
        .compute_diff(&source_with_modified, &target)
        .unwrap();

    // Check that prioritization works correctly
    diff.prioritize_changes();

    assert_eq!(
        diff.changes[0].path, "modified.rs",
        "Modified files should be first"
    );
    assert_eq!(
        diff.changes[1].path, "small.txt",
        "Small files should be second"
    );
    assert_eq!(
        diff.changes[2].path, "large.bin",
        "Large files should be last"
    );
}

#[test]
fn test_bidirectional_diff() {
    let chunk_cache = Arc::new(HashSet::new());
    let computer = DiffComputer::new(chunk_cache);

    let mut manifest_a = create_test_manifest("test", 1);
    manifest_a.files.push(create_test_entry(
        "only_in_a.txt",
        1000,
        "hash_a",
        vec!["chunk_a"],
    ));
    manifest_a.files.push(create_test_entry(
        "shared.txt",
        2000,
        "hash_shared_old",
        vec!["chunk_shared1"],
    ));

    let mut manifest_b = create_test_manifest("test", 2);
    manifest_b.files.push(create_test_entry(
        "only_in_b.txt",
        3000,
        "hash_b",
        vec!["chunk_b"],
    ));
    manifest_b.files.push(create_test_entry(
        "shared.txt",
        2500,
        "hash_shared_new",
        vec!["chunk_shared2"],
    ));

    // Diff A -> B (what B needs from A)
    let diff_a_to_b = computer.compute_diff(&manifest_a, &manifest_b).unwrap();
    assert_eq!(diff_a_to_b.stats.files_added, 1); // only_in_b.txt
    assert_eq!(diff_a_to_b.stats.files_modified, 1); // shared.txt
    assert_eq!(diff_a_to_b.stats.files_deleted, 1); // only_in_a.txt

    // Diff B -> A (what A needs from B)
    let diff_b_to_a = computer.compute_diff(&manifest_b, &manifest_a).unwrap();
    assert_eq!(diff_b_to_a.stats.files_added, 1); // only_in_a.txt
    assert_eq!(diff_b_to_a.stats.files_modified, 1); // shared.txt
    assert_eq!(diff_b_to_a.stats.files_deleted, 1); // only_in_b.txt
}

#[test]
fn test_large_manifest_diff_performance() {
    let chunk_cache = Arc::new(HashSet::new());
    let computer = DiffComputer::new(chunk_cache);

    let mut source = create_test_manifest("large", 1);
    let mut target = create_test_manifest("large", 2);

    // Create manifests with many files
    for i in 0..1000 {
        let path = format!("file_{:04}.txt", i);
        let hash = format!("hash_{:04}", i);
        let chunks: Vec<&str> = vec!["chunk1", "chunk2"];

        source
            .files
            .push(create_test_entry(&path, 1024, &hash, chunks.clone()));

        // Modify every 10th file in target
        if i % 10 == 0 {
            let new_hash = format!("hash_{:04}_new", i);
            let new_chunks = vec!["chunk1", "chunk3"];
            target
                .files
                .push(create_test_entry(&path, 2048, &new_hash, new_chunks));
        } else {
            target
                .files
                .push(create_test_entry(&path, 1024, &hash, chunks));
        }
    }

    // Add some new files to target
    for i in 1000..1050 {
        let path = format!("new_file_{:04}.txt", i);
        let hash = format!("new_hash_{:04}", i);
        let chunks = vec!["new_chunk1", "new_chunk2"];
        target
            .files
            .push(create_test_entry(&path, 512, &hash, chunks));
    }

    let start = std::time::Instant::now();
    let diff = computer.compute_diff(&source, &target).unwrap();
    let duration = start.elapsed();

    // Performance check - should complete quickly even with many files
    assert!(
        duration.as_millis() < 100,
        "Diff took too long: {:?}",
        duration
    );

    // Verify correctness
    assert_eq!(diff.stats.files_modified, 100); // Every 10th file modified
    assert_eq!(diff.stats.files_added, 50); // 50 new files
    assert_eq!(diff.stats.files_deleted, 0);
}

#[test]
fn test_metadata_only_changes() {
    let chunk_cache = Arc::new(HashSet::new());
    let computer = DiffComputer::new(chunk_cache);

    let mut source = create_test_manifest("test", 1);
    let mut entry1 = create_test_entry("file.txt", 1024, "hash1", vec!["chunk1", "chunk2"]);
    entry1.mode = Some(0o644);
    source.files.push(entry1);

    let mut target = create_test_manifest("test", 2);
    let mut entry2 = create_test_entry(
        "file.txt",
        1024,
        "hash2", // Different hash but same chunks (metadata change)
        vec!["chunk1", "chunk2"],
    );
    entry2.mode = Some(0o755); // Permission change
    target.files.push(entry2);

    let diff = computer.compute_diff(&source, &target).unwrap();

    // When content hash differs but chunks are the same and size is the same,
    // it's classified as metadata-only change
    assert_eq!(diff.stats.files_metadata_only, 1);
    assert_eq!(diff.stats.files_modified, 0);

    // Check that it's a metadata-only change
    let change = &diff.changes[0];
    assert_eq!(change.change_type, ChangeType::MetadataOnly);
    // No chunks need to be transferred
    assert_eq!(change.required_chunks.len(), 0);
}
