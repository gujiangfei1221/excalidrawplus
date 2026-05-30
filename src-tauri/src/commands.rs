//! Tauri command layer exposing Rust backend functionality to the frontend.
//!
//! All commands are async and return `Result<T, String>` for error
//! propagation across the Tauri IPC boundary. The `AppState` struct
//! holds shared references to the sync engine and database, managed
//! by Tauri's state system.
//!
//! Task 10.1 implements COS configuration commands:
//!   * `save_cos_config` — persist credentials to local SQLite
//!   * `validate_cos_config` — test connection with 10-second timeout
//!   * `get_cos_config` — retrieve persisted config
//!
//! Task 10.2 implements file operation commands:
//!   * `save_canvas` — save canvas data to local store and enqueue upload
//!   * `load_canvas` — load canvas data with hash-based cache decision
//!   * `download_canvas` — force-download canvas data from COS
//!   * `create_new_file` — generate UUID v4 file ID and create empty canvas
//!   * `delete_file` — soft-delete file and enqueue delete operation
//!   * `rename_file` — validate title and update metadata
//!   * `export_file` — export via native file dialog
//!
//! Task 10.3 implements file list and sync commands:
//!   * `get_file_list` — returns all files sorted by last_modified DESC
//!   * `trigger_sync` — triggers a manifest sync
//!   * `get_sync_status` — returns the sync status of a specific file
//!
//! Validates: Requirements 2.2, 2.3, 2.4, 2.5, 2.6, 3.4, 5.1, 4.1,
//!            9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 9.7, 5.5, 5.6

use std::sync::Arc;

use serde::Serialize;
use tauri::Emitter;
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

use crate::cos_client::CosClient;
use crate::file_store::compute_content_hash;
use crate::models::{CosConfig, FileMeta, QueuedUpload, SyncStatus, UploadOperation};
use crate::sync_engine::{cos_object_key_for_title, SyncEngine};

/// Application state managed by Tauri's state system.
///
/// Holds shared references to the sync engine (which itself contains
/// the database, COS client, file store, and connectivity monitor).
/// The `Mutex` is required because `SyncEngine` is not `Sync` (it
/// holds `JoinHandle`s and mutable state for background tasks).
pub struct AppState {
    pub sync_engine: Arc<Mutex<SyncEngine>>,
}

const SYNC_STATUS_EVENT: &str = "cloud-sync://sync-status";
const FILE_LIST_CHANGED_EVENT: &str = "cloud-sync://file-list-changed";

pub(crate) fn validate_file_title(title: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("Title must not be empty or whitespace-only".to_string());
    }

    if title.chars().count() > 100 {
        return Err("Title must not exceed 100 characters".to_string());
    }

    Ok(())
}

fn next_untitled_title(existing_files: &[FileMeta]) -> String {
    let titles: std::collections::HashSet<&str> = existing_files
        .iter()
        .filter(|file| !file.deleted)
        .map(|file| file.title.as_str())
        .collect();

    if !titles.contains("Untitled") {
        return "Untitled".to_string();
    }

    for index in 2.. {
        let candidate = format!("Untitled {index}");
        if !titles.contains(candidate.as_str()) {
            return candidate;
        }
    }

    unreachable!("unbounded loop should always return a title")
}

fn validate_unique_file_title(
    existing_files: &[FileMeta],
    current_file_id: &str,
    title: &str,
) -> Result<(), String> {
    if existing_files.iter().any(|file| {
        !file.deleted && file.id != current_file_id && file.title.eq_ignore_ascii_case(title)
    }) {
        return Err("A file with this title already exists".to_string());
    }

    Ok(())
}

// ── COS Configuration Commands (Task 10.1) ──────────────────────────

/// Persist a COS configuration to the local SQLite database.
///
/// This command stores the credentials in the application data directory
/// and never transmits them to the frontend WebView. On update, the
/// existing configuration is overwritten (single-row pattern).
///
/// Validates: Requirements 2.2, 2.5, 3.4
#[tauri::command]
pub async fn save_cos_config(
    config: CosConfig,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!("save_cos_config requested");
    // Validate that all fields are non-empty
    if config.secret_id.trim().is_empty() {
        return Err("SecretId must not be empty".to_string());
    }
    if config.secret_key.trim().is_empty() {
        return Err("SecretKey must not be empty".to_string());
    }
    if config.bucket.trim().is_empty() {
        return Err("Bucket must not be empty".to_string());
    }
    if config.region.trim().is_empty() {
        return Err("Region must not be empty".to_string());
    }

    let cos_client = CosClient::new(&config)?;

    let mut engine = state.sync_engine.lock().await;
    engine
        .db
        .save_cos_config(&config)
        .map_err(|e| format!("Failed to save COS config: {e}"))?;
    engine.enable_cloud_sync(cos_client, app_handle)?;
    info!("save_cos_config succeeded");

    Ok(())
}

/// Validate a COS configuration by attempting a test connection.
///
/// Creates a temporary `CosClient` from the provided config and calls
/// `test_connection()` with a 10-second timeout. Returns `Ok(true)` on
/// success, or an error string describing the failure reason so the
/// frontend can display it to the user.
///
/// Validates: Requirements 2.3, 2.4
#[tauri::command]
pub async fn validate_cos_config(config: CosConfig) -> Result<bool, String> {
    info!("validate_cos_config requested");
    // Validate that all fields are non-empty before attempting connection
    if config.secret_id.trim().is_empty() {
        return Err("SecretId must not be empty".to_string());
    }
    if config.secret_key.trim().is_empty() {
        return Err("SecretKey must not be empty".to_string());
    }
    if config.bucket.trim().is_empty() {
        return Err("Bucket must not be empty".to_string());
    }
    if config.region.trim().is_empty() {
        return Err("Region must not be empty".to_string());
    }

    // Build a COS client from the provided configuration
    let client = CosClient::new(&config)?;

    // Attempt test connection with a 10-second timeout
    let result = timeout(Duration::from_secs(10), client.test_connection()).await;

    match result {
        Ok(Ok(success)) => {
            info!(success, "validate_cos_config completed");
            Ok(success)
        }
        Ok(Err(e)) => {
            warn!(error = %e, "validate_cos_config failed");
            Err(format!("COS connection validation failed: {e}"))
        }
        Err(_) => {
            warn!("validate_cos_config timed out");
            Err("COS connection validation timed out: no response within 10 seconds".to_string())
        }
    }
}

/// Retrieve the persisted COS configuration from the local database.
///
/// Returns `Ok(None)` if no configuration has been saved yet (first
/// launch). The frontend uses this to decide whether to show the
/// configuration form or proceed to the main editor.
///
/// Note: Credentials are stored only in the local app data directory.
/// This command returns the config so the backend can use it for
/// initialization, but the frontend should only use presence (non-None)
/// to determine routing.
///
/// Validates: Requirements 2.6, 3.4
#[tauri::command]
pub async fn get_cos_config(
    state: tauri::State<'_, AppState>,
) -> Result<Option<CosConfig>, String> {
    info!("get_cos_config requested");
    let engine = state.sync_engine.lock().await;
    engine
        .db
        .get_cos_config()
        .map_err(|e| format!("Failed to retrieve COS config: {e}"))
}

// ── File Operation Commands (Task 10.2) ──────────────────────────────

/// Save canvas data to local storage and enqueue for cloud upload.
///
/// Delegates to `SyncEngine::save_canvas` which writes the data to the
/// local file store, computes the content hash, updates SQLite metadata,
/// and enqueues an upload operation for background processing.
///
/// Validates: Requirements 9.1, 3.1
#[tauri::command]
pub async fn save_canvas(
    file_id: String,
    data: String,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<SyncStatus, String> {
    info!(file_id = %file_id, "save_canvas requested");
    let engine = state.sync_engine.lock().await;
    let status = engine.save_canvas(&file_id, &data).await?;
    let _ = app_handle.emit(
        SYNC_STATUS_EVENT,
        serde_json::json!({ "fileId": file_id, "status": status }),
    );
    let _ = app_handle.emit(FILE_LIST_CHANGED_EVENT, ());
    info!(file_id = %file_id, status = ?status, "save_canvas completed");
    Ok(status)
}

/// Load canvas data for a given file ID.
///
/// Delegates to `SyncEngine::load_canvas` which uses hash-based cache
/// decisions: serves from local cache when hashes match, downloads from
/// COS when they differ or the file is absent locally.
///
/// Validates: Requirements 4.3, 4.4
#[tauri::command]
pub async fn load_canvas(
    file_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    info!(file_id = %file_id, "load_canvas requested");
    let engine = state.sync_engine.lock().await;
    engine.load_canvas(&file_id).await
}

/// Force-download canvas data for a given file ID.
///
/// Delegates to `SyncEngine::download_canvas`, which always fetches the
/// remote object from COS and overwrites the local cache.
#[tauri::command]
pub async fn download_canvas(
    file_id: String,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    info!(file_id = %file_id, "download_canvas requested");
    let engine = state.sync_engine.lock().await;
    let content = engine.download_canvas(&file_id).await?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let _ = app_handle.emit(
        SYNC_STATUS_EVENT,
        serde_json::json!({
            "fileId": file_id,
            "status": SyncStatus::Synced,
            "lastSyncTime": now_ms,
        }),
    );
    let _ = app_handle.emit(FILE_LIST_CHANGED_EVENT, ());
    info!(file_id = %file_id, "download_canvas completed");
    Ok(content)
}

/// Create a new file with a unique UUID v4 ID and empty canvas data.
///
/// Steps:
/// 1. Generate a UUID v4 file ID.
/// 2. Create an empty `.excalidraw` canvas (no elements).
/// 3. Write to local file store and persist metadata in SQLite.
/// 4. Enqueue upload for background sync to COS.
/// 5. Return the new `FileEntry` for the frontend sidebar.
///
/// Validates: Requirements 9.2, 9.3, 9.4
#[tauri::command]
pub async fn create_new_file(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<FileEntry, String> {
    info!("create_new_file requested");
    let file_id = uuid::Uuid::new_v4().to_string();

    // Empty canvas data matching the Excalidraw file format.
    let empty_canvas = r#"{"type":"excalidraw","version":2,"source":"cloud-sync-desktop","elements":[],"appState":{},"files":{}}"#;

    let content_hash = compute_content_hash(empty_canvas);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let engine = state.sync_engine.lock().await;
    let existing_files = engine
        .db
        .get_all_files()
        .map_err(|e| format!("Failed to get existing files: {}", e))?;
    let title = next_untitled_title(&existing_files);
    let is_cloud_sync_enabled = engine.is_cloud_sync_enabled();
    let cos_object_key = if is_cloud_sync_enabled {
        Some(cos_object_key_for_title(&title))
    } else {
        None
    };

    let meta = FileMeta {
        id: file_id.clone(),
        title,
        last_modified: now_ms,
        content_hash: content_hash.clone(),
        cos_object_key,
        sync_status: if is_cloud_sync_enabled {
            SyncStatus::PendingSync
        } else {
            SyncStatus::Synced
        },
        base_content_hash: if is_cloud_sync_enabled {
            None
        } else {
            Some(content_hash)
        },
        is_conflict_copy: false,
        parent_file_id: None,
        deleted: false,
        created_at: now_ms,
    };

    // Write empty canvas to local file store.
    engine
        .file_store
        .write_canvas(&file_id, empty_canvas)
        .map_err(|e| format!("Failed to write new canvas file: {}", e))?;

    // Persist metadata in SQLite.
    engine
        .db
        .upsert_file_meta(&meta)
        .map_err(|e| format!("Failed to save new file metadata: {}", e))?;

    if is_cloud_sync_enabled {
        let upload_entry = QueuedUpload {
            id: 0, // auto-incremented by SQLite
            file_id: file_id.clone(),
            operation: UploadOperation::Upload,
            payload: None,
            retry_count: 0,
            max_retries: 5,
            created_at: now_ms,
        };

        engine
            .db
            .enqueue_upload(&upload_entry)
            .map_err(|e| format!("Failed to enqueue upload for new file: {}", e))?;
    }

    let entry = file_meta_to_entry(&meta);
    let _ = app_handle.emit(FILE_LIST_CHANGED_EVENT, ());
    info!(file_id = %file_id, "create_new_file completed");

    Ok(entry)
}

/// Delete a file by marking it as deleted and enqueuing a delete operation.
///
/// Performs a soft-delete: sets `deleted = true` in the local metadata
/// and enqueues a delete operation for the background sync to propagate
/// the deletion to COS and the manifest.
///
/// Validates: Requirements 5.7
#[tauri::command]
pub async fn delete_file(
    file_id: String,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!(file_id = %file_id, "delete_file requested");
    let engine = state.sync_engine.lock().await;
    let is_cloud_sync_enabled = engine.is_cloud_sync_enabled();

    if !is_cloud_sync_enabled {
        engine
            .file_store
            .delete_canvas(&file_id)
            .map_err(|e| format!("Failed to delete local canvas file: {}", e))?;
        engine
            .db
            .delete_file_meta(&file_id)
            .map_err(|e| format!("Failed to delete local file metadata: {}", e))?;

        let _ = app_handle.emit(FILE_LIST_CHANGED_EVENT, ());
        info!(file_id = %file_id, "delete_file completed locally");
        return Ok(());
    }

    // Get existing metadata.
    let mut meta = engine
        .db
        .get_file_meta(&file_id)
        .map_err(|e| format!("Failed to get file metadata: {}", e))?
        .ok_or_else(|| format!("File not found: {}", file_id))?;

    // Mark as deleted.
    meta.deleted = true;
    meta.sync_status = SyncStatus::PendingSync;
    meta.last_modified = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    engine
        .db
        .upsert_file_meta(&meta)
        .map_err(|e| format!("Failed to update file metadata: {}", e))?;

    // Delete local file (idempotent).
    engine
        .file_store
        .delete_canvas(&file_id)
        .map_err(|e| format!("Failed to delete local canvas file: {}", e))?;

    // Enqueue delete operation for COS sync.
    let upload_entry = QueuedUpload {
        id: 0,
        file_id: file_id.clone(),
        operation: UploadOperation::Delete,
        payload: None,
        retry_count: 0,
        max_retries: 5,
        created_at: meta.last_modified,
    };

    engine
        .db
        .enqueue_upload(&upload_entry)
        .map_err(|e| format!("Failed to enqueue delete operation: {}", e))?;

    let _ = app_handle.emit(FILE_LIST_CHANGED_EVENT, ());
    info!(file_id = %file_id, "delete_file completed");

    Ok(())
}

/// Rename a file, validating the new title.
///
/// Title validation rules:
/// - Must not be empty or whitespace-only → returns Err
/// - Must not exceed 100 characters → returns Err
///
/// On success, updates the title in local metadata and enqueues a
/// rename operation for COS sync.
///
/// Validates: Requirements 5.5, 5.6
#[tauri::command]
pub async fn rename_file(
    file_id: String,
    new_title: String,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!(file_id = %file_id, "rename_file requested");
    validate_file_title(&new_title)?;

    let engine = state.sync_engine.lock().await;
    let is_cloud_sync_enabled = engine.is_cloud_sync_enabled();
    let existing_files = engine
        .db
        .get_all_files()
        .map_err(|e| format!("Failed to get existing files: {}", e))?;
    validate_unique_file_title(&existing_files, &file_id, &new_title)?;

    // Get existing metadata.
    let mut meta = engine
        .db
        .get_file_meta(&file_id)
        .map_err(|e| format!("Failed to get file metadata: {}", e))?
        .ok_or_else(|| format!("File not found: {}", file_id))?;

    let old_cos_object_key = meta.cos_object_key.clone();

    // Update title and last_modified.
    meta.title = new_title.clone();
    meta.cos_object_key = if is_cloud_sync_enabled {
        Some(cos_object_key_for_title(&new_title))
    } else {
        meta.cos_object_key
    };
    meta.sync_status = if is_cloud_sync_enabled {
        SyncStatus::PendingSync
    } else {
        SyncStatus::Synced
    };
    meta.last_modified = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    engine
        .db
        .upsert_file_meta(&meta)
        .map_err(|e| format!("Failed to update file metadata: {}", e))?;

    if is_cloud_sync_enabled {
        let payload = serde_json::json!({
            "newTitle": new_title,
            "oldObjectKey": old_cos_object_key,
        })
        .to_string();
        let upload_entry = QueuedUpload {
            id: 0,
            file_id: file_id.clone(),
            operation: UploadOperation::Rename,
            payload: Some(payload),
            retry_count: 0,
            max_retries: 5,
            created_at: meta.last_modified,
        };

        engine
            .db
            .enqueue_upload(&upload_entry)
            .map_err(|e| format!("Failed to enqueue rename operation: {}", e))?;
    }

    let _ = app_handle.emit(FILE_LIST_CHANGED_EVENT, ());
    info!(file_id = %file_id, "rename_file completed");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_file_title;
    use std::collections::HashSet;

    #[test]
    fn validate_file_title_rejects_empty_and_whitespace() {
        assert!(validate_file_title("").is_err());
        assert!(validate_file_title("   \t\n").is_err());
    }

    #[test]
    fn validate_file_title_rejects_titles_over_100_chars() {
        let title = "a".repeat(101);
        assert!(validate_file_title(&title).is_err());
    }

    #[test]
    fn validate_file_title_accepts_non_empty_title_up_to_100_chars() {
        let title = "a".repeat(100);
        assert!(validate_file_title(&title).is_ok());
        assert!(validate_file_title("Design sketch").is_ok());
    }

    #[test]
    fn generated_file_ids_are_unique() {
        let mut seen = HashSet::new();

        for _ in 0..1_000 {
            let id = uuid::Uuid::new_v4().to_string();
            assert!(seen.insert(id));
        }
    }
}

/// Export a file to a user-chosen location using the native file dialog.
///
/// Opens a native "Save As" dialog with a `.excalidraw` filter. If the
/// user selects a path, the canvas data is written there. If the user
/// cancels, returns `Ok(())` without modifying any data.
///
/// Validates: Requirements 9.5, 9.6, 9.7
#[tauri::command]
pub async fn export_file(
    file_id: String,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // Load the canvas data from local store.
    let engine = state.sync_engine.lock().await;

    let meta = engine
        .db
        .get_file_meta(&file_id)
        .map_err(|e| format!("Failed to get file metadata: {}", e))?
        .ok_or_else(|| format!("File not found: {}", file_id))?;

    let canvas_data = engine
        .file_store
        .read_canvas(&file_id)
        .map_err(|e| format!("Failed to read canvas data: {}", e))?;

    // Drop the engine lock before showing the dialog to avoid holding
    // the mutex during user interaction.
    drop(engine);

    // Determine default filename from the file title.
    let default_filename = format!("{}.excalidraw", meta.title);

    // Use tauri_plugin_dialog to show a native save file dialog.
    let file_path = app_handle
        .dialog()
        .file()
        .set_file_name(&default_filename)
        .add_filter("Excalidraw Files", &["excalidraw"])
        .blocking_save_file();

    match file_path {
        Some(file_path) => {
            // On desktop platforms, FilePath is always a Path variant.
            // Use as_path() to get a reference to the underlying Path.
            let path = file_path
                .as_path()
                .ok_or_else(|| "Selected path is not a valid filesystem path".to_string())?;
            std::fs::write(path, &canvas_data)
                .map_err(|e| format!("Failed to export file: {}", e))?;
            Ok(())
        }
        None => {
            // User cancelled the dialog — return without error.
            Ok(())
        }
    }
}

// ── File List and Sync Commands (Task 10.3) ──────────────────────────

/// A file entry formatted for the frontend file list sidebar.
///
/// Maps from the internal `FileMeta` to the shape expected by the
/// TypeScript `FileEntry` interface. Fields are serialized as camelCase
/// to match the frontend conventions.
///
/// Only includes the `FileSyncStatus` subset of sync states that are
/// meaningful in the sidebar context: "synced", "pending-sync", "conflict".
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub id: String,
    pub title: String,
    /// Unix timestamp in milliseconds.
    pub last_modified: i64,
    pub sync_status: String,
    pub is_conflict_copy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_file_id: Option<String>,
}

/// Convert a `FileMeta` to a frontend-friendly `FileEntry`.
///
/// Maps the `SyncStatus` enum to the `FileSyncStatus` string subset
/// used by the sidebar: "synced", "pending-sync", or "conflict".
/// Statuses like `Saving` and `Error` are mapped to "pending-sync"
/// since they represent in-progress or failed sync attempts.
fn file_meta_to_entry(meta: &FileMeta) -> FileEntry {
    let sync_status = match &meta.sync_status {
        SyncStatus::Synced => "synced",
        SyncStatus::PendingSync => "pending-sync",
        SyncStatus::Conflict => "conflict",
        SyncStatus::Saving => "pending-sync",
        SyncStatus::Error => "pending-sync",
    };

    FileEntry {
        id: meta.id.clone(),
        title: meta.title.clone(),
        last_modified: meta.last_modified,
        sync_status: sync_status.to_string(),
        is_conflict_copy: meta.is_conflict_copy,
        parent_file_id: meta.parent_file_id.clone(),
    }
}

/// Return all files sorted by last_modified DESC for the file list sidebar.
///
/// Calls `db.get_all_files()` which already returns entries sorted by
/// `last_modified DESC` (via the SQL `ORDER BY` clause), then maps each
/// `FileMeta` to a frontend-friendly `FileEntry`.
///
/// Validates: Requirements 5.1, 4.1
#[tauri::command]
pub async fn get_file_list(state: tauri::State<'_, AppState>) -> Result<Vec<FileEntry>, String> {
    let engine = state.sync_engine.lock().await;
    let files = engine
        .db
        .get_all_files()
        .map_err(|e| format!("Failed to get file list: {}", e))?;

    let entries: Vec<FileEntry> = files
        .iter()
        .filter(|file| !file.deleted)
        .map(file_meta_to_entry)
        .collect();
    Ok(entries)
}

/// Trigger a manual manifest sync operation.
///
/// Downloads the remote manifest from COS, merges with local metadata,
/// and re-uploads the merged result. This is the on-demand counterpart
/// to the automatic 30-second polling cycle.
///
/// Validates: Requirement 4.1
#[tauri::command]
pub async fn trigger_sync(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let engine = state.sync_engine.lock().await;
    engine.sync_manifest().await?;
    let _ = app_handle.emit(FILE_LIST_CHANGED_EVENT, ());
    Ok(())
}

/// Get the sync status of a specific file by ID.
///
/// Looks up the file's metadata in the SQLite database and returns
/// its current `SyncStatus` value serialized as a kebab-case string.
///
/// Validates: Requirement 5.1
#[tauri::command]
pub async fn get_sync_status(
    file_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<SyncStatus, String> {
    let engine = state.sync_engine.lock().await;
    let meta = engine
        .db
        .get_file_meta(&file_id)
        .map_err(|e| format!("Failed to get file metadata: {}", e))?
        .ok_or_else(|| format!("File not found: {}", file_id))?;

    Ok(meta.sync_status)
}

#[tauri::command]
pub async fn log_frontend_error(
    message: String,
    stack: Option<String>,
    component_stack: Option<String>,
) -> Result<(), String> {
    if let Some(stack) = stack {
        if let Some(component_stack) = component_stack {
            tracing::error!(
                message = %message,
                stack = %stack,
                component_stack = %component_stack,
                "frontend error"
            );
        } else {
            tracing::error!(message = %message, stack = %stack, "frontend error");
        }
    } else if let Some(component_stack) = component_stack {
        tracing::error!(
            message = %message,
            component_stack = %component_stack,
            "frontend error"
        );
    } else {
        tracing::error!(message = %message, "frontend error");
    }

    Ok(())
}
