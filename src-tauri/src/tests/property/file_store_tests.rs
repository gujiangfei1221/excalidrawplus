//! Property-based tests for the file_store module.
//!
//! **Validates: Requirements 7.2**
//!
//! Property 12: Canvas Data Filesystem Round-Trip
//! For any valid canvas data string, writing it to the file store and reading
//! it back produces an identical result. Also tests that compute_content_hash
//! is deterministic (same input always produces same hash).

use proptest::prelude::*;
use tempfile::TempDir;

use crate::file_store::{compute_content_hash, FileStore};

/// Strategy to generate arbitrary JSON-like canvas data strings.
/// These simulate realistic `.excalidraw` content with varying complexity.
fn canvas_data_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Simple JSON objects
        Just(r#"{"type":"excalidraw","version":2,"elements":[]}"#.to_string()),
        // Generate arbitrary non-empty strings (simulating raw canvas content)
        "[\\x20-\\x7E]{1,2000}".prop_map(|s| s),
        // Generate JSON-like strings with varying element counts
        (1..50u32).prop_map(|n| {
            let elements: Vec<String> = (0..n)
                .map(|i| format!(r#"{{"id":"elem-{}","type":"rectangle","x":{},"y":{}}}"#, i, i * 10, i * 20))
                .collect();
            format!(
                r#"{{"type":"excalidraw","version":2,"elements":[{}]}}"#,
                elements.join(",")
            )
        }),
        // Unicode content
        "\\PC{1,500}".prop_map(|s| format!(r#"{{"type":"excalidraw","data":"{}"}}"#, s.replace('\\', "\\\\").replace('"', "\\\""))),
        // Large content
        "[a-zA-Z0-9 ]{500,5000}".prop_map(|s| s),
    ]
}

/// Strategy to generate valid file IDs (alphanumeric with hyphens, like UUIDs).
fn file_id_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9][a-zA-Z0-9\\-]{1,50}".prop_map(|s| s)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 12: Canvas Data Filesystem Round-Trip
    ///
    /// **Validates: Requirements 7.2**
    ///
    /// For any valid canvas data string, writing it to the file store and
    /// reading it back SHALL produce an identical result.
    #[test]
    fn canvas_data_round_trip(
        data in canvas_data_strategy(),
        file_id in file_id_strategy(),
    ) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let store = FileStore::new(tmp_dir.path()).expect("failed to create FileStore");

        // Write canvas data
        store.write_canvas(&file_id, &data).expect("write_canvas failed");

        // Read it back
        let loaded = store.read_canvas(&file_id).expect("read_canvas failed");

        // Must be identical
        prop_assert_eq!(&loaded, &data, "Round-trip failed: data written != data read");
    }

    /// Property 12 (hash determinism): compute_content_hash is deterministic
    ///
    /// **Validates: Requirements 7.2**
    ///
    /// For any input string, calling compute_content_hash multiple times
    /// SHALL always produce the same hash value.
    #[test]
    fn content_hash_deterministic(data in "\\PC{0,5000}") {
        let hash1 = compute_content_hash(&data);
        let hash2 = compute_content_hash(&data);
        let hash3 = compute_content_hash(&data);

        prop_assert_eq!(&hash1, &hash2, "Hash not deterministic on second call");
        prop_assert_eq!(&hash2, &hash3, "Hash not deterministic on third call");

        // SHA-256 produces 64 hex characters
        prop_assert_eq!(hash1.len(), 64, "Hash length should be 64 hex chars");

        // Hash should only contain lowercase hex characters
        prop_assert!(
            hash1.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "Hash should be lowercase hex only, got: {}",
            hash1
        );
    }

    /// Additional property: different data produces different hashes (collision resistance)
    ///
    /// **Validates: Requirements 7.2**
    ///
    /// For any two distinct inputs, compute_content_hash SHALL produce
    /// distinct hash values (with overwhelming probability for SHA-256).
    #[test]
    fn content_hash_different_inputs_different_hashes(
        data_a in "\\PC{1,1000}",
        data_b in "\\PC{1,1000}",
    ) {
        prop_assume!(data_a != data_b);

        let hash_a = compute_content_hash(&data_a);
        let hash_b = compute_content_hash(&data_b);

        prop_assert_ne!(
            hash_a, hash_b,
            "Different inputs should produce different hashes"
        );
    }
}
