import { fireEvent, render, screen } from "@testing-library/react";

import { FileListSidebar } from "../../components/FileListSidebar";
import type { FileEntry } from "../../types";

const files: FileEntry[] = [
  {
    id: "old",
    title: "Old file",
    lastModified: 1,
    syncStatus: "synced",
    isConflictCopy: false,
  },
  {
    id: "new",
    title: "New file",
    lastModified: 2,
    syncStatus: "pending-sync",
    isConflictCopy: false,
  },
];

describe("FileListSidebar", () => {
  it("renders files newest first", () => {
    render(
      <FileListSidebar
        activeFileId="new"
        files={files}
        onFileDelete={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
      />,
    );

    const titles = screen.getAllByText(/file$/).map((node) => node.textContent);
    expect(titles).toEqual(["New file", "Old file"]);
  });

  it("invokes new file action", () => {
    const onNewFile = vi.fn();
    render(
      <FileListSidebar
        activeFileId={null}
        files={files}
        onFileDelete={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={onNewFile}
      />,
    );

    fireEvent.click(screen.getByLabelText("New file"));

    expect(onNewFile).toHaveBeenCalledTimes(1);
  });
});
