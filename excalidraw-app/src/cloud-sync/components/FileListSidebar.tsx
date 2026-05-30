import { useMemo, useState } from "react";

import {
  formatRelativeTime,
  sortFilesByLastModifiedDesc,
  truncateFileTitle,
  validateFileTitle,
} from "../utils";

import type { FileEntry, FileListSidebarProps } from "../types";

const STATUS_LABEL: Record<FileEntry["syncStatus"], string> = {
  synced: "已同步",
  "pending-sync": "待同步",
  conflict: "冲突",
};

const Icon = ({
  name,
}: {
  name:
    | "chevron-left"
    | "chevron-right"
    | "import"
    | "pencil"
    | "plus"
    | "settings"
    | "trash";
}) => {
  // Icons sourced from Lucide (https://lucide.dev) — MIT licensed.
  // Consistent 24×24 viewBox, 2px stroke, round caps/joins.
  switch (name) {
    case "chevron-left":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="m15 18-6-6 6-6" />
        </svg>
      );
    case "chevron-right":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="m9 18 6-6-6-6" />
        </svg>
      );
    case "import":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
          <polyline points="7 10 12 15 17 10" />
          <line x1="12" x2="12" y1="15" y2="3" />
        </svg>
      );
    case "pencil":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />
          <path d="m15 5 4 4" />
        </svg>
      );
    case "plus":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M5 12h14" />
          <path d="M12 5v14" />
        </svg>
      );
    case "settings":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
          <circle cx="12" cy="12" r="3" />
        </svg>
      );
    case "trash":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M3 6h18" />
          <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
          <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
          <line x1="10" x2="10" y1="11" y2="17" />
          <line x1="14" x2="14" y1="11" y2="17" />
        </svg>
      );
  }
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
  onFileImport,
  onNewFile,
}: FileListSidebarProps) => {
  const sortedFiles = useMemo(
    () => sortFilesByLastModifiedDesc(files),
    [files],
  );
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draftTitle, setDraftTitle] = useState("");
  const [error, setError] = useState("");
  const [pendingDeleteFile, setPendingDeleteFile] = useState<FileEntry | null>(
    null,
  );

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

  const requestDelete = (file: FileEntry) => {
    setPendingDeleteFile(file);
  };

  const cancelDelete = () => {
    setPendingDeleteFile(null);
  };

  const confirmDelete = () => {
    if (!pendingDeleteFile) {
      return;
    }
    onFileDelete(pendingDeleteFile.id);
    setPendingDeleteFile(null);
  };

  return (
    <aside
      className={
        isCollapsed ? "cloud-sync-sidebar is-collapsed" : "cloud-sync-sidebar"
      }
    >
      <div className="cloud-sync-sidebar__header">
        {!isCollapsed && <strong>文件列表</strong>}
        <div className="cloud-sync-sidebar__actions">
          <button
            aria-label={isCollapsed ? "展开侧边栏" : "收起侧边栏"}
            className="cloud-sync-icon-button"
            onClick={onToggleCollapse}
            title={isCollapsed ? "展开侧边栏" : "收起侧边栏"}
            type="button"
          >
            <Icon name={isCollapsed ? "chevron-right" : "chevron-left"} />
          </button>
          {!isCollapsed && onOpenSettings && (
            <button
              aria-label="设置"
              className="cloud-sync-icon-button"
              onClick={onOpenSettings}
              title="设置"
              type="button"
            >
              <Icon name="settings" />
            </button>
          )}
          {!isCollapsed && (
            <button
              aria-label="导入文件"
              className="cloud-sync-icon-button"
              onClick={onFileImport}
              title="导入文件"
              type="button"
            >
              <Icon name="import" />
            </button>
          )}
          {!isCollapsed && (
            <button
              aria-label="新建文件"
              className="cloud-sync-icon-button"
              onClick={onNewFile}
              title="新建文件"
              type="button"
            >
              <Icon name="plus" />
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
              暂无文件，点击新建按钮创建。
            </li>
          )}
          {sortedFiles.map((file) => {
            const isEditing = editingId === file.id;
            const isDeleting = deletingFileIds?.has(file.id) ?? false;
            const statusLabel = file.isConflictCopy
              ? "冲突副本"
              : isCloudSyncEnabled
              ? STATUS_LABEL[file.syncStatus]
              : "仅本地";
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
                        aria-label="文件标题"
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
                    aria-label={`重命名 ${file.title}`}
                    className="cloud-sync-icon-button"
                    onClick={() => startRename(file)}
                    title="重命名"
                    type="button"
                  >
                    <Icon name="pencil" />
                  </button>
                  <button
                    aria-label={`删除 ${file.title}`}
                    className="cloud-sync-icon-button"
                    disabled={isDeleting}
                    onClick={() => {
                      if (isDeleting) {
                        return;
                      }
                      requestDelete(file);
                    }}
                    title={isDeleting ? "删除中..." : "删除"}
                    type="button"
                  >
                    <Icon name="trash" />
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {pendingDeleteFile && (
        <div
          aria-labelledby="cloud-sync-delete-title"
          aria-modal="true"
          className="cloud-sync-confirm"
          role="dialog"
        >
          <div className="cloud-sync-confirm__panel">
            <strong id="cloud-sync-delete-title">删除文件</strong>
            <p className="cloud-sync-confirm__message">
              确认删除“{truncateFileTitle(pendingDeleteFile.title)}”？
            </p>
            {isCloudSyncEnabled && (
              <p className="cloud-sync-confirm__warning" role="alert">
                此操作会<strong>同时删除云端 COS 上的文件</strong>，删除后无法恢复。
              </p>
            )}
            <div className="cloud-sync-confirm__actions">
              <button
                autoFocus
                className="cloud-sync-confirm__cancel"
                onClick={cancelDelete}
                type="button"
              >
                取消
              </button>
              <button
                className="cloud-sync-confirm__delete"
                onClick={confirmDelete}
                type="button"
              >
                确认删除
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
};
