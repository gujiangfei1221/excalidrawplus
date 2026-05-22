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
import { createDebouncedSave, saveBeforeSwitch } from "./autosave";
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
}: {
  isCloudSyncEnabled: boolean;
  onOpenSettings: () => void;
}) => {
  const excalidrawAPI = useExcalidrawAPI();
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [activeFileId, setActiveFileId] = useState<string | null>(null);
  const [status, setStatus] = useState<SyncStatus>("idle");
  const [lastSyncTime, setLastSyncTime] = useState<number | undefined>();
  const [error, setError] = useState("");
  const hasUnsavedChangesRef = useRef(false);
  const latestCanvasRef = useRef(JSON.stringify(EMPTY_CANVAS));
  const fallbackFilePromiseRef = useRef<Promise<FileEntry> | null>(null);

  const refreshFiles = useCallback(async () => {
    const nextFiles = await cloudSyncBridge.getFileList();
    setFiles(nextFiles);
    return nextFiles;
  }, []);

  const createFallbackFile = useCallback(() => {
    if (!fallbackFilePromiseRef.current) {
      fallbackFilePromiseRef.current = cloudSyncBridge
        .createNewFile()
        .finally(() => {
          fallbackFilePromiseRef.current = null;
        });
    }

    return fallbackFilePromiseRef.current;
  }, []);

  const ensureFileListHasFile = useCallback(
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
    if (!activeFileId) {
      return;
    }

    setStatus("saving");
    const nextStatus = await cloudSyncBridge.saveCanvas(
      activeFileId,
      latestCanvasRef.current,
    );
    hasUnsavedChangesRef.current = false;
    setStatus(nextStatus);
    if (nextStatus === "synced") {
      setLastSyncTime(Date.now());
    }
    await refreshFiles();
  }, [activeFileId, refreshFiles]);

  const debouncedSave = useMemo(
    () => createDebouncedSave(2000, () => void saveActiveCanvas()),
    [saveActiveCanvas],
  );

  useEffect(() => {
    refreshFiles()
      .then(async (loadedFiles) => {
        const availableFiles = await ensureFileListHasFile(loadedFiles);
        setActiveFileId((current) => current ?? availableFiles[0]?.id ?? null);
      })
      .catch((err: Error) => setError(err.message));
  }, [ensureFileListHasFile, refreshFiles]);

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
    if (!activeFileId || !excalidrawAPI) {
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
      })
      .catch((err: Error) => {
        setStatus("error");
        setError(err.message);
      });
  }, [activeFileId, excalidrawAPI]);

  const handleChange = (
    elements: readonly OrderedExcalidrawElement[],
    appState: AppState,
    binaryFiles: BinaryFiles,
  ) => {
    latestCanvasRef.current = serializeCanvas(elements, appState, binaryFiles);
    hasUnsavedChangesRef.current = true;
    setStatus("saving");
    debouncedSave.schedule(latestCanvasRef.current);
  };

  const selectFile = async (fileId: string) => {
    if (fileId === activeFileId) {
      return;
    }

    try {
      await saveBeforeSwitch({
        hasUnsavedChanges: hasUnsavedChangesRef.current,
        activeFileId,
        canvasData: latestCanvasRef.current,
        saveCanvas: cloudSyncBridge.saveCanvas,
      });
      hasUnsavedChangesRef.current = false;
      setActiveFileId(fileId);
      await refreshFiles();
    } catch (err: any) {
      setStatus("error");
      setError(err.message);
    }
  };

  const createFile = async () => {
    try {
      await saveBeforeSwitch({
        hasUnsavedChanges: hasUnsavedChangesRef.current,
        activeFileId,
        canvasData: latestCanvasRef.current,
        saveCanvas: cloudSyncBridge.saveCanvas,
      });
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
    if (fileId === activeFileId) {
      debouncedSave.flush();
      hasUnsavedChangesRef.current = false;
    }

    await cloudSyncBridge.deleteFile(fileId);
    const nextFiles = await refreshFiles();
    if (fileId === activeFileId) {
      const availableFiles = await ensureFileListHasFile(nextFiles);
      setActiveFileId(availableFiles[0]?.id ?? null);
    }
  };

  const activeConflictCount = activeFileId
    ? countConflictCopies(files, activeFileId)
    : 0;

  return (
    <div className="cloud-sync-app">
      <FileListSidebar
        activeFileId={activeFileId}
        files={files}
        isCloudSyncEnabled={isCloudSyncEnabled}
        onFileDelete={deleteFile}
        onFileRename={renameFile}
        onFileSelect={selectFile}
        onNewFile={createFile}
        onOpenSettings={onOpenSettings}
      />
      <main className="cloud-sync-editor">
        <div className="cloud-sync-toolbar">
          <SyncStatusIndicator
            lastSyncTime={lastSyncTime}
            status={isCloudSyncEnabled ? status : "local-only"}
          />
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

  useEffect(() => {
    cloudSyncBridge
      .getCosConfig()
      .then((config) => setCosConfig(config))
      .catch(() => setCosConfig(null))
      .finally(() => setIsLoading(false));
  }, []);

  const submitConfig = async (config: CosConfig) => {
    setConfigError("");
    try {
      await cloudSyncBridge.validateCosConfig(config);
      await cloudSyncBridge.saveCosConfig(config);
      setCosConfig(config);
      setIsSettingsOpen(false);
    } catch (err: any) {
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
        onOpenSettings={() => {
          setConfigError("");
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
