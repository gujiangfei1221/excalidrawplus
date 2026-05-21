export interface SaveBeforeSwitchInput<TCanvasData> {
  hasUnsavedChanges: boolean;
  activeFileId: string | null;
  canvasData: TCanvasData;
  saveCanvas: (fileId: string, data: TCanvasData) => Promise<unknown>;
}

export const saveBeforeSwitch = async <TCanvasData>({
  hasUnsavedChanges,
  activeFileId,
  canvasData,
  saveCanvas,
}: SaveBeforeSwitchInput<TCanvasData>) => {
  if (!hasUnsavedChanges || !activeFileId) {
    return false;
  }

  await saveCanvas(activeFileId, canvasData);
  return true;
};

export const createDebouncedSave = <TCanvasData>(
  delayMs: number,
  save: (data: TCanvasData) => void,
) => {
  let timeoutId: number | null = null;

  const schedule = (data: TCanvasData) => {
    if (timeoutId) {
      window.clearTimeout(timeoutId);
    }

    timeoutId = window.setTimeout(() => {
      timeoutId = null;
      save(data);
    }, delayMs);
  };

  const flush = () => {
    if (!timeoutId) {
      return false;
    }

    window.clearTimeout(timeoutId);
    timeoutId = null;
    return true;
  };

  return { schedule, flush };
};
