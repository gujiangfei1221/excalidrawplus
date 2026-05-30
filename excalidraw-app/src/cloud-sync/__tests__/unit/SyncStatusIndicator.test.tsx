import { render, screen } from "@testing-library/react";

import { SyncStatusIndicator } from "../../components/SyncStatusIndicator";

vi.mock("../../tauri-bridge", () => ({
  listenToCloudSyncEvent: vi.fn(async () => () => {}),
}));

describe("SyncStatusIndicator", () => {
  it.each([
    ["idle", "空闲"],
    ["saving", "同步中"],
    ["synced", "已同步"],
    ["pending-sync", "待同步"],
    ["error", "同步失败"],
  ] as const)("renders %s status", (status, text) => {
    render(<SyncStatusIndicator status={status} />);

    expect(screen.getByText(text)).toBeInTheDocument();
  });

  it("shows last sync time when synced", () => {
    const { container } = render(
      <SyncStatusIndicator lastSyncTime={1_700_000_000_000} status="synced" />,
    );

    expect(screen.getByText("已同步")).toBeInTheDocument();
    expect(container.querySelector("time")).toBeInTheDocument();
  });
});
