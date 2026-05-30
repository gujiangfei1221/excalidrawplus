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
        isCollapsed={false}
        onFileDelete={() => {}}
        onFileImport={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
        onToggleCollapse={() => {}}
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
        isCollapsed={false}
        onFileDelete={() => {}}
        onFileImport={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={onNewFile}
        onToggleCollapse={() => {}}
      />,
    );

    fireEvent.click(screen.getByLabelText("新建文件"));

    expect(onNewFile).toHaveBeenCalledTimes(1);
  });

  it("invokes import file action", () => {
    const onFileImport = vi.fn();
    render(
      <FileListSidebar
        activeFileId={null}
        files={files}
        isCollapsed={false}
        onFileDelete={() => {}}
        onFileImport={onFileImport}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
        onToggleCollapse={() => {}}
      />,
    );

    fireEvent.click(screen.getByLabelText("导入文件"));

    expect(onFileImport).toHaveBeenCalledTimes(1);
  });

  it("toggles collapsed state", () => {
    const onToggleCollapse = vi.fn();
    render(
      <FileListSidebar
        activeFileId={null}
        files={files}
        isCollapsed={false}
        onFileDelete={() => {}}
        onFileImport={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
        onToggleCollapse={onToggleCollapse}
      />,
    );

    fireEvent.click(screen.getByLabelText("收起侧边栏"));

    expect(onToggleCollapse).toHaveBeenCalledTimes(1);
  });

  it("renders file status as a color dot without visible text", () => {
    render(
      <FileListSidebar
        activeFileId="new"
        files={files}
        isCollapsed={false}
        onFileDelete={() => {}}
        onFileImport={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
        onToggleCollapse={() => {}}
      />,
    );

    const status = screen.getByLabelText("待同步");
    expect(status).toHaveTextContent("");
    expect(status).toHaveClass("cloud-sync-file__status", "is-pending-sync");
  });

  it("does not delete immediately but shows a confirmation dialog", () => {
    const onFileDelete = vi.fn();
    render(
      <FileListSidebar
        activeFileId="new"
        files={files}
        isCollapsed={false}
        onFileDelete={onFileDelete}
        onFileImport={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
        onToggleCollapse={() => {}}
      />,
    );

    fireEvent.click(screen.getByLabelText("删除 New file"));

    expect(onFileDelete).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(
      screen.getByText(/同时删除云端 COS 上的文件/),
    ).toBeInTheDocument();
  });

  it("cancels deletion without invoking the delete callback", () => {
    const onFileDelete = vi.fn();
    render(
      <FileListSidebar
        activeFileId="new"
        files={files}
        isCollapsed={false}
        onFileDelete={onFileDelete}
        onFileImport={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
        onToggleCollapse={() => {}}
      />,
    );

    fireEvent.click(screen.getByLabelText("删除 New file"));
    fireEvent.click(screen.getByText("取消"));

    expect(onFileDelete).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("deletes the file only after confirming in the dialog", () => {
    const onFileDelete = vi.fn();
    render(
      <FileListSidebar
        activeFileId="new"
        files={files}
        isCollapsed={false}
        onFileDelete={onFileDelete}
        onFileImport={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
        onToggleCollapse={() => {}}
      />,
    );

    fireEvent.click(screen.getByLabelText("删除 New file"));
    fireEvent.click(screen.getByText("确认删除"));

    expect(onFileDelete).toHaveBeenCalledTimes(1);
    expect(onFileDelete).toHaveBeenCalledWith("new");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("hides the cloud deletion warning when cloud sync is disabled", () => {
    render(
      <FileListSidebar
        activeFileId="new"
        files={files}
        isCloudSyncEnabled={false}
        isCollapsed={false}
        onFileDelete={() => {}}
        onFileImport={() => {}}
        onFileRename={() => {}}
        onFileSelect={() => {}}
        onNewFile={() => {}}
        onToggleCollapse={() => {}}
      />,
    );

    fireEvent.click(screen.getByLabelText("删除 New file"));

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(
      screen.queryByText(/同时删除云端 COS 上的文件/),
    ).not.toBeInTheDocument();
  });
});
