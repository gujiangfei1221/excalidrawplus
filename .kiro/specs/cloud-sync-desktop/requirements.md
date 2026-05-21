# Requirements Document

## Introduction

This feature transforms the Excalidraw whiteboard tool into a Tauri-based desktop application with cloud storage capabilities via Tencent Cloud Object Storage (COS). The application enables a single user to save, load, and sync `.excalidraw` files across multiple devices without requiring login. A local SQLite database and filesystem cache provide offline support, while a `manifest.json` file on COS serves as the cross-device file index.

## Glossary

- **Desktop_App**: The Tauri v2 desktop application wrapping the Excalidraw React frontend in a native window with a Rust backend
- **Rust_Backend**: The Tauri Rust layer responsible for COS API calls, SQLite operations, and local file I/O
- **Frontend**: The Excalidraw React application running inside the Tauri WebView
- **COS**: Tencent Cloud Object Storage, the remote storage service for `.excalidraw` files
- **COS_Config**: The local configuration containing SecretId, SecretKey, Bucket, and Region for COS access
- **Manifest**: A `manifest.json` file stored on COS that serves as the index of all files, including metadata such as file ID, title, last modified timestamp, and content hash
- **Local_Cache**: The combination of SQLite metadata database and local filesystem file storage used for offline access
- **Canvas_Data**: The `.excalidraw` JSON content representing a single drawing (elements, appState, files)
- **File_List_Sidebar**: A UI panel within the Excalidraw interface displaying all saved files with the ability to open any file
- **Sync_Engine**: The component responsible for detecting changes, uploading to COS, downloading from COS, and resolving conflicts
- **Content_Hash**: A hash (e.g., SHA-256) computed from the Canvas_Data used to detect whether file content has changed

## Requirements

### Requirement 1: Desktop Application Shell

**User Story:** As a user, I want to run Excalidraw as a native desktop application on macOS and Windows, so that I have a dedicated app experience without needing a browser.

#### Acceptance Criteria

1. THE Desktop_App SHALL package the Excalidraw React frontend inside a Tauri v2 native window with a minimum window size of 800x600 pixels
2. THE Desktop_App SHALL support building for macOS (version 11 and above) and Windows (version 10 and above) platforms
3. WHEN the Desktop_App is launched, THE Frontend SHALL render the Excalidraw editor including the toolbar, canvas, and drawing tools in an interactive state within the native window within 5 seconds
4. THE Rust_Backend SHALL expose Tauri commands that the Frontend can invoke for file and cloud operations as defined in Requirements 3 through 9
5. IF the Desktop_App fails to initialize the Frontend within the native window, THEN THE Desktop_App SHALL display a native error dialog indicating the initialization failure and terminate gracefully

### Requirement 2: COS Configuration

**User Story:** As a user, I want to configure my Tencent COS credentials locally, so that the application can access my cloud storage without requiring a login system.

#### Acceptance Criteria

1. WHEN the Desktop_App is launched without a persisted COS_Config (all four fields: SecretId, SecretKey, Bucket, and Region present and non-empty), THE Frontend SHALL display a configuration form requesting SecretId, SecretKey, Bucket, and Region
2. WHEN the user submits the COS_Config form with all four fields non-empty, THE Rust_Backend SHALL persist the configuration to a local file in the application data directory
3. WHEN the user submits the COS_Config form, THE Rust_Backend SHALL validate the COS_Config by attempting a test connection to the specified Bucket within 10 seconds
4. IF the COS_Config validation fails or the test connection does not respond within 10 seconds, THEN THE Frontend SHALL display an error message indicating the connection failure reason and retain the submitted form values so the user can correct them
5. WHEN the user updates the COS_Config, THE Rust_Backend SHALL overwrite the existing configuration and re-validate the connection
6. WHEN the Desktop_App is launched with a persisted and previously validated COS_Config, THE Frontend SHALL skip the configuration form and proceed to the main editor view

### Requirement 3: Save Canvas to Cloud

**User Story:** As a user, I want my drawings to be automatically saved to Tencent COS, so that my work is backed up and accessible from other devices.

#### Acceptance Criteria

1. WHEN the user modifies Canvas_Data and 2 seconds elapse without further modification, THE Sync_Engine SHALL save the Canvas_Data to the Local_Cache
2. WHEN Canvas_Data is saved to the Local_Cache and a valid COS_Config exists and network connectivity is available, THE Sync_Engine SHALL upload the Canvas_Data to COS as a `.excalidraw` JSON file not exceeding 100 MB in size
3. WHEN a file is successfully uploaded to COS, THE Sync_Engine SHALL update the Manifest on COS with the file metadata including file ID, title, last modified timestamp, and Content_Hash
4. THE Rust_Backend SHALL store COS credentials only in the local application data directory and never transmit them to the Frontend
5. IF the upload to COS fails, THEN THE Sync_Engine SHALL mark the file as pending-sync in the Local_Cache and retry automatically within 30 seconds, up to a maximum of 5 consecutive retry attempts per file
6. IF the Sync_Engine exceeds the maximum retry attempts for a file, THEN THE Sync_Engine SHALL keep the file marked as pending-sync in the Local_Cache and display a sync failure indicator to the user
7. WHEN Canvas_Data is saved to the Local_Cache or uploaded to COS, THE Frontend SHALL display a sync status indicator showing the current state of the file as saving, synced, or pending-sync
8. IF the COS_Config is missing or invalid when an upload is triggered, THEN THE Sync_Engine SHALL retain the Canvas_Data in the Local_Cache marked as pending-sync and not attempt the upload until a valid COS_Config is provided

### Requirement 4: Load Canvas from Cloud

**User Story:** As a user, I want to browse and open any of my saved drawings, so that I can continue working on previous canvases.

#### Acceptance Criteria

1. WHEN the Desktop_App starts with a valid COS_Config, THE Sync_Engine SHALL download the Manifest from COS within 10 seconds and merge it with the local metadata using last-modified-timestamp to resolve differing entries
2. IF the Manifest download fails at startup, THEN THE Sync_Engine SHALL use the locally cached metadata to populate the File_List_Sidebar and indicate that the file list may be outdated
3. WHEN the user selects a file from the File_List_Sidebar and the local Content_Hash matches the Manifest Content_Hash, THE Sync_Engine SHALL load the Canvas_Data from the Local_Cache
4. IF the selected file is not in the Local_Cache or the local Content_Hash differs from the Manifest Content_Hash, THEN THE Sync_Engine SHALL download the Canvas_Data from COS within 30 seconds
5. IF the Canvas_Data download from COS fails, THEN THE Frontend SHALL display an error message indicating the download failure and retain the current canvas unchanged
6. WHEN Canvas_Data is loaded and the current canvas has unsaved modifications, THE Frontend SHALL save the current Canvas_Data to the Local_Cache before rendering the newly loaded drawing in the Excalidraw editor
7. WHEN Canvas_Data is loaded and the current canvas has no unsaved modifications, THE Frontend SHALL render the newly loaded drawing in the Excalidraw editor replacing the current canvas

### Requirement 5: File List Sidebar

**User Story:** As a user, I want a sidebar showing all my saved drawings, so that I can quickly switch between files.

#### Acceptance Criteria

1. THE File_List_Sidebar SHALL display a list of all files from the merged local and remote Manifest, sorted by last modified timestamp in descending order, and SHALL visually highlight the currently active file entry
2. WHEN the user clicks a file entry in the File_List_Sidebar, THE Sync_Engine SHALL auto-save any unsaved changes to the current canvas before THE Frontend loads the selected file into the editor
3. THE File_List_Sidebar SHALL display the file title (truncated to 50 characters with an ellipsis if longer) and last modified date in relative format (e.g., "2 minutes ago", "3 days ago") for each entry
4. WHEN the user creates a new canvas, THE File_List_Sidebar SHALL add a new entry with a default title of "Untitled" and the current timestamp
5. WHEN the user renames a file via the File_List_Sidebar, THE Sync_Engine SHALL update the title in both the Local_Cache and the Manifest on COS, limited to a maximum of 100 characters
6. IF the user provides an empty or whitespace-only title when renaming a file, THEN THE File_List_Sidebar SHALL reject the rename and retain the previous title
7. WHEN the user deletes a file via the File_List_Sidebar, THE Frontend SHALL display a confirmation prompt before THE Sync_Engine removes the file from the Local_Cache and marks it as deleted in the Manifest on COS

### Requirement 6: Cross-Device Sync via Manifest

**User Story:** As a user, I want all devices with the same COS configuration to see the same set of files, so that I can seamlessly switch between computers.

#### Acceptance Criteria

1. THE Manifest SHALL be stored as a single `manifest.json` file at a fixed root-level path in the COS Bucket
2. THE Manifest SHALL contain an array of file entries, each with file ID, title, last modified timestamp, Content_Hash, COS object key, and a deleted flag indicating whether the file has been removed
3. WHEN the Sync_Engine uploads or modifies a file, THE Sync_Engine SHALL update the Manifest on COS by downloading the current Manifest, merging changes, and re-uploading within a single operation
4. IF the Manifest on COS has been modified by another device between download and re-upload, THEN THE Sync_Engine SHALL re-download the latest Manifest, re-merge changes, and retry the upload up to 3 times before reporting a sync failure
5. WHEN the Desktop_App gains network connectivity, THE Sync_Engine SHALL pull the latest Manifest from COS and reconcile it with the local metadata by adding entries that exist only in the remote Manifest, updating entries whose remote Content_Hash differs from the local Content_Hash, and removing entries marked as deleted in the remote Manifest
6. WHILE the Desktop_App has network connectivity, THE Sync_Engine SHALL poll the Manifest from COS at an interval of 30 seconds to detect changes made by other devices
7. WHEN a new file entry appears in the remote Manifest that does not exist locally, THE Sync_Engine SHALL add the entry to the local metadata and download the Canvas_Data when the user selects the file from the File_List_Sidebar

### Requirement 7: Offline Support

**User Story:** As a user, I want to continue creating and editing drawings without an internet connection, so that my workflow is not interrupted.

#### Acceptance Criteria

1. THE Local_Cache SHALL store file metadata in a SQLite database in the application data directory
2. THE Local_Cache SHALL store Canvas_Data as `.excalidraw` JSON files on the local filesystem in the application data directory
3. WHILE the Desktop_App has no network connectivity, THE Sync_Engine SHALL save all changes to the Local_Cache and persist them in a durable upload queue stored in the SQLite database so that queued changes survive application restarts
4. WHEN network connectivity is restored, THE Sync_Engine SHALL upload all queued changes to COS in chronological order within 30 seconds of detecting connectivity
5. IF an individual queued upload fails during the sync-back process, THEN THE Sync_Engine SHALL skip the failed item, continue uploading remaining queued changes, and retry the failed item on the next sync cycle
6. WHILE offline, THE File_List_Sidebar SHALL display all locally cached files and indicate their sync status (synced, pending-sync, or conflict)
7. THE Sync_Engine SHALL detect network connectivity changes by attempting to reach the configured COS endpoint and SHALL re-evaluate connectivity at least once every 30 seconds while offline

### Requirement 8: Conflict Resolution

**User Story:** As a user, I want the application to handle conflicts when the same file is edited on two devices while offline, so that I do not lose any work.

#### Acceptance Criteria

1. WHEN the Sync_Engine detects that a file's remote Content_Hash differs from the base Content_Hash recorded at the time of the file's last successful sync or download, THE Sync_Engine SHALL identify this as a conflict
2. WHEN a conflict is detected, THE Sync_Engine SHALL preserve the remote version by saving it as a separate conflict copy with the title suffix " - Conflict " followed by the date of the conflict detection in YYYY-MM-DD format, and SHALL upload the local version as the new primary file on COS
3. WHEN a conflict copy is created, THE File_List_Sidebar SHALL display the conflict copy as a separate file entry with an icon distinguishing it from non-conflict files
4. WHEN the user opens a file that has conflict copies, THE Frontend SHALL display a non-blocking notification identifying the number of conflict copies and their titles
5. THE Sync_Engine SHALL use a last-writer-wins strategy for the Manifest entry, comparing the local last-modified timestamp against the remote last-modified timestamp and pointing to the version with the later timestamp as the primary file
6. WHEN the user deletes a conflict copy via the File_List_Sidebar, THE Sync_Engine SHALL remove the conflict copy from the Local_Cache and from COS, and SHALL remove its association with the original file
7. THE Sync_Engine SHALL retain a maximum of 5 conflict copies per file; IF a new conflict would exceed this limit, THEN THE Sync_Engine SHALL delete the oldest conflict copy before creating the new one

### Requirement 9: File Creation and Management

**User Story:** As a user, I want to create new drawings and manage existing ones, so that I can organize my work.

#### Acceptance Criteria

1. WHEN the user requests a new canvas and the current canvas has unsaved changes, THE Frontend SHALL save the current Canvas_Data to the Local_Cache before clearing the editor
2. WHEN the user requests a new canvas, THE Frontend SHALL clear the current editor and create a new file entry with a unique ID and an empty Canvas_Data containing no elements
3. THE Rust_Backend SHALL generate unique file IDs using UUID v4
4. WHEN a new file is created, THE Sync_Engine SHALL save it to the Local_Cache and queue it for upload to COS within 1 second of creation
5. WHEN the user exports a file, THE Desktop_App SHALL use the native file dialog to save the Canvas_Data as a `.excalidraw` file to a user-chosen location
6. IF the user cancels the export file dialog, THEN THE Desktop_App SHALL return to the editor without modifying any data
7. IF the export operation fails due to a filesystem error, THEN THE Desktop_App SHALL display an error message indicating the failure reason and preserve the Canvas_Data in the editor
