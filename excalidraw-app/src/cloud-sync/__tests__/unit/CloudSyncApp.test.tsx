import { StrictMode } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { CloudSyncApp } from "../../CloudSyncApp";

import type { ReactNode } from "react";
import type { FileEntry } from "../../types";

const bridgeMocks = vi.hoisted(() => ({
  createNewFile: vi.fn(),
  deleteFile: vi.fn(),
  downloadCanvas: vi.fn(),
  getCosConfig: vi.fn(),
  getFileList: vi.fn(),
  loadCanvas: vi.fn(),
  saveCosConfig: vi.fn(),
  validateCosConfig: vi.fn(),
  saveCanvas: vi.fn(),
}));

vi.mock("@excalidraw/excalidraw", () => ({
  CaptureUpdateAction: {
    IMMEDIATELY: "IMMEDIATELY",
  },
  Excalidraw: () => <div data-testid="excalidraw-editor" />,
  ExcalidrawAPIProvider: ({ children }: { children: ReactNode }) => (
    <>{children}</>
  ),
  restoreAppState: (appState: unknown) => appState,
  restoreElements: (elements: unknown) => elements,
  serializeAsJSON: () => "{}",
  useExcalidrawAPI: () => ({
    addFiles: vi.fn(),
    updateScene: vi.fn(),
  }),
}));

vi.mock("../../tauri-bridge", () => ({
  cloudSyncBridge: {
    createNewFile: bridgeMocks.createNewFile,
    deleteFile: bridgeMocks.deleteFile,
    downloadCanvas: bridgeMocks.downloadCanvas,
    getCosConfig: bridgeMocks.getCosConfig,
    getFileList: bridgeMocks.getFileList,
    loadCanvas: bridgeMocks.loadCanvas,
    saveCosConfig: bridgeMocks.saveCosConfig,
    validateCosConfig: bridgeMocks.validateCosConfig,
    saveCanvas: bridgeMocks.saveCanvas,
  },
  listenToCloudSyncEvent: vi.fn(async () => () => {}),
}));

const createdFile: FileEntry = {
  id: "created",
  isConflictCopy: false,
  lastModified: 2,
  syncStatus: "pending-sync",
  title: "Created file",
};

const onlyFile: FileEntry = {
  id: "only",
  isConflictCopy: false,
  lastModified: 1,
  syncStatus: "synced",
  title: "Only file",
};

describe("CloudSyncApp file creation fallback", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    bridgeMocks.getCosConfig.mockResolvedValue(null);
    bridgeMocks.loadCanvas.mockResolvedValue(
      JSON.stringify({
        appState: {},
        elements: [],
        files: {},
      }),
    );
    bridgeMocks.createNewFile.mockResolvedValue(createdFile);
    bridgeMocks.deleteFile.mockResolvedValue(undefined);
    bridgeMocks.downloadCanvas.mockResolvedValue(
      JSON.stringify({
        appState: {},
        elements: [],
        files: {},
      }),
    );
    bridgeMocks.saveCosConfig.mockResolvedValue(undefined);
    bridgeMocks.validateCosConfig.mockResolvedValue(true);
    bridgeMocks.saveCanvas.mockResolvedValue("pending-sync");
    vi.spyOn(window, "confirm").mockReturnValue(true);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("creates one fallback file when the list is empty under StrictMode", async () => {
    bridgeMocks.getFileList.mockResolvedValue([]);

    render(
      <StrictMode>
        <CloudSyncApp />
      </StrictMode>,
    );

    await waitFor(() => {
      expect(bridgeMocks.createNewFile).toHaveBeenCalledTimes(1);
    });
  });

  it("does not auto-create a replacement when the user deletes the last file", async () => {
    bridgeMocks.getFileList
      .mockResolvedValueOnce([onlyFile])
      .mockResolvedValue([]);

    render(<CloudSyncApp />);

    const deleteButton = await screen.findByLabelText("Delete Only file");
    fireEvent.click(deleteButton);

    await waitFor(() => {
      expect(bridgeMocks.deleteFile).toHaveBeenCalledTimes(1);
    });

    // The user explicitly deleted the only file. We must respect their
    // intent and leave the list empty instead of spawning a replacement.
    expect(bridgeMocks.createNewFile).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(screen.getByText(/No files\./i)).toBeInTheDocument();
    });
  });

  it("ignores extra delete clicks while a delete is already in flight", async () => {
    bridgeMocks.getFileList
      .mockResolvedValueOnce([onlyFile])
      .mockResolvedValue([]);

    let resolveDelete: (() => void) | undefined;
    bridgeMocks.deleteFile.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolveDelete = resolve;
        }),
    );

    render(<CloudSyncApp />);

    const deleteButton = await screen.findByLabelText("Delete Only file");
    fireEvent.click(deleteButton);
    await waitFor(() => expect(deleteButton).toBeDisabled());
    // Spam-click while the first delete is still in flight; these MUST be
    // ignored — they used to indirectly trigger fallback file creation.
    fireEvent.click(deleteButton);
    fireEvent.click(deleteButton);
    fireEvent.click(deleteButton);
    resolveDelete?.();

    await waitFor(() => {
      expect(bridgeMocks.deleteFile).toHaveBeenCalledTimes(1);
    });
    expect(bridgeMocks.createNewFile).not.toHaveBeenCalled();
  });

  it("closes the settings dialog after a successful connection", async () => {
    bridgeMocks.getFileList.mockResolvedValue([]);

    render(<CloudSyncApp />);

    fireEvent.click(await screen.findByLabelText("Cloud sync settings"));

    fireEvent.change(screen.getByLabelText("SecretId"), {
      target: { value: "secret-id" },
    });
    fireEvent.change(screen.getByLabelText("SecretKey"), {
      target: { value: "secret-key" },
    });
    fireEvent.change(screen.getByLabelText("Bucket"), {
      target: { value: "bucket" },
    });
    fireEvent.change(screen.getByLabelText("Region"), {
      target: { value: "ap-shanghai" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));

    await waitFor(() => {
      expect(bridgeMocks.validateCosConfig).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(screen.getByText("Cloud Sync connected.")).toBeInTheDocument();
  });

  it("shows manual save and download actions when cloud sync is enabled", async () => {
    bridgeMocks.getCosConfig.mockResolvedValueOnce({
      secretId: "secret-id",
      secretKey: "secret-key",
      bucket: "bucket",
      region: "ap-shanghai",
    });
    bridgeMocks.getFileList.mockResolvedValue([onlyFile]);

    render(<CloudSyncApp />);

    const saveButton = await screen.findByRole("button", {
      name: "保存云端",
    });
    const downloadButton = screen.getByRole("button", { name: "下载本地" });

    fireEvent.click(saveButton);

    await waitFor(() => {
      expect(bridgeMocks.saveCanvas).toHaveBeenCalledTimes(1);
    });

    fireEvent.click(downloadButton);

    await waitFor(() => {
      expect(bridgeMocks.downloadCanvas).toHaveBeenCalledTimes(1);
    });
  });
});
