//! Integration-style tests for local-first sync flows.
//!
//! These tests intentionally avoid real COS network calls. They exercise the
//! same `SyncEngine` public surface used by Tauri commands for save/load and
//! offline queue behavior.

use std::sync::Arc;

use tempfile::TempDir;

use crate::connectivity::ConnectivityMonitor;
use crate::cos_client::CosClient;
use crate::database::Database;
use crate::file_store::FileStore;
use crate::models::{CosConfig, SyncStatus};
use crate::sync_engine::SyncEngine;

fn test_cos_config() -> CosConfig {
    CosConfig {
        secret_id: "AKID-test".to_string(),
        secret_key: "secret-test".to_string(),
        bucket: "test-bucket-1250000000".to_string(),
        region: "ap-guangzhou".to_string(),
    }
}

fn create_test_engine_in(tmp_dir: &std::path::Path) -> SyncEngine {
    let config = test_cos_config();
    let cos_client = CosClient::new(&config).unwrap();
    let conn_monitor = ConnectivityMonitor::new(Arc::new(cos_client.clone()));
    let db = Database::open(&tmp_dir.join("metadata.sqlite")).unwrap();
    let file_store = FileStore::new(tmp_dir.join("files")).unwrap();

    SyncEngine::new(cos_client, db, file_store, conn_monitor)
}

#[tokio::test]
async fn full_local_save_cycle_persists_canvas_and_queue_entry() {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let engine = create_test_engine_in(tmp_dir.path());
    let canvas = r#"{"type":"excalidraw","version":2,"source":"test","elements":[],"appState":{},"files":{}}"#;

    let status = engine.save_canvas("file-1", canvas).await.unwrap();

    assert_eq!(status, SyncStatus::PendingSync);
    assert_eq!(engine.file_store.read_canvas("file-1").unwrap(), canvas);

    let meta = engine.db.get_file_meta("file-1").unwrap().unwrap();
    assert_eq!(meta.sync_status, SyncStatus::PendingSync);

    let pending = engine.db.get_pending_uploads().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].file_id, "file-1");
}

#[tokio::test]
async fn full_local_load_cycle_uses_cache_when_hashes_match() {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let engine = create_test_engine_in(tmp_dir.path());
    let canvas = r#"{"type":"excalidraw","version":2,"source":"test","elements":[],"appState":{},"files":{}}"#;

    engine.save_canvas("file-1", canvas).await.unwrap();
    let mut meta = engine.db.get_file_meta("file-1").unwrap().unwrap();
    meta.base_content_hash = Some(meta.content_hash.clone());
    meta.sync_status = SyncStatus::Synced;
    engine.db.upsert_file_meta(&meta).unwrap();

    let loaded = engine.load_canvas("file-1").await.unwrap();

    assert_eq!(loaded, canvas);
}

#[tokio::test]
async fn offline_queue_processing_leaves_pending_items_for_retry() {
    let tmp_dir = TempDir::new().expect("failed to create temp dir");
    let engine = create_test_engine_in(tmp_dir.path());
    let canvas = r#"{"type":"excalidraw","version":2,"source":"test","elements":[],"appState":{},"files":{}}"#;

    engine.save_canvas("file-1", canvas).await.unwrap();
    engine.process_upload_queue().await.unwrap();

    let pending = engine.db.get_pending_uploads().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].file_id, "file-1");
}
