import { StrictMode } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { CloudSyncApp } from "../../CloudSyncApp";

import type { ReactNode } from "react";
import type { FileEntry } from "../../types";

const bridgeMocks = vi.hoisted(() => ({
  createNewFile: vi.fn(),
  deleteFile: vi.fn(),
  getCosConfig: vi.fn(),
  getFileList: vi.fn(),
  loadCanvas: vi.fn(),
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
    getCosConfig: bridgeMocks.getCosConfig,
    getFileList: bridgeMocks.getFileList,
    loadCanvas: bridgeMocks.loadCanvas,
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

  it("creates one replacement when deleting the last active file repeatedly", async () => {
    bridgeMocks.getFileList
      .mockResolvedValueOnce([onlyFile])
      .mockResolvedValue([]);

    render(<CloudSyncApp />);

    const deleteButton = await screen.findByLabelText("Delete Only file");
    fireEvent.click(deleteButton);
    fireEvent.click(deleteButton);

    await waitFor(() => {
      expect(bridgeMocks.deleteFile).toHaveBeenCalledTimes(2);
      expect(bridgeMocks.createNewFile).toHaveBeenCalledTimes(1);
    });
  });
});
