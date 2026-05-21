import { useMemo, useState } from "react";

import type { FileEntry, FileListSidebarProps } from "../types";
import {
  formatRelativeTime,
  sortFilesByLastModifiedDesc,
  truncateFileTitle,
  validateFileTitle,
} from "../utils";

const STATUS_LABEL: Record<FileEntry["syncStatus"], string> = {
  synced: "OK",
  "pending-sync": "...",
  conflict: "!",
};

export const FileListSidebar = ({
  files,
  activeFileId,
  onFileSelect,
  onFileRename,
  onFileDelete,
  onNewFile,
}: FileListSidebarProps) => {
  const sortedFiles = useMemo(() => sortFilesByLastModifiedDesc(files), [files]);
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
    <aside className="cloud-sync-sidebar">
      <div className="cloud-sync-sidebar__header">
        <strong>Files</strong>
        <button aria-label="New file" onClick={onNewFile} title="New file">
          +
        </button>
      </div>
      {error && (
        <p className="cloud-sync-error" role="alert">
          {error}
        </p>
      )}
      <ul className="cloud-sync-file-list">
        {sortedFiles.map((file) => {
          const isEditing = editingId === file.id;

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
                  aria-label={
                    file.isConflictCopy ? "Conflict copy" : file.syncStatus
                  }
                  className="cloud-sync-file__status"
                  title={
                    file.isConflictCopy ? "Conflict copy" : file.syncStatus
                  }
                >
                  {file.isConflictCopy ? "C" : STATUS_LABEL[file.syncStatus]}
                </span>
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
                  onClick={() => {
                    if (window.confirm(`Delete "${file.title}"?`)) {
                      onFileDelete(file.id);
                    }
                  }}
                  title="Delete"
                  type="button"
                >
                  X
                </button>
              </div>
            </li>
          );
        })}
      </ul>
    </aside>
  );
};
