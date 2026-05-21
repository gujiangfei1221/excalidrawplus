# Implementation Plan: Cloud Sync Desktop

## Overview

This plan implements the Excalidraw cloud-sync desktop application using Tauri v2 (Rust backend) and the existing React frontend. Tasks are organized to build foundational layers first (project structure, data models, SQLite), then core sync logic, then frontend components, and finally integration wiring. Rust backend uses `aws-sdk-s3`, `rusqlite`, and `uuid`. Frontend uses TypeScript with React and `fast-check` for property tests.

## Tasks

- [x] 1. Set up Tauri v2 project structure and core interfaces
  - [x] 1.1 Initialize Tauri v2 project with Rust backend and configure Cargo.toml dependencies
    - Add `tauri`, `serde`, `serde_json`, `rusqlite`, `aws-sdk-s3`, `aws-config`, `sha2`, `uuid`, `tokio` to Cargo.toml
    - Configure `tauri.conf.json` with window settings (min 800x600), app identifier, and build targets for macOS 11+ and Windows 10+
    - Set up the `src-tauri/src/main.rs` entry point with Tauri builder
    - _Requirements: 1.1, 1.2, 1.3_

  - [x] 1.2 Define Rust data structures and enums
    - Create `src-tauri/src/models.rs` with `CosConfig`, `FileMeta`, `ManifestEntry`, `Manifest`, `SyncStatus`, `QueuedUpload`, `UploadOperation`, `Conflict` structs/enums as specified in the design
    - Derive `Serialize`, `Deserialize`, `Clone` for all types
    - _Requirements: 6.2, 3.3_

  - [x] 1.3 Define TypeScript interfaces for frontend
    - Create `excalidraw-app/src/cloud-sync/types.ts` with `FileEntry`, `SyncStatus`, `CosConfig`, `SyncStatusIndicatorProps`, `FileListSidebarProps`, `CosConfigFormProps` interfaces
    - _Requirements: 5.1, 5.3, 2.1_

- [x] 2. Implement SQLite database layer
  - [x] 2.1 Create SQLite database initialization and schema migration
    - Create `src-tauri/src/database.rs` with `Database` struct
    - Implement `open(path)` that creates the database file and runs schema creation (files, upload_queue, cos_config tables with indexes)
    - _Requirements: 7.1_

  - [x] 2.2 Implement file metadata CRUD operations
    - Implement `upsert_file_meta`, `get_file_meta`, `get_all_files`, `delete_file_meta` methods
    - Ensure `get_all_files` returns entries sorted by `last_modified DESC`
    - _Requirements: 7.1, 5.1_

  - [x] 2.3 Implement upload queue operations
    - Implement `enqueue_upload`, `dequeue_upload`, `get_pending_uploads` methods
    - Ensure `get_pending_uploads` returns entries ordered by `created_at ASC`
    - _Requirements: 7.3, 7.4_

  - [x] 2.4 Implement COS config persistence operations
    - Implement `save_cos_config`, `get_cos_config` methods
    - Use the single-row pattern (id=1) for config storage
    - _Requirements: 2.2, 2.5_

  - [x] 2.5 Write property tests for SQLite layer (Rust proptest)
    - **Property 1: COS Configuration Round-Trip**
    - **Property 12: Canvas Data Filesystem Round-Trip** (metadata portion)
    - **Property 13: Offline Queue Persistence**
    - **Validates: Requirements 2.2, 7.2, 7.3**

- [x] 3. Implement local file store
  - [x] 3.1 Create local filesystem file store
    - Create `src-tauri/src/file_store.rs` with `FileStore` struct
    - Implement `write_canvas(file_id, data)` to save `.excalidraw` JSON files to app data directory
    - Implement `read_canvas(file_id)` to load `.excalidraw` JSON files
    - Implement `delete_canvas(file_id)` to remove local files
    - Implement `compute_content_hash(data)` using SHA-256
    - _Requirements: 7.2, 3.1_

  - [x] 3.2 Write property tests for file store (Rust proptest)
    - **Property 12: Canvas Data Filesystem Round-Trip**
    - **Validates: Requirements 7.2**

- [x] 4. Implement COS client
  - [x] 4.1 Create COS client using aws-sdk-s3
    - Create `src-tauri/src/cos_client.rs` with `CosClient` struct
    - Implement `new(config)` that configures the S3 client for Tencent COS endpoint (`https://{bucket}.cos.{region}.myqcloud.com`)
    - Implement `put_object`, `get_object`, `delete_object`, `head_object`, `test_connection` methods
    - _Requirements: 2.3, 3.2, 3.4_

  - [x] 4.2 Write unit tests for COS client
    - Test connection validation logic
    - Test error handling for network failures
    - _Requirements: 2.3, 2.4_

- [x] 5. Implement connectivity monitor
  - [x] 5.1 Create connectivity monitor component
    - Create `src-tauri/src/connectivity.rs` with `ConnectivityMonitor` struct
    - Implement periodic connectivity check (every 30 seconds) by attempting to reach the COS endpoint
    - Expose `is_online()` method and emit connectivity change events
    - _Requirements: 7.7_

- [x] 6. Checkpoint - Core infrastructure
  - Ensure all tests pass, ask the user if questions arise.

- [x] 7. Implement Sync Engine - Core operations
  - [x] 7.1 Create sync engine structure and initialization
    - Create `src-tauri/src/sync_engine.rs` with `SyncEngine` struct
    - Implement `start(app_handle)` to initialize background tasks (manifest polling, queue processing)
    - Implement `stop()` to gracefully shut down background tasks
    - _Requirements: 3.1, 6.6_

  - [x] 7.2 Implement save_canvas with debounce and local persistence
    - Implement `save_canvas(file_id, data)` that writes to local file store, computes content hash, updates SQLite metadata, and enqueues upload
    - Emit sync status events ("saving" → "synced" or "pending-sync")
    - _Requirements: 3.1, 3.7, 7.2_

  - [x] 7.3 Implement upload to COS with retry logic
    - Implement upload logic that reads from local file store and uploads to COS
    - Implement retry mechanism: retry up to 5 times at 30-second intervals on failure
    - Mark file as pending-sync on failure, emit failure indicator after max retries exceeded
    - Skip upload if COS config is missing/invalid
    - _Requirements: 3.2, 3.5, 3.6, 3.8_

  - [x] 7.4 Implement manifest operations (download, merge, upload)
    - Implement `sync_manifest()` that downloads manifest from COS, merges with local metadata using last-modified-timestamp, and re-uploads
    - Handle concurrent modification: re-download, re-merge, retry up to 3 times
    - _Requirements: 6.1, 6.3, 6.4_

  - [x] 7.5 Implement manifest polling (30-second interval)
    - Start a background task that polls manifest every 30 seconds while online
    - On new remote entries: add to local metadata
    - On updated remote entries: update local content hash, mark for re-download
    - On deleted remote entries: remove from local metadata
    - _Requirements: 6.5, 6.6, 6.7_

  - [x] 7.6 Implement load_canvas with hash-based cache decision
    - Implement `load_canvas(file_id)` that compares local content hash with manifest content hash
    - If hashes match: load from local cache
    - If hashes differ or file absent locally: download from COS
    - _Requirements: 4.3, 4.4_

  - [x] 7.7 Write property tests for sync engine (Rust proptest)
    - **Property 2: Debounce Saves Final State**
    - **Property 4: Upload Retry Respects Maximum Attempts**
    - **Property 5: Sync Status State Machine**
    - **Property 7: Cache Decision by Hash Comparison**
    - **Validates: Requirements 3.1, 3.5, 3.6, 3.7, 4.3, 4.4**

- [x] 8. Implement Sync Engine - Conflict resolution and queue processing
  - [x] 8.1 Implement conflict detection
    - Implement `detect_conflicts(remote_manifest)` that compares remote content hash against base content hash
    - A conflict exists when both local and remote hashes differ from the base hash
    - If only remote differs (local unchanged), treat as remote update (not conflict)
    - _Requirements: 8.1_

  - [x] 8.2 Implement conflict resolution
    - Implement `resolve_conflict(conflict)` that saves remote version as conflict copy with title "{title} - Conflict {YYYY-MM-DD}"
    - Upload local version as new primary file on COS
    - Enforce maximum 5 conflict copies per file; delete oldest if limit exceeded
    - _Requirements: 8.2, 8.5, 8.7_

  - [x] 8.3 Implement upload queue processing
    - Implement `process_upload_queue()` that processes queued uploads in chronological order
    - On individual item failure: skip failed item, continue with remaining, retry failed on next cycle
    - Process queue when connectivity is restored (within 30 seconds)
    - _Requirements: 7.4, 7.5_

  - [x] 8.4 Write property tests for conflict resolution and queue (Rust proptest)
    - **Property 6: Manifest Merge by Timestamp**
    - **Property 14: Queue Upload Chronological Order**
    - **Property 15: Queue Processing Resilience**
    - **Property 16: Conflict Detection**
    - **Property 17: Conflict Resolution Creates Named Copy**
    - **Property 18: Maximum Conflict Copies Invariant**
    - **Validates: Requirements 4.1, 6.3, 6.7, 7.4, 7.5, 8.1, 8.2, 8.5, 8.7**

- [x] 9. Checkpoint - Sync engine complete
  - Ensure all tests pass, ask the user if questions arise.

- [x] 10. Implement Tauri command layer
  - [x] 10.1 Implement COS configuration commands
    - Create `src-tauri/src/commands.rs` with `save_cos_config`, `validate_cos_config`, `get_cos_config` Tauri commands
    - `validate_cos_config` must attempt test connection within 10 seconds
    - Store credentials only in local app data directory, never expose to frontend
    - _Requirements: 2.2, 2.3, 2.4, 2.5, 2.6, 3.4_

  - [x] 10.2 Implement file operation commands
    - Implement `save_canvas`, `load_canvas`, `create_new_file`, `delete_file`, `rename_file`, `export_file` Tauri commands
    - `create_new_file` generates UUID v4 file ID and creates empty canvas
    - `rename_file` validates title (max 100 chars, non-empty/whitespace)
    - `export_file` uses native file dialog
    - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 9.7, 5.5, 5.6_

  - [x] 10.3 Implement file list and sync commands
    - Implement `get_file_list`, `trigger_sync`, `get_sync_status` Tauri commands
    - `get_file_list` returns files sorted by last_modified DESC
    - _Requirements: 5.1, 4.1_

  - [x] 10.4 Write unit tests for Tauri commands
    - Test title validation (empty, whitespace, >100 chars)
    - Test file creation returns unique IDs
    - Test config persistence and retrieval
    - _Requirements: 5.5, 5.6, 9.3_

- [x] 11. Implement frontend - COS Configuration Form
  - [x] 11.1 Create COS Configuration Form component
    - Create `excalidraw-app/src/cloud-sync/components/CosConfigForm.tsx`
    - Implement form with SecretId, SecretKey, Bucket, Region fields
    - Client-side validation: all fields must be non-empty before submission
    - On submit: invoke `validate_cos_config` Tauri command
    - On validation failure: display error message, retain form values
    - On success: proceed to main editor view
    - _Requirements: 2.1, 2.3, 2.4_

  - [x] 11.2 Write unit tests for COS Configuration Form
    - Test form renders all four fields
    - Test submission blocked when fields are empty
    - Test error display on validation failure
    - _Requirements: 2.1, 2.4_

- [x] 12. Implement frontend - File List Sidebar
  - [x] 12.1 Create File List Sidebar component
    - Create `excalidraw-app/src/cloud-sync/components/FileListSidebar.tsx`
    - Display files sorted by last modified (descending)
    - Show file title (truncated to 50 chars with ellipsis if longer) and relative time
    - Highlight currently active file
    - Show sync status icons (synced, pending-sync, conflict)
    - Distinguish conflict copies with a special icon
    - _Requirements: 5.1, 5.3, 7.6, 8.3_

  - [x] 12.2 Implement file management actions in sidebar
    - Implement rename: inline edit with validation (non-empty, max 100 chars)
    - Implement delete: confirmation prompt before deletion
    - Implement new file: add "Untitled" entry with current timestamp
    - On file select: invoke auto-save of current canvas, then load selected file
    - _Requirements: 5.2, 5.4, 5.5, 5.6, 5.7, 4.6, 4.7_

  - [x] 12.3 Write property tests for File List Sidebar (fast-check)
    - **Property 9: File List Sort Order**
    - **Property 10: Title Display Truncation**
    - **Property 11: Title Validation**
    - **Validates: Requirements 5.1, 5.3, 5.5, 5.6**

- [x] 13. Implement frontend - Sync Status Indicator
  - [x] 13.1 Create Sync Status Indicator component
    - Create `excalidraw-app/src/cloud-sync/components/SyncStatusIndicator.tsx`
    - Display current sync state: idle, saving, synced, pending-sync, error
    - Subscribe to Tauri sync status events for real-time updates
    - Show last sync time when available
    - _Requirements: 3.7_

  - [x] 13.2 Write unit tests for Sync Status Indicator
    - Test correct icon/text for each status state
    - Test event subscription updates display
    - _Requirements: 3.7_

- [x] 14. Implement frontend - App integration and auto-save
  - [x] 14.1 Create cloud-sync app wrapper and routing logic
    - Create `excalidraw-app/src/cloud-sync/CloudSyncApp.tsx`
    - On launch: check for persisted COS config via `get_cos_config` command
    - If no valid config: show COS Configuration Form
    - If valid config: show main editor with sidebar and sync indicator
    - Integrate File List Sidebar alongside the Excalidraw editor
    - _Requirements: 2.1, 2.6, 1.3_

  - [x] 14.2 Implement auto-save with 2-second debounce
    - Hook into Excalidraw's `onChange` callback
    - Implement 2-second debounce timer that resets on each modification
    - After debounce: invoke `save_canvas` Tauri command with current canvas data
    - Handle save-before-switch: save current canvas before loading another file
    - _Requirements: 3.1, 4.6, 9.1_

  - [x] 14.3 Implement conflict notification display
    - When opening a file with conflict copies, show non-blocking notification
    - Display number of conflict copies and their titles
    - Allow user to delete conflict copies from sidebar
    - _Requirements: 8.3, 8.4, 8.6_

  - [x] 14.4 Write property tests for auto-save logic (fast-check)
    - **Property 8: Save Before File Switch**
    - **Validates: Requirements 4.6, 4.7, 5.2, 9.1**

- [x] 15. Checkpoint - All components implemented
  - Ensure all tests pass, ask the user if questions arise.

- [x] 16. Wire everything together and final integration
  - [x] 16.1 Register all Tauri commands and configure app startup
    - In `src-tauri/src/main.rs`, register all command handlers with the Tauri builder
    - Initialize SyncEngine on app start, start manifest polling and connectivity monitoring
    - Configure error dialog for initialization failures
    - Set up event emission from Rust to frontend for sync status updates
    - _Requirements: 1.4, 1.5, 6.6_

  - [x] 16.2 Implement Tauri event bridge for frontend
    - Create `excalidraw-app/src/cloud-sync/tauri-bridge.ts` with typed wrappers around `invoke()` and `listen()` calls
    - Map Rust events to frontend state updates (sync status, file list changes, connectivity)
    - _Requirements: 1.4, 3.7, 6.5_

  - [x] 16.3 Write integration tests for end-to-end sync flows
    - Test full save cycle: modify → debounce → local save → upload → manifest update
    - Test full load cycle: select file → hash check → download → render
    - Test offline/online transition: changes offline → connectivity restored → queue processes
    - Test conflict scenario: detect conflict → create conflict copy → display notification
    - _Requirements: 3.1, 3.2, 4.3, 4.4, 7.3, 7.4, 8.1, 8.2_

  - [x] 16.4 Write property test for new file uniqueness (Rust proptest)
    - **Property 19: New File Uniqueness**
    - **Validates: Requirements 9.2**

  - [x] 16.5 Write property test for manifest serialization (Rust proptest)
    - **Property 3: Manifest Serialization Completeness**
    - **Validates: Requirements 3.3, 6.2**

- [x] 17. Final checkpoint - All tests pass and application builds
  - Ensure all tests pass, ask the user if questions arise.

## Notes

- Tasks marked with `*` are optional and can be skipped for faster MVP
- Each task references specific requirements for traceability
- Checkpoints ensure incremental validation
- Property tests validate universal correctness properties from the design document
- Unit tests validate specific examples and edge cases
- Rust backend tests use `proptest` crate; frontend tests use `fast-check`
- The Tauri command layer acts as the boundary between frontend and backend — all IPC goes through typed commands
- COS credentials are never exposed to the frontend WebView process

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "1.2", "1.3"] },
    { "id": 1, "tasks": ["2.1", "3.1", "4.1", "5.1"] },
    { "id": 2, "tasks": ["2.2", "2.3", "2.4", "3.2", "4.2"] },
    { "id": 3, "tasks": ["2.5", "7.1"] },
    { "id": 4, "tasks": ["7.2", "7.3", "7.4"] },
    { "id": 5, "tasks": ["7.5", "7.6", "7.7"] },
    { "id": 6, "tasks": ["8.1", "8.2", "8.3"] },
    { "id": 7, "tasks": ["8.4", "10.1", "10.2", "10.3"] },
    { "id": 8, "tasks": ["10.4", "11.1", "12.1", "13.1"] },
    { "id": 9, "tasks": ["11.2", "12.2", "13.2"] },
    { "id": 10, "tasks": ["12.3", "14.1"] },
    { "id": 11, "tasks": ["14.2", "14.3"] },
    { "id": 12, "tasks": ["14.4", "16.1", "16.2"] },
    { "id": 13, "tasks": ["16.3", "16.4", "16.5"] }
  ]
}
```
