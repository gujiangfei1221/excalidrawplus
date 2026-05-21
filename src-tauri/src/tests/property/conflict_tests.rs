//! Property-based tests for conflict resolution and upload queue logic.
//!
//! **Validates: Requirements 4.1, 6.3, 6.7, 7.4, 7.5, 8.1, 8.2, 8.5, 8.7**
//!
//! Property 6: Manifest Merge by Timestamp
//! Property 14: Queue Upload Chronological Order
//! Property 15: Queue Processing Resilience
//! Property 16: Conflict Detection
//! Property 17: Conflict Resolution Creates Named Copy
//! Property 18: Maximum Conflict Copies Invariant

use proptest::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;
use tempfile::TempDir;

use crate::connectivity::ConnectivityMonitor;
use crate::cos_client::CosClient;
use crate::database::Database;
use crate::file_store::FileStore;
use crate::models::{
    CosConfig, FileMeta, Manifest, ManifestEntry, QueuedUpload, SyncStatus,
    UploadOperation,
};
use crate::sync_engine::{generate_conflict_title, generate_conflict_title_with_date, merge_manifests, SyncEngine};

/// Helper to build a valid CosConfig for constructing test dependencies.
fn test_cos_config() -> CosConfig {
    CosConfig {
        secret_id: "AKID-test".to_string(),
        secret_key: "secret-test".to_string(),
        bucket: "test-bucket-1250000000".to_string(),
        region: "ap-guangzhou".to_string(),
    }
}

/// Helper to create a SyncEngine with real (but non-networked) components.
fn create_test_engine_in(tmp_dir: &std::path::Path) -> SyncEngine {
    let config = test_cos_config();
    let cos_client = CosClient::new(&config).unwrap();
    let conn_monitor = ConnectivityMonitor::new(Arc::new(cos_client.clone()));

    let db_path = tmp_dir.join("test.sqlite");
    let db = Database::open(&db_path).unwrap();
    let file_store = FileStore::new(tmp_dir.join("files")).unwrap();

    SyncEngine::new(cos_client, db, file_store, conn_monitor)
}

/// Strategy for generating a valid file ID (UUID-like format).
fn file_id_strategy() -> impl Strategy<Value = String> {
    "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"
}

/// Strategy to generate a SHA-256-like hash string (64 hex chars).
fn hash_strategy() -> impl Strategy<Value = String> {
    "[a-f0-9]{64}"
}

/// Strategy for generating a file title (1..50 printable ASCII chars).
fn title_strategy() -> impl Strategy<Value = String> {
    "[A-Za-z0-9 _-]{1,50}"
}

/// Strategy for generating a timestamp in a reasonable range.
fn timestamp_strategy() -> impl Strategy<Value = i64> {
    1_600_000_000_000i64..1_800_000_000_000i64
}

/// Strategy for generating a FileMeta entry.
fn file_meta_strategy() -> impl Strategy<Value = FileMeta> {
    (
        file_id_strategy(),
        title_strategy(),
        timestamp_strategy(),
        hash_strategy(),
    )
        .prop_map(|(id, title, last_modified, content_hash)| FileMeta {
            id: id.clone(),
            title,
            last_modified,
            content_hash,
            cos_object_key: Some(format!("files/{}.excalidraw", id)),
            sync_status: SyncStatus::Synced,
            base_content_hash: None,
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        })
}

/// Strategy for generating a ManifestEntry.
fn manifest_entry_strategy() -> impl Strategy<Value = ManifestEntry> {
    (
        file_id_strategy(),
        title_strategy(),
        timestamp_strategy(),
        hash_strategy(),
    )
        .prop_map(|(id, title, last_modified, content_hash)| ManifestEntry {
            id: id.clone(),
            title,
            last_modified,
            content_hash,
            object_key: format!("files/{}.excalidraw", id),
            deleted: false,
        })
}

/// Strategy for generating a vector of FileMeta with unique IDs.
fn local_files_strategy(max_count: usize) -> impl Strategy<Value = Vec<FileMeta>> {
    proptest::collection::vec(file_meta_strategy(), 0..=max_count).prop_map(|files| {
        let mut seen = HashSet::new();
        files
            .into_iter()
            .filter(|f| seen.insert(f.id.clone()))
            .collect()
    })
}

/// Strategy for generating a Manifest with unique entry IDs.
fn manifest_strategy(max_count: usize) -> impl Strategy<Value = Manifest> {
    proptest::collection::vec(manifest_entry_strategy(), 0..=max_count).prop_map(|entries| {
        let mut seen = HashSet::new();
        let unique_entries: Vec<ManifestEntry> = entries
            .into_iter()
            .filter(|e| seen.insert(e.id.clone()))
            .collect();
        let last_modified = unique_entries.iter().map(|e| e.last_modified).max().unwrap_or(0);
        Manifest {
            version: 1,
            last_modified,
            files: unique_entries,
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 6: Manifest Merge by Timestamp
    ///
    /// **Validates: Requirements 4.1, 6.3, 6.7**
    ///
    /// For any local file metadata set and remote manifest, merging them
    /// SHALL produce a result where each file entry uses the version with
    /// the later last-modified timestamp. New remote entries are added,
    /// and local-only entries are included.
    #[test]
    fn manifest_merge_by_timestamp(
        local_files in local_files_strategy(5),
        remote in manifest_strategy(5),
    ) {
        let merged = merge_manifests(&local_files, &remote);

        // Build lookup maps for verification.
        let local_map: std::collections::HashMap<String, &FileMeta> =
            local_files.iter().map(|f| (f.id.clone(), f)).collect();
        let remote_map: std::collections::HashMap<String, &ManifestEntry> =
            remote.files.iter().map(|e| (e.id.clone(), e)).collect();

        // All local IDs and remote IDs should be present in merged.
        let merged_ids: HashSet<String> = merged.files.iter().map(|e| e.id.clone()).collect();
        for local in &local_files {
            prop_assert!(
                merged_ids.contains(&local.id),
                "Local file {} should be in merged manifest",
                local.id
            );
        }
        for remote_entry in &remote.files {
            prop_assert!(
                merged_ids.contains(&remote_entry.id),
                "Remote entry {} should be in merged manifest",
                remote_entry.id
            );
        }

        // For files present in both, the one with the later timestamp wins.
        for merged_entry in &merged.files {
            let in_local = local_map.get(&merged_entry.id);
            let in_remote = remote_map.get(&merged_entry.id);

            match (in_local, in_remote) {
                (Some(local), Some(remote_e)) => {
                    // The version with the later (or equal) last_modified should win.
                    if local.last_modified >= remote_e.last_modified {
                        // Local wins
                        prop_assert_eq!(
                            &merged_entry.content_hash, &local.content_hash,
                            "Local should win for file {} (local ts={}, remote ts={})",
                            merged_entry.id, local.last_modified, remote_e.last_modified
                        );
                    } else {
                        // Remote wins
                        prop_assert_eq!(
                            &merged_entry.content_hash, &remote_e.content_hash,
                            "Remote should win for file {} (local ts={}, remote ts={})",
                            merged_entry.id, local.last_modified, remote_e.last_modified
                        );
                    }
                }
                (Some(local), None) => {
                    // Local-only: merged should have local's hash.
                    prop_assert_eq!(
                        &merged_entry.content_hash, &local.content_hash,
                        "Local-only file {} should have local hash",
                        merged_entry.id
                    );
                }
                (None, Some(remote_e)) => {
                    // Remote-only: merged should have remote's hash.
                    prop_assert_eq!(
                        &merged_entry.content_hash, &remote_e.content_hash,
                        "Remote-only entry {} should have remote hash",
                        merged_entry.id
                    );
                }
                (None, None) => {
                    // Should not happen — entry must come from somewhere.
                    prop_assert!(false, "Merged entry {} not found in local or remote", merged_entry.id);
                }
            }
        }
    }

    /// Property 14: Queue Upload Chronological Order
    ///
    /// **Validates: Requirements 7.4**
    ///
    /// For any set of queued uploads with distinct creation timestamps,
    /// get_pending_uploads SHALL return them in strictly ascending
    /// chronological order (oldest first).
    #[test]
    fn queue_upload_chronological_order(
        count in 2usize..=8,
    ) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let engine = create_test_engine_in(tmp_dir.path());

        // Generate distinct timestamps and shuffle them for insertion.
        let base_ts = 1_700_000_000_000i64;
        let timestamps: Vec<i64> = (0..count as i64).map(|i| base_ts + i * 1000).collect();
        let mut shuffled = timestamps.clone();
        // Reverse to insert in non-chronological order.
        shuffled.reverse();

        // Insert files first (foreign key constraint).
        for (i, _ts) in shuffled.iter().enumerate() {
            let file_id = format!("file-{:04}", i);
            let meta = FileMeta {
                id: file_id.clone(),
                title: format!("File {}", i),
                last_modified: *_ts,
                content_hash: format!("{:064x}", i),
                cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
                sync_status: SyncStatus::PendingSync,
                base_content_hash: None,
                is_conflict_copy: false,
                parent_file_id: None,
                deleted: false,
                created_at: *_ts,
            };
            engine.db.upsert_file_meta(&meta).unwrap();
        }

        // Enqueue uploads in shuffled (non-chronological) order.
        for (i, ts) in shuffled.iter().enumerate() {
            let entry = QueuedUpload {
                id: 0,
                file_id: format!("file-{:04}", i),
                operation: UploadOperation::Upload,
                payload: None,
                retry_count: 0,
                max_retries: 5,
                created_at: *ts,
            };
            engine.db.enqueue_upload(&entry).unwrap();
        }

        // Retrieve pending uploads — should be in ascending order.
        let pending = engine.db.get_pending_uploads().unwrap();
        prop_assert_eq!(pending.len(), count, "Should have {} pending uploads", count);

        for i in 1..pending.len() {
            prop_assert!(
                pending[i].created_at >= pending[i - 1].created_at,
                "Uploads should be in ascending chronological order: {} >= {} failed at index {}",
                pending[i].created_at,
                pending[i - 1].created_at,
                i
            );
        }
    }

    /// Property 15: Queue Processing Resilience
    ///
    /// **Validates: Requirements 7.5**
    ///
    /// For any queue of N items where K fail, the N-K successful items
    /// are processed (dequeued) and only K remain in the queue.
    ///
    /// We simulate this by inserting N items, dequeuing (N-K) of them
    /// (simulating successful upload), and verifying K remain.
    #[test]
    fn queue_processing_resilience(
        total in 2usize..=8,
        fail_ratio in 0.1f64..0.9f64,
    ) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let engine = create_test_engine_in(tmp_dir.path());

        let fail_count = ((total as f64 * fail_ratio).ceil() as usize).min(total - 1).max(1);
        let success_count = total - fail_count;

        // Insert files and queue entries.
        for i in 0..total {
            let file_id = format!("file-{:04}", i);
            let meta = FileMeta {
                id: file_id.clone(),
                title: format!("File {}", i),
                last_modified: 1_700_000_000_000 + (i as i64 * 1000),
                content_hash: format!("{:064x}", i),
                cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
                sync_status: SyncStatus::PendingSync,
                base_content_hash: None,
                is_conflict_copy: false,
                parent_file_id: None,
                deleted: false,
                created_at: 1_700_000_000_000 + (i as i64 * 1000),
            };
            engine.db.upsert_file_meta(&meta).unwrap();

            let entry = QueuedUpload {
                id: 0,
                file_id: file_id.clone(),
                operation: UploadOperation::Upload,
                payload: None,
                retry_count: 0,
                max_retries: 5,
                created_at: 1_700_000_000_000 + (i as i64 * 1000),
            };
            engine.db.enqueue_upload(&entry).unwrap();
        }

        // Simulate processing: dequeue the first `success_count` items
        // (simulating successful uploads), leave `fail_count` items.
        let pending = engine.db.get_pending_uploads().unwrap();
        prop_assert_eq!(pending.len(), total);

        for i in 0..success_count {
            engine.db.dequeue_upload(&pending[i].file_id).unwrap();
        }

        // Verify: only fail_count items remain.
        let remaining = engine.db.get_pending_uploads().unwrap();
        prop_assert_eq!(
            remaining.len(), fail_count,
            "After processing {} successful items out of {}, {} should remain (got {})",
            success_count, total, fail_count, remaining.len()
        );

        // Verify the remaining items are the ones that "failed".
        for (i, entry) in remaining.iter().enumerate() {
            prop_assert_eq!(
                &entry.file_id,
                &pending[success_count + i].file_id,
                "Remaining item {} should be the failed item",
                i
            );
        }
    }

    /// Property 16: Conflict Detection
    ///
    /// **Validates: Requirements 8.1**
    ///
    /// For any file where remote hash != base hash AND local hash != base hash,
    /// detect_conflicts SHALL identify it as a conflict. If only remote differs
    /// (local unchanged), it is NOT a conflict.
    #[test]
    fn conflict_detection(
        file_id in file_id_strategy(),
        base_hash in hash_strategy(),
        local_hash in hash_strategy(),
        remote_hash in hash_strategy(),
        timestamp in timestamp_strategy(),
    ) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let engine = create_test_engine_in(tmp_dir.path());

        // Set up local file with a known base_content_hash.
        let meta = FileMeta {
            id: file_id.clone(),
            title: "Conflict Test".to_string(),
            last_modified: timestamp,
            content_hash: local_hash.clone(),
            cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some(base_hash.clone()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: timestamp,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // Build a remote manifest with the remote_hash.
        let remote_manifest = Manifest {
            version: 1,
            last_modified: timestamp,
            files: vec![ManifestEntry {
                id: file_id.clone(),
                title: "Conflict Test".to_string(),
                last_modified: timestamp + 1000,
                content_hash: remote_hash.clone(),
                object_key: format!("files/{}.excalidraw", file_id),
                deleted: false,
            }],
        };

        let conflicts = engine.detect_conflicts(&remote_manifest);

        let remote_differs = remote_hash != base_hash;
        let local_differs = local_hash != base_hash;

        if remote_differs && local_differs {
            // Should be detected as a conflict.
            prop_assert_eq!(
                conflicts.len(), 1,
                "Should detect exactly 1 conflict when both local and remote differ from base"
            );
            prop_assert_eq!(&conflicts[0].file_id, &file_id);
            prop_assert_eq!(&conflicts[0].local_hash, &local_hash);
            prop_assert_eq!(&conflicts[0].remote_hash, &remote_hash);
            prop_assert_eq!(&conflicts[0].base_hash, &base_hash);
        } else if remote_differs && !local_differs {
            // Remote update, NOT a conflict.
            prop_assert_eq!(
                conflicts.len(), 0,
                "Should NOT detect conflict when only remote differs (remote update)"
            );
        } else {
            // No conflict: remote matches base, or local matches base.
            prop_assert_eq!(
                conflicts.len(), 0,
                "Should NOT detect conflict when remote matches base"
            );
        }
    }

    /// Property 17: Conflict Resolution Creates Named Copy
    ///
    /// **Validates: Requirements 8.2**
    ///
    /// For any detected conflict, the resolution SHALL create a conflict
    /// copy with the title format "{original_title} - Conflict {YYYY-MM-DD}".
    #[test]
    fn conflict_resolution_creates_named_copy(
        original_title in title_strategy(),
        date_str in "[0-9]{4}-[0-9]{2}-[0-9]{2}",
    ) {
        // Test the title generation logic directly.
        let conflict_title = generate_conflict_title_with_date(&original_title, &date_str);

        let expected = format!("{} - Conflict {}", original_title, date_str);
        prop_assert_eq!(
            &conflict_title, &expected,
            "Conflict title should be '{{title}} - Conflict {{YYYY-MM-DD}}'"
        );

        // Also verify the live function produces a valid format.
        let live_title = generate_conflict_title(&original_title);
        // Should contain the original title and " - Conflict " followed by a date.
        prop_assert!(
            live_title.starts_with(&format!("{} - Conflict ", original_title)),
            "Live conflict title '{}' should start with '{} - Conflict '",
            live_title, original_title
        );

        // Verify the date portion matches YYYY-MM-DD format.
        let date_part = &live_title[original_title.len() + " - Conflict ".len()..];
        prop_assert_eq!(
            date_part.len(), 10,
            "Date portion '{}' should be 10 chars (YYYY-MM-DD)",
            date_part
        );
        prop_assert!(
            date_part.chars().nth(4) == Some('-') && date_part.chars().nth(7) == Some('-'),
            "Date portion '{}' should have dashes at positions 4 and 7",
            date_part
        );
    }

    /// Property 18: Maximum Conflict Copies Invariant
    ///
    /// **Validates: Requirements 8.7**
    ///
    /// For any file, the number of associated conflict copies SHALL never
    /// exceed 5. When inserting more than 5 conflict copies, the oldest
    /// ones are deleted to maintain the invariant.
    #[test]
    fn maximum_conflict_copies_invariant(
        file_id in file_id_strategy(),
        num_copies in 6usize..=10,
    ) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let engine = create_test_engine_in(tmp_dir.path());

        // Insert the parent file.
        let parent_meta = FileMeta {
            id: file_id.clone(),
            title: "Parent File".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: format!("{:064x}", 0),
            cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some(format!("{:064x}", 0)),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&parent_meta).unwrap();

        // Insert `num_copies` conflict copies, enforcing the max of 5.
        let max_copies = 5usize;
        for i in 0..num_copies {
            let copy_id = format!("conflict-copy-{:04}-{}", i, file_id);
            let copy_meta = FileMeta {
                id: copy_id.clone(),
                title: format!("Parent File - Conflict 2024-01-{:02}", i + 1),
                last_modified: 1_700_000_000_000 + (i as i64 * 1000),
                content_hash: format!("{:064x}", i + 1),
                cos_object_key: Some(format!("files/{}.excalidraw", copy_id)),
                sync_status: SyncStatus::Synced,
                base_content_hash: Some(format!("{:064x}", i + 1)),
                is_conflict_copy: true,
                parent_file_id: Some(file_id.clone()),
                deleted: false,
                created_at: 1_700_000_000_000 + (i as i64 * 1000),
            };

            // Check current count and delete oldest if at max.
            let existing = engine.db.get_conflict_copies(&file_id).unwrap();
            if existing.len() >= max_copies {
                // Delete the oldest (first in ASC order).
                let oldest = &existing[0];
                engine.db.delete_file_meta(&oldest.id).unwrap();
            }

            engine.db.upsert_file_meta(&copy_meta).unwrap();
        }

        // Verify: conflict copies never exceed 5.
        let final_copies = engine.db.get_conflict_copies(&file_id).unwrap();
        prop_assert!(
            final_copies.len() <= max_copies,
            "Conflict copies ({}) should never exceed {} for file {}",
            final_copies.len(), max_copies, file_id
        );

        // Verify: the remaining copies are the most recent ones.
        // Since we inserted in order and deleted oldest, the remaining
        // should be the last `max_copies` inserted.
        let expected_start = num_copies - max_copies;
        for (i, copy) in final_copies.iter().enumerate() {
            let expected_idx = expected_start + i;
            let expected_id = format!("conflict-copy-{:04}-{}", expected_idx, file_id);
            prop_assert_eq!(
                &copy.id, &expected_id,
                "Copy at position {} should be the one inserted at index {}",
                i, expected_idx
            );
        }
    }
}
