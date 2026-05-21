//! Local filesystem store for `.excalidraw` canvas files.
//!
//! Provides read/write/delete operations for canvas JSON data and
//! SHA-256 content hashing used by the sync engine to detect changes.

use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::PathBuf;

/// Manages local `.excalidraw` files within a base directory.
pub struct FileStore {
    base_dir: PathBuf,
}

impl FileStore {
    /// Creates a new `FileStore` rooted at `base_dir`.
    ///
    /// The directory is created (including parents) if it does not exist.
    pub fn new(base_dir: impl Into<PathBuf>) -> io::Result<Self> {
        let base_dir = base_dir.into();
        fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    /// Writes canvas data as a `.excalidraw` JSON file.
    ///
    /// `data` should be a valid JSON string representing the canvas state.
    pub fn write_canvas(&self, file_id: &str, data: &str) -> io::Result<()> {
        let path = self.file_path(file_id);
        fs::write(&path, data)
    }

    /// Reads a previously stored `.excalidraw` JSON file and returns its
    /// contents as a string.
    ///
    /// Returns an `io::Error` with `ErrorKind::NotFound` if the file does
    /// not exist.
    pub fn read_canvas(&self, file_id: &str) -> io::Result<String> {
        let path = self.file_path(file_id);
        fs::read_to_string(&path)
    }

    /// Deletes a local `.excalidraw` file.
    ///
    /// Returns `Ok(())` even if the file does not exist (idempotent).
    pub fn delete_canvas(&self, file_id: &str) -> io::Result<()> {
        let path = self.file_path(file_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Returns the resolved filesystem path for a given file ID.
    fn file_path(&self, file_id: &str) -> PathBuf {
        self.base_dir.join(format!("{}.excalidraw", file_id))
    }
}

/// Computes the SHA-256 hash of canvas data and returns it as a lowercase
/// hex string.
///
/// This is a standalone function so the sync engine can compute hashes
/// without needing a `FileStore` instance.
pub fn compute_content_hash(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temporary directory for tests.
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("file_store_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_and_read_canvas_round_trip() {
        let dir = temp_dir();
        let store = FileStore::new(&dir).unwrap();

        let data = r#"{"type":"excalidraw","version":2,"elements":[]}"#;
        store.write_canvas("test-file-1", data).unwrap();

        let loaded = store.read_canvas("test-file-1").unwrap();
        assert_eq!(loaded, data);

        // Cleanup
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_nonexistent_file_returns_not_found() {
        let dir = temp_dir();
        let store = FileStore::new(&dir).unwrap();

        let result = store.read_canvas("does-not-exist");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_canvas_removes_file() {
        let dir = temp_dir();
        let store = FileStore::new(&dir).unwrap();

        store.write_canvas("to-delete", "{}").unwrap();
        assert!(store.file_path("to-delete").exists());

        store.delete_canvas("to-delete").unwrap();
        assert!(!store.file_path("to-delete").exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_nonexistent_file_is_idempotent() {
        let dir = temp_dir();
        let store = FileStore::new(&dir).unwrap();

        // Should not error even though the file never existed.
        store.delete_canvas("never-existed").unwrap();

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overwrite_existing_file() {
        let dir = temp_dir();
        let store = FileStore::new(&dir).unwrap();

        store.write_canvas("overwrite-me", "v1").unwrap();
        store.write_canvas("overwrite-me", "v2").unwrap();

        let loaded = store.read_canvas("overwrite-me").unwrap();
        assert_eq!(loaded, "v2");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compute_content_hash_deterministic() {
        let data = r#"{"type":"excalidraw","version":2,"elements":[{"id":"abc"}]}"#;
        let hash1 = compute_content_hash(data);
        let hash2 = compute_content_hash(data);
        assert_eq!(hash1, hash2);
        // SHA-256 produces 64 hex chars.
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn compute_content_hash_different_data_different_hash() {
        let hash_a = compute_content_hash("hello");
        let hash_b = compute_content_hash("world");
        assert_ne!(hash_a, hash_b);
    }
}
