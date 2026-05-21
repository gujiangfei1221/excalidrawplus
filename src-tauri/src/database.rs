//! SQLite-backed metadata store for the cloud-sync desktop app.
//!
//! This module owns the connection to the local SQLite database that
//! persists file metadata, the durable upload queue, and the COS
//! configuration. The schema mirrors the "SQLite Schema" section of the
//! `cloud-sync-desktop` design document.
//!
//! Task 2.1 establishes the database file and schema. The full CRUD
//! surface defined on `Database` (see design.md) is implemented in
//! follow-up tasks 2.2 (file metadata), 2.3 (upload queue), and 2.4
//! (COS config persistence).
//!
//! Validates: Requirements 7.1 — local metadata stored in a SQLite
//! database in the application data directory.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, Result};

use crate::models::{CosConfig, FileMeta, QueuedUpload, SyncStatus, UploadOperation};

/// SQL statements that create the schema described in the design
/// document. The statements are idempotent (`IF NOT EXISTS`) so
/// re-opening an existing database is a no-op.
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS files (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT 'Untitled',
    last_modified INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    cos_object_key TEXT,
    sync_status TEXT NOT NULL DEFAULT 'pending-sync',
    base_content_hash TEXT,
    is_conflict_copy INTEGER NOT NULL DEFAULT 0,
    parent_file_id TEXT,
    deleted INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS upload_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    payload TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 5,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (file_id) REFERENCES files(id)
);

CREATE TABLE IF NOT EXISTS cos_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    secret_id TEXT NOT NULL,
    secret_key TEXT NOT NULL,
    bucket TEXT NOT NULL,
    region TEXT NOT NULL,
    validated INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_files_last_modified
    ON files(last_modified DESC);

CREATE INDEX IF NOT EXISTS idx_files_sync_status
    ON files(sync_status);

CREATE INDEX IF NOT EXISTS idx_upload_queue_created
    ON upload_queue(created_at ASC);
"#;

/// Owns the SQLite connection used by the sync engine and command layer.
///
/// All persistence operations defined in the design document are
/// implemented as methods on this struct. Task 2.1 only creates the
/// database file and schema; the CRUD methods (`upsert_file_meta`,
/// `enqueue_upload`, `save_cos_config`, etc.) are added in tasks
/// 2.2-2.4.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open (or create) the SQLite database at `path` and ensure the
    /// schema is in place.
    ///
    /// The parent directory of `path` is created if it does not yet
    /// exist so callers can pass a path inside the Tauri app data
    /// directory directly without pre-creating folders.
    ///
    /// Foreign-key enforcement is enabled per-connection because
    /// SQLite defaults to `OFF`. Without this, the
    /// `upload_queue.file_id -> files.id` constraint would not be
    /// enforced.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|err| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                        Some(format!(
                            "failed to create database directory {}: {}",
                            parent.display(),
                            err
                        )),
                    )
                })?;
            }
        }

        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA_SQL)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock_conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|err| {
            rusqlite::Error::InvalidParameterName(format!(
                "database connection lock poisoned: {err}"
            ))
        })
    }

    /// Borrow the underlying connection.
    ///
    /// Exposed at crate visibility so the CRUD methods added in tasks
    /// 2.2-2.4 (and their tests) can issue prepared statements without
    /// needing to declare them as methods on `Database` ahead of time.
    #[allow(dead_code)]
    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("database connection lock poisoned")
    }

    /// Persist a COS configuration using the single-row pattern (id=1).
    ///
    /// Uses INSERT OR REPLACE so the first call inserts and subsequent
    /// calls overwrite the existing row. The `validated` column is set
    /// to 0 (false) on save — callers should validate separately after
    /// persisting.
    ///
    /// Validates: Requirement 2.2 (persist config), 2.5 (overwrite on update).
    pub fn save_cos_config(&self, config: &CosConfig) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO cos_config (id, secret_id, secret_key, bucket, region, validated) \
             VALUES (1, ?1, ?2, ?3, ?4, 0)",
            params![config.secret_id, config.secret_key, config.bucket, config.region],
        )?;
        Ok(())
    }

    /// Load the persisted COS configuration, if any.
    ///
    /// Returns `None` when the `cos_config` table is empty (i.e., the
    /// user has never submitted a configuration).
    ///
    /// Validates: Requirement 2.2, 2.6 (check for persisted config on launch).
    pub fn get_cos_config(&self) -> Result<Option<CosConfig>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT secret_id, secret_key, bucket, region FROM cos_config WHERE id = 1",
        )?;

        let mut rows = stmt.query([])?;
        match rows.next()? {
            Some(row) => Ok(Some(CosConfig {
                secret_id: row.get(0)?,
                secret_key: row.get(1)?,
                bucket: row.get(2)?,
                region: row.get(3)?,
            })),
            None => Ok(None),
        }
    }

    // --- File Metadata CRUD (Task 2.2) ---

    /// Insert or update file metadata in the `files` table.
    ///
    /// Uses SQLite `INSERT OR REPLACE` so that inserting a `FileMeta`
    /// whose `id` already exists overwrites the previous row entirely.
    /// This is the primary write path for both initial file creation and
    /// subsequent metadata updates (sync status changes, title renames,
    /// hash updates, etc.).
    ///
    /// `SyncStatus` is stored as its kebab-case text representation
    /// (e.g. `"pending-sync"`) so that ad-hoc queries against the
    /// database remain human-readable.
    ///
    /// Validates: Requirements 7.1, 5.1.
    pub fn upsert_file_meta(&self, meta: &FileMeta) -> Result<()> {
        let sync_status_text = sync_status_to_text(&meta.sync_status);
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO files \
             (id, title, last_modified, content_hash, cos_object_key, sync_status, \
              base_content_hash, is_conflict_copy, parent_file_id, deleted, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                meta.id,
                meta.title,
                meta.last_modified,
                meta.content_hash,
                meta.cos_object_key,
                sync_status_text,
                meta.base_content_hash,
                meta.is_conflict_copy as i32,
                meta.parent_file_id,
                meta.deleted as i32,
                meta.created_at,
            ],
        )?;
        Ok(())
    }

    /// Retrieve file metadata by ID.
    ///
    /// Returns `None` if no row with the given `file_id` exists in the
    /// `files` table.
    ///
    /// Validates: Requirement 7.1.
    pub fn get_file_meta(&self, file_id: &str) -> Result<Option<FileMeta>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, last_modified, content_hash, cos_object_key, sync_status, \
             base_content_hash, is_conflict_copy, parent_file_id, deleted, created_at \
             FROM files WHERE id = ?1",
        )?;

        let mut rows = stmt.query(params![file_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_file_meta(row)?)),
            None => Ok(None),
        }
    }

    /// Return all file metadata entries sorted by `last_modified DESC`.
    ///
    /// This powers the File List Sidebar which displays files in
    /// reverse-chronological order.
    ///
    /// Validates: Requirements 7.1, 5.1.
    pub fn get_all_files(&self) -> Result<Vec<FileMeta>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, last_modified, content_hash, cos_object_key, sync_status, \
             base_content_hash, is_conflict_copy, parent_file_id, deleted, created_at \
             FROM files ORDER BY last_modified DESC",
        )?;

        let rows = stmt.query_map([], |row| row_to_file_meta(row))?;

        let mut files = Vec::new();
        for row_result in rows {
            files.push(row_result?);
        }
        Ok(files)
    }

    /// Delete a file metadata entry by ID.
    ///
    /// This performs a hard delete (removes the row). For soft-delete
    /// semantics (marking `deleted = true`), use `upsert_file_meta`
    /// with the `deleted` flag set.
    ///
    /// Returns `Ok(())` even if the row did not exist (idempotent).
    ///
    /// Validates: Requirement 7.1.
    pub fn delete_file_meta(&self, file_id: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM files WHERE id = ?1",
            params![file_id],
        )?;
        Ok(())
    }

    // ── Upload Queue Operations (Task 2.3) ───────────────────────────

    /// Insert a new entry into the durable upload queue.
    ///
    /// The `entry.id` field is ignored on insert — SQLite assigns an
    /// auto-incremented primary key. The `operation` field is
    /// serialized as a lowercase string ("upload", "delete", "rename")
    /// matching the values stored in the `upload_queue.operation`
    /// column.
    ///
    /// Validates: Requirement 7.3 — changes are persisted in a durable
    /// upload queue stored in the SQLite database so that queued
    /// changes survive application restarts.
    pub fn enqueue_upload(&self, entry: &QueuedUpload) -> Result<()> {
        let operation_str = match entry.operation {
            UploadOperation::Upload => "upload",
            UploadOperation::Delete => "delete",
            UploadOperation::Rename => "rename",
        };

        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO upload_queue (file_id, operation, payload, retry_count, max_retries, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.file_id,
                operation_str,
                entry.payload,
                entry.retry_count,
                entry.max_retries,
                entry.created_at,
            ],
        )?;
        Ok(())
    }

    /// Remove all upload queue entries for a given `file_id`.
    ///
    /// This is called after a successful upload to clear the file's
    /// pending operations from the queue.
    ///
    /// Validates: Requirement 7.4 — once uploaded, queued entries are
    /// removed so they are not re-processed.
    pub fn dequeue_upload(&self, file_id: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM upload_queue WHERE file_id = ?1",
            params![file_id],
        )?;
        Ok(())
    }

    /// Increment the `retry_count` for a specific upload queue entry by ID.
    ///
    /// This is called by the sync engine after a failed upload attempt
    /// to track how many retries have been consumed for the entry.
    ///
    /// Validates: Requirement 3.5 — retry up to 5 times on failure.
    pub fn increment_retry_count(&self, queue_id: i64) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE upload_queue SET retry_count = retry_count + 1 WHERE id = ?1",
            params![queue_id],
        )?;
        Ok(())
    }

    /// Retrieve all pending upload queue entries, ordered by
    /// `created_at ASC` (oldest first).
    ///
    /// The chronological ordering guarantees that the sync engine
    /// processes changes in the order they were made, preserving
    /// causal consistency.
    ///
    /// Validates: Requirement 7.4 — upload queued changes to COS in
    /// chronological order.
    pub fn get_pending_uploads(&self) -> Result<Vec<QueuedUpload>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, file_id, operation, payload, retry_count, max_retries, created_at \
             FROM upload_queue ORDER BY created_at ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            let op_str: String = row.get(2)?;
            let operation = match op_str.as_str() {
                "upload" => UploadOperation::Upload,
                "delete" => UploadOperation::Delete,
                "rename" => UploadOperation::Rename,
                _ => UploadOperation::Upload, // fallback; schema enforces valid values
            };

            Ok(QueuedUpload {
                id: row.get(0)?,
                file_id: row.get(1)?,
                operation,
                payload: row.get(3)?,
                retry_count: row.get(4)?,
                max_retries: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;

        rows.collect()
    }

    // ── Conflict Copy Queries (Task 8.2) ─────────────────────────────

    /// Retrieve all conflict copies for a given parent file, ordered by
    /// `created_at ASC` (oldest first).
    ///
    /// A conflict copy is identified by `is_conflict_copy = 1` AND
    /// `parent_file_id = <file_id>`.
    ///
    /// Validates: Requirement 8.7 — maximum 5 conflict copies per file.
    pub fn get_conflict_copies(&self, parent_file_id: &str) -> Result<Vec<FileMeta>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, last_modified, content_hash, cos_object_key, sync_status, \
             base_content_hash, is_conflict_copy, parent_file_id, deleted, created_at \
             FROM files \
             WHERE is_conflict_copy = 1 AND parent_file_id = ?1 \
             ORDER BY created_at ASC",
        )?;

        let rows = stmt.query_map(params![parent_file_id], |row| row_to_file_meta(row))?;

        let mut files = Vec::new();
        for row_result in rows {
            files.push(row_result?);
        }
        Ok(files)
    }
}

/// Convert a `SyncStatus` enum variant to the kebab-case text stored in
/// the SQLite `sync_status` column.
fn sync_status_to_text(status: &SyncStatus) -> &'static str {
    match status {
        SyncStatus::Synced => "synced",
        SyncStatus::PendingSync => "pending-sync",
        SyncStatus::Saving => "saving",
        SyncStatus::Conflict => "conflict",
        SyncStatus::Error => "error",
    }
}

/// Parse a kebab-case text value from the SQLite `sync_status` column
/// into a `SyncStatus` enum variant. Defaults to `PendingSync` for
/// unrecognized values to avoid crashing on unexpected data.
fn text_to_sync_status(text: &str) -> SyncStatus {
    match text {
        "synced" => SyncStatus::Synced,
        "pending-sync" => SyncStatus::PendingSync,
        "saving" => SyncStatus::Saving,
        "conflict" => SyncStatus::Conflict,
        "error" => SyncStatus::Error,
        _ => SyncStatus::PendingSync,
    }
}

/// Map a SQLite row (from the `files` table) into a `FileMeta` struct.
fn row_to_file_meta(row: &rusqlite::Row) -> rusqlite::Result<FileMeta> {
    let sync_status_text: String = row.get(5)?;
    let is_conflict_copy: i32 = row.get(7)?;
    let deleted: i32 = row.get(9)?;

    Ok(FileMeta {
        id: row.get(0)?,
        title: row.get(1)?,
        last_modified: row.get(2)?,
        content_hash: row.get(3)?,
        cos_object_key: row.get(4)?,
        sync_status: text_to_sync_status(&sync_status_text),
        base_content_hash: row.get(6)?,
        is_conflict_copy: is_conflict_copy != 0,
        parent_file_id: row.get(8)?,
        deleted: deleted != 0,
        created_at: row.get(10)?,
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a unique temp path for an isolated database file. We
    /// deliberately avoid `:memory:` here because the goal of these
    /// tests is to verify that the on-disk file and schema are
    /// created correctly inside a (possibly nested) directory.
    fn temp_db_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        path.push(format!(
            "excalidraw-cloud-sync-test-{}-{}-{}",
            name,
            std::process::id(),
            nanos
        ));
        path
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn open_creates_database_file_and_parent_directories() {
        let dir = temp_db_path("create_parent");
        let db_path = dir.join("nested").join("metadata.sqlite");

        let _db = Database::open(&db_path).expect("open should succeed");

        assert!(db_path.exists(), "database file should exist on disk");

        cleanup(&dir);
    }

    #[test]
    fn open_creates_all_required_tables_and_indexes() {
        let dir = temp_db_path("schema");
        let db_path = dir.join("metadata.sqlite");

        let db = Database::open(&db_path).expect("open should succeed");

        let tables: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();

        assert!(tables.contains(&"files".to_string()));
        assert!(tables.contains(&"upload_queue".to_string()));
        assert!(tables.contains(&"cos_config".to_string()));

        let indexes: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();

        assert!(indexes.contains(&"idx_files_last_modified".to_string()));
        assert!(indexes.contains(&"idx_files_sync_status".to_string()));
        assert!(indexes.contains(&"idx_upload_queue_created".to_string()));

        cleanup(&dir);
    }

    #[test]
    fn open_is_idempotent_when_database_already_exists() {
        let dir = temp_db_path("idempotent");
        let db_path = dir.join("metadata.sqlite");

        {
            let db = Database::open(&db_path).expect("first open should succeed");
            db.conn()
                .execute(
                    "INSERT INTO files (id, last_modified, content_hash, created_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params!["file-1", 1_700_000_000_000_i64, "hash-1", 1_700_000_000_000_i64],
                )
                .unwrap();
        }

        let db = Database::open(&db_path).expect("re-open should succeed");
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 1, "existing data must survive re-opening");

        cleanup(&dir);
    }

    #[test]
    fn open_enables_foreign_key_enforcement() {
        let dir = temp_db_path("foreign_keys");
        let db_path = dir.join("metadata.sqlite");

        let db = Database::open(&db_path).expect("open should succeed");

        let foreign_keys_on: i64 = db
            .conn()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys_on, 1, "foreign key enforcement must be enabled");

        // Inserting into upload_queue with a file_id that does not
        // exist in `files` must fail when foreign keys are enforced.
        let result = db.conn().execute(
            "INSERT INTO upload_queue (file_id, operation, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params!["does-not-exist", "upload", 1_700_000_000_000_i64],
        );
        assert!(result.is_err(), "foreign key violation should be rejected");

        cleanup(&dir);
    }

    #[test]
    fn get_cos_config_returns_none_when_empty() {
        let dir = temp_db_path("cos_config_empty");
        let db_path = dir.join("metadata.sqlite");

        let db = Database::open(&db_path).expect("open should succeed");
        let config = db.get_cos_config().expect("get_cos_config should succeed");

        assert!(config.is_none(), "should return None when no config has been saved");

        cleanup(&dir);
    }

    #[test]
    fn save_and_get_cos_config_round_trip() {
        let dir = temp_db_path("cos_config_roundtrip");
        let db_path = dir.join("metadata.sqlite");

        let db = Database::open(&db_path).expect("open should succeed");

        let config = CosConfig {
            secret_id: "AKID-test-123".to_string(),
            secret_key: "secret-key-abc".to_string(),
            bucket: "my-bucket-1250000000".to_string(),
            region: "ap-guangzhou".to_string(),
        };

        db.save_cos_config(&config).expect("save_cos_config should succeed");

        let loaded = db
            .get_cos_config()
            .expect("get_cos_config should succeed")
            .expect("config should be Some after saving");

        assert_eq!(loaded.secret_id, config.secret_id);
        assert_eq!(loaded.secret_key, config.secret_key);
        assert_eq!(loaded.bucket, config.bucket);
        assert_eq!(loaded.region, config.region);

        cleanup(&dir);
    }

    #[test]
    fn save_cos_config_overwrites_existing() {
        let dir = temp_db_path("cos_config_overwrite");
        let db_path = dir.join("metadata.sqlite");

        let db = Database::open(&db_path).expect("open should succeed");

        let config1 = CosConfig {
            secret_id: "first-id".to_string(),
            secret_key: "first-key".to_string(),
            bucket: "first-bucket".to_string(),
            region: "ap-beijing".to_string(),
        };
        db.save_cos_config(&config1).expect("first save should succeed");

        let config2 = CosConfig {
            secret_id: "second-id".to_string(),
            secret_key: "second-key".to_string(),
            bucket: "second-bucket".to_string(),
            region: "ap-shanghai".to_string(),
        };
        db.save_cos_config(&config2).expect("second save should succeed");

        let loaded = db
            .get_cos_config()
            .expect("get_cos_config should succeed")
            .expect("config should be Some");

        assert_eq!(loaded.secret_id, "second-id");
        assert_eq!(loaded.secret_key, "second-key");
        assert_eq!(loaded.bucket, "second-bucket");
        assert_eq!(loaded.region, "ap-shanghai");

        // Verify only one row exists (single-row pattern)
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM cos_config", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "cos_config table must always have at most one row");

        cleanup(&dir);
    }

    #[test]
    fn save_cos_config_sets_validated_to_zero() {
        let dir = temp_db_path("cos_config_validated");
        let db_path = dir.join("metadata.sqlite");

        let db = Database::open(&db_path).expect("open should succeed");

        let config = CosConfig {
            secret_id: "id".to_string(),
            secret_key: "key".to_string(),
            bucket: "bucket".to_string(),
            region: "region".to_string(),
        };
        db.save_cos_config(&config).expect("save should succeed");

        let validated: i64 = db
            .conn()
            .query_row(
                "SELECT validated FROM cos_config WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(validated, 0, "validated should be 0 after save");

        cleanup(&dir);
    }

    // --- File Metadata CRUD tests (Task 2.2) ---

    /// Helper to create a sample `FileMeta` for testing.
    fn sample_file_meta(id: &str, last_modified: i64) -> FileMeta {
        FileMeta {
            id: id.to_string(),
            title: format!("File {}", id),
            last_modified,
            content_hash: format!("hash-{}", id),
            cos_object_key: Some(format!("files/{}.excalidraw", id)),
            sync_status: SyncStatus::Synced,
            base_content_hash: Some(format!("base-hash-{}", id)),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        }
    }

    #[test]
    fn upsert_and_get_file_meta_round_trip() {
        let dir = temp_db_path("file_meta_roundtrip");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let meta = FileMeta {
            id: "file-abc".to_string(),
            title: "My Drawing".to_string(),
            last_modified: 1_700_000_100_000,
            content_hash: "sha256-deadbeef".to_string(),
            cos_object_key: Some("files/file-abc.excalidraw".to_string()),
            sync_status: SyncStatus::PendingSync,
            base_content_hash: Some("sha256-base".to_string()),
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };

        db.upsert_file_meta(&meta).expect("upsert should succeed");

        let loaded = db
            .get_file_meta("file-abc")
            .expect("get should succeed")
            .expect("file should exist");

        assert_eq!(loaded.id, meta.id);
        assert_eq!(loaded.title, meta.title);
        assert_eq!(loaded.last_modified, meta.last_modified);
        assert_eq!(loaded.content_hash, meta.content_hash);
        assert_eq!(loaded.cos_object_key, meta.cos_object_key);
        assert_eq!(loaded.sync_status, SyncStatus::PendingSync);
        assert_eq!(loaded.base_content_hash, meta.base_content_hash);
        assert_eq!(loaded.is_conflict_copy, false);
        assert_eq!(loaded.parent_file_id, None);
        assert_eq!(loaded.deleted, false);
        assert_eq!(loaded.created_at, meta.created_at);

        cleanup(&dir);
    }

    #[test]
    fn upsert_file_meta_overwrites_existing_entry() {
        let dir = temp_db_path("file_meta_overwrite");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let mut meta = sample_file_meta("file-1", 1_700_000_000_000);
        db.upsert_file_meta(&meta).expect("first upsert should succeed");

        // Update title and sync status
        meta.title = "Renamed Drawing".to_string();
        meta.sync_status = SyncStatus::Conflict;
        meta.last_modified = 1_700_000_200_000;
        db.upsert_file_meta(&meta).expect("second upsert should succeed");

        let loaded = db
            .get_file_meta("file-1")
            .expect("get should succeed")
            .expect("file should exist");

        assert_eq!(loaded.title, "Renamed Drawing");
        assert_eq!(loaded.sync_status, SyncStatus::Conflict);
        assert_eq!(loaded.last_modified, 1_700_000_200_000);

        // Ensure only one row exists
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        cleanup(&dir);
    }

    #[test]
    fn get_file_meta_returns_none_for_nonexistent_id() {
        let dir = temp_db_path("file_meta_none");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let result = db
            .get_file_meta("does-not-exist")
            .expect("get should succeed");

        assert!(result.is_none());

        cleanup(&dir);
    }

    #[test]
    fn get_all_files_returns_empty_vec_when_no_files() {
        let dir = temp_db_path("file_meta_empty_list");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let files = db.get_all_files().expect("get_all_files should succeed");
        assert!(files.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn get_all_files_returns_entries_sorted_by_last_modified_desc() {
        let dir = temp_db_path("file_meta_sort");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        // Insert files with different last_modified timestamps (not in order)
        let meta_old = sample_file_meta("file-old", 1_700_000_000_000);
        let meta_mid = sample_file_meta("file-mid", 1_700_000_100_000);
        let meta_new = sample_file_meta("file-new", 1_700_000_200_000);

        db.upsert_file_meta(&meta_mid).unwrap();
        db.upsert_file_meta(&meta_old).unwrap();
        db.upsert_file_meta(&meta_new).unwrap();

        let files = db.get_all_files().expect("get_all_files should succeed");

        assert_eq!(files.len(), 3);
        assert_eq!(files[0].id, "file-new", "newest file should be first");
        assert_eq!(files[1].id, "file-mid", "middle file should be second");
        assert_eq!(files[2].id, "file-old", "oldest file should be last");

        cleanup(&dir);
    }

    #[test]
    fn delete_file_meta_removes_entry() {
        let dir = temp_db_path("file_meta_delete");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let meta = sample_file_meta("file-to-delete", 1_700_000_000_000);
        db.upsert_file_meta(&meta).unwrap();

        // Verify it exists
        assert!(db.get_file_meta("file-to-delete").unwrap().is_some());

        // Delete it
        db.delete_file_meta("file-to-delete")
            .expect("delete should succeed");

        // Verify it no longer exists
        assert!(db.get_file_meta("file-to-delete").unwrap().is_none());

        cleanup(&dir);
    }

    #[test]
    fn delete_file_meta_is_idempotent_for_nonexistent_id() {
        let dir = temp_db_path("file_meta_delete_nonexist");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        // Deleting a non-existent ID should not error
        let result = db.delete_file_meta("no-such-file");
        assert!(result.is_ok());

        cleanup(&dir);
    }

    #[test]
    fn upsert_file_meta_preserves_all_sync_status_variants() {
        let dir = temp_db_path("file_meta_sync_status");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let statuses = vec![
            ("s1", SyncStatus::Synced),
            ("s2", SyncStatus::PendingSync),
            ("s3", SyncStatus::Saving),
            ("s4", SyncStatus::Conflict),
            ("s5", SyncStatus::Error),
        ];

        for (id, status) in &statuses {
            let mut meta = sample_file_meta(id, 1_700_000_000_000);
            meta.sync_status = status.clone();
            db.upsert_file_meta(&meta).unwrap();
        }

        for (id, expected_status) in &statuses {
            let loaded = db.get_file_meta(id).unwrap().unwrap();
            assert_eq!(&loaded.sync_status, expected_status, "sync status mismatch for {}", id);
        }

        cleanup(&dir);
    }

    #[test]
    fn upsert_file_meta_handles_optional_fields_as_none() {
        let dir = temp_db_path("file_meta_nulls");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let meta = FileMeta {
            id: "file-nulls".to_string(),
            title: "Untitled".to_string(),
            last_modified: 1_700_000_000_000,
            content_hash: "hash-123".to_string(),
            cos_object_key: None,
            sync_status: SyncStatus::PendingSync,
            base_content_hash: None,
            is_conflict_copy: false,
            parent_file_id: None,
            deleted: false,
            created_at: 1_700_000_000_000,
        };

        db.upsert_file_meta(&meta).unwrap();

        let loaded = db.get_file_meta("file-nulls").unwrap().unwrap();
        assert_eq!(loaded.cos_object_key, None);
        assert_eq!(loaded.base_content_hash, None);
        assert_eq!(loaded.parent_file_id, None);

        cleanup(&dir);
    }

    #[test]
    fn upsert_file_meta_handles_conflict_copy_fields() {
        let dir = temp_db_path("file_meta_conflict");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        // Insert the parent file first
        let parent = sample_file_meta("parent-file", 1_700_000_000_000);
        db.upsert_file_meta(&parent).unwrap();

        let conflict_copy = FileMeta {
            id: "conflict-copy-1".to_string(),
            title: "My Drawing - Conflict 2024-01-15".to_string(),
            last_modified: 1_700_000_100_000,
            content_hash: "hash-conflict".to_string(),
            cos_object_key: Some("files/conflict-copy-1.excalidraw".to_string()),
            sync_status: SyncStatus::Synced,
            base_content_hash: None,
            is_conflict_copy: true,
            parent_file_id: Some("parent-file".to_string()),
            deleted: false,
            created_at: 1_700_000_100_000,
        };

        db.upsert_file_meta(&conflict_copy).unwrap();

        let loaded = db.get_file_meta("conflict-copy-1").unwrap().unwrap();
        assert_eq!(loaded.is_conflict_copy, true);
        assert_eq!(loaded.parent_file_id, Some("parent-file".to_string()));

        cleanup(&dir);
    }

    #[test]
    fn upsert_file_meta_handles_deleted_flag() {
        let dir = temp_db_path("file_meta_deleted");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let mut meta = sample_file_meta("soft-deleted", 1_700_000_000_000);
        meta.deleted = true;
        db.upsert_file_meta(&meta).unwrap();

        let loaded = db.get_file_meta("soft-deleted").unwrap().unwrap();
        assert_eq!(loaded.deleted, true);

        cleanup(&dir);
    }

    // --- Upload Queue tests (Task 2.3) ---

    /// Helper: insert a file row so that FK constraints are satisfied
    /// when enqueuing uploads.
    fn insert_file_for_queue(db: &Database, file_id: &str) {
        let meta = sample_file_meta(file_id, 1_700_000_000_000);
        db.upsert_file_meta(&meta).unwrap();
    }

    #[test]
    fn enqueue_and_get_pending_uploads_round_trip() {
        let dir = temp_db_path("queue_roundtrip");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        insert_file_for_queue(&db, "file-1");

        let entry = QueuedUpload {
            id: 0, // ignored on insert
            file_id: "file-1".to_string(),
            operation: UploadOperation::Upload,
            payload: Some(r#"{"key":"value"}"#.to_string()),
            retry_count: 0,
            max_retries: 5,
            created_at: 1_700_000_001_000,
        };

        db.enqueue_upload(&entry).expect("enqueue should succeed");

        let pending = db.get_pending_uploads().expect("get_pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].file_id, "file-1");
        assert_eq!(pending[0].payload, Some(r#"{"key":"value"}"#.to_string()));
        assert_eq!(pending[0].retry_count, 0);
        assert_eq!(pending[0].max_retries, 5);
        assert_eq!(pending[0].created_at, 1_700_000_001_000);

        // Verify the auto-assigned ID is positive
        assert!(pending[0].id > 0);

        cleanup(&dir);
    }

    #[test]
    fn enqueue_upload_stores_all_operation_types() {
        let dir = temp_db_path("queue_operations");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        insert_file_for_queue(&db, "file-ops");

        let operations = vec![
            UploadOperation::Upload,
            UploadOperation::Delete,
            UploadOperation::Rename,
        ];

        for (i, op) in operations.iter().enumerate() {
            let entry = QueuedUpload {
                id: 0,
                file_id: "file-ops".to_string(),
                operation: op.clone(),
                payload: None,
                retry_count: 0,
                max_retries: 5,
                created_at: 1_700_000_000_000 + i as i64,
            };
            db.enqueue_upload(&entry).unwrap();
        }

        let pending = db.get_pending_uploads().unwrap();
        assert_eq!(pending.len(), 3);

        // Verify operation types are correctly round-tripped
        match &pending[0].operation {
            UploadOperation::Upload => {}
            _ => panic!("expected Upload"),
        }
        match &pending[1].operation {
            UploadOperation::Delete => {}
            _ => panic!("expected Delete"),
        }
        match &pending[2].operation {
            UploadOperation::Rename => {}
            _ => panic!("expected Rename"),
        }

        cleanup(&dir);
    }

    #[test]
    fn get_pending_uploads_returns_entries_ordered_by_created_at_asc() {
        let dir = temp_db_path("queue_order");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        insert_file_for_queue(&db, "file-a");
        insert_file_for_queue(&db, "file-b");
        insert_file_for_queue(&db, "file-c");

        // Insert in non-chronological order
        let entries = vec![
            ("file-b", 1_700_000_200_000_i64),
            ("file-a", 1_700_000_100_000_i64),
            ("file-c", 1_700_000_300_000_i64),
        ];

        for (file_id, created_at) in &entries {
            let entry = QueuedUpload {
                id: 0,
                file_id: file_id.to_string(),
                operation: UploadOperation::Upload,
                payload: None,
                retry_count: 0,
                max_retries: 5,
                created_at: *created_at,
            };
            db.enqueue_upload(&entry).unwrap();
        }

        let pending = db.get_pending_uploads().unwrap();
        assert_eq!(pending.len(), 3);

        // Should be ordered oldest-first (ASC)
        assert_eq!(pending[0].file_id, "file-a");
        assert_eq!(pending[0].created_at, 1_700_000_100_000);
        assert_eq!(pending[1].file_id, "file-b");
        assert_eq!(pending[1].created_at, 1_700_000_200_000);
        assert_eq!(pending[2].file_id, "file-c");
        assert_eq!(pending[2].created_at, 1_700_000_300_000);

        cleanup(&dir);
    }

    #[test]
    fn dequeue_upload_removes_all_entries_for_file() {
        let dir = temp_db_path("queue_dequeue");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        insert_file_for_queue(&db, "file-x");
        insert_file_for_queue(&db, "file-y");

        // Enqueue multiple entries for file-x and one for file-y
        for i in 0..3 {
            let entry = QueuedUpload {
                id: 0,
                file_id: "file-x".to_string(),
                operation: UploadOperation::Upload,
                payload: None,
                retry_count: 0,
                max_retries: 5,
                created_at: 1_700_000_000_000 + i,
            };
            db.enqueue_upload(&entry).unwrap();
        }

        let entry_y = QueuedUpload {
            id: 0,
            file_id: "file-y".to_string(),
            operation: UploadOperation::Delete,
            payload: None,
            retry_count: 0,
            max_retries: 5,
            created_at: 1_700_000_010_000,
        };
        db.enqueue_upload(&entry_y).unwrap();

        // Dequeue file-x
        db.dequeue_upload("file-x").expect("dequeue should succeed");

        let pending = db.get_pending_uploads().unwrap();
        assert_eq!(pending.len(), 1, "only file-y entry should remain");
        assert_eq!(pending[0].file_id, "file-y");

        cleanup(&dir);
    }

    #[test]
    fn dequeue_upload_is_idempotent_for_nonexistent_file() {
        let dir = temp_db_path("queue_dequeue_nonexist");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        // Dequeuing a file_id with no entries should not error
        let result = db.dequeue_upload("no-such-file");
        assert!(result.is_ok());

        cleanup(&dir);
    }

    #[test]
    fn get_pending_uploads_returns_empty_vec_when_queue_is_empty() {
        let dir = temp_db_path("queue_empty");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        let pending = db.get_pending_uploads().expect("get_pending should succeed");
        assert!(pending.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn enqueue_upload_with_retry_count_and_payload() {
        let dir = temp_db_path("queue_retry_payload");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        insert_file_for_queue(&db, "file-retry");

        let entry = QueuedUpload {
            id: 0,
            file_id: "file-retry".to_string(),
            operation: UploadOperation::Rename,
            payload: Some(r#"{"newTitle":"Renamed"}"#.to_string()),
            retry_count: 3,
            max_retries: 5,
            created_at: 1_700_000_050_000,
        };

        db.enqueue_upload(&entry).unwrap();

        let pending = db.get_pending_uploads().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].retry_count, 3);
        assert_eq!(pending[0].max_retries, 5);
        assert_eq!(
            pending[0].payload,
            Some(r#"{"newTitle":"Renamed"}"#.to_string())
        );

        cleanup(&dir);
    }

    #[test]
    fn enqueue_upload_with_null_payload() {
        let dir = temp_db_path("queue_null_payload");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        insert_file_for_queue(&db, "file-null");

        let entry = QueuedUpload {
            id: 0,
            file_id: "file-null".to_string(),
            operation: UploadOperation::Upload,
            payload: None,
            retry_count: 0,
            max_retries: 5,
            created_at: 1_700_000_000_000,
        };

        db.enqueue_upload(&entry).unwrap();

        let pending = db.get_pending_uploads().unwrap();
        assert_eq!(pending[0].payload, None);

        cleanup(&dir);
    }

    #[test]
    fn enqueue_upload_rejects_invalid_foreign_key() {
        let dir = temp_db_path("queue_fk_violation");
        let db_path = dir.join("metadata.sqlite");
        let db = Database::open(&db_path).expect("open should succeed");

        // Do NOT insert a file — the FK constraint should reject this
        let entry = QueuedUpload {
            id: 0,
            file_id: "nonexistent-file".to_string(),
            operation: UploadOperation::Upload,
            payload: None,
            retry_count: 0,
            max_retries: 5,
            created_at: 1_700_000_000_000,
        };

        let result = db.enqueue_upload(&entry);
        assert!(result.is_err(), "FK violation should be rejected");

        cleanup(&dir);
    }
}
