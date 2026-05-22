/**
 * TypeScript interfaces for the cloud-sync desktop feature.
 *
 * These types mirror the data structures exposed across the Tauri IPC
 * boundary and consumed by the cloud-sync React components (FileListSidebar,
 * SyncStatusIndicator, CosConfigForm).
 *
 * See `.kiro/specs/cloud-sync-desktop/design.md` (Frontend Components section)
 * for the authoritative specification.
 */

/**
 * Sync state used by the sidebar entry of a single file.
 *
 * - "synced":       file is up-to-date with COS
 * - "pending-sync": local changes are queued for upload
 * - "conflict":     a conflict copy exists for this file
 */
export type FileSyncStatus = "synced" | "pending-sync" | "conflict";

/**
 * Sync state surfaced by the global sync status indicator. Includes
 * transient and error states that don't apply to individual file entries.
 */
export type SyncStatus =
  | "idle"
  | "local-only"
  | "saving"
  | "synced"
  | "pending-sync"
  | "error";

/**
 * A single file entry as displayed in the file list sidebar. The shape
 * matches the merged view of the local SQLite metadata and the remote
 * `manifest.json` on COS.
 */
export interface FileEntry {
  id: string;
  title: string;
  /** Unix timestamp in milliseconds. */
  lastModified: number;
  syncStatus: FileSyncStatus;
  isConflictCopy: boolean;
  /** Set for conflict copies; references the original file's id. */
  parentFileId?: string;
}

/**
 * Tencent COS connection configuration. Persisted locally by the Rust
 * backend; the frontend only sees these values when the user is editing
 * the configuration form.
 */
export interface CosConfig {
  secretId: string;
  secretKey: string;
  bucket: string;
  region: string;
}

/**
 * Props for the global sync status indicator component.
 */
export interface SyncStatusIndicatorProps {
  status: SyncStatus;
  /** Unix timestamp in milliseconds of the last successful sync. */
  lastSyncTime?: number;
}

/**
 * Props for the file list sidebar component.
 */
export interface FileListSidebarProps {
  files: FileEntry[];
  activeFileId: string | null;
  isCloudSyncEnabled?: boolean;
  onOpenSettings?: () => void;
  onFileSelect: (fileId: string) => void;
  onFileRename: (fileId: string, newTitle: string) => void;
  onFileDelete: (fileId: string) => void;
  onNewFile: () => void;
}

/**
 * Props for the COS configuration form component shown on first launch
 * or when an existing configuration fails validation.
 */
export interface CosConfigFormProps {
  initialValues?: Partial<CosConfig>;
  onSubmit: (config: CosConfig) => Promise<void>;
  onCancel?: () => void;
  error?: string;
}
