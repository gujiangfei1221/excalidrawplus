import type { CosConfig, FileEntry, SyncStatus } from "./types";

type Unlisten = () => void;

type TauriCore = {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
};

type TauriEvent = {
  listen: <T>(
    event: string,
    handler: (event: { payload: T }) => void,
  ) => Promise<Unlisten>;
};

const loadTauriCore = async (): Promise<TauriCore | null> => {
  try {
    return await import("@tauri-apps/api/core");
  } catch {
    return null;
  }
};

const loadTauriEvent = async (): Promise<TauriEvent | null> => {
  try {
    return await import("@tauri-apps/api/event");
  } catch {
    return null;
  }
};

const invoke = async <T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> => {
  const tauri = await loadTauriCore();

  if (!tauri) {
    throw new Error("Tauri runtime is not available.");
  }

  return tauri.invoke<T>(command, args);
};

export const cloudSyncBridge = {
  saveCosConfig: (config: CosConfig) =>
    invoke<void>("save_cos_config", { config }),
  validateCosConfig: (config: CosConfig) =>
    invoke<boolean>("validate_cos_config", { config }),
  getCosConfig: () => invoke<CosConfig | null>("get_cos_config"),
  saveCanvas: (fileId: string, data: string) =>
    invoke<SyncStatus>("save_canvas", { fileId, data }),
  loadCanvas: (fileId: string) => invoke<string>("load_canvas", { fileId }),
  createNewFile: () => invoke<FileEntry>("create_new_file"),
  deleteFile: (fileId: string) => invoke<void>("delete_file", { fileId }),
  renameFile: (fileId: string, newTitle: string) =>
    invoke<void>("rename_file", { fileId, newTitle }),
  exportFile: (fileId: string) => invoke<void>("export_file", { fileId }),
  getFileList: () => invoke<FileEntry[]>("get_file_list"),
  triggerSync: () => invoke<void>("trigger_sync"),
  getSyncStatus: (fileId: string) =>
    invoke<SyncStatus>("get_sync_status", { fileId }),
};

export const listenToCloudSyncEvent = async <T>(
  eventName: "cloud-sync://sync-status" | "cloud-sync://file-list-changed" | "cloud-sync://connectivity",
  handler: (payload: T) => void,
) => {
  const tauriEvent = await loadTauriEvent();

  if (!tauriEvent) {
    return () => {};
  }

  return tauriEvent.listen<T>(eventName, (event) => handler(event.payload));
};
