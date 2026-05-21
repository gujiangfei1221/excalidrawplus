//! Property-based tests for the SQLite database layer.
//!
//! **Validates: Requirements 2.2, 7.2, 7.3**
//!
//! Property 1: COS Configuration Round-Trip
//! For any valid COS configuration (non-empty SecretId, SecretKey, Bucket, Region),
//! saving it and loading it back produces an identical configuration.
//!
//! Property 12 (metadata portion): Canvas Data Filesystem Round-Trip
//! For any valid FileMeta, upserting it and reading it back produces an equivalent FileMeta.
//!
//! Property 13: Offline Queue Persistence
//! For any set of QueuedUpload entries enqueued while "offline", after simulated
//! restart (close/reopen database), all entries are still present and unmodified.

use proptest::prelude::*;
use tempfile::TempDir;

use crate::database::Database;
use crate::models::{CosConfig, FileMeta, QueuedUpload, SyncStatus, UploadOperation};

/// Strategy to generate non-empty strings suitable for COS config fields.
/// COS fields must be non-empty, so we generate 1..100 printable ASCII characters.
fn non_empty_ascii_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_\\-\\.]{1,100}"
}

/// Strategy to generate valid COS configurations with non-empty fields.
fn cos_config_strategy() -> impl Strategy<Value = CosConfig> {
    (
        non_empty_ascii_string(), // secret_id
        non_empty_ascii_string(), // secret_key
        non_empty_ascii_string(), // bucket
        non_empty_ascii_string(), // region
    )
        .prop_map(|(secret_id, secret_key, bucket, region)| CosConfig {
            secret_id,
            secret_key,
            bucket,
            region,
        })
}

/// Strategy to generate a valid SyncStatus variant.
fn sync_status_strategy() -> impl Strategy<Value = SyncStatus> {
    prop_oneof![
        Just(SyncStatus::Synced),
        Just(SyncStatus::PendingSync),
        Just(SyncStatus::Saving),
        Just(SyncStatus::Conflict),
        Just(SyncStatus::Error),
    ]
}

/// Strategy to generate valid file IDs (UUID-like alphanumeric with hyphens).
fn file_id_strategy() -> impl Strategy<Value = String> {
    "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"
}

/// Strategy to generate valid FileMeta entries.
fn file_meta_strategy() -> impl Strategy<Value = FileMeta> {
    (
        file_id_strategy(),                           // id
        "[a-zA-Z0-9 _\\-]{1,80}",                    // title
        1_600_000_000_000i64..1_800_000_000_000i64,   // last_modified
        "[a-f0-9]{64}",                               // content_hash
        proptest::option::of("[a-z]+/[a-f0-9\\-]+\\.excalidraw"), // cos_object_key
        sync_status_strategy(),                       // sync_status
        proptest::option::of("[a-f0-9]{64}"),          // base_content_hash
        any::<bool>(),                                // is_conflict_copy
        proptest::option::of(file_id_strategy()),      // parent_file_id
        any::<bool>(),                                // deleted
        1_600_000_000_000i64..1_800_000_000_000i64,   // created_at
    )
        .prop_map(
            |(
                id,
                title,
                last_modified,
                content_hash,
                cos_object_key,
                sync_status,
                base_content_hash,
                is_conflict_copy,
                parent_file_id,
                deleted,
                created_at,
            )| {
                FileMeta {
                    id,
                    title,
                    last_modified,
                    content_hash,
                    cos_object_key,
                    sync_status,
                    base_content_hash,
                    is_conflict_copy,
                    parent_file_id,
                    deleted,
                    created_at,
                }
            },
        )
}

/// Strategy to generate a valid UploadOperation variant.
fn upload_operation_strategy() -> impl Strategy<Value = UploadOperation> {
    prop_oneof![
        Just(UploadOperation::Upload),
        Just(UploadOperation::Delete),
        Just(UploadOperation::Rename),
    ]
}

/// Strategy to generate a QueuedUpload entry.
/// The `id` field is ignored on insert (SQLite auto-assigns it), so we
/// use 0 as a placeholder.
fn queued_upload_strategy(file_id: String) -> impl Strategy<Value = QueuedUpload> {
    (
        upload_operation_strategy(),
        proptest::option::of("[a-zA-Z0-9 _\\-\\{\\}:\"]{0,200}"), // payload
        0u32..3u32,                                                 // retry_count
        3u32..10u32,                                                // max_retries
        1_600_000_000_000i64..1_800_000_000_000i64,                 // created_at
    )
        .prop_map(move |(operation, payload, retry_count, max_retries, created_at)| {
            QueuedUpload {
                id: 0, // auto-assigned by SQLite
                file_id: file_id.clone(),
                operation,
                payload,
                retry_count,
                max_retries,
                created_at,
            }
        })
}

/// Helper: compare two UploadOperation values for equality.
fn operations_equal(a: &UploadOperation, b: &UploadOperation) -> bool {
    matches!(
        (a, b),
        (UploadOperation::Upload, UploadOperation::Upload)
            | (UploadOperation::Delete, UploadOperation::Delete)
            | (UploadOperation::Rename, UploadOperation::Rename)
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 1: COS Configuration Round-Trip
    ///
    /// **Validates: Requirements 2.2**
    ///
    /// For any valid COS configuration (non-empty SecretId, SecretKey, Bucket,
    /// and Region), saving it to the SQLite store and then loading it back
    /// SHALL produce an identical configuration object.
    #[test]
    fn cos_config_round_trip(config in cos_config_strategy()) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp_dir.path().join("test.sqlite");
        let db = Database::open(&db_path).expect("failed to open database");

        // Save the config
        db.save_cos_config(&config).expect("save_cos_config failed");

        // Load it back
        let loaded = db
            .get_cos_config()
            .expect("get_cos_config failed")
            .expect("config should be Some after saving");

        // All fields must match exactly
        prop_assert_eq!(&loaded.secret_id, &config.secret_id,
            "secret_id mismatch");
        prop_assert_eq!(&loaded.secret_key, &config.secret_key,
            "secret_key mismatch");
        prop_assert_eq!(&loaded.bucket, &config.bucket,
            "bucket mismatch");
        prop_assert_eq!(&loaded.region, &config.region,
            "region mismatch");
    }

    /// Property 12 (metadata portion): FileMeta Round-Trip
    ///
    /// **Validates: Requirements 7.2**
    ///
    /// For any valid FileMeta, upserting it into the SQLite database and
    /// reading it back SHALL produce an equivalent FileMeta with all fields
    /// preserved.
    #[test]
    fn file_meta_round_trip(meta in file_meta_strategy()) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp_dir.path().join("test.sqlite");
        let db = Database::open(&db_path).expect("failed to open database");

        // Upsert the file metadata
        db.upsert_file_meta(&meta).expect("upsert_file_meta failed");

        // Read it back
        let loaded = db
            .get_file_meta(&meta.id)
            .expect("get_file_meta failed")
            .expect("file meta should be Some after upserting");

        // All fields must match
        prop_assert_eq!(&loaded.id, &meta.id, "id mismatch");
        prop_assert_eq!(&loaded.title, &meta.title, "title mismatch");
        prop_assert_eq!(loaded.last_modified, meta.last_modified,
            "last_modified mismatch");
        prop_assert_eq!(&loaded.content_hash, &meta.content_hash,
            "content_hash mismatch");
        prop_assert_eq!(&loaded.cos_object_key, &meta.cos_object_key,
            "cos_object_key mismatch");
        prop_assert_eq!(loaded.sync_status, meta.sync_status,
            "sync_status mismatch");
        prop_assert_eq!(&loaded.base_content_hash, &meta.base_content_hash,
            "base_content_hash mismatch");
        prop_assert_eq!(loaded.is_conflict_copy, meta.is_conflict_copy,
            "is_conflict_copy mismatch");
        prop_assert_eq!(&loaded.parent_file_id, &meta.parent_file_id,
            "parent_file_id mismatch");
        prop_assert_eq!(loaded.deleted, meta.deleted, "deleted mismatch");
        prop_assert_eq!(loaded.created_at, meta.created_at,
            "created_at mismatch");
    }

    /// Property 13: Offline Queue Persistence
    ///
    /// **Validates: Requirements 7.3**
    ///
    /// For any set of QueuedUpload entries enqueued while "offline", after a
    /// simulated application restart (close and reopen the database), all
    /// queued entries SHALL still be present and unmodified.
    #[test]
    fn offline_queue_persistence(
        meta in file_meta_strategy(),
        entry_count in 1usize..6,
    ) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp_dir.path().join("test.sqlite");

        // We need to collect entries deterministically based on input.
        // Generate a fixed set of entries manually to avoid nested strategies.
        let entries: Vec<QueuedUpload> = (0..entry_count)
            .map(|i| QueuedUpload {
                id: 0,
                file_id: meta.id.clone(),
                operation: match i % 3 {
                    0 => UploadOperation::Upload,
                    1 => UploadOperation::Delete,
                    _ => UploadOperation::Rename,
                },
                payload: if i % 2 == 0 {
                    Some(format!("payload-{}", i))
                } else {
                    None
                },
                retry_count: (i as u32) % 4,
                max_retries: 5,
                created_at: meta.created_at + (i as i64) * 1000,
            })
            .collect();

        // Phase 1: Open database, insert the file metadata (FK requirement),
        // then enqueue all upload entries.
        {
            let db = Database::open(&db_path).expect("failed to open database");
            db.upsert_file_meta(&meta).expect("upsert_file_meta failed");

            for entry in &entries {
                db.enqueue_upload(entry).expect("enqueue_upload failed");
            }

            // Verify entries are present before "restart"
            let pending = db.get_pending_uploads().expect("get_pending_uploads failed");
            prop_assert_eq!(
                pending.len(), entries.len(),
                "entries should be present before restart"
            );
        }
        // Database dropped here, simulating app close.

        // Phase 2: Reopen the database (simulating application restart).
        {
            let db = Database::open(&db_path).expect("failed to reopen database");
            let pending = db.get_pending_uploads().expect("get_pending_uploads failed");

            // All entries must still be present
            prop_assert_eq!(
                pending.len(), entries.len(),
                "all entries should survive restart, expected {} got {}",
                entries.len(), pending.len()
            );

            // Verify each entry's fields match (order is by created_at ASC)
            for (loaded, original) in pending.iter().zip(entries.iter()) {
                prop_assert_eq!(&loaded.file_id, &original.file_id,
                    "file_id mismatch after restart");
                prop_assert!(
                    operations_equal(&loaded.operation, &original.operation),
                    "operation mismatch after restart"
                );
                prop_assert_eq!(&loaded.payload, &original.payload,
                    "payload mismatch after restart");
                prop_assert_eq!(loaded.retry_count, original.retry_count,
                    "retry_count mismatch after restart");
                prop_assert_eq!(loaded.max_retries, original.max_retries,
                    "max_retries mismatch after restart");
                prop_assert_eq!(loaded.created_at, original.created_at,
                    "created_at mismatch after restart");
            }
        }
    }
}
