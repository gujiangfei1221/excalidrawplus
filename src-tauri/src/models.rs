//! Data structures shared across the Rust backend.
//!
//! These types correspond to the "Rust Data Structures" section of the
//! `cloud-sync-desktop` design document and are exchanged with the frontend
//! through Tauri commands as well as persisted to SQLite and Tencent COS.
//!
//! All types derive `Serialize`, `Deserialize`, and `Clone` so they can be:
//!   - Sent across the Tauri IPC boundary (JSON-serialized).
//!   - Stored in / loaded from SQLite (via `serde_json` for nested fields).
//!   - Uploaded to / downloaded from COS as part of `manifest.json`.
//!
//! Field names use snake_case in Rust but are serialized to camelCase to
//! match the frontend TypeScript interfaces and the manifest JSON schema
//! defined in the design document.

use serde::{Deserialize, Serialize};

/// Tencent COS credentials and bucket location.
///
/// Persisted in the local SQLite database (single-row `cos_config` table)
/// and never transmitted to the frontend WebView.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CosConfig {
    pub secret_id: String,
    pub secret_key: String,
    pub bucket: String,
    pub region: String,
}

/// Local metadata for a single `.excalidraw` file.
///
/// Mirrors the `files` table in SQLite. `base_content_hash` records the
/// content hash at the last successful sync and is used by the conflict
/// detection algorithm (see `detect_conflicts`).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub id: String,
    pub title: String,
    pub last_modified: i64,
    pub content_hash: String,
    pub cos_object_key: Option<String>,
    pub sync_status: SyncStatus,
    pub base_content_hash: Option<String>,
    pub is_conflict_copy: bool,
    pub parent_file_id: Option<String>,
    pub deleted: bool,
    pub created_at: i64,
}

/// A single entry inside the cross-device `manifest.json` stored on COS.
///
/// The manifest acts as the index of all files for a given COS bucket.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub id: String,
    pub title: String,
    pub last_modified: i64,
    pub content_hash: String,
    pub object_key: String,
    pub deleted: bool,
}

/// Top-level structure of `manifest.json` on COS.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub last_modified: i64,
    pub files: Vec<ManifestEntry>,
}

/// Sync state of a file as observed by the frontend.
///
/// Serialized as kebab-case strings (e.g. `"pending-sync"`) to match the
/// values used by the frontend TypeScript types and the SQLite
/// `sync_status` column.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum SyncStatus {
    Synced,
    PendingSync,
    Saving,
    Conflict,
    Error,
}

/// A pending operation in the durable upload queue.
///
/// Mirrors the `upload_queue` SQLite table. `payload` carries any
/// operation-specific JSON-encoded data (e.g., the new title for a
/// rename).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueuedUpload {
    pub id: i64,
    pub file_id: String,
    pub operation: UploadOperation,
    pub payload: Option<String>,
    pub retry_count: u32,
    pub max_retries: u32,
    pub created_at: i64,
}

/// The kind of operation enqueued in the upload queue.
///
/// Serialized as lowercase strings (`"upload"`, `"delete"`, `"rename"`)
/// to match the values stored in the `upload_queue.operation` column.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum UploadOperation {
    Upload,
    Delete,
    Rename,
}

/// Description of a detected sync conflict for a single file.
///
/// A conflict exists when both the local and remote content hashes
/// differ from the base hash recorded at the last successful sync.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub file_id: String,
    pub local_hash: String,
    pub remote_hash: String,
    pub base_hash: String,
    pub remote_last_modified: i64,
}
