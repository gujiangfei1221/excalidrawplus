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
          <path d="M12 3v12" />
          <path d="m7 10 5 5 5-5" />
          <path d="M5 21h14" />
        </svg>
      );
    case "pencil":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M12 20h9" />
          <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" />
        </svg>
      );
    case "plus":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M12 5v14" />
          <path d="M5 12h14" />
        </svg>
      );
    case "settings":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <circle cx="12" cy="12" r="3.2" />
          <path d="M12 3v3" />
          <path d="M12 18v3" />
          <path d="M3 12h3" />
          <path d="M18 12h3" />
          <path d="m5.6 5.6 2.1 2.1" />
          <path d="m16.3 16.3 2.1 2.1" />
          <path d="m18.4 5.6-2.1 2.1" />
          <path d="m7.7 16.3-2.1 2.1" />
        </svg>
      );
    case "trash":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M3 6h18" />
          <path d="M8 6V4h8v2" />
          <path d="m19 6-1 14H6L5 6" />
          <path d="M10 11v5" />
          <path d="M14 11v5" />
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
      className={
        isCollapsed ? "cloud-sync-sidebar is-collapsed" : "cloud-sync-sidebar"
      }
    >
      <div className="cloud-sync-sidebar__header">
        {!isCollapsed && <strong>文件</strong>}
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
              className="cloud-sync-text-button"
              onClick={onFileImport}
              title="导入文件"
              type="button"
            >
              <Icon name="import" />
              <span>导入</span>
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
                      if (window.confirm(`确认删除“${file.title}”？`)) {
                        onFileDelete(file.id);
                      }
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
    </aside>
  );
};
