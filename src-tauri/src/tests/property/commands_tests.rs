//! Property-based tests for command-layer invariants.
//!
//! **Validates: Requirements 3.3, 6.2, 9.2**
//!
//! Property 3: Manifest Serialization Completeness
//! Property 19: New File Uniqueness

use proptest::prelude::*;
use std::collections::HashSet;

use crate::models::{Manifest, ManifestEntry};

fn hash_strategy() -> impl Strategy<Value = String> {
    "[a-f0-9]{64}"
}

fn manifest_entry_strategy() -> impl Strategy<Value = ManifestEntry> {
    (
        "[a-f0-9-]{36}",
        "[A-Za-z0-9 _-]{1,100}",
        0i64..2_000_000_000_000i64,
        hash_strategy(),
        any::<bool>(),
    )
        .prop_map(|(id, title, last_modified, content_hash, deleted)| ManifestEntry {
            object_key: format!("files/{}.excalidraw", id),
            id,
            title,
            last_modified,
            content_hash,
            deleted,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn manifest_serialization_completeness(entries in proptest::collection::vec(manifest_entry_strategy(), 0..20)) {
        let manifest = Manifest {
            version: 1,
            last_modified: entries.iter().map(|entry| entry.last_modified).max().unwrap_or(0),
            files: entries,
        };

        let serialized = serde_json::to_string(&manifest).unwrap();
        let deserialized: Manifest = serde_json::from_str(&serialized).unwrap();

        prop_assert_eq!(deserialized.version, manifest.version);
        prop_assert_eq!(deserialized.last_modified, manifest.last_modified);
        prop_assert_eq!(deserialized.files.len(), manifest.files.len());

        for (actual, expected) in deserialized.files.iter().zip(manifest.files.iter()) {
            prop_assert_eq!(&actual.id, &expected.id);
            prop_assert_eq!(&actual.title, &expected.title);
            prop_assert_eq!(actual.last_modified, expected.last_modified);
            prop_assert_eq!(&actual.content_hash, &expected.content_hash);
            prop_assert_eq!(&actual.object_key, &expected.object_key);
            prop_assert_eq!(actual.deleted, expected.deleted);
        }
    }

    #[test]
    fn new_file_uniqueness(count in 1usize..200) {
        let mut ids = HashSet::new();

        for _ in 0..count {
            let id = uuid::Uuid::new_v4().to_string();
            prop_assert!(ids.insert(id));
        }
    }
}
