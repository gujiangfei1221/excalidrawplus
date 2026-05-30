import type { FileEntry } from "./types";

export const MAX_DISPLAY_TITLE_LENGTH = 50;
export const MAX_FILE_TITLE_LENGTH = 100;

export const sortFilesByLastModifiedDesc = (files: readonly FileEntry[]) => {
  return [...files].sort((a, b) => b.lastModified - a.lastModified);
};

export const truncateFileTitle = (title: string) => {
  if (title.length <= MAX_DISPLAY_TITLE_LENGTH) {
    return title;
  }

  return `${title.slice(0, MAX_DISPLAY_TITLE_LENGTH)}...`;
};

export const validateFileTitle = (title: string) => {
  const trimmed = title.trim();

  if (!trimmed) {
    return "文件名不能为空。";
  }

  if (title.length > MAX_FILE_TITLE_LENGTH) {
    return "文件名不能超过 100 个字符。";
  }

  return null;
};

export const formatRelativeTime = (timestamp: number, now = Date.now()) => {
  const diffMs = Math.max(0, now - timestamp);
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;

  if (diffMs < minute) {
    return "刚刚";
  }

  if (diffMs < hour) {
    const minutes = Math.floor(diffMs / minute);
    return `${minutes} 分钟前`;
  }

  if (diffMs < day) {
    const hours = Math.floor(diffMs / hour);
    return `${hours} 小时前`;
  }

  const days = Math.floor(diffMs / day);
  return `${days} 天前`;
};

export const countConflictCopies = (
  files: readonly FileEntry[],
  parentFileId: string,
) => {
  return files.filter(
    (file) => file.isConflictCopy && file.parentFileId === parentFileId,
  ).length;
};
