import { useEffect, useState } from "react";

import { listenToCloudSyncEvent } from "../tauri-bridge";

import type { SyncStatus, SyncStatusIndicatorProps } from "../types";

const STATUS_TEXT: Record<SyncStatus, string> = {
  idle: "空闲",
  "local-only": "仅本地",
  saving: "同步中",
  synced: "已同步",
  "pending-sync": "待同步",
  error: "同步失败",
};

export const SyncStatusIndicator = ({
  status,
  lastSyncTime,
}: SyncStatusIndicatorProps) => {
  const [currentStatus, setCurrentStatus] = useState(status);
  const [currentLastSyncTime, setCurrentLastSyncTime] = useState(lastSyncTime);

  useEffect(() => {
    setCurrentStatus(status);
  }, [status]);

  useEffect(() => {
    setCurrentLastSyncTime(lastSyncTime);
  }, [lastSyncTime]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listenToCloudSyncEvent<{ status: SyncStatus; lastSyncTime?: number }>(
      "cloud-sync://sync-status",
      (payload) => {
        setCurrentStatus(payload.status);
        setCurrentLastSyncTime(payload.lastSyncTime);
      },
    ).then((listener) => {
      unlisten = listener;
    });

    return () => {
      unlisten?.();
    };
  }, []);

  return (
    <div className={`cloud-sync-status is-${currentStatus}`}>
      <span aria-hidden="true" className="cloud-sync-status__dot" />
      <span>{STATUS_TEXT[currentStatus]}</span>
      {currentLastSyncTime && currentStatus === "synced" && (
        <time dateTime={new Date(currentLastSyncTime).toISOString()}>
          {new Date(currentLastSyncTime).toLocaleTimeString()}
        </time>
      )}
    </div>
  );
};
