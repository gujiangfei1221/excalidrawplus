import {
  CaptureUpdateAction,
  Excalidraw,
  ExcalidrawAPIProvider,
  exportToBlob,
  restoreAppState,
  restoreElements,
  serializeAsJSON,
  useExcalidrawAPI,
} from "@excalidraw/excalidraw";
import {
  Component,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type {
  AppState,
  BinaryFiles,
  ExcalidrawImperativeAPI,
} from "@excalidraw/excalidraw/types";
import type {
  ExcalidrawElement,
  OrderedExcalidrawElement,
} from "@excalidraw/element/types";

import { CosConfigForm } from "./components/CosConfigForm";
import { FileListSidebar } from "./components/FileListSidebar";
import { SyncStatusIndicator } from "./components/SyncStatusIndicator";
import { cloudSyncBridge, listenToCloudSyncEvent } from "./tauri-bridge";

import { countConflictCopies } from "./utils";

import "./cloud-sync.scss";

import type { ReactNode } from "react";

import type { CosConfig, FileEntry, SyncStatus } from "./types";

type ParsedCanvas = {
  type?: string;
  version?: number;
  source?: string;
  elements?: readonly ExcalidrawElement[];
  appState?: Record<string, unknown>;
  files?: BinaryFiles;
};

const EMPTY_CANVAS: ParsedCanvas = {
  type: "excalidraw",
  version: 2,
  source: "cloud-sync-desktop",
  elements: [],
  appState: {},
  files: {},
};

const serializeCanvas = (
  elements: readonly OrderedExcalidrawElement[],
  appState: AppState,
  files: BinaryFiles,
) => {
  return serializeAsJSON(elements, appState, files, "local");
};

const parseCanvas = (rawCanvas: string): ParsedCanvas => {
  try {
    return JSON.parse(rawCanvas || JSON.stringify(EMPTY_CANVAS)) as ParsedCanvas;
  } catch {
    return EMPTY_CANVAS;
  }
};

const restoreCanvasAppState = (appState: Record<string, unknown> = {}) => {
  const { collaborators, ...rest } = appState;
  return restoreAppState(rest, null);
};

const restoreCanvasElements = (
  elements: readonly ExcalidrawElement[] | undefined,
) => {
  return restoreElements(elements ?? [], null, {
    repairBindings: true,
    deleteInvisibleElements: true,
  });
};

const applyCanvasToEditor = (
  excalidrawAPI: ExcalidrawImperativeAPI,
  canvas: ReturnType<typeof parseCanvas>,
  options?: { scrollToFitContent?: boolean },
) => {
  const elements = restoreCanvasElements(canvas.elements);
  excalidrawAPI.updateScene({
    elements,
    appState: restoreCanvasAppState(canvas.appState),
    captureUpdate: CaptureUpdateAction.IMMEDIATELY,
  });
  if (canvas.files) {
    excalidrawAPI.addFiles(Object.values(canvas.files) as any);
  }
  if (options?.scrollToFitContent && elements.length > 0) {
    excalidrawAPI.scrollToContent(undefined, {
      fitToContent: true,
      animate: false,
    });
  }
};

const ToolbarIcon = ({
  name,
}: {
  name: "cloud-download" | "cloud-upload" | "share";
}) => {
  // Icons sourced from Lucide (https://lucide.dev) — MIT licensed.
  switch (name) {
    case "cloud-download":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M12 13v8" />
          <path d="m8 17 4 4 4-4" />
          <path d="M20.88 18.09A5 5 0 0 0 18 9h-1.26A8 8 0 1 0 3 16.29" />
        </svg>
      );
    case "cloud-upload":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M12 13V5" />
          <path d="m8 9 4-4 4 4" />
          <path d="M20.88 18.09A5 5 0 0 0 18 9h-1.26A8 8 0 1 0 3 16.29" />
        </svg>
      );
    case "share":
      return (
        <svg aria-hidden="true" viewBox="0 0 24 24">
          <circle cx="18" cy="5" r="3" />
          <circle cx="6" cy="12" r="3" />
          <circle cx="18" cy="19" r="3" />
          <path d="m8.59 13.51 6.83 3.98" />
          <path d="m15.41 6.51-6.82 3.98" />
        </svg>
      );
  }
};

class CloudSyncErrorBoundary extends Component<
  { children: ReactNode },
  { hasError: boolean }
> {
  state = { hasError: false };

  static getDerivedStateFromError() {
    return { hasError: true };
  }

  componentDidCatch(error: Error) {
    console.error("[cloud-sync] render failed:", error);
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="cloud-sync-loading" role="alert">
          云同步界面渲染失败，请重新打开应用。
        </div>
      );
    }

    return this.props.children;
  }
}

const CloudSyncEditor = ({
  isCloudSyncEnabled,
  onOpenSettings,
  connectionNotice,
}: {
  isCloudSyncEnabled: boolean;
  onOpenSettings: () => void;
  connectionNotice: string;
}) => {
  const excalidrawAPI = useExcalidrawAPI();
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [activeFileId, setActiveFileId] = useState<string | null>(null);
  const [isSidebarCollapsed, setIsSidebarCollapsed] = useState(false);
  const [status, setStatus] = useState<SyncStatus>("idle");
  const [lastSyncTime, setLastSyncTime] = useState<number | undefined>();
  const [error, setError] = useState("");
  const [isSavingToCloud, setIsSavingToCloud] = useState(false);
  const [isDownloadingToLocal, setIsDownloadingToLocal] = useState(false);
  const [isManualSyncing, setIsManualSyncing] = useState(false);
  const [hasUnsavedChanges, setHasUnsavedChanges] = useState(false);
  const [fileStatusOverrides, setFileStatusOverrides] = useState<
    Record<string, FileEntry["syncStatus"]>
  >({});
  const [deletingFileIds, setDeletingFileIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [shareUrl, setShareUrl] = useState<string | null>(null);
  const [isSharing, setIsSharing] = useState(false);
  const [isCopied, setIsCopied] = useState(false);
  const hasUnsavedChangesRef = useRef(false);
  const latestCanvasRef = useRef(JSON.stringify(EMPTY_CANVAS));
  const draftCanvasByFileIdRef = useRef(new Map<string, string>());
  const fallbackFilePromiseRef = useRef<Promise<FileEntry> | null>(null);
  const deletingFileIdsRef = useRef(new Set<string>());

  const refreshFiles = useCallback(async () => {
    const nextFiles = await cloudSyncBridge.getFileList();
    console.debug(
      "[cloud-sync] refreshFiles loaded",
      nextFiles.length,
      "file(s)",
    );
    setFiles(nextFiles);
    setFileStatusOverrides((current) => {
      const nextIds = new Set(nextFiles.map((file) => file.id));
      const nextOverrides: Record<string, FileEntry["syncStatus"]> = {};

      for (const [fileId, status] of Object.entries(current)) {
        if (nextIds.has(fileId)) {
          nextOverrides[fileId] = status;
        }
      }

      return nextOverrides;
    });
    return nextFiles;
  }, []);

  const createFallbackFile = useCallback(() => {
    if (!fallbackFilePromiseRef.current) {
      console.debug("[cloud-sync] createFallbackFile: requesting new file");
      fallbackFilePromiseRef.current = cloudSyncBridge
        .createNewFile()
        .finally(() => {
          fallbackFilePromiseRef.current = null;
        });
    } else {
      console.debug(
        "[cloud-sync] createFallbackFile: reusing in-flight request",
      );
    }

    return fallbackFilePromiseRef.current;
  }, []);

  /**
   * Used ONLY on the initial-load path so that a freshly opened app with an
   * empty workspace still has a canvas the user can immediately draw on.
   *
   * IMPORTANT: do not call this after a user-initiated delete. If the user
   * deletes the last file we must respect their intent and leave the list
   * empty; otherwise we end up in a "delete creates new file" loop where
   * the last entry can never actually be removed.
   */
  const ensureFileListHasFileOnLoad = useCallback(
    async (currentFiles?: FileEntry[]) => {
      const loadedFiles = currentFiles ?? (await refreshFiles());

      if (loadedFiles.length > 0) {
        return loadedFiles;
      }

      const created = await createFallbackFile();
      setFiles((files) =>
        files.some((file) => file.id === created.id) ? files : [created],
      );
      return [created];
    },
    [createFallbackFile, refreshFiles],
  );

  const saveActiveCanvas = useCallback(async () => {
    if (!activeFileId || isSavingToCloud) {
      return;
    }

    const canvasData =
      draftCanvasByFileIdRef.current.get(activeFileId) ??
      latestCanvasRef.current;

    setError("");
    setIsSavingToCloud(true);
    setStatus("saving");

    try {
      const nextStatus = await cloudSyncBridge.saveCanvas(
        activeFileId,
        canvasData,
      );
      hasUnsavedChangesRef.current = false;
      setHasUnsavedChanges(false);
      draftCanvasByFileIdRef.current.delete(activeFileId);
      setFileStatusOverrides((current) => ({
        ...current,
        [activeFileId]: nextStatus,
      }));
      setStatus(nextStatus === "conflict" ? "pending-sync" : nextStatus);
      if (nextStatus === "synced") {
        setLastSyncTime(Date.now());
      }
      await refreshFiles();
      setFileStatusOverrides((current) => {
        const next = { ...current };
        delete next[activeFileId];
        return next;
      });
    } catch (err: any) {
      setStatus("error");
      setError(err.message);
    } finally {
      setIsSavingToCloud(false);
    }
  }, [activeFileId, isSavingToCloud, refreshFiles]);

  const downloadActiveCanvas = useCallback(async () => {
    if (!activeFileId || isDownloadingToLocal) {
      return;
    }

    setError("");
    setIsDownloadingToLocal(true);
    setStatus("saving");

    try {
      const rawCanvas = await cloudSyncBridge.downloadCanvas(activeFileId);
      const canvas = parseCanvas(rawCanvas);
      // Update the ref BEFORE updateScene so that the onChange callback
      // sees the new content as "current" and won't mark it as unsaved.
      latestCanvasRef.current = rawCanvas;
      draftCanvasByFileIdRef.current.delete(activeFileId);
      hasUnsavedChangesRef.current = false;
      setHasUnsavedChanges(false);
      excalidrawAPI &&
        applyCanvasToEditor(excalidrawAPI, canvas, {
          scrollToFitContent: true,
        });
      setStatus("synced");
      setLastSyncTime(Date.now());
      await refreshFiles();
      setFileStatusOverrides((current) => {
        const next = { ...current };
        delete next[activeFileId];
        return next;
      });
    } catch (err: any) {
      setStatus("error");
      setError(err.message);
    } finally {
      setIsDownloadingToLocal(false);
    }
  }, [activeFileId, excalidrawAPI, isDownloadingToLocal, refreshFiles]);

  const syncToCloud = useCallback(async () => {
    if (!isCloudSyncEnabled) {
      onOpenSettings();
      return;
    }

    if (isManualSyncing || isSavingToCloud || isDownloadingToLocal) {
      return;
    }

    setError("");
    setIsManualSyncing(true);
    setStatus("saving");

    try {
      if (activeFileId && hasUnsavedChangesRef.current) {
        await saveActiveCanvas();
      }

      await cloudSyncBridge.triggerSync();
      const nextFiles = await refreshFiles();
      const activeFile = activeFileId
        ? nextFiles.find((file) => file.id === activeFileId)
        : undefined;
      const nextStatus = activeFile?.syncStatus ?? "synced";

      setFileStatusOverrides({});
      setStatus(nextStatus === "conflict" ? "pending-sync" : nextStatus);
      if (nextStatus === "synced") {
        setLastSyncTime(Date.now());
      }
    } catch (err: any) {
      setStatus("error");
      setError(err.message);
    } finally {
      setIsManualSyncing(false);
    }
  }, [
    activeFileId,
    isCloudSyncEnabled,
    isDownloadingToLocal,
    isManualSyncing,
    isSavingToCloud,
    onOpenSettings,
    refreshFiles,
    saveActiveCanvas,
  ]);

  useEffect(() => {
    refreshFiles()
      .then(async (loadedFiles) => {
        const availableFiles = await ensureFileListHasFileOnLoad(loadedFiles);
        setActiveFileId((current) => current ?? availableFiles[0]?.id ?? null);
      })
      .catch((err: Error) => setError(err.message));
  }, [ensureFileListHasFileOnLoad, refreshFiles]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listenToCloudSyncEvent<FileEntry[]>(
      "cloud-sync://file-list-changed",
      () => void refreshFiles(),
    ).then((listener) => {
      unlisten = listener;
    });

    return () => {
      unlisten?.();
    };
  }, [refreshFiles]);

  useEffect(() => {
    if (!excalidrawAPI) {
      return;
    }

    if (!activeFileId) {
      // No active file (e.g. user deleted the last file). Clear the editor
      // so the canvas doesn't keep showing stale content from the deleted
      // file. Reset the in-memory canvas snapshot too so any pending local
      // draft can't accidentally persist back to a now-gone file.
      console.debug("[cloud-sync] no active file, clearing editor");
      excalidrawAPI.updateScene({
        elements: [],
        appState: restoreCanvasAppState(),
        captureUpdate: CaptureUpdateAction.IMMEDIATELY,
      });
      latestCanvasRef.current = JSON.stringify(EMPTY_CANVAS);
      hasUnsavedChangesRef.current = false;
      setHasUnsavedChanges(false);
      setStatus(isCloudSyncEnabled ? "idle" : "local-only");
      return;
    }

    const draftCanvas = draftCanvasByFileIdRef.current.get(activeFileId);
    if (draftCanvas) {
      const canvas = parseCanvas(draftCanvas);
      applyCanvasToEditor(excalidrawAPI, canvas);
      latestCanvasRef.current = draftCanvas;
      hasUnsavedChangesRef.current = true;
      setHasUnsavedChanges(true);
      setStatus("pending-sync");
      return;
    }

    cloudSyncBridge
      .loadCanvas(activeFileId)
      .then((rawCanvas) => {
        const canvas = parseCanvas(rawCanvas);
        applyCanvasToEditor(excalidrawAPI, canvas, {
          scrollToFitContent: true,
        });
        latestCanvasRef.current = rawCanvas;
        hasUnsavedChangesRef.current = false;
        setHasUnsavedChanges(false);
      })
      .catch((err: Error) => {
        setStatus("error");
        setError(err.message);
      });
  }, [activeFileId, excalidrawAPI, isCloudSyncEnabled]);

  useEffect(() => {
    if (!isCloudSyncEnabled) {
      setStatus("local-only");
      return;
    }

    if (
      !activeFileId ||
      hasUnsavedChanges ||
      isSavingToCloud ||
      isDownloadingToLocal
    ) {
      return;
    }

    const fileStatus =
      fileStatusOverrides[activeFileId] ??
      files.find((file) => file.id === activeFileId)?.syncStatus ??
      "synced";

    setStatus(fileStatus === "conflict" ? "pending-sync" : fileStatus);
  }, [
    activeFileId,
    fileStatusOverrides,
    files,
    isCloudSyncEnabled,
    hasUnsavedChanges,
    isSavingToCloud,
    isDownloadingToLocal,
  ]);

  const handleChange = (
    elements: readonly OrderedExcalidrawElement[],
    appState: AppState,
    binaryFiles: BinaryFiles,
  ) => {
    const nextCanvas = serializeCanvas(elements, appState, binaryFiles);

    if (nextCanvas === latestCanvasRef.current) {
      return;
    }

    latestCanvasRef.current = nextCanvas;

    if (!activeFileId) {
      // No active file: don't schedule a save (it would either fail or, worse,
      // recreate a deleted file on disk via upsert).
      hasUnsavedChangesRef.current = false;
      setHasUnsavedChanges(false);
      return;
    }

    draftCanvasByFileIdRef.current.set(activeFileId, latestCanvasRef.current);
    hasUnsavedChangesRef.current = true;
    setHasUnsavedChanges(true);
    setFileStatusOverrides((current) => ({
      ...current,
      [activeFileId]: "pending-sync",
    }));
    setStatus("pending-sync");
  };

  const selectFile = async (fileId: string) => {
    if (fileId === activeFileId) {
      return;
    }

    try {
      setActiveFileId(fileId);
    } catch (err: any) {
      setStatus("error");
      setError(err.message);
    }
  };

  const createFile = async () => {
    try {
      const created = await cloudSyncBridge.createNewFile();
      setFiles((current) => [created, ...current]);
      setActiveFileId(created.id);
    } catch (err: any) {
      setStatus("error");
      setError(err.message);
    }
  };

  const importFile = async () => {
    try {
      const imported = await cloudSyncBridge.importFile();
      if (!imported) {
        return;
      }
      setFiles((current) => [imported, ...current]);
      setActiveFileId(imported.id);
      await refreshFiles();
    } catch (err: any) {
      setStatus("error");
      setError(err.message);
    }
  };

  const renameFile = async (fileId: string, newTitle: string) => {
    await cloudSyncBridge.renameFile(fileId, newTitle);
    await refreshFiles();
  };

  const deleteFile = async (fileId: string) => {
    if (deletingFileIdsRef.current.has(fileId)) {
      console.debug(
        "[cloud-sync] deleteFile ignored (already in flight):",
        fileId,
      );
      return;
    }

    deletingFileIdsRef.current.add(fileId);
    setDeletingFileIds(new Set(deletingFileIdsRef.current));

    const isDeletingActiveFile = fileId === activeFileId;
    console.debug(
      "[cloud-sync] deleteFile start:",
      fileId,
      "isActive=",
      isDeletingActiveFile,
    );

    try {
      if (isDeletingActiveFile) {
        // Drop the active file id immediately so any in-flight editor
        // updates cannot revive the file we are about to delete.
        setActiveFileId(null);
        hasUnsavedChangesRef.current = false;
        setHasUnsavedChanges(false);
        draftCanvasByFileIdRef.current.delete(fileId);
      }

      await cloudSyncBridge.deleteFile(fileId);
      const nextFiles = await refreshFiles();

      if (isDeletingActiveFile) {
        // Pick the next available file as the new active one if any remain.
        // If none remain we intentionally leave activeFileId = null and let
        // the editor render an empty canvas — DO NOT auto-create a fallback
        // here, otherwise the user can never actually empty the file list
        // (deleting the last entry would just spawn a fresh one each time).
        if (nextFiles.length > 0) {
          const sorted = [...nextFiles].sort(
            (a, b) => b.lastModified - a.lastModified,
          );
          console.debug(
            "[cloud-sync] deleteFile: switching active file to",
            sorted[0].id,
          );
          setActiveFileId(sorted[0].id);
        } else {
          console.debug(
            "[cloud-sync] deleteFile: file list is now empty, leaving active file null",
          );
        }
      }
    } catch (err: any) {
      console.error("[cloud-sync] deleteFile failed:", fileId, err);
      setStatus("error");
      setError(err.message);
    } finally {
      deletingFileIdsRef.current.delete(fileId);
      setDeletingFileIds(new Set(deletingFileIdsRef.current));
    }
  };

  const activeConflictCount = activeFileId
    ? countConflictCopies(files, activeFileId)
    : 0;

  const shareAsImage = useCallback(async () => {
    if (!excalidrawAPI || !activeFileId || isSharing) {
      return;
    }

    setIsSharing(true);
    setError("");

    try {
      const elements = excalidrawAPI.getSceneElements();
      const appState = excalidrawAPI.getAppState();
      const binaryFiles = excalidrawAPI.getFiles();

      const blob = await exportToBlob({
        elements,
        appState: {
          ...appState,
          exportBackground: true,
        },
        files: binaryFiles,
        mimeType: "image/png",
      });

      const arrayBuffer = await blob.arrayBuffer();
      const imageData = Array.from(new Uint8Array(arrayBuffer));

      const url = await cloudSyncBridge.uploadShareImage(
        activeFileId,
        imageData,
      );
      setShareUrl(url);
    } catch (err: any) {
      setError(err.message || "分享失败");
    } finally {
      setIsSharing(false);
    }
  }, [excalidrawAPI, activeFileId, isSharing]);
  const displayFiles = useMemo(
    () =>
      files.map((file) =>
        fileStatusOverrides[file.id]
          ? { ...file, syncStatus: fileStatusOverrides[file.id] }
          : file,
      ),
    [fileStatusOverrides, files],
  );

  return (
    <div className="cloud-sync-app">
      <FileListSidebar
        activeFileId={activeFileId}
        deletingFileIds={deletingFileIds}
        files={displayFiles}
        isCollapsed={isSidebarCollapsed}
        isCloudSyncEnabled={isCloudSyncEnabled}
        onFileDelete={deleteFile}
        onFileImport={importFile}
        onFileRename={renameFile}
        onFileSelect={selectFile}
        onNewFile={createFile}
        onOpenSettings={onOpenSettings}
        onToggleCollapse={() => setIsSidebarCollapsed((current) => !current)}
      />
      <main className="cloud-sync-editor">
        <div className="cloud-sync-toolbar">
          <SyncStatusIndicator
            lastSyncTime={lastSyncTime}
            status={isCloudSyncEnabled ? status : "local-only"}
          />
          <div className="cloud-sync-toolbar__actions">
            <button
              aria-busy={isManualSyncing}
              aria-label={
                isCloudSyncEnabled
                  ? isManualSyncing
                    ? "Push 中..."
                    : "Push 推送"
                  : "同步云端"
              }
              className="cloud-sync-icon-button"
              disabled={
                isCloudSyncEnabled &&
                (isManualSyncing || isSavingToCloud || isDownloadingToLocal)
              }
              onClick={() => void syncToCloud()}
              title={
                isCloudSyncEnabled
                  ? isManualSyncing
                    ? "Push 中..."
                    : "Push 推送"
                  : "同步云端"
              }
              type="button"
            >
              <ToolbarIcon name="cloud-upload" />
            </button>
            {isCloudSyncEnabled && activeFileId && (
              <button
                aria-label="Pull 拉取"
                className="cloud-sync-icon-button"
                disabled={
                  isManualSyncing || isSavingToCloud || isDownloadingToLocal
                }
                onClick={() => void downloadActiveCanvas()}
                title="Pull 拉取"
                type="button"
              >
                <ToolbarIcon name="cloud-download" />
              </button>
            )}
            {isCloudSyncEnabled && activeFileId && (
              <button
                aria-busy={isSharing}
                aria-label={isSharing ? "分享中..." : "分享"}
                className="cloud-sync-icon-button"
                disabled={isSharing || !excalidrawAPI}
                onClick={() => void shareAsImage()}
                title={isSharing ? "分享中..." : "分享"}
                type="button"
              >
                <ToolbarIcon name="share" />
              </button>
            )}
          </div>
          {activeConflictCount > 0 && (
            <div className="cloud-sync-conflicts" role="status">
              {activeConflictCount} 个冲突副本
            </div>
          )}
          {error && (
            <div className="cloud-sync-error" role="alert">
              {error}
            </div>
          )}
          {connectionNotice && (
            <div className="cloud-sync-connection-notice" role="status">
              {connectionNotice}
            </div>
          )}
        </div>
        <div className="cloud-sync-canvas">
          <Excalidraw
            autoFocus={true}
            detectScroll={false}
            handleKeyboardGlobally={true}
            onChange={handleChange}
          />
        </div>
        {shareUrl && (
          <div
            aria-labelledby="cloud-sync-share-title"
            aria-modal="true"
            className="cloud-sync-confirm"
            role="dialog"
          >
            <div className="cloud-sync-confirm__panel">
              <strong id="cloud-sync-share-title">分享链接</strong>
              <p className="cloud-sync-confirm__message">
                图片已上传，复制以下链接即可分享：
              </p>
              <input
                className="cloud-sync-share__url"
                onClick={(e) => (e.target as HTMLInputElement).select()}
                readOnly
                value={shareUrl}
              />
              <div className="cloud-sync-confirm__actions">
                <button
                  className="cloud-sync-confirm__cancel"
                  onClick={() => {
                    setShareUrl(null);
                    setIsCopied(false);
                  }}
                  type="button"
                >
                  关闭
                </button>
                <button
                  onClick={() => {
                    navigator.clipboard.writeText(shareUrl);
                    setIsCopied(true);
                    setTimeout(() => setIsCopied(false), 2000);
                  }}
                  type="button"
                >
                  {isCopied ? "已复制 ✓" : "复制链接"}
                </button>
              </div>
            </div>
          </div>
        )}
      </main>
    </div>
  );
};

export const CloudSyncApp = () => {
  const [isLoading, setIsLoading] = useState(true);
  const [cosConfig, setCosConfig] = useState<CosConfig | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [configError, setConfigError] = useState("");
  const [connectionNotice, setConnectionNotice] = useState("");

  useEffect(() => {
    cloudSyncBridge
      .getCosConfig()
      .then((config) => setCosConfig(config))
      .catch(() => setCosConfig(null))
      .finally(() => setIsLoading(false));
  }, []);

  useEffect(() => {
    if (!connectionNotice) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setConnectionNotice("");
    }, 2500);

    return () => window.clearTimeout(timeoutId);
  }, [connectionNotice]);

  const submitConfig = async (config: CosConfig) => {
    setConfigError("");
    setConnectionNotice("正在验证 COS 连接...");
    try {
      await cloudSyncBridge.validateCosConfig(config);
      await cloudSyncBridge.saveCosConfig(config);
      setConnectionNotice("云同步已连接。");
      setCosConfig(config);
      setIsSettingsOpen(false);
    } catch (err: any) {
      setConnectionNotice("");
      setConfigError(err.message);
    }
  };

  if (isLoading) {
    return <div className="cloud-sync-loading">加载中...</div>;
  }

  return (
    <CloudSyncErrorBoundary>
      <ExcalidrawAPIProvider>
        <CloudSyncEditor
          isCloudSyncEnabled={!!cosConfig}
          connectionNotice={connectionNotice}
          onOpenSettings={() => {
            setConfigError("");
            setConnectionNotice("");
            setIsSettingsOpen(true);
          }}
        />
        {isSettingsOpen && (
          <div aria-label="设置" className="cloud-sync-settings" role="dialog">
            <div className="cloud-sync-settings__panel">
              <nav className="cloud-sync-settings__menu">
                <strong>设置</strong>
                <button className="is-active" type="button">
                  COS 设置
                </button>
              </nav>
              <CosConfigForm
                error={configError}
                initialValues={cosConfig ?? undefined}
                onCancel={() => {
                  setConfigError("");
                  setIsSettingsOpen(false);
                }}
                onSubmit={submitConfig}
              />
            </div>
          </div>
        )}
      </ExcalidrawAPIProvider>
    </CloudSyncErrorBoundary>
  );
};
