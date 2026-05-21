import fc from "fast-check";

import { saveBeforeSwitch } from "../../autosave";

describe("cloud-sync autosave properties", () => {
  it("Property 8: Save Before File Switch", async () => {
    await fc.assert(
      fc.asyncProperty(
        fc.boolean(),
        fc.option(fc.uuid(), { nil: null }),
        fc.string(),
        async (hasUnsavedChanges, activeFileId, canvasData) => {
          const calls: Array<{ fileId: string; data: string }> = [];

          const didSave = await saveBeforeSwitch({
            hasUnsavedChanges,
            activeFileId,
            canvasData,
            saveCanvas: async (fileId, data) => {
              calls.push({ fileId, data });
            },
          });

          if (hasUnsavedChanges && activeFileId) {
            expect(didSave).toBe(true);
            expect(calls).toEqual([{ fileId: activeFileId, data: canvasData }]);
          } else {
            expect(didSave).toBe(false);
            expect(calls).toEqual([]);
          }
        },
      ),
    );
  });
});
