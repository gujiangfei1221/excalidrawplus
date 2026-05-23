import { useMemo, useState } from "react";

import {
  formatRelativeTime,
  sortFilesByLastModifiedDesc,
  truncateFileTitle,
  validateFileTitle,
} from "../utils";

import type { FileEntry, FileListSidebarProps } from "../types";

const STATUS_LABEL: Record<FileEntry["syncStatus"], string> = {
  synced: "Synced",
  "pending-sync": "Pending sync",
  conflict: "Conflict",
};

export const FileListSidebar = ({
  files,
  activeFileId,
  deletingFileIds,
  isCloudSyncEnabled = true,
  isCollapsed,
  onOpenSettings,
  onToggleCollapse,
  onFileSelect,
  onFileRename,
  onFileDelete,
  onNewFile,
}: FileListSidebarProps) => {
  const sortedFiles = useMemo(
    () => sortFilesByLastModifiedDesc(files),
    [files],
  );
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draftTitle, setDraftTitle] = useState("");
  const [error, setError] = useState("");

  const startRename = (file: FileEntry) => {
    setEditingId(file.id);
    setDraftTitle(file.title);
    setError("");
  };

  const commitRename = (file: FileEntry) => {
    const titleError = validateFileTitle(draftTitle);
    if (titleError) {
      setError(titleError);
      return;
    }

    if (draftTitle !== file.title) {
      onFileRename(file.id, draftTitle);
    }
    setEditingId(null);
    setError("");
  };

  return (
    <aside
      className={isCollapsed ? "cloud-sync-sidebar is-collapsed" : "cloud-sync-sidebar"}
    >
      <div className="cloud-sync-sidebar__header">
        {!isCollapsed && <strong>Files</strong>}
        <div className="cloud-sync-sidebar__actions">
          <button
            aria-label={isCollapsed ? "Expand sidebar" : "Collapse sidebar"}
            onClick={onToggleCollapse}
            title={isCollapsed ? "Expand sidebar" : "Collapse sidebar"}
            type="button"
          >
            {isCollapsed ? ">" : "<"}
          </button>
          {!isCollapsed && onOpenSettings && (
            <button
              aria-label="Cloud sync settings"
              onClick={onOpenSettings}
              title="Cloud sync settings"
              type="button"
            >
              Sync
            </button>
          )}
          {!isCollapsed && (
            <button aria-label="New file" onClick={onNewFile} title="New file">
              +
            </button>
          )}
        </div>
      </div>
      {!isCollapsed && error && (
        <p className="cloud-sync-error" role="alert">
          {error}
        </p>
      )}
      {!isCollapsed && (
        <ul className="cloud-sync-file-list">
          {sortedFiles.length === 0 && (
            <li className="cloud-sync-file-list__empty">
              No files. Press <strong>+</strong> to create one.
            </li>
          )}
          {sortedFiles.map((file) => {
            const isEditing = editingId === file.id;
            const isDeleting = deletingFileIds?.has(file.id) ?? false;
            const statusLabel = file.isConflictCopy
              ? "Conflict copy"
              : isCloudSyncEnabled
              ? STATUS_LABEL[file.syncStatus]
              : "Local only";
            const statusClass = file.isConflictCopy
              ? "is-conflict"
              : isCloudSyncEnabled
              ? `is-${file.syncStatus}`
              : "is-local-only";

            return (
              <li
                className={
                  file.id === activeFileId
                    ? "cloud-sync-file is-active"
                    : "cloud-sync-file"
                }
                key={file.id}
              >
                <button
                  className="cloud-sync-file__main"
                  onClick={() => onFileSelect(file.id)}
                  type="button"
                >
                  <span
                    aria-label={statusLabel}
                    className={`cloud-sync-file__status ${statusClass}`}
                    title={statusLabel}
                  />
                  <span className="cloud-sync-file__text">
                    {isEditing ? (
                      <input
                        aria-label="File title"
                        autoFocus
                        onBlur={() => commitRename(file)}
                        onChange={(event) => setDraftTitle(event.target.value)}
                        onClick={(event) => event.stopPropagation()}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") {
                            commitRename(file);
                          }
                          if (event.key === "Escape") {
                            setEditingId(null);
                            setError("");
                          }
                        }}
                        value={draftTitle}
                      />
                    ) : (
                      <span className="cloud-sync-file__title">
                        {truncateFileTitle(file.title)}
                      </span>
                    )}
                    <span className="cloud-sync-file__time">
                      {formatRelativeTime(file.lastModified)}
                    </span>
                  </span>
                </button>
                <div className="cloud-sync-file__actions">
                  <button
                    aria-label={`Rename ${file.title}`}
                    onClick={() => startRename(file)}
                    title="Rename"
                    type="button"
                  >
                    R
                  </button>
                  <button
                    aria-label={`Delete ${file.title}`}
                    disabled={isDeleting}
                    onClick={() => {
                      if (isDeleting) {
                        return;
                      }
                      if (window.confirm(`Delete "${file.title}"?`)) {
                        onFileDelete(file.id);
                      }
                    }}
                    title={isDeleting ? "Deleting..." : "Delete"}
                    type="button"
                  >
                    X
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
};
