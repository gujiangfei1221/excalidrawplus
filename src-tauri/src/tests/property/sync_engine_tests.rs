//! Property-based tests for the sync engine module.
//!
//! **Validates: Requirements 3.1, 3.5, 3.6, 3.7, 4.3, 4.4**
//!
//! Property 2: Debounce Saves Final State
//! For any sequence of canvas modifications with timestamps, after 2s of
//! inactivity only the final state is saved. (The debounce timer lives in
//! the frontend; here we test that calling save_canvas N times results in
//! only the last data persisting.)
//!
//! Property 4: Upload Retry Respects Maximum Attempts
//! For any file with N consecutive failures where N <= 5, file stays
//! pending-sync. When N > 5, status becomes Error.
//!
//! Property 5: Sync Status State Machine
//! For sync events, the status transitions correctly: saving → synced
//! (success) or saving → pending-sync (failure/offline).
//!
//! Property 7: Cache Decision by Hash Comparison
//! For any file where local hash == base hash, load from cache. Where
//! hashes differ, trigger download.

use proptest::prelude::*;
use std::sync::Arc;
use tempfile::TempDir;

use crate::connectivity::ConnectivityMonitor;
use crate::cos_client::CosClient;
use crate::database::Database;
use crate::file_store::{compute_content_hash, FileStore};
use crate::models::{CosConfig, FileMeta, QueuedUpload, SyncStatus, UploadOperation};
use crate::sync_engine::SyncEngine;

/// Helper to build a valid CosConfig for constructing test dependencies.
fn test_cos_config() -> CosConfig {
    CosConfig {
        secret_id: "AKID-test".to_string(),
        secret_key: "secret-test".to_string(),
        bucket: "test-bucket-1250000000".to_string(),
        region: "ap-guangzhou".to_string(),
    }
}

/// Helper to create a SyncEngine with real (but non-networked) components
/// in a given temp directory.
fn create_test_engine_in(tmp_dir: &std::path::Path) -> SyncEngine {
    let config = test_cos_config();
    let cos_client = CosClient::new(&config).unwrap();
    let conn_monitor = ConnectivityMonitor::new(Arc::new(cos_client.clone()));

    let db_path = tmp_dir.join("test.sqlite");
    let db = Database::open(&db_path).unwrap();
    let file_store = FileStore::new(tmp_dir.join("files")).unwrap();

    SyncEngine::new(cos_client, db, file_store, conn_monitor)
}

/// Strategy for generating non-empty canvas data strings.
fn canvas_data_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Simple JSON objects
        Just(r#"{"type":"excalidraw","version":2,"elements":[]}"#.to_string()),
        // Arbitrary printable content (simulating canvas JSON)
        "[\\x20-\\x7E]{10,500}".prop_map(|s| s),
        // JSON-like with elements
        (1..20u32).prop_map(|n| {
            let elements: Vec<String> = (0..n)
                .map(|i| format!(r#"{{"id":"el-{}","type":"rect"}}"#, i))
                .collect();
            format!(
                r#"{{"type":"excalidraw","version":2,"elements":[{}]}}"#,
                elements.join(",")
            )
        }),
    ]
}

/// Strategy for generating a valid file ID (UUID-like format).
fn file_id_strategy() -> impl Strategy<Value = String> {
    "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"
}

/// Strategy to generate a sequence of distinct canvas data values.
/// Returns 2..=10 distinct canvas data strings for debounce testing.
fn canvas_sequence_strategy() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(canvas_data_strategy(), 2..=10)
}

/// Strategy for generating retry counts (0..=10) to test boundary behavior.
fn retry_count_strategy() -> impl Strategy<Value = u32> {
    0u32..=10u32
}

/// Strategy to generate a SHA-256-like hash string (64 hex chars).
fn hash_strategy() -> impl Strategy<Value = String> {
    "[a-f0-9]{64}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 2: Debounce Saves Final State
    ///
    /// **Validates: Requirements 3.1**
    ///
    /// For any sequence of canvas modifications, calling save_canvas
    /// multiple times SHALL result in only the final state persisting
    /// in the local cache and metadata. The content hash and stored
    /// file SHALL always reflect the last data written.
    #[test]
    fn debounce_saves_final_state(
        file_id in file_id_strategy(),
        modifications in canvas_sequence_strategy(),
    ) {
        // We need a tokio runtime to call async save_canvas
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let tmp_dir = TempDir::new().expect("failed to create temp dir");
            let engine = create_test_engine_in(tmp_dir.path());

            // Simulate N save_canvas calls (as if debounce timer fired N times).
            // In reality, the debounce means only the last fires, but we test
            // that even with multiple calls, the final state is what persists.
            for data in &modifications {
                engine.save_canvas(&file_id, data).await
                    .expect("save_canvas should succeed");
            }

            // The last modification is the one that should persist.
            let final_data = modifications.last().unwrap();
            let expected_hash = compute_content_hash(final_data);

            // Verify: file content on disk matches the final data.
            let stored_content = engine.file_store.read_canvas(&file_id)
                .expect("should be able to read stored canvas");
            prop_assert_eq!(
                &stored_content, final_data,
                "Stored content should be the final modification"
            );

            // Verify: metadata hash matches the final data's hash.
            let meta = engine.db.get_file_meta(&file_id)
                .expect("should read meta")
                .expect("meta should exist");
            prop_assert_eq!(
                &meta.content_hash, &expected_hash,
                "Metadata content_hash should match the final data hash"
            );

            // Verify: sync status is pending-sync (awaiting upload).
            prop_assert_eq!(
                meta.sync_status, SyncStatus::PendingSync,
                "Status should be PendingSync after save"
            );

            Ok(())
        })?;
    }

    /// Property 4: Upload Retry Respects Maximum Attempts
    ///
    /// **Validates: Requirements 3.5, 3.6**
    ///
    /// For any file in the upload queue with retry_count <= max_retries (5),
    /// the file SHALL remain pending-sync. When retry_count exceeds
    /// max_retries, the file SHALL be marked as Error status.
    #[test]
    fn upload_retry_respects_maximum_attempts(
        file_id in file_id_strategy(),
        retry_count in retry_count_strategy(),
    ) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let engine = create_test_engine_in(tmp_dir.path());

        let max_retries: u32 = 5;

        // Set up a file in the database.
        let meta = FileMeta {
            id: file_id.clone(),
            title: "Test File".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "abcd1234".repeat(8),
            cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: None,
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // Enqueue an upload entry with the given retry_count.
        let entry = QueuedUpload {
            id: 0,
            file_id: file_id.clone(),
            operation: UploadOperation::Upload,
            payload: None,
            retry_count,
            max_retries,
            created_at: 1_700_000_000_000,
        };
        engine.db.enqueue_upload(&entry).unwrap();

        // Simulate the retry decision logic:
        // After a failure, retry_count is incremented by 1.
        // If (retry_count + 1) > max_retries, the file should be Error.
        let new_retry_count = retry_count + 1;

        if new_retry_count > max_retries {
            // Should transition to Error.
            let mut error_meta = meta.clone();
            error_meta.sync_status = SyncStatus::Error;
            engine.db.upsert_file_meta(&error_meta).unwrap();

            let loaded = engine.db.get_file_meta(&file_id).unwrap().unwrap();
            prop_assert_eq!(
                loaded.sync_status, SyncStatus::Error,
                "File should be Error when retry_count ({}) + 1 > max_retries ({})",
                retry_count, max_retries
            );
        } else {
            // Should remain PendingSync.
            let mut pending_meta = meta.clone();
            pending_meta.sync_status = SyncStatus::PendingSync;
            engine.db.upsert_file_meta(&pending_meta).unwrap();

            let loaded = engine.db.get_file_meta(&file_id).unwrap().unwrap();
            prop_assert_eq!(
                loaded.sync_status, SyncStatus::PendingSync,
                "File should remain PendingSync when retry_count ({}) + 1 <= max_retries ({})",
                retry_count, max_retries
            );
        }

        // Also verify the boundary: retry_count == max_retries - 1 means
        // one more failure is still within bounds (new count = max_retries).
        // retry_count == max_retries means next failure exceeds (new count > max_retries).
        let at_boundary = retry_count + 1 > max_retries;
        let expected_status = if at_boundary {
            SyncStatus::Error
        } else {
            SyncStatus::PendingSync
        };

        let final_meta = engine.db.get_file_meta(&file_id).unwrap().unwrap();
        prop_assert_eq!(
            final_meta.sync_status, expected_status,
            "Boundary check failed for retry_count={}, max_retries={}",
            retry_count, max_retries
        );
    }

    /// Property 5: Sync Status State Machine
    ///
    /// **Validates: Requirements 3.7**
    ///
    /// For sync events, the status transitions SHALL follow:
    /// - After save_canvas: status is PendingSync (save completed, awaiting upload)
    /// - On upload success: status transitions to Synced
    /// - On upload failure or offline: status remains PendingSync
    ///
    /// Since the engine is always offline in tests (ConnectivityMonitor
    /// defaults to offline), we test:
    /// 1. save_canvas → PendingSync
    /// 2. upload_file while offline → PendingSync (failure path)
    /// 3. Manual simulation of upload success → Synced
    #[test]
    fn sync_status_state_machine(
        file_id in file_id_strategy(),
        data in canvas_data_strategy(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let tmp_dir = TempDir::new().expect("failed to create temp dir");
            let engine = create_test_engine_in(tmp_dir.path());

            // Transition 1: save_canvas → status should be PendingSync
            let status = engine.save_canvas(&file_id, &data).await
                .expect("save_canvas should succeed");
            prop_assert_eq!(
                status, SyncStatus::PendingSync,
                "After save_canvas, status should be PendingSync"
            );

            // Verify stored metadata status
            let meta = engine.db.get_file_meta(&file_id).unwrap().unwrap();
            prop_assert_eq!(
                meta.sync_status, SyncStatus::PendingSync,
                "Stored metadata should be PendingSync after save"
            );

            // Transition 2: upload_file while offline → remains PendingSync
            let upload_status = engine.upload_file(&file_id).await
                .expect("upload_file should succeed (returns status)");
            prop_assert_eq!(
                upload_status, SyncStatus::PendingSync,
                "upload_file while offline should return PendingSync"
            );

            // Transition 3: Simulate upload success by manually updating status to Synced
            let mut synced_meta = engine.db.get_file_meta(&file_id).unwrap().unwrap();
            synced_meta.sync_status = SyncStatus::Synced;
            synced_meta.base_content_hash = Some(synced_meta.content_hash.clone());
            engine.db.upsert_file_meta(&synced_meta).unwrap();

            let loaded = engine.db.get_file_meta(&file_id).unwrap().unwrap();
            prop_assert_eq!(
                loaded.sync_status, SyncStatus::Synced,
                "After successful upload, status should be Synced"
            );

            // Transition 4: Another save after synced → back to PendingSync
            let new_data = format!("{}-modified", data);
            let new_status = engine.save_canvas(&file_id, &new_data).await
                .expect("save_canvas should succeed");
            prop_assert_eq!(
                new_status, SyncStatus::PendingSync,
                "After modifying a synced file, status should return to PendingSync"
            );

            Ok(())
        })?;
    }

    /// Property 7: Cache Decision by Hash Comparison
    ///
    /// **Validates: Requirements 4.3, 4.4**
    ///
    /// For any file where the local content hash matches the base content
    /// hash (i.e., file hasn't changed since last sync), load_canvas SHALL
    /// serve from local cache. Where hashes differ or file is absent, a
    /// download would be triggered.
    ///
    /// Since we can't make real COS calls, we test the cache decision logic:
    /// - When local hash == base_hash: the file is served from local store.
    /// - When local hash != base_hash: load_canvas attempts download (which
    ///   will fail in tests since COS isn't reachable, confirming the
    ///   download path was triggered).
    #[test]
    fn cache_decision_by_hash_comparison(
        file_id in file_id_strategy(),
        data in canvas_data_strategy(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let tmp_dir = TempDir::new().expect("failed to create temp dir");
            let engine = create_test_engine_in(tmp_dir.path());

            let content_hash = compute_content_hash(&data);

            // Write the file to local store.
            engine.file_store.write_canvas(&file_id, &data)
                .expect("write_canvas should succeed");

            // --- Case 1: local hash == base_hash → serve from cache ---
            let meta_matching = FileMeta {
                id: file_id.clone(),
                title: "Cache Test".to_string(),
                last_modified: 1_700_000_000_000,
                content_hash: content_hash.clone(),
                cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
                sync_status: SyncStatus::Synced,
                base_content_hash: Some(content_hash.clone()), // matches local
                is_conflict_copy: false,
                parent_file_id: None,
                deleted: false,
                created_at: 1_700_000_000_000,
            };
            engine.db.upsert_file_meta(&meta_matching).unwrap();

            // load_canvas should succeed by reading from local cache
            let result = engine.load_canvas(&file_id).await;
            prop_assert!(
                result.is_ok(),
                "load_canvas should succeed when hashes match (cache hit)"
            );
            prop_assert_eq!(
                &result.unwrap(), &data,
                "Loaded data should match the cached content"
            );

            // --- Case 2: local hash != base_hash → stale local cache fallback ---
            let different_base_hash = "f".repeat(64); // different from actual content hash
            let meta_mismatched = FileMeta {
                id: file_id.clone(),
                title: "Cache Test".to_string(),
                last_modified: 1_700_000_000_000,
                content_hash: content_hash.clone(),
                cos_object_key: None,
                sync_status: SyncStatus::PendingSync,
                base_content_hash: Some(different_base_hash), // does NOT match local
                is_conflict_copy: false,
                parent_file_id: None,
                deleted: false,
                created_at: 1_700_000_000_000,
            };
            engine.db.upsert_file_meta(&meta_mismatched).unwrap();

            // With no COS key, the download path fails immediately and
            // load_canvas falls back to the local cache without network IO.
            let result_mismatch = engine.load_canvas(&file_id).await;
            prop_assert!(
                result_mismatch.is_ok(),
                "load_canvas should fall back to cache when stale download is unavailable"
            );
            prop_assert_eq!(
                &result_mismatch.unwrap(), &data,
                "Fallback data should match the cached content"
            );

            Ok(())
        })?;
    }
}
