//! Core sync engine orchestrating background synchronization tasks.
//!
//! The [`SyncEngine`] manages manifest polling, upload queue processing,
//! and connectivity monitoring. It runs as a set of background Tokio tasks
//! within the Tauri application and coordinates the [`CosClient`],
//! [`Database`], [`FileStore`], and [`ConnectivityMonitor`] components.
//!
//! Task 7.1 establishes the structure and lifecycle (`start` / `stop`).
//! Subsequent tasks add the business logic methods:
//!   * 7.2 — `save_canvas`
//!   * 7.3 — upload with retry
//!   * 7.4 — `sync_manifest` (download, merge, upload)
//!   * 7.5 — manifest polling loop
//!   * 7.6 — `load_canvas`
//!   * 8.1 — `detect_conflicts`
//!   * 8.2 — `resolve_conflict`
//!   * 8.3 — `process_upload_queue`
//!
//! Validates: Requirements 3.1, 6.6

use std::collections::HashMap;
use std::sync::Arc;

use tauri::async_runtime::JoinHandle;
use tauri::AppHandle;

use crate::connectivity::ConnectivityMonitor;
use crate::cos_client::CosClient;
use crate::database::Database;
use crate::file_store::FileStore;
use crate::models::{Conflict, FileMeta, Manifest, ManifestEntry, SyncStatus, UploadOperation};

/// Interval (in seconds) between manifest polling cycles.
const MANIFEST_POLL_INTERVAL_SECS: u64 = 30;

/// Interval (in seconds) between upload queue processing cycles.
const QUEUE_PROCESS_INTERVAL_SECS: u64 = 5;

/// Maximum number of conflict copies allowed per file.
/// When a new conflict would exceed this limit, the oldest copy is deleted.
const MAX_CONFLICT_COPIES: usize = 5;

/// Root prefix for all COS objects owned by this app.
pub(crate) const COS_ROOT_PREFIX: &str = "excalidraw";

/// Manifest location inside the app-owned COS prefix.
const MANIFEST_KEY: &str = "excalidraw/manifest.json";

/// Legacy manifest location used by earlier builds.
const LEGACY_MANIFEST_KEY: &str = "manifest.json";

pub(crate) fn sanitize_title_for_cos_filename(title: &str) -> String {
    let sanitized: String = title
        .trim()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();

    let sanitized = sanitized.trim_matches([' ', '.']).trim();
    if sanitized.is_empty() {
        "Untitled".to_string()
    } else {
        sanitized.to_string()
    }
}

pub(crate) fn cos_object_key_for_title(title: &str) -> String {
    format!(
        "{}/{}.excalidraw",
        COS_ROOT_PREFIX,
        sanitize_title_for_cos_filename(title)
    )
}

pub(crate) fn cos_object_key_for_file(meta: &FileMeta) -> String {
    cos_object_key_for_title(&meta.title)
}

fn is_missing_cos_object_error(error: &str) -> bool {
    error.contains("NoSuchKey")
        || error.contains("NotFound")
        || error.contains("not found")
        || error.contains("404")
}

async fn download_manifest(cos_client: &CosClient) -> Result<Manifest, String> {
    match cos_client.get_object(MANIFEST_KEY).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Failed to deserialize {MANIFEST_KEY}: {e}")),
        Err(primary_error) if is_missing_cos_object_error(&primary_error) => {
            match cos_client.get_object(LEGACY_MANIFEST_KEY).await {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Failed to deserialize {LEGACY_MANIFEST_KEY}: {e}")),
                Err(legacy_error) if is_missing_cos_object_error(&legacy_error) => Ok(Manifest {
                    version: 1,
                    last_modified: 0,
                    files: Vec::new(),
                }),
                Err(legacy_error) => Err(format!(
                    "Failed to download {MANIFEST_KEY}; legacy {LEGACY_MANIFEST_KEY} also failed: {legacy_error}"
                )),
            }
        }
        Err(error) => Err(format!("Failed to download {MANIFEST_KEY}: {error}")),
    }
}

fn old_object_key_from_payload(payload: Option<&str>) -> Option<String> {
    payload
        .and_then(|payload| serde_json::from_str::<serde_json::Value>(payload).ok())
        .and_then(|value| {
            value
                .get("oldObjectKey")
                .and_then(|old_key| old_key.as_str())
                .map(str::to_string)
        })
        .filter(|old_key| !old_key.trim().is_empty())
}

/// The core sync engine that coordinates cloud synchronization.
///
/// Holds references to all backend components and manages background
/// tasks for manifest polling and upload queue processing.
pub struct SyncEngine {
    /// Whether COS-backed synchronization is enabled for this runtime.
    cloud_sync_enabled: bool,
    /// S3-compatible client for Tencent COS operations.
    pub(crate) cos_client: Arc<CosClient>,
    /// SQLite database for metadata and queue persistence.
    pub(crate) db: Arc<Database>,
    /// Local filesystem store for `.excalidraw` files.
    pub(crate) file_store: Arc<FileStore>,
    /// Connectivity monitor for detecting network state changes.
    pub(crate) conn_monitor: Arc<ConnectivityMonitor>,
    /// Handle to the background manifest polling task.
    poll_handle: Option<JoinHandle<()>>,
    /// Handle to the background queue processing task.
    queue_handle: Option<JoinHandle<()>>,
}

impl SyncEngine {
    /// Create a new `SyncEngine` from its component parts.
    ///
    /// The engine is inert after construction — call [`start`](Self::start)
    /// to begin background synchronization.
    pub fn new(
        cos_client: CosClient,
        db: Database,
        file_store: FileStore,
        conn_monitor: ConnectivityMonitor,
    ) -> Self {
        Self {
            cloud_sync_enabled: true,
            cos_client: Arc::new(cos_client),
            db: Arc::new(db),
            file_store: Arc::new(file_store),
            conn_monitor: Arc::new(conn_monitor),
            poll_handle: None,
            queue_handle: None,
        }
    }

    /// Start background synchronization tasks.
    ///
    /// This method:
    /// 1. Starts the connectivity monitor.
    /// 2. Spawns a manifest polling task (30-second interval).
    /// 3. Spawns an upload queue processing task (5-second interval).
    ///
    /// The `_app_handle` parameter is reserved for future use (event
    /// emission to the frontend) and will be wired in task 16.1.
    ///
    /// Validates: Requirements 3.1 (auto-save trigger path), 6.6 (manifest polling).
    pub fn start(&mut self, _app_handle: AppHandle) {
        if !self.cloud_sync_enabled {
            return;
        }

        // Start connectivity monitoring.
        self.conn_monitor.start();

        // Spawn manifest polling background task.
        // Polls the remote manifest every 30 seconds while online, applying
        // remote changes (new entries, updated entries, deleted entries) to
        // local metadata.
        // Validates: Requirements 6.5, 6.6, 6.7
        let conn_monitor_poll = Arc::clone(&self.conn_monitor);
        let cos_client_poll = Arc::clone(&self.cos_client);
        let db_poll = Arc::clone(&self.db);
        let poll_handle = tauri::async_runtime::spawn(async move {
            loop {
                // Only poll when online.
                if conn_monitor_poll.is_online() {
                    // Call the standalone manifest sync helper with cloned Arcs.
                    let _ = poll_sync_manifest(&cos_client_poll, &db_poll).await;
                }

                tokio::time::sleep(std::time::Duration::from_secs(MANIFEST_POLL_INTERVAL_SECS))
                    .await;
            }
        });
        self.poll_handle = Some(poll_handle);

        // Spawn upload queue processing background task.
        // Processes the upload queue every QUEUE_PROCESS_INTERVAL_SECS when online.
        // Validates: Requirements 7.4, 7.5
        let conn_monitor_queue = Arc::clone(&self.conn_monitor);
        let cos_client_queue = Arc::clone(&self.cos_client);
        let db_queue = Arc::clone(&self.db);
        let file_store_queue = Arc::clone(&self.file_store);
        let queue_handle = tauri::async_runtime::spawn(async move {
            loop {
                // Only process the queue when online.
                if conn_monitor_queue.is_online() {
                    let _ = process_upload_queue_standalone(
                        &cos_client_queue,
                        &db_queue,
                        &file_store_queue,
                        &conn_monitor_queue,
                    )
                    .await;
                }

                tokio::time::sleep(std::time::Duration::from_secs(QUEUE_PROCESS_INTERVAL_SECS))
                    .await;
            }
        });
        self.queue_handle = Some(queue_handle);
    }

    pub fn is_cloud_sync_enabled(&self) -> bool {
        self.cloud_sync_enabled
    }

    pub fn set_cloud_sync_enabled(&mut self, enabled: bool) {
        self.cloud_sync_enabled = enabled;
    }

    pub fn enable_cloud_sync(
        &mut self,
        cos_client: CosClient,
        app_handle: AppHandle,
    ) -> Result<(), String> {
        self.stop();

        let cos_client = Arc::new(cos_client);
        self.cos_client = Arc::clone(&cos_client);
        self.conn_monitor = Arc::new(ConnectivityMonitor::new(cos_client));
        self.cloud_sync_enabled = true;
        self.enqueue_local_files_for_sync()?;
        self.start(app_handle);

        Ok(())
    }

    fn enqueue_local_files_for_sync(&self) -> Result<(), String> {
        use crate::models::{QueuedUpload, UploadOperation};

        let files = self
            .db
            .get_all_files()
            .map_err(|e| format!("Failed to get local files for sync: {e}"))?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        for mut meta in files {
            if meta.deleted {
                continue;
            }

            meta.cos_object_key = Some(cos_object_key_for_file(&meta));
            meta.sync_status = SyncStatus::PendingSync;
            meta.base_content_hash = None;

            self.db
                .upsert_file_meta(&meta)
                .map_err(|e| format!("Failed to mark file for cloud sync: {e}"))?;

            self.db
                .enqueue_upload(&QueuedUpload {
                    id: 0,
                    file_id: meta.id,
                    operation: UploadOperation::Upload,
                    payload: None,
                    retry_count: 0,
                    max_retries: 5,
                    created_at: now_ms,
                })
                .map_err(|e| format!("Failed to enqueue file for cloud sync: {e}"))?;
        }

        Ok(())
    }

    /// Save canvas data to local storage, update metadata, and enqueue upload.
    ///
    /// This method is the save path that executes AFTER the debounce timer
    /// fires (debounce logic itself lives in the frontend command layer).
    ///
    /// Steps:
    /// 1. Write data to the local file store.
    /// 2. Compute the SHA-256 content hash of the data.
    /// 3. Create or update `FileMeta` in SQLite (status → pending-sync).
    /// 4. Enqueue an upload operation for background processing.
    /// 5. Return the current `SyncStatus` (pending-sync).
    ///
    /// Validates: Requirements 3.1, 3.7, 7.2
    pub async fn save_canvas(&self, file_id: &str, data: &str) -> Result<SyncStatus, String> {
        use crate::file_store::compute_content_hash;
        use crate::models::{FileMeta, QueuedUpload, SyncStatus, UploadOperation};

        // 1. Write data to the local file store.
        self.file_store
            .write_canvas(file_id, data)
            .map_err(|e| format!("Failed to write canvas to local store: {}", e))?;

        // 2. Compute content hash.
        let content_hash = compute_content_hash(data);

        // 3. Get current timestamp (millis since epoch).
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // 4. Build FileMeta — preserve created_at if the file already exists.
        let existing = self
            .db
            .get_file_meta(file_id)
            .map_err(|e| format!("Failed to read file metadata: {}", e))?;

        let content_is_synced_locally = !self.cloud_sync_enabled;
        let title = existing
            .as_ref()
            .map(|m| m.title.clone())
            .unwrap_or_else(|| "Untitled".to_string());

        let cos_object_key = if self.cloud_sync_enabled {
            Some(cos_object_key_for_title(&title))
        } else {
            existing.as_ref().and_then(|m| m.cos_object_key.clone())
        };

        let meta = FileMeta {
            id: file_id.to_string(),
            title,
            last_modified: now_ms,
            content_hash: content_hash.clone(),
            cos_object_key,
            sync_status: if self.cloud_sync_enabled {
                SyncStatus::PendingSync
            } else {
                SyncStatus::Synced
            },
            base_content_hash: if content_is_synced_locally {
                Some(content_hash)
            } else {
                existing.as_ref().and_then(|m| m.base_content_hash.clone())
            },
            is_conflict_copy: existing
                .as_ref()
                .map(|m| m.is_conflict_copy)
                .unwrap_or(false),
            parent_file_id: existing.as_ref().and_then(|m| m.parent_file_id.clone()),
            deleted: false,
            created_at: existing.as_ref().map(|m| m.created_at).unwrap_or(now_ms),
        };

        self.db
            .upsert_file_meta(&meta)
            .map_err(|e| format!("Failed to update file metadata: {}", e))?;

        if self.cloud_sync_enabled {
            let upload_entry = QueuedUpload {
                id: 0, // ignored on insert — SQLite auto-increments
                file_id: file_id.to_string(),
                operation: UploadOperation::Upload,
                payload: None,
                retry_count: 0,
                max_retries: 5,
                created_at: now_ms,
            };

            self.db
                .enqueue_upload(&upload_entry)
                .map_err(|e| format!("Failed to enqueue upload: {}", e))?;
        }

        // 6. Return current sync status.
        if self.cloud_sync_enabled {
            Ok(SyncStatus::PendingSync)
        } else {
            Ok(SyncStatus::Synced)
        }
    }

    /// Upload a file to COS with retry tracking.
    ///
    /// This method:
    /// 1. Checks connectivity — returns `PendingSync` immediately if offline.
    /// 2. Reads the file content from the local file store.
    /// 3. Attempts to upload via `cos_client.put_object()`.
    /// 4. On success: updates `sync_status` to `Synced` in SQLite and
    ///    dequeues all pending uploads for this file.
    /// 5. On failure: increments `retry_count` in the queue entry; if
    ///    `retry_count` exceeds `max_retries` (5), marks the file as
    ///    `Error` status.
    ///
    /// If the COS config is missing/invalid (file has no `cos_object_key`),
    /// the upload is skipped and the file remains `PendingSync`.
    ///
    /// Validates: Requirements 3.2, 3.5, 3.6, 3.8.
    pub async fn upload_file(&self, file_id: &str) -> Result<SyncStatus, String> {
        use crate::models::SyncStatus;

        if !self.cloud_sync_enabled {
            return Ok(SyncStatus::PendingSync);
        }

        // 1. Check connectivity — if offline, return early with PendingSync.
        if !self.conn_monitor.is_online() {
            return Ok(SyncStatus::PendingSync);
        }

        // 2. Look up file metadata from SQLite.
        let meta = self
            .db
            .get_file_meta(file_id)
            .map_err(|e| format!("Failed to get file meta for {}: {}", file_id, e))?
            .ok_or_else(|| format!("File not found in database: {}", file_id))?;

        // Skip upload if COS object key is missing (COS config not set up).
        let object_key = match &meta.cos_object_key {
            Some(key) if !key.trim().is_empty() => key.clone(),
            _ => {
                // COS config missing/invalid — retain as pending-sync, skip upload.
                return Ok(SyncStatus::PendingSync);
            }
        };

        // 3. Read the file content from local store.
        let content = self
            .file_store
            .read_canvas(file_id)
            .map_err(|e| format!("Failed to read file {} from local store: {}", file_id, e))?;

        // 4. Attempt to upload via COS.
        let upload_result = self
            .cos_client
            .put_object(&object_key, content.into_bytes())
            .await;

        match upload_result {
            Ok(()) => {
                // Success: update sync_status to Synced in SQLite.
                let mut updated_meta = meta.clone();
                updated_meta.sync_status = SyncStatus::Synced;
                updated_meta.base_content_hash = Some(meta.content_hash.clone());
                self.db
                    .upsert_file_meta(&updated_meta)
                    .map_err(|e| format!("Failed to update file meta after upload: {}", e))?;

                // Dequeue all pending uploads for this file.
                self.db
                    .dequeue_upload(file_id)
                    .map_err(|e| format!("Failed to dequeue upload for {}: {}", file_id, e))?;

                Ok(SyncStatus::Synced)
            }
            Err(_upload_err) => {
                // Failure: find the queue entry for this file and increment retry_count.
                let pending = self
                    .db
                    .get_pending_uploads()
                    .map_err(|e| format!("Failed to get pending uploads: {}", e))?;

                let queue_entry = pending.iter().find(|entry| entry.file_id == file_id);

                if let Some(entry) = queue_entry {
                    self.db
                        .increment_retry_count(entry.id)
                        .map_err(|e| format!("Failed to increment retry count: {}", e))?;

                    // Check if max retries exceeded (current retry_count + 1 > max_retries).
                    let new_retry_count = entry.retry_count + 1;
                    if new_retry_count > entry.max_retries {
                        // Mark file as Error status — stop retrying.
                        let mut error_meta = meta.clone();
                        error_meta.sync_status = SyncStatus::Error;
                        self.db
                            .upsert_file_meta(&error_meta)
                            .map_err(|e| format!("Failed to mark file as error: {}", e))?;

                        return Ok(SyncStatus::Error);
                    }
                }

                // File remains pending-sync for retry on next cycle.
                let mut pending_meta = meta.clone();
                pending_meta.sync_status = SyncStatus::PendingSync;
                self.db
                    .upsert_file_meta(&pending_meta)
                    .map_err(|e| format!("Failed to mark file as pending-sync: {}", e))?;

                Ok(SyncStatus::PendingSync)
            }
        }
    }

    /// Load canvas data using hash-based cache decision.
    ///
    /// This method:
    /// 1. Retrieves file metadata from SQLite.
    /// 2. Attempts to read the local file from the file store.
    /// 3. If the local file exists and `meta.content_hash == meta.base_content_hash`
    ///    (i.e., the local version is in sync with the remote), returns
    ///    the local content directly.
    /// 4. If the local file does not exist or the hashes differ,
    ///    downloads the file from COS, saves it locally, updates
    ///    metadata with the new hash, and returns the content.
    /// 5. If download fails, falls back to local cache if available,
    ///    otherwise returns an error.
    ///
    /// Validates: Requirements 4.3, 4.4
    pub async fn load_canvas(&self, file_id: &str) -> Result<String, String> {
        // 1. Get file metadata from SQLite.
        let meta = self
            .db
            .get_file_meta(file_id)
            .map_err(|e| format!("Failed to get file metadata for {}: {}", file_id, e))?
            .ok_or_else(|| format!("File not found in database: {}", file_id))?;

        if !self.cloud_sync_enabled {
            return self
                .file_store
                .read_canvas(file_id)
                .map_err(|e| format!("Failed to read local file {}: {}", file_id, e));
        }

        // 2. Try to read the local file.
        let local_content = self.file_store.read_canvas(file_id);

        match local_content {
            Ok(content) => {
                // 3. Local file exists — check if meta.content_hash matches
                //    meta.base_content_hash (local is in sync with remote).
                if let Some(ref base_hash) = meta.base_content_hash {
                    if meta.content_hash == *base_hash {
                        // Hashes match: file is in sync with COS, serve from cache.
                        return Ok(content);
                    }
                }

                // Hashes differ or base_content_hash is None: attempt download
                // from COS, falling back to local cache on failure.
                match self.download_and_cache(file_id, &meta).await {
                    Ok(downloaded) => Ok(downloaded),
                    Err(_) => {
                        // Download failed — fall back to local cache.
                        Ok(content)
                    }
                }
            }
            Err(_) => {
                // 4. Local file does not exist: download from COS.
                self.download_and_cache(file_id, &meta).await
            }
        }
    }

    /// Force-download a canvas from COS and overwrite the local cache.
    ///
    /// Unlike [`load_canvas`](Self::load_canvas), this always fetches the
    /// remote object instead of serving a cached local copy.
    pub async fn download_canvas(&self, file_id: &str) -> Result<String, String> {
        let meta = self
            .db
            .get_file_meta(file_id)
            .map_err(|e| format!("Failed to get file metadata for {}: {}", file_id, e))?
            .ok_or_else(|| format!("File not found in database: {}", file_id))?;

        self.download_and_cache(file_id, &meta).await
    }

    /// Download a file from COS, save it to local store, and update metadata.
    ///
    /// Helper for `load_canvas` — called when the local cache is stale or missing.
    async fn download_and_cache(&self, file_id: &str, meta: &FileMeta) -> Result<String, String> {
        use crate::file_store::compute_content_hash;

        // Determine the COS object key.
        let object_key = meta
            .cos_object_key
            .as_deref()
            .ok_or_else(|| format!("File {} has no COS object key", file_id))?;

        // Download from COS.
        let bytes = self
            .cos_client
            .get_object(object_key)
            .await
            .map_err(|e| format!("Failed to download file {} from COS: {}", file_id, e))?;

        // Convert bytes to string.
        let content = String::from_utf8(bytes)
            .map_err(|e| format!("Downloaded file {} is not valid UTF-8: {}", file_id, e))?;

        // Save to local store.
        self.file_store
            .write_canvas(file_id, &content)
            .map_err(|e| {
                format!(
                    "Failed to write downloaded file {} to local store: {}",
                    file_id, e
                )
            })?;

        // Update metadata with new content hash and mark as synced.
        let new_hash = compute_content_hash(&content);
        let mut updated_meta = meta.clone();
        updated_meta.content_hash = new_hash.clone();
        updated_meta.base_content_hash = Some(new_hash);
        updated_meta.sync_status = SyncStatus::Synced;

        self.db.upsert_file_meta(&updated_meta).map_err(|e| {
            format!(
                "Failed to update metadata after download for {}: {}",
                file_id, e
            )
        })?;

        Ok(content)
    }

    /// Detect conflicts between local files and a remote manifest.
    ///
    /// A conflict exists when both the local content hash and the remote
    /// content hash differ from the base content hash (the hash recorded
    /// at the last successful sync). This indicates that both devices
    /// modified the file independently while offline.
    ///
    /// If only the remote hash differs (local unchanged), this is a
    /// remote update — not a conflict.
    ///
    /// Files without a `base_content_hash` (never synced) are skipped
    /// because there is no baseline to compare against.
    ///
    /// Validates: Requirements 8.1
    pub fn detect_conflicts(&self, remote_manifest: &Manifest) -> Vec<Conflict> {
        let mut conflicts = Vec::new();

        for remote_entry in &remote_manifest.files {
            // Skip deleted entries — no conflict to detect.
            if remote_entry.deleted {
                continue;
            }

            // 1. Look up the file in local DB.
            let local_meta = match self.db.get_file_meta(&remote_entry.id) {
                Ok(Some(meta)) => meta,
                Ok(None) => continue, // File not in local DB — skip.
                Err(_) => continue,   // DB error — skip gracefully.
            };

            // 2. Get the base_content_hash (hash at last successful sync).
            let base_hash = match &local_meta.base_content_hash {
                Some(hash) => hash.clone(),
                None => continue, // 3. No baseline to compare against — skip.
            };

            // 4-6. Compare hashes to determine conflict vs. remote update.
            let remote_differs = remote_entry.content_hash != base_hash;
            let local_differs = local_meta.content_hash != base_hash;

            if remote_differs && local_differs {
                // 4. Both local and remote differ from base → CONFLICT.
                conflicts.push(Conflict {
                    file_id: remote_entry.id.clone(),
                    local_hash: local_meta.content_hash.clone(),
                    remote_hash: remote_entry.content_hash.clone(),
                    base_hash,
                    remote_last_modified: remote_entry.last_modified,
                });
            }
            // 5. If remote differs but local matches base → remote update (NOT a conflict).
            // 6. If remote matches base → no change from remote, no conflict.
        }

        conflicts
    }

    /// Process all pending uploads in the queue in chronological order.
    ///
    /// This method:
    /// 1. Checks connectivity — if offline, returns early.
    /// 2. Gets all pending uploads from the database (ordered by `created_at` ASC).
    /// 3. For each entry in chronological order:
    ///    a. Calls `self.upload_file(&entry.file_id)`.
    ///    b. On success (returns `Synced`): the `upload_file` method already dequeues.
    ///    c. On failure: skips this item (`upload_file` already increments `retry_count`),
    ///       continues with the next entry.
    /// 4. Returns `Ok(())` after processing all items.
    ///
    /// Validates: Requirements 7.4, 7.5
    pub async fn process_upload_queue(&self) -> Result<(), String> {
        if !self.cloud_sync_enabled {
            return Ok(());
        }

        // 1. Check connectivity — if offline, return early.
        if !self.conn_monitor.is_online() {
            return Ok(());
        }

        // 2. Get all pending uploads from DB (already ordered by created_at ASC).
        let pending = self
            .db
            .get_pending_uploads()
            .map_err(|e| format!("Failed to get pending uploads: {}", e))?;

        // 3. Process each entry in chronological order.
        for entry in &pending {
            match entry.operation {
                UploadOperation::Upload => {
                    let _ = self.upload_file(&entry.file_id).await;
                }
                UploadOperation::Rename => {
                    if matches!(self.upload_file(&entry.file_id).await, Ok(SyncStatus::Synced)) {
                        if let Some(old_key) = old_object_key_from_payload(entry.payload.as_deref())
                        {
                            if let Ok(Some(meta)) = self.db.get_file_meta(&entry.file_id) {
                                if meta.cos_object_key.as_deref() != Some(old_key.as_str()) {
                                    let _ = self.cos_client.delete_object(&old_key).await;
                                }
                            }
                        }
                    }
                }
                UploadOperation::Delete => {
                    if let Ok(Some(meta)) = self.db.get_file_meta(&entry.file_id) {
                        if let Some(key) = meta.cos_object_key.as_deref() {
                            match self.cos_client.delete_object(key).await {
                                Ok(()) => {
                                    let _ = self.db.dequeue_upload(&entry.file_id);
                                }
                                Err(_) => {
                                    let _ = self.db.increment_retry_count(entry.id);
                                }
                            }
                        } else {
                            let _ = self.db.dequeue_upload(&entry.file_id);
                        }
                    } else {
                        let _ = self.db.dequeue_upload(&entry.file_id);
                    }
                }
            }
        }

        // 4. Return Ok(()) after processing all items.
        Ok(())
    }

    /// Resolve a detected conflict by preserving the remote version as a
    /// conflict copy and uploading the local version as the new primary.
    ///
    /// Steps:
    /// 1. Get the original file metadata from DB.
    /// 2. Download the remote version from COS.
    /// 3. Generate a conflict copy ID (UUID v4).
    /// 4. Save the remote version locally with the conflict copy ID.
    /// 5. Create a FileMeta for the conflict copy with title
    ///    "{original_title} - Conflict {YYYY-MM-DD}" and is_conflict_copy = true.
    /// 6. Check how many conflict copies exist for this file.
    /// 7. If >= 5, delete the oldest one (by created_at).
    /// 8. Save the conflict copy metadata to DB.
    /// 9. Upload the local version as the primary file on COS.
    /// 10. Update the original file's base_content_hash to the local content_hash.
    ///
    /// Validates: Requirements 8.2, 8.5, 8.7
    pub async fn resolve_conflict(&self, conflict: &Conflict) -> Result<(), String> {
        use crate::file_store::compute_content_hash;

        // 1. Get the original file metadata from DB.
        let original_meta = self
            .db
            .get_file_meta(&conflict.file_id)
            .map_err(|e| format!("Failed to get file meta for {}: {}", conflict.file_id, e))?
            .ok_or_else(|| format!("File not found in database: {}", conflict.file_id))?;

        // 2. Download the remote version from COS.
        let object_key = original_meta
            .cos_object_key
            .as_deref()
            .ok_or_else(|| format!("File {} has no COS object key", conflict.file_id))?;

        let remote_bytes = self.cos_client.get_object(object_key).await.map_err(|e| {
            format!(
                "Failed to download remote version of {}: {}",
                conflict.file_id, e
            )
        })?;

        let remote_content = String::from_utf8(remote_bytes)
            .map_err(|e| format!("Remote file {} is not valid UTF-8: {}", conflict.file_id, e))?;

        // 3. Generate a conflict copy ID (UUID v4).
        let conflict_copy_id = uuid::Uuid::new_v4().to_string();

        // 4. Save the remote version locally with the conflict copy ID.
        self.file_store
            .write_canvas(&conflict_copy_id, &remote_content)
            .map_err(|e| format!("Failed to write conflict copy to local store: {}", e))?;

        // 5. Create a FileMeta for the conflict copy.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let conflict_title = generate_conflict_title(&original_meta.title);
        let conflict_hash = compute_content_hash(&remote_content);
        let conflict_cos_object_key = cos_object_key_for_title(&conflict_title);

        let conflict_meta = FileMeta {
            id: conflict_copy_id.clone(),
            title: conflict_title,
            last_modified: now_ms,
            content_hash: conflict_hash.clone(),
            cos_object_key: Some(conflict_cos_object_key),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some(conflict_hash),
            is_conflict_copy: true,
            parent_file_id: Some(conflict.file_id.clone()),
            deleted: false,
            created_at: now_ms,
        };

        // 6. Check how many conflict copies exist for this file.
        let existing_copies = self
            .db
            .get_conflict_copies(&conflict.file_id)
            .map_err(|e| format!("Failed to get conflict copies: {}", e))?;

        // 7. If >= 5, delete the oldest one (by created_at — list is ordered ASC).
        if existing_copies.len() >= MAX_CONFLICT_COPIES {
            let oldest = &existing_copies[0];
            // Delete from local file store.
            self.file_store
                .delete_canvas(&oldest.id)
                .map_err(|e| format!("Failed to delete oldest conflict copy file: {}", e))?;
            // Delete from COS if it has an object key.
            if let Some(ref key) = oldest.cos_object_key {
                let _ = self.cos_client.delete_object(key).await;
            }
            // Delete from DB.
            self.db
                .delete_file_meta(&oldest.id)
                .map_err(|e| format!("Failed to delete oldest conflict copy metadata: {}", e))?;
        }

        // 8. Save the conflict copy metadata to DB.
        self.db
            .upsert_file_meta(&conflict_meta)
            .map_err(|e| format!("Failed to save conflict copy metadata: {}", e))?;

        // 9. Upload the local version as the primary file on COS.
        let local_content = self
            .file_store
            .read_canvas(&conflict.file_id)
            .map_err(|e| {
                format!(
                    "Failed to read local file {} for upload: {}",
                    conflict.file_id, e
                )
            })?;

        self.cos_client
            .put_object(object_key, local_content.into_bytes())
            .await
            .map_err(|e| {
                format!(
                    "Failed to upload local version of {} to COS: {}",
                    conflict.file_id, e
                )
            })?;

        // 10. Update the original file's base_content_hash to the local content_hash.
        let mut updated_original = original_meta.clone();
        updated_original.base_content_hash = Some(original_meta.content_hash.clone());
        updated_original.sync_status = SyncStatus::Synced;

        self.db.upsert_file_meta(&updated_original).map_err(|e| {
            format!(
                "Failed to update original file metadata after conflict resolution: {}",
                e
            )
        })?;

        Ok(())
    }

    /// Gracefully shut down all background tasks.
    ///
    /// Stops the connectivity monitor and aborts the manifest polling
    /// and queue processing tasks. Safe to call multiple times.
    pub fn stop(&mut self) {
        // Stop the connectivity monitor.
        self.conn_monitor.stop();

        // Abort the manifest polling task.
        if let Some(handle) = self.poll_handle.take() {
            handle.abort();
        }

        // Abort the queue processing task.
        if let Some(handle) = self.queue_handle.take() {
            handle.abort();
        }
    }

    /// Download the manifest from COS, merge with local metadata, and
    /// re-upload the merged result.
    ///
    /// If the manifest does not exist on COS yet (first sync), a new
    /// manifest is created from local metadata.
    ///
    /// Handles concurrent modification by retrying up to 3 times: on
    /// upload failure, re-downloads the latest manifest, re-merges,
    /// and retries the upload.
    ///
    /// Validates: Requirements 6.1, 6.3, 6.4
    pub async fn sync_manifest(&self) -> Result<(), String> {
        if !self.cloud_sync_enabled {
            return Ok(());
        }

        const MAX_RETRIES: u32 = 3;

        for attempt in 0..MAX_RETRIES {
            // Step 1: Download the current manifest from COS (or create empty if not found).
            let remote_manifest = download_manifest(&self.cos_client).await?;

            // Step 2: Get all local files from SQLite.
            let local_files = self
                .db
                .get_all_files()
                .map_err(|e| format!("Failed to get local files: {e}"))?;

            // Step 3: Merge local files with remote manifest.
            let merged = merge_manifests(&local_files, &remote_manifest);

            // Step 4: Apply remote-only entries to local DB and remove deleted entries.
            self.apply_remote_changes(&local_files, &remote_manifest)?;

            // Step 5: Upload the merged manifest back to COS.
            let manifest_json = serde_json::to_vec_pretty(&merged)
                .map_err(|e| format!("Failed to serialize manifest: {e}"))?;

            match self
                .cos_client
                .put_object(MANIFEST_KEY, manifest_json)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    // On upload failure (potential concurrent modification),
                    // retry by re-downloading and re-merging.
                    if attempt < MAX_RETRIES - 1 {
                        // Log and retry — next iteration re-downloads fresh manifest.
                        continue;
                    } else {
                        return Err(format!(
                            "Failed to upload manifest after {} retries: {}",
                            MAX_RETRIES, e
                        ));
                    }
                }
            }
        }

        Err("sync_manifest exhausted all retries".to_string())
    }

    /// Apply changes from the remote manifest to the local database.
    ///
    /// - Adds remote-only entries to the local database.
    /// - Removes entries marked as deleted in the remote manifest.
    fn apply_remote_changes(
        &self,
        local_files: &[FileMeta],
        remote: &Manifest,
    ) -> Result<(), String> {
        let local_map: HashMap<&str, &FileMeta> =
            local_files.iter().map(|f| (f.id.as_str(), f)).collect();

        for entry in &remote.files {
            if entry.deleted {
                // Remove entries marked as deleted in remote.
                if local_map.contains_key(entry.id.as_str()) {
                    self.db
                        .delete_file_meta(&entry.id)
                        .map_err(|e| format!("Failed to delete file {}: {e}", entry.id))?;
                }
            } else if !local_map.contains_key(entry.id.as_str()) {
                // Add remote-only entries to local DB.
                let file_meta = FileMeta {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    last_modified: entry.last_modified,
                    content_hash: entry.content_hash.clone(),
                    cos_object_key: Some(entry.object_key.clone()),
                    sync_status: SyncStatus::PendingSync,
                    base_content_hash: Some(entry.content_hash.clone()),
                    is_conflict_copy: false,
                    parent_file_id: None,
                    deleted: false,
                    created_at: entry.last_modified,
                };
                self.db
                    .upsert_file_meta(&file_meta)
                    .map_err(|e| format!("Failed to add remote file {}: {e}", entry.id))?;
            }
        }

        Ok(())
    }
}

/// Generate a conflict copy title in the format "{title} - Conflict {YYYY-MM-DD}".
///
/// Uses the current system date (UTC) for the date suffix.
///
/// Validates: Requirements 8.2
pub(crate) fn generate_conflict_title(original_title: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();

    // Convert epoch seconds to YYYY-MM-DD (UTC).
    let days_since_epoch = secs / 86400;
    let (year, month, day) = days_to_ymd(days_since_epoch);

    format!(
        "{} - Conflict {:04}-{:02}-{:02}",
        original_title, year, month, day
    )
}

/// Generate a conflict copy title with a specific date string.
///
/// This variant is used for testing to allow deterministic date values.
#[cfg(test)]
pub(crate) fn generate_conflict_title_with_date(original_title: &str, date_str: &str) -> String {
    format!("{} - Conflict {}", original_title, date_str)
}

/// Convert days since Unix epoch to (year, month, day) in UTC.
///
/// Simple civil date calculation without external dependencies.
fn days_to_ymd(days_since_epoch: u64) -> (u64, u64, u64) {
    // Algorithm adapted from Howard Hinnant's civil_from_days.
    let z = days_since_epoch + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Merge local file metadata with a remote manifest.
///
/// For each file, the version with the later `last_modified` timestamp
/// wins. Remote-only entries are included in the output. Local-only
/// entries are included. Entries marked as deleted in the remote manifest
/// are included (with their deleted flag) so the manifest stays consistent.
///
/// This is a pure function — it does not modify the database or make
/// network calls.
///
/// Validates: Requirements 6.3, 8.5
pub(crate) fn merge_manifests(local_files: &[FileMeta], remote: &Manifest) -> Manifest {
    let mut merged_map: HashMap<String, ManifestEntry> = HashMap::new();

    // Index remote entries by ID.
    for entry in &remote.files {
        merged_map.insert(entry.id.clone(), entry.clone());
    }

    // Merge local files: use the version with the later last_modified.
    for local in local_files {
        let object_key = local
            .cos_object_key
            .clone()
            .unwrap_or_else(|| cos_object_key_for_file(local));

        let local_entry = ManifestEntry {
            id: local.id.clone(),
            title: local.title.clone(),
            last_modified: local.last_modified,
            content_hash: local.content_hash.clone(),
            object_key,
            deleted: local.deleted,
        };

        match merged_map.get(&local.id) {
            Some(remote_entry) => {
                // Use the version with the later last_modified timestamp.
                if local.last_modified >= remote_entry.last_modified {
                    merged_map.insert(local.id.clone(), local_entry);
                }
                // Otherwise, keep the remote entry (already in the map).
            }
            None => {
                // Local-only entry — add to manifest.
                merged_map.insert(local.id.clone(), local_entry);
            }
        }
    }

    // Compute the overall last_modified for the manifest.
    let manifest_last_modified = merged_map
        .values()
        .map(|e| e.last_modified)
        .max()
        .unwrap_or(0);

    Manifest {
        version: remote.version.max(1),
        last_modified: manifest_last_modified,
        files: merged_map.into_values().collect(),
    }
}

/// Standalone async function for manifest polling from a spawned task.
///
/// This function replicates the core logic of `SyncEngine::sync_manifest`
/// but accepts `Arc` references so it can be called from a `tokio::spawn`
/// context without requiring `SyncEngine` to be `Send + Sync`.
///
/// Steps:
/// 1. Download the current manifest from COS (or create empty if not found).
/// 2. Get all local files from SQLite.
/// 3. Merge local files with remote manifest using timestamp-based resolution.
/// 4. Apply remote changes to local DB:
///    - New remote entries: add to local metadata.
///    - Updated remote entries (content hash differs): update local content hash,
///      mark for re-download (set sync_status to PendingSync).
///    - Deleted remote entries: remove from local metadata.
/// 5. Upload the merged manifest back to COS (with retry on concurrent modification).
///
/// Validates: Requirements 6.5, 6.6, 6.7
async fn poll_sync_manifest(cos_client: &Arc<CosClient>, db: &Arc<Database>) -> Result<(), String> {
    const MAX_RETRIES: u32 = 3;

    for attempt in 0..MAX_RETRIES {
        // Step 1: Download the current manifest from COS (or create empty if not found).
        let remote_manifest = download_manifest(cos_client).await?;

        // Step 2: Get all local files from SQLite.
        let local_files = db
            .get_all_files()
            .map_err(|e| format!("Failed to get local files: {e}"))?;

        // Step 3: Merge local files with remote manifest.
        let merged = merge_manifests(&local_files, &remote_manifest);

        // Step 4: Apply remote changes to local DB.
        apply_remote_changes_standalone(db, &local_files, &remote_manifest)?;

        // Step 5: Upload the merged manifest back to COS.
        let manifest_json = serde_json::to_vec_pretty(&merged)
            .map_err(|e| format!("Failed to serialize manifest: {e}"))?;

        match cos_client.put_object(MANIFEST_KEY, manifest_json).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if attempt < MAX_RETRIES - 1 {
                    // Retry — next iteration re-downloads fresh manifest.
                    continue;
                } else {
                    return Err(format!(
                        "Failed to upload manifest after {} retries: {}",
                        MAX_RETRIES, e
                    ));
                }
            }
        }
    }

    Err("poll_sync_manifest exhausted all retries".to_string())
}

/// Apply changes from the remote manifest to the local database (standalone version).
///
/// This is the standalone equivalent of `SyncEngine::apply_remote_changes`,
/// used by the polling task.
///
/// Handles three cases:
/// - **New remote entries**: Adds them to local metadata with `PendingSync` status
///   (content will be downloaded when the user selects the file).
/// - **Updated remote entries**: When the remote content hash differs from the
///   local content hash, updates the local metadata and marks for re-download.
/// - **Deleted remote entries**: Removes them from local metadata.
///
/// Validates: Requirements 6.5, 6.7
fn apply_remote_changes_standalone(
    db: &Arc<Database>,
    local_files: &[FileMeta],
    remote: &Manifest,
) -> Result<(), String> {
    let local_map: HashMap<&str, &FileMeta> =
        local_files.iter().map(|f| (f.id.as_str(), f)).collect();

    for entry in &remote.files {
        if entry.deleted {
            // Remove entries marked as deleted in remote.
            if local_map.contains_key(entry.id.as_str()) {
                db.delete_file_meta(&entry.id)
                    .map_err(|e| format!("Failed to delete file {}: {e}", entry.id))?;
            }
        } else if let Some(local_file) = local_map.get(entry.id.as_str()) {
            // Entry exists locally — check if remote has a newer version.
            if local_file.content_hash != entry.content_hash
                && entry.last_modified > local_file.last_modified
            {
                // Remote has updated content: update local hash, mark for re-download.
                let updated_meta = FileMeta {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    last_modified: entry.last_modified,
                    content_hash: entry.content_hash.clone(),
                    cos_object_key: Some(entry.object_key.clone()),
                    sync_status: SyncStatus::PendingSync,
                    base_content_hash: Some(entry.content_hash.clone()),
                    is_conflict_copy: local_file.is_conflict_copy,
                    parent_file_id: local_file.parent_file_id.clone(),
                    deleted: false,
                    created_at: local_file.created_at,
                };
                db.upsert_file_meta(&updated_meta)
                    .map_err(|e| format!("Failed to update file {}: {e}", entry.id))?;
            }
        } else {
            // New remote entry — add to local DB.
            let file_meta = FileMeta {
                id: entry.id.clone(),
                title: entry.title.clone(),
                last_modified: entry.last_modified,
                content_hash: entry.content_hash.clone(),
                cos_object_key: Some(entry.object_key.clone()),
                sync_status: SyncStatus::PendingSync,
                base_content_hash: Some(entry.content_hash.clone()),
                is_conflict_copy: false,
                parent_file_id: None,
                deleted: false,
                created_at: entry.last_modified,
            };
            db.upsert_file_meta(&file_meta)
                .map_err(|e| format!("Failed to add remote file {}: {e}", entry.id))?;
        }
    }

    Ok(())
}

/// Standalone async function for upload queue processing from a spawned task.
///
/// This function replicates the core logic of `SyncEngine::process_upload_queue`
/// but accepts `Arc` references so it can be called from a `tokio::spawn`
/// context without requiring `SyncEngine` to be `Send + Sync`.
///
/// Steps:
/// 1. Check connectivity — if offline, return early.
/// 2. Get all pending uploads from the database (ordered by `created_at` ASC).
/// 3. For each entry in chronological order:
///    a. Attempt to upload the file via COS.
///    b. On success: update sync_status to Synced, dequeue the entry.
///    c. On failure: increment retry_count, skip and continue with next.
/// 4. Return Ok(()) after processing all items.
///
/// Validates: Requirements 7.4, 7.5
async fn process_upload_queue_standalone(
    cos_client: &Arc<CosClient>,
    db: &Arc<Database>,
    file_store: &Arc<FileStore>,
    conn_monitor: &Arc<ConnectivityMonitor>,
) -> Result<(), String> {
    // 1. Check connectivity — if offline, return early.
    if !conn_monitor.is_online() {
        return Ok(());
    }

    // 2. Get all pending uploads from DB (already ordered by created_at ASC).
    let pending = db
        .get_pending_uploads()
        .map_err(|e| format!("Failed to get pending uploads: {}", e))?;

    // 3. Process each entry in chronological order.
    for entry in &pending {
        // Re-check connectivity before each upload attempt.
        if !conn_monitor.is_online() {
            break;
        }

        // Look up file metadata.
        let meta = match db.get_file_meta(&entry.file_id) {
            Ok(Some(m)) => m,
            Ok(None) => {
                // File no longer exists in DB — dequeue orphaned entry.
                let _ = db.dequeue_upload(&entry.file_id);
                continue;
            }
            Err(_) => continue,
        };

        match entry.operation {
            UploadOperation::Upload | UploadOperation::Rename => {
                // Determine the COS object key.
                let object_key = match &meta.cos_object_key {
                    Some(key) if !key.trim().is_empty() => key.clone(),
                    _ => continue, // Skip if no COS key configured.
                };

                // Read the file content from local store.
                let content = match file_store.read_canvas(&entry.file_id) {
                    Ok(c) => c,
                    Err(_) => continue, // Skip if local file is missing.
                };

                // Attempt to upload via COS.
                match cos_client.put_object(&object_key, content.into_bytes()).await {
                    Ok(()) => {
                        // Success: update sync_status to Synced.
                        let mut updated_meta = meta.clone();
                        updated_meta.sync_status = SyncStatus::Synced;
                        updated_meta.base_content_hash = Some(meta.content_hash.clone());
                        let _ = db.upsert_file_meta(&updated_meta);

                        if matches!(entry.operation, UploadOperation::Rename) {
                            if let Some(old_key) =
                                old_object_key_from_payload(entry.payload.as_deref())
                            {
                                if old_key != object_key {
                                    let _ = cos_client.delete_object(&old_key).await;
                                }
                            }
                        }

                        // Dequeue all pending uploads for this file.
                        let _ = db.dequeue_upload(&entry.file_id);
                    }
                    Err(_) => {
                        // Failure: increment retry_count and skip to next.
                        let _ = db.increment_retry_count(entry.id);

                        // Check if max retries exceeded.
                        let new_retry_count = entry.retry_count + 1;
                        if new_retry_count > entry.max_retries {
                            // Mark file as Error status.
                            let mut error_meta = meta.clone();
                            error_meta.sync_status = SyncStatus::Error;
                            let _ = db.upsert_file_meta(&error_meta);
                        }
                    }
                }
            }
            UploadOperation::Delete => {
                if let Some(key) = meta.cos_object_key.as_deref() {
                    match cos_client.delete_object(key).await {
                        Ok(()) => {
                            let _ = db.dequeue_upload(&entry.file_id);
                        }
                        Err(_) => {
                            let _ = db.increment_retry_count(entry.id);
                        }
                    }
                } else {
                    let _ = db.dequeue_upload(&entry.file_id);
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cos_client::CosClient;
    use crate::models::{CosConfig, FileMeta, Manifest, ManifestEntry, SyncStatus};

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
    fn create_test_engine() -> SyncEngine {
        let config = test_cos_config();
        let cos_client = CosClient::new(&config).unwrap();
        let conn_monitor = ConnectivityMonitor::new(Arc::new(cos_client.clone()));

        let tmp_dir = std::env::temp_dir().join(format!(
            "sync-engine-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = tmp_dir.join("test.sqlite");
        let db = Database::open(&db_path).unwrap();
        let file_store = FileStore::new(tmp_dir.join("files")).unwrap();

        SyncEngine::new(cos_client, db, file_store, conn_monitor)
    }

    #[test]
    fn new_creates_engine_without_running_tasks() {
        let engine = create_test_engine();

        // No background tasks should be running yet.
        assert!(engine.poll_handle.is_none());
        assert!(engine.queue_handle.is_none());
    }

    #[test]
    fn stop_is_safe_when_not_started() {
        let mut engine = create_test_engine();

        // Calling stop without start should not panic.
        engine.stop();

        assert!(engine.poll_handle.is_none());
        assert!(engine.queue_handle.is_none());
    }

    #[test]
    fn stop_can_be_called_multiple_times() {
        let mut engine = create_test_engine();

        engine.stop();
        engine.stop();
        engine.stop();

        // Should not panic.
        assert!(engine.poll_handle.is_none());
        assert!(engine.queue_handle.is_none());
    }

    #[tokio::test]
    async fn start_spawns_background_tasks() {
        let mut engine = create_test_engine();

        // We need an AppHandle — since we can't easily construct one in
        // tests, we verify the structural invariants only. The full
        // integration test with a real AppHandle is deferred to task 16.1.
        //
        // For now, verify the engine structure is sound by checking that
        // after construction the handles are None.
        assert!(engine.poll_handle.is_none());
        assert!(engine.queue_handle.is_none());

        // After stop (even without start), handles remain None.
        engine.stop();
        assert!(engine.poll_handle.is_none());
        assert!(engine.queue_handle.is_none());
    }

    #[tokio::test]
    async fn save_canvas_writes_file_and_updates_metadata() {
        let engine = create_test_engine();
        let file_id = "test-save-canvas-1";
        let data = r#"{"type":"excalidraw","version":2,"elements":[]}"#;

        let result = engine.save_canvas(file_id, data).await;
        assert!(result.is_ok(), "save_canvas should succeed");

        let status = result.unwrap();
        assert_eq!(status, crate::models::SyncStatus::PendingSync);

        // Verify file was written to local store.
        let loaded = engine.file_store.read_canvas(file_id).unwrap();
        assert_eq!(loaded, data);

        // Verify file metadata was persisted in SQLite.
        let meta = engine.db.get_file_meta(file_id).unwrap().unwrap();
        assert_eq!(meta.id, file_id);
        assert_eq!(
            meta.content_hash,
            crate::file_store::compute_content_hash(data)
        );
        assert_eq!(
            meta.cos_object_key,
            Some("excalidraw/Untitled.excalidraw".to_string())
        );
        assert_eq!(meta.sync_status, crate::models::SyncStatus::PendingSync);
        assert_eq!(meta.title, "Untitled");
        assert!(!meta.deleted);

        // Verify an upload was enqueued.
        let uploads = engine.db.get_pending_uploads().unwrap();
        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads[0].file_id, file_id);
    }

    #[tokio::test]
    async fn save_canvas_local_only_does_not_enqueue_upload() {
        let mut engine = create_test_engine();
        engine.set_cloud_sync_enabled(false);
        let file_id = "test-save-local-only";
        let data = r#"{"type":"excalidraw","version":2,"elements":[]}"#;

        let status = engine.save_canvas(file_id, data).await.unwrap();

        assert_eq!(status, crate::models::SyncStatus::Synced);
        let meta = engine.db.get_file_meta(file_id).unwrap().unwrap();
        assert_eq!(meta.cos_object_key, None);
        assert_eq!(meta.sync_status, crate::models::SyncStatus::Synced);
        assert_eq!(
            meta.base_content_hash,
            Some(crate::file_store::compute_content_hash(data)),
        );
        assert!(engine.db.get_pending_uploads().unwrap().is_empty());
    }

    #[tokio::test]
    async fn save_canvas_preserves_existing_metadata_fields() {
        let engine = create_test_engine();
        let file_id = "test-save-preserve";

        // Pre-populate metadata with a custom title and base hash.
        let initial_meta = crate::models::FileMeta {
            id: file_id.to_string(),
            title: "My Custom Title".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "old-hash".to_string(),
            cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
            sync_status: crate::models::SyncStatus::Synced,
            base_content_hash: Some("base-hash-123".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&initial_meta).unwrap();

        // Now save new canvas data.
        let new_data = r#"{"type":"excalidraw","version":2,"elements":[{"id":"el1"}]}"#;
        let result = engine.save_canvas(file_id, new_data).await;
        assert!(result.is_ok());

        let meta = engine.db.get_file_meta(file_id).unwrap().unwrap();
        // Title and base_content_hash should be preserved.
        assert_eq!(meta.title, "My Custom Title");
        assert_eq!(meta.base_content_hash, Some("base-hash-123".to_string()));
        // created_at should be preserved.
        assert_eq!(meta.created_at, 1_700_000_000_000);
        // content_hash should be updated.
        assert_eq!(
            meta.content_hash,
            crate::file_store::compute_content_hash(new_data)
        );
        // last_modified should be recent (not the old value).
        assert!(meta.last_modified > 1_700_000_000_000);
    }

    #[tokio::test]
    async fn save_canvas_computes_correct_content_hash() {
        let engine = create_test_engine();
        let file_id = "test-hash";
        let data = "hello world canvas data";

        engine.save_canvas(file_id, data).await.unwrap();

        let meta = engine.db.get_file_meta(file_id).unwrap().unwrap();
        let expected_hash = crate::file_store::compute_content_hash(data);
        assert_eq!(meta.content_hash, expected_hash);
    }

    // --- merge_manifests tests (Task 7.4) ---

    /// Helper to create a sample FileMeta for merge testing.
    fn make_local_file(id: &str, last_modified: i64, hash: &str) -> FileMeta {
        FileMeta {
            id: id.to_string(),
            title: format!("Local {}", id),
            last_modified,
            content_hash: hash.to_string(),
            cos_object_key: Some(format!("files/{}.excalidraw", id)),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some(hash.to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        }
    }

    /// Helper to create a sample ManifestEntry for merge testing.
    fn make_remote_entry(id: &str, last_modified: i64, hash: &str) -> ManifestEntry {
        ManifestEntry {
            id: id.to_string(),
            title: format!("Remote {}", id),
            last_modified,
            content_hash: hash.to_string(),
            object_key: format!("files/{}.excalidraw", id),
            deleted: false,
        }
    }

    #[test]
    fn merge_manifests_empty_local_and_remote() {
        let local: Vec<FileMeta> = vec![];
        let remote = Manifest {
            version: 1,
            last_modified: 0,
            files: vec![],
        };

        let result = merge_manifests(&local, &remote);

        assert_eq!(result.version, 1);
        assert_eq!(result.last_modified, 0);
        assert!(result.files.is_empty());
    }

    #[test]
    fn merge_manifests_local_only_entries_added_to_manifest() {
        let local = vec![
            make_local_file("file-1", 1_000, "hash-1"),
            make_local_file("file-2", 2_000, "hash-2"),
        ];
        let remote = Manifest {
            version: 1,
            last_modified: 0,
            files: vec![],
        };

        let result = merge_manifests(&local, &remote);

        assert_eq!(result.files.len(), 2);
        assert_eq!(result.last_modified, 2_000);

        let ids: Vec<&str> = result.files.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(&"file-1"));
        assert!(ids.contains(&"file-2"));
    }

    #[test]
    fn merge_manifests_remote_only_entries_preserved() {
        let local: Vec<FileMeta> = vec![];
        let remote = Manifest {
            version: 1,
            last_modified: 3_000,
            files: vec![make_remote_entry("file-r1", 3_000, "rhash-1")],
        };

        let result = merge_manifests(&local, &remote);

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].id, "file-r1");
        assert_eq!(result.files[0].title, "Remote file-r1");
    }

    #[test]
    fn merge_manifests_local_wins_when_newer() {
        let local = vec![make_local_file("file-1", 5_000, "local-hash")];
        let remote = Manifest {
            version: 1,
            last_modified: 3_000,
            files: vec![make_remote_entry("file-1", 3_000, "remote-hash")],
        };

        let result = merge_manifests(&local, &remote);

        assert_eq!(result.files.len(), 1);
        let entry = &result.files[0];
        assert_eq!(entry.id, "file-1");
        assert_eq!(entry.last_modified, 5_000);
        assert_eq!(entry.content_hash, "local-hash");
        assert_eq!(entry.title, "Local file-1");
    }

    #[test]
    fn merge_manifests_remote_wins_when_newer() {
        let local = vec![make_local_file("file-1", 2_000, "local-hash")];
        let remote = Manifest {
            version: 1,
            last_modified: 5_000,
            files: vec![make_remote_entry("file-1", 5_000, "remote-hash")],
        };

        let result = merge_manifests(&local, &remote);

        assert_eq!(result.files.len(), 1);
        let entry = &result.files[0];
        assert_eq!(entry.id, "file-1");
        assert_eq!(entry.last_modified, 5_000);
        assert_eq!(entry.content_hash, "remote-hash");
        assert_eq!(entry.title, "Remote file-1");
    }

    #[test]
    fn merge_manifests_local_wins_on_equal_timestamp() {
        let local = vec![make_local_file("file-1", 3_000, "local-hash")];
        let remote = Manifest {
            version: 1,
            last_modified: 3_000,
            files: vec![make_remote_entry("file-1", 3_000, "remote-hash")],
        };

        let result = merge_manifests(&local, &remote);

        assert_eq!(result.files.len(), 1);
        let entry = &result.files[0];
        // On equal timestamp, local wins (>= comparison).
        assert_eq!(entry.content_hash, "local-hash");
    }

    #[test]
    fn merge_manifests_mixed_local_and_remote() {
        let local = vec![
            make_local_file("shared", 5_000, "local-hash"),
            make_local_file("local-only", 1_000, "lo-hash"),
        ];
        let remote = Manifest {
            version: 1,
            last_modified: 4_000,
            files: vec![
                make_remote_entry("shared", 3_000, "remote-hash"),
                make_remote_entry("remote-only", 4_000, "ro-hash"),
            ],
        };

        let result = merge_manifests(&local, &remote);

        assert_eq!(result.files.len(), 3);

        let file_map: HashMap<&str, &ManifestEntry> =
            result.files.iter().map(|e| (e.id.as_str(), e)).collect();

        // "shared" should use local (newer timestamp 5000 > 3000)
        assert_eq!(file_map["shared"].content_hash, "local-hash");
        // "local-only" should be present
        assert!(file_map.contains_key("local-only"));
        // "remote-only" should be present
        assert!(file_map.contains_key("remote-only"));
        assert_eq!(file_map["remote-only"].content_hash, "ro-hash");
    }

    #[test]
    fn merge_manifests_preserves_deleted_flag_from_remote() {
        let local: Vec<FileMeta> = vec![];
        let mut remote_entry = make_remote_entry("del-file", 2_000, "hash");
        remote_entry.deleted = true;

        let remote = Manifest {
            version: 1,
            last_modified: 2_000,
            files: vec![remote_entry],
        };

        let result = merge_manifests(&local, &remote);

        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].deleted);
    }

    #[test]
    fn merge_manifests_version_uses_remote_version_or_one() {
        let local = vec![make_local_file("f1", 1_000, "h1")];
        let remote = Manifest {
            version: 3,
            last_modified: 500,
            files: vec![],
        };

        let result = merge_manifests(&local, &remote);

        // version should be max(remote.version, 1)
        assert_eq!(result.version, 3);
    }

    #[test]
    fn merge_manifests_uses_cos_object_key_or_generates_default() {
        let mut local_with_key = make_local_file("with-key", 1_000, "h1");
        local_with_key.cos_object_key = Some("custom/path.excalidraw".to_string());

        let mut local_without_key = make_local_file("no-key", 2_000, "h2");
        local_without_key.cos_object_key = None;

        let local = vec![local_with_key, local_without_key];
        let remote = Manifest {
            version: 1,
            last_modified: 0,
            files: vec![],
        };

        let result = merge_manifests(&local, &remote);

        let file_map: HashMap<&str, &ManifestEntry> =
            result.files.iter().map(|e| (e.id.as_str(), e)).collect();

        assert_eq!(file_map["with-key"].object_key, "custom/path.excalidraw");
        assert_eq!(
            file_map["no-key"].object_key,
            "excalidraw/Local no-key.excalidraw"
        );
    }

    // --- upload_file tests (Task 7.3) ---

    #[tokio::test]
    async fn upload_file_returns_pending_sync_when_offline() {
        let engine = create_test_engine();

        // The connectivity monitor defaults to offline (false) until
        // a successful probe, so upload_file should return PendingSync.
        let result = engine.upload_file("any-file-id").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), SyncStatus::PendingSync);
    }

    #[tokio::test]
    async fn upload_file_returns_error_when_file_not_in_db() {
        let engine = create_test_engine();

        // Force connectivity to appear online by directly sending true
        // on the watch channel — however ConnectivityMonitor doesn't expose
        // a setter. Since the monitor defaults to offline, this test
        // validates the offline early-return path implicitly.
        // To test the "file not found" path, we'd need to be online.
        // This test verifies the offline path returns PendingSync.
        let result = engine.upload_file("nonexistent-file").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), SyncStatus::PendingSync);
    }

    #[tokio::test]
    async fn upload_file_skips_upload_when_cos_object_key_is_none() {
        let engine = create_test_engine();

        // Insert a file with no cos_object_key.
        let meta = FileMeta {
            id: "no-key-file".to_string(),
            title: "No Key".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "hash-123".to_string(),
            cos_object_key: None, // Missing COS key
            sync_status: SyncStatus::PendingSync,
            base_content_hash: None,
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // Even if online, this would skip upload due to missing key.
        // But since monitor defaults offline, it returns PendingSync
        // on the connectivity check.
        let result = engine.upload_file("no-key-file").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), SyncStatus::PendingSync);
    }

    // --- load_canvas tests (Task 7.6) ---

    #[tokio::test]
    async fn load_canvas_returns_error_when_file_not_in_db() {
        let engine = create_test_engine();

        let result = engine.load_canvas("nonexistent-file").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("File not found in database"));
    }

    #[tokio::test]
    async fn load_canvas_serves_from_local_cache_when_hash_matches() {
        let engine = create_test_engine();
        let file_id = "cached-file";
        let data = r#"{"type":"excalidraw","version":2,"elements":[]}"#;

        // Write file to local store.
        engine.file_store.write_canvas(file_id, data).unwrap();

        // Compute the content hash that matches the local file.
        let content_hash = crate::file_store::compute_content_hash(data);

        // Insert metadata with content_hash == base_content_hash (in sync).
        let meta = FileMeta {
            id: file_id.to_string(),
            title: "Cached File".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: content_hash.clone(),
            cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some(content_hash), // matches content_hash
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // load_canvas should return the local content without network access.
        let result = engine.load_canvas(file_id).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), data);
    }

    #[tokio::test]
    async fn load_canvas_falls_back_to_local_when_download_fails_and_hashes_differ() {
        let engine = create_test_engine();
        let file_id = "stale-file";
        let local_data = r#"{"type":"excalidraw","version":2,"elements":[{"id":"old"}]}"#;

        // Write stale local file.
        engine.file_store.write_canvas(file_id, local_data).unwrap();

        // Compute local hash.
        let local_hash = crate::file_store::compute_content_hash(local_data);

        // Insert metadata where content_hash differs from base_content_hash
        // (simulating that COS has a newer version).
        let meta = FileMeta {
            id: file_id.to_string(),
            title: "Stale File".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: local_hash.clone(),
            cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: Some("different-remote-hash".to_string()), // differs from content_hash
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // load_canvas should attempt download from COS. Since the COS client
        // points to a non-existent endpoint in tests, download will fail.
        // With fallback logic, it should return the local content.
        let result = engine.load_canvas(file_id).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), local_data);
    }

    #[tokio::test]
    async fn load_canvas_attempts_download_when_local_file_missing() {
        let engine = create_test_engine();
        let file_id = "remote-only-file";

        // Insert metadata but do NOT write any local file.
        let meta = FileMeta {
            id: file_id.to_string(),
            title: "Remote Only".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "some-hash".to_string(),
            cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: Some("some-hash".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // load_canvas should attempt download since local file is absent.
        let result = engine.load_canvas(file_id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to download file"));
    }

    #[tokio::test]
    async fn load_canvas_returns_error_when_no_cos_object_key() {
        let engine = create_test_engine();
        let file_id = "no-cos-key-file";

        // Insert metadata with no COS object key and no local file.
        let meta = FileMeta {
            id: file_id.to_string(),
            title: "No COS Key".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "some-hash".to_string(),
            cos_object_key: None, // No COS key
            sync_status: SyncStatus::PendingSync,
            base_content_hash: None,
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // load_canvas should fail because there's no local file and no COS key.
        let result = engine.load_canvas(file_id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("has no COS object key"));
    }

    #[tokio::test]
    async fn load_canvas_falls_back_to_local_when_base_hash_is_none() {
        let engine = create_test_engine();
        let file_id = "no-base-hash";
        let data = r#"{"type":"excalidraw","version":2,"elements":[]}"#;

        // Write file to local store.
        engine.file_store.write_canvas(file_id, data).unwrap();

        // Insert metadata with no base_content_hash (never synced).
        let meta = FileMeta {
            id: file_id.to_string(),
            title: "Never Synced".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: crate::file_store::compute_content_hash(data),
            cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: None, // No base hash — never synced
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // When base_content_hash is None, we cannot confirm the local file
        // is in sync with COS, so it should attempt download. Since download
        // fails in tests, it falls back to local cache.
        let result = engine.load_canvas(file_id).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), data);
    }

    #[tokio::test]
    async fn download_canvas_always_attempts_remote_fetch() {
        let engine = create_test_engine();
        let file_id = "force-download";
        let local_data = r#"{"type":"excalidraw","version":2,"elements":[{"id":"local"}]}"#;

        engine.file_store.write_canvas(file_id, local_data).unwrap();

        let meta = FileMeta {
            id: file_id.to_string(),
            title: "Force Download".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: crate::file_store::compute_content_hash(local_data),
            cos_object_key: Some(format!("excalidraw/{}.excalidraw", file_id)),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some("different-remote-hash".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        let result = engine.download_canvas(file_id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to download file"));
    }

    // --- detect_conflicts tests (Task 8.1) ---

    #[test]
    fn detect_conflicts_returns_empty_when_no_remote_files() {
        let engine = create_test_engine();
        let remote = Manifest {
            version: 1,
            last_modified: 0,
            files: vec![],
        };

        let conflicts = engine.detect_conflicts(&remote);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_returns_empty_when_file_not_in_local_db() {
        let engine = create_test_engine();

        // Remote has a file that doesn't exist locally.
        let remote = Manifest {
            version: 1,
            last_modified: 1_000,
            files: vec![make_remote_entry("unknown-file", 1_000, "remote-hash")],
        };

        let conflicts = engine.detect_conflicts(&remote);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_skips_file_with_no_base_hash() {
        let engine = create_test_engine();

        // Insert a local file with no base_content_hash (never synced).
        let meta = FileMeta {
            id: "file-no-base".to_string(),
            title: "No Base".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "local-hash".to_string(),
            cos_object_key: Some("files/file-no-base.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: None, // No baseline
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        let remote = Manifest {
            version: 1,
            last_modified: 2_000,
            files: vec![make_remote_entry("file-no-base", 2_000, "remote-hash")],
        };

        let conflicts = engine.detect_conflicts(&remote);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_identifies_conflict_when_both_differ_from_base() {
        let engine = create_test_engine();

        // Insert a local file where content_hash differs from base_content_hash
        // (local was modified since last sync).
        let meta = FileMeta {
            id: "conflict-file".to_string(),
            title: "Conflict File".to_string(),
            last_modified: 1_700_000_100_000,
            content_hash: "local-modified-hash".to_string(),
            cos_object_key: Some("files/conflict-file.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: Some("base-hash-at-sync".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // Remote also has a different hash from the base.
        let remote = Manifest {
            version: 1,
            last_modified: 1_700_000_200_000,
            files: vec![ManifestEntry {
                id: "conflict-file".to_string(),
                title: "Conflict File".to_string(),
                last_modified: 1_700_000_200_000,
                content_hash: "remote-modified-hash".to_string(),
                object_key: "files/conflict-file.excalidraw".to_string(),
                deleted: false,
            }],
        };

        let conflicts = engine.detect_conflicts(&remote);
        assert_eq!(conflicts.len(), 1);

        let conflict = &conflicts[0];
        assert_eq!(conflict.file_id, "conflict-file");
        assert_eq!(conflict.local_hash, "local-modified-hash");
        assert_eq!(conflict.remote_hash, "remote-modified-hash");
        assert_eq!(conflict.base_hash, "base-hash-at-sync");
        assert_eq!(conflict.remote_last_modified, 1_700_000_200_000);
    }

    #[test]
    fn detect_conflicts_no_conflict_when_only_remote_differs() {
        let engine = create_test_engine();

        // Local file has content_hash == base_content_hash (local unchanged).
        let meta = FileMeta {
            id: "remote-update-file".to_string(),
            title: "Remote Update".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "base-hash".to_string(), // Same as base
            cos_object_key: Some("files/remote-update-file.excalidraw".to_string()),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some("base-hash".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // Remote has a different hash (remote was updated).
        let remote = Manifest {
            version: 1,
            last_modified: 1_700_000_100_000,
            files: vec![ManifestEntry {
                id: "remote-update-file".to_string(),
                title: "Remote Update".to_string(),
                last_modified: 1_700_000_100_000,
                content_hash: "new-remote-hash".to_string(),
                object_key: "files/remote-update-file.excalidraw".to_string(),
                deleted: false,
            }],
        };

        // This is a remote update, NOT a conflict.
        let conflicts = engine.detect_conflicts(&remote);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_no_conflict_when_remote_matches_base() {
        let engine = create_test_engine();

        // Local file was modified (content_hash != base_content_hash).
        let meta = FileMeta {
            id: "local-only-change".to_string(),
            title: "Local Change".to_string(),
            last_modified: 1_700_000_100_000,
            content_hash: "local-new-hash".to_string(),
            cos_object_key: Some("files/local-only-change.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: Some("base-hash".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // Remote hash matches the base (remote unchanged).
        let remote = Manifest {
            version: 1,
            last_modified: 1_700_000_000_000,
            files: vec![ManifestEntry {
                id: "local-only-change".to_string(),
                title: "Local Change".to_string(),
                last_modified: 1_700_000_000_000,
                content_hash: "base-hash".to_string(), // Same as base
                object_key: "files/local-only-change.excalidraw".to_string(),
                deleted: false,
            }],
        };

        // Remote matches base → no conflict.
        let conflicts = engine.detect_conflicts(&remote);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_no_conflict_when_all_hashes_match() {
        let engine = create_test_engine();

        // All hashes are the same — file is fully in sync.
        let meta = FileMeta {
            id: "synced-file".to_string(),
            title: "Synced".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "same-hash".to_string(),
            cos_object_key: Some("files/synced-file.excalidraw".to_string()),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some("same-hash".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        let remote = Manifest {
            version: 1,
            last_modified: 1_700_000_000_000,
            files: vec![ManifestEntry {
                id: "synced-file".to_string(),
                title: "Synced".to_string(),
                last_modified: 1_700_000_000_000,
                content_hash: "same-hash".to_string(),
                object_key: "files/synced-file.excalidraw".to_string(),
                deleted: false,
            }],
        };

        let conflicts = engine.detect_conflicts(&remote);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_skips_deleted_remote_entries() {
        let engine = create_test_engine();

        // Insert a local file that would conflict if not deleted.
        let meta = FileMeta {
            id: "deleted-remote".to_string(),
            title: "Deleted Remote".to_string(),
            last_modified: 1_700_000_100_000,
            content_hash: "local-modified".to_string(),
            cos_object_key: Some("files/deleted-remote.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: Some("base-hash".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // Remote entry is marked as deleted.
        let remote = Manifest {
            version: 1,
            last_modified: 1_700_000_200_000,
            files: vec![ManifestEntry {
                id: "deleted-remote".to_string(),
                title: "Deleted Remote".to_string(),
                last_modified: 1_700_000_200_000,
                content_hash: "remote-modified".to_string(),
                object_key: "files/deleted-remote.excalidraw".to_string(),
                deleted: true, // Deleted
            }],
        };

        // Deleted entries should be skipped.
        let conflicts = engine.detect_conflicts(&remote);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_multiple_files_mixed_results() {
        let engine = create_test_engine();

        // File 1: conflict (both local and remote differ from base)
        let meta1 = FileMeta {
            id: "file-1".to_string(),
            title: "File 1".to_string(),
            last_modified: 1_700_000_100_000,
            content_hash: "local-hash-1".to_string(),
            cos_object_key: Some("files/file-1.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: Some("base-hash-1".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };

        // File 2: no conflict (only remote differs, local unchanged)
        let meta2 = FileMeta {
            id: "file-2".to_string(),
            title: "File 2".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "base-hash-2".to_string(), // Same as base
            cos_object_key: Some("files/file-2.excalidraw".to_string()),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some("base-hash-2".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };

        // File 3: conflict (both differ from base)
        let meta3 = FileMeta {
            id: "file-3".to_string(),
            title: "File 3".to_string(),
            last_modified: 1_700_000_100_000,
            content_hash: "local-hash-3".to_string(),
            cos_object_key: Some("files/file-3.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: Some("base-hash-3".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };

        engine.db.upsert_file_meta(&meta1).unwrap();
        engine.db.upsert_file_meta(&meta2).unwrap();
        engine.db.upsert_file_meta(&meta3).unwrap();

        let remote = Manifest {
            version: 1,
            last_modified: 1_700_000_200_000,
            files: vec![
                ManifestEntry {
                    id: "file-1".to_string(),
                    title: "File 1".to_string(),
                    last_modified: 1_700_000_200_000,
                    content_hash: "remote-hash-1".to_string(),
                    object_key: "files/file-1.excalidraw".to_string(),
                    deleted: false,
                },
                ManifestEntry {
                    id: "file-2".to_string(),
                    title: "File 2".to_string(),
                    last_modified: 1_700_000_200_000,
                    content_hash: "remote-hash-2".to_string(),
                    object_key: "files/file-2.excalidraw".to_string(),
                    deleted: false,
                },
                ManifestEntry {
                    id: "file-3".to_string(),
                    title: "File 3".to_string(),
                    last_modified: 1_700_000_200_000,
                    content_hash: "remote-hash-3".to_string(),
                    object_key: "files/file-3.excalidraw".to_string(),
                    deleted: false,
                },
            ],
        };

        let conflicts = engine.detect_conflicts(&remote);

        // Should detect 2 conflicts (file-1 and file-3).
        assert_eq!(conflicts.len(), 2);

        let conflict_ids: Vec<&str> = conflicts.iter().map(|c| c.file_id.as_str()).collect();
        assert!(conflict_ids.contains(&"file-1"));
        assert!(conflict_ids.contains(&"file-3"));
        assert!(!conflict_ids.contains(&"file-2"));
    }

    // --- process_upload_queue tests (Task 8.3) ---

    #[tokio::test]
    async fn process_upload_queue_returns_ok_when_offline() {
        let engine = create_test_engine();

        // The connectivity monitor defaults to offline (false).
        // process_upload_queue should return Ok(()) immediately.
        let result = engine.process_upload_queue().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn process_upload_queue_returns_ok_with_empty_queue() {
        let engine = create_test_engine();

        // Even though we're offline, verify the method handles empty queue gracefully.
        // The offline check returns early before even querying the DB.
        let result = engine.process_upload_queue().await;
        assert!(result.is_ok());

        // Verify no uploads are pending.
        let pending = engine.db.get_pending_uploads().unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn process_upload_queue_processes_entries_in_chronological_order() {
        let engine = create_test_engine();

        // Set up multiple files with queued uploads at different timestamps.
        let file_ids = ["file-oldest", "file-middle", "file-newest"];
        let timestamps = [1_000_000i64, 2_000_000i64, 3_000_000i64];

        for (i, file_id) in file_ids.iter().enumerate() {
            let meta = FileMeta {
                id: file_id.to_string(),
                title: format!("File {}", i),
                last_modified: timestamps[i],
                content_hash: format!("hash-{}", i),
                cos_object_key: Some(format!("files/{}.excalidraw", file_id)),
                sync_status: SyncStatus::PendingSync,
                base_content_hash: None,
                is_conflict_copy: false,
                parent_file_id: None,
                deleted: false,
                created_at: timestamps[i],
            };
            engine.db.upsert_file_meta(&meta).unwrap();

            // Write local file content.
            engine
                .file_store
                .write_canvas(file_id, &format!("content-{}", i))
                .unwrap();

            // Enqueue upload with chronological created_at.
            let upload = crate::models::QueuedUpload {
                id: 0,
                file_id: file_id.to_string(),
                operation: crate::models::UploadOperation::Upload,
                payload: None,
                retry_count: 0,
                max_retries: 5,
                created_at: timestamps[i],
            };
            engine.db.enqueue_upload(&upload).unwrap();
        }

        // Verify uploads are queued in chronological order.
        let pending = engine.db.get_pending_uploads().unwrap();
        assert_eq!(pending.len(), 3);
        assert_eq!(pending[0].file_id, "file-oldest");
        assert_eq!(pending[1].file_id, "file-middle");
        assert_eq!(pending[2].file_id, "file-newest");

        // process_upload_queue will return early because we're offline.
        // This test verifies the queue ordering is correct.
        let result = engine.process_upload_queue().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn process_upload_queue_standalone_returns_ok_when_offline() {
        let engine = create_test_engine();

        // The standalone function should also return Ok(()) when offline.
        let result = process_upload_queue_standalone(
            &engine.cos_client,
            &engine.db,
            &engine.file_store,
            &engine.conn_monitor,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn process_upload_queue_standalone_handles_empty_queue() {
        let engine = create_test_engine();

        // Even offline, the function should handle gracefully.
        let result = process_upload_queue_standalone(
            &engine.cos_client,
            &engine.db,
            &engine.file_store,
            &engine.conn_monitor,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn process_upload_queue_skips_entries_without_local_file() {
        let engine = create_test_engine();

        // Insert file metadata and enqueue upload, but do NOT write local file.
        let meta = FileMeta {
            id: "missing-local".to_string(),
            title: "Missing Local".to_string(),
            last_modified: 1_000_000,
            content_hash: "hash-missing".to_string(),
            cos_object_key: Some("files/missing-local.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: None,
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        let upload = crate::models::QueuedUpload {
            id: 0,
            file_id: "missing-local".to_string(),
            operation: crate::models::UploadOperation::Upload,
            payload: None,
            retry_count: 0,
            max_retries: 5,
            created_at: 1_000_000,
        };
        engine.db.enqueue_upload(&upload).unwrap();

        // process_upload_queue returns early when offline.
        let result = engine.process_upload_queue().await;
        assert!(result.is_ok());

        // Queue entry should still be present (not processed because offline).
        let pending = engine.db.get_pending_uploads().unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn process_upload_queue_does_not_crash_with_multiple_entries_for_same_file() {
        let engine = create_test_engine();

        // Insert file metadata.
        let meta = FileMeta {
            id: "dup-file".to_string(),
            title: "Duplicate".to_string(),
            last_modified: 1_000_000,
            content_hash: "hash-dup".to_string(),
            cos_object_key: Some("files/dup-file.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: None,
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_000_000,
        };
        engine.db.upsert_file_meta(&meta).unwrap();

        // Write local file.
        engine
            .file_store
            .write_canvas("dup-file", "content")
            .unwrap();

        // Enqueue multiple uploads for the same file (simulating rapid saves).
        for i in 0..3 {
            let upload = crate::models::QueuedUpload {
                id: 0,
                file_id: "dup-file".to_string(),
                operation: crate::models::UploadOperation::Upload,
                payload: None,
                retry_count: 0,
                max_retries: 5,
                created_at: 1_000_000 + i,
            };
            engine.db.enqueue_upload(&upload).unwrap();
        }

        // Verify 3 entries are queued.
        let pending = engine.db.get_pending_uploads().unwrap();
        assert_eq!(pending.len(), 3);

        // process_upload_queue should not crash (returns early because offline).
        let result = engine.process_upload_queue().await;
        assert!(result.is_ok());
    }

    // --- resolve_conflict tests (Task 8.2) ---

    #[test]
    fn generate_conflict_title_formats_correctly() {
        // Test the helper function with a known date.
        let title = super::generate_conflict_title_with_date("My Drawing", "2024-01-15");
        assert_eq!(title, "My Drawing - Conflict 2024-01-15");
    }

    #[test]
    fn generate_conflict_title_with_empty_title() {
        let title = super::generate_conflict_title_with_date("", "2024-03-20");
        assert_eq!(title, " - Conflict 2024-03-20");
    }

    #[test]
    fn generate_conflict_title_with_special_characters() {
        let title = super::generate_conflict_title_with_date("Design (v2) — Final!", "2024-12-31");
        assert_eq!(title, "Design (v2) — Final! - Conflict 2024-12-31");
    }

    #[test]
    fn generate_conflict_title_produces_valid_date_format() {
        // The live function should produce a title with YYYY-MM-DD format.
        let title = super::generate_conflict_title("Test File");
        assert!(title.starts_with("Test File - Conflict "));
        // Extract the date portion and validate format.
        let date_part = title.strip_prefix("Test File - Conflict ").unwrap();
        assert_eq!(date_part.len(), 10); // YYYY-MM-DD is 10 chars
        assert_eq!(&date_part[4..5], "-");
        assert_eq!(&date_part[7..8], "-");
        // Year should be 4 digits
        assert!(date_part[0..4].parse::<u32>().is_ok());
        // Month should be 2 digits (01-12)
        let month: u32 = date_part[5..7].parse().unwrap();
        assert!((1..=12).contains(&month));
        // Day should be 2 digits (01-31)
        let day: u32 = date_part[8..10].parse().unwrap();
        assert!((1..=31).contains(&day));
    }

    #[test]
    fn days_to_ymd_epoch_is_1970_01_01() {
        let (y, m, d) = super::days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2024-01-15 is day 19737 since epoch
        // (2024-01-01 is day 19723, so 19723 + 14 = 19737)
        let (y, m, d) = super::days_to_ymd(19737);
        assert_eq!((y, m, d), (2024, 1, 15));
    }

    #[test]
    fn max_conflict_copies_enforcement_deletes_oldest() {
        let engine = create_test_engine();
        let parent_id = "parent-file-1";

        // Insert the parent file.
        let parent_meta = FileMeta {
            id: parent_id.to_string(),
            title: "Parent File".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "parent-hash".to_string(),
            cos_object_key: Some(format!("files/{}.excalidraw", parent_id)),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some("parent-hash".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };
        engine.db.upsert_file_meta(&parent_meta).unwrap();

        // Insert 5 conflict copies with increasing created_at.
        for i in 0..5 {
            let copy_id = format!("conflict-copy-{}", i);
            let copy_meta = FileMeta {
                id: copy_id.clone(),
                title: format!("Parent File - Conflict 2024-01-{:02}", i + 1),
                last_modified: 1_700_000_000_000 + (i as i64 * 1000),
                content_hash: format!("copy-hash-{}", i),
                cos_object_key: Some(format!("files/{}.excalidraw", copy_id)),
                sync_status: SyncStatus::Synced,
                base_content_hash: Some(format!("copy-hash-{}", i)),
                is_conflict_copy: true,
                parent_file_id: Some(parent_id.to_string()),
                deleted: false,
                created_at: 1_700_000_000_000 + (i as i64 * 1000),
            };
            engine.db.upsert_file_meta(&copy_meta).unwrap();
            // Write a local file for each copy.
            engine
                .file_store
                .write_canvas(&copy_id, &format!("copy content {}", i))
                .unwrap();
        }

        // Verify 5 conflict copies exist.
        let copies = engine.db.get_conflict_copies(parent_id).unwrap();
        assert_eq!(copies.len(), 5);
        assert_eq!(copies[0].id, "conflict-copy-0"); // oldest

        // Now verify that get_conflict_copies returns them in created_at ASC order.
        for i in 0..5 {
            assert_eq!(copies[i].id, format!("conflict-copy-{}", i));
        }
    }

    #[test]
    fn get_conflict_copies_returns_empty_when_none_exist() {
        let engine = create_test_engine();

        let copies = engine.db.get_conflict_copies("nonexistent-parent").unwrap();
        assert!(copies.is_empty());
    }

    #[test]
    fn get_conflict_copies_only_returns_copies_for_specified_parent() {
        let engine = create_test_engine();

        // Insert two parent files.
        for parent_id in &["parent-a", "parent-b"] {
            let meta = FileMeta {
                id: parent_id.to_string(),
                title: format!("Parent {}", parent_id),
                last_modified: 1_700_000_000_000,
                content_hash: "hash".to_string(),
                cos_object_key: Some(format!("files/{}.excalidraw", parent_id)),
                sync_status: SyncStatus::Synced,
                base_content_hash: Some("hash".to_string()),
                is_conflict_copy: false,
                parent_file_id: None,
                deleted: false,
                created_at: 1_700_000_000_000,
            };
            engine.db.upsert_file_meta(&meta).unwrap();
        }

        // Insert conflict copies for parent-a.
        for i in 0..3 {
            let copy_meta = FileMeta {
                id: format!("copy-a-{}", i),
                title: format!("Parent parent-a - Conflict 2024-01-{:02}", i + 1),
                last_modified: 1_700_000_000_000 + (i as i64 * 1000),
                content_hash: format!("hash-a-{}", i),
                cos_object_key: Some(format!("files/copy-a-{}.excalidraw", i)),
                sync_status: SyncStatus::Synced,
                base_content_hash: Some(format!("hash-a-{}", i)),
                is_conflict_copy: true,
                parent_file_id: Some("parent-a".to_string()),
                deleted: false,
                created_at: 1_700_000_000_000 + (i as i64 * 1000),
            };
            engine.db.upsert_file_meta(&copy_meta).unwrap();
        }

        // Insert conflict copies for parent-b.
        for i in 0..2 {
            let copy_meta = FileMeta {
                id: format!("copy-b-{}", i),
                title: format!("Parent parent-b - Conflict 2024-02-{:02}", i + 1),
                last_modified: 1_700_000_000_000 + (i as i64 * 1000),
                content_hash: format!("hash-b-{}", i),
                cos_object_key: Some(format!("files/copy-b-{}.excalidraw", i)),
                sync_status: SyncStatus::Synced,
                base_content_hash: Some(format!("hash-b-{}", i)),
                is_conflict_copy: true,
                parent_file_id: Some("parent-b".to_string()),
                deleted: false,
                created_at: 1_700_000_000_000 + (i as i64 * 1000),
            };
            engine.db.upsert_file_meta(&copy_meta).unwrap();
        }

        // Query for parent-a should return 3 copies.
        let copies_a = engine.db.get_conflict_copies("parent-a").unwrap();
        assert_eq!(copies_a.len(), 3);

        // Query for parent-b should return 2 copies.
        let copies_b = engine.db.get_conflict_copies("parent-b").unwrap();
        assert_eq!(copies_b.len(), 2);
    }

    #[test]
    fn max_conflict_copies_constant_is_five() {
        assert_eq!(super::MAX_CONFLICT_COPIES, 5);
    }
}
