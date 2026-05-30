import {
  CaptureUpdateAction,
  Excalidraw,
  ExcalidrawAPIProvider,
  restoreAppState,
  restoreElements,
  serializeAsJSON,
  useExcalidrawAPI,
} from "@excalidraw/excalidraw";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { AppState, BinaryFiles } from "@excalidraw/excalidraw/types";
import type { OrderedExcalidrawElement } from "@excalidraw/element/types";

import { CosConfigForm } from "./components/CosConfigForm";
import { FileListSidebar } from "./components/FileListSidebar";
import { SyncStatusIndicator } from "./components/SyncStatusIndicator";
import { cloudSyncBridge, listenToCloudSyncEvent } from "./tauri-bridge";

import { countConflictCopies } from "./utils";

import "./cloud-sync.scss";

import type { CosConfig, FileEntry, SyncStatus } from "./types";

const EMPTY_CANVAS = {
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

const parseCanvas = (rawCanvas: string) => {
  try {
    return JSON.parse(rawCanvas || JSON.stringify(EMPTY_CANVAS));
  } catch {
    return EMPTY_CANVAS;
  }
};

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
  const [hasUnsavedChanges, setHasUnsavedChanges] = useState(false);
  const [fileStatusOverrides, setFileStatusOverrides] = useState<
    Record<string, FileEntry["syncStatus"]>
  >({});
  const [deletingFileIds, setDeletingFileIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
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
      excalidrawAPI?.updateScene({
        elements: restoreElements(canvas.elements || [], null, {
          repairBindings: true,
          deleteInvisibleElements: true,
        }),
        appState: restoreAppState(canvas.appState || {}, null),
        captureUpdate: CaptureUpdateAction.IMMEDIATELY,
      });
      if (canvas.files) {
        excalidrawAPI?.addFiles(Object.values(canvas.files) as any);
      }
      latestCanvasRef.current = rawCanvas;
      draftCanvasByFileIdRef.current.delete(activeFileId);
      hasUnsavedChangesRef.current = false;
      setHasUnsavedChanges(false);
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
        appState: restoreAppState({}, null),
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
      excalidrawAPI.updateScene({
        elements: restoreElements(canvas.elements || [], null, {
          repairBindings: true,
          deleteInvisibleElements: true,
        }),
        appState: restoreAppState(canvas.appState || {}, null),
        captureUpdate: CaptureUpdateAction.IMMEDIATELY,
      });
      if (canvas.files) {
        excalidrawAPI.addFiles(Object.values(canvas.files) as any);
      }
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
        excalidrawAPI.updateScene({
          elements: restoreElements(canvas.elements || [], null, {
            repairBindings: true,
            deleteInvisibleElements: true,
          }),
          appState: restoreAppState(canvas.appState || {}, null),
          captureUpdate: CaptureUpdateAction.IMMEDIATELY,
        });
        if (canvas.files) {
          excalidrawAPI.addFiles(Object.values(canvas.files) as any);
        }
        latestCanvasRef.current = rawCanvas;
        hasUnsavedChangesRef.current = false;
        setHasUnsavedChanges(false);
      })
      .catch((err: Error) => {
        setStatus("error");
        setError(err.message);
      });
  }, [activeFileId, excalidrawAPI]);

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
    latestCanvasRef.current = serializeCanvas(elements, appState, binaryFiles);

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
        onFileRename={renameFile}
        onFileSelect={selectFile}
        onNewFile={createFile}
        onOpenSettings={onOpenSettings}
        onToggleCollapse={() =>
          setIsSidebarCollapsed((current) => !current)
        }
      />
      <main className="cloud-sync-editor">
        <div className="cloud-sync-toolbar">
          <SyncStatusIndicator
            lastSyncTime={lastSyncTime}
            status={isCloudSyncEnabled ? status : "local-only"}
          />
          {isCloudSyncEnabled && activeFileId && (
            <div className="cloud-sync-toolbar__actions">
              <button
                disabled={isSavingToCloud || isDownloadingToLocal}
                onClick={() => void saveActiveCanvas()}
                type="button"
              >
                保存云端
              </button>
              <button
                disabled={isSavingToCloud || isDownloadingToLocal}
                onClick={() => void downloadActiveCanvas()}
                type="button"
              >
                下载本地
              </button>
            </div>
          )}
          {activeConflictCount > 0 && (
            <div className="cloud-sync-conflicts" role="status">
              {activeConflictCount} conflict copy
              {activeConflictCount > 1 ? "ies" : ""}
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
    setConnectionNotice("Checking the COS connection...");
    try {
      await cloudSyncBridge.validateCosConfig(config);
      await cloudSyncBridge.saveCosConfig(config);
      setConnectionNotice("Cloud Sync connected.");
      setCosConfig(config);
      setIsSettingsOpen(false);
    } catch (err: any) {
      setConnectionNotice("");
      setConfigError(err.message);
    }
  };

  if (isLoading) {
    return <div className="cloud-sync-loading">Loading...</div>;
  }

  return (
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
        <div className="cloud-sync-settings" role="dialog">
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
      )}
    </ExcalidrawAPIProvider>
  );
};
