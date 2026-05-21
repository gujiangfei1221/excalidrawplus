import fc from "fast-check";

import type { FileEntry } from "../../types";
import {
  sortFilesByLastModifiedDesc,
  truncateFileTitle,
  validateFileTitle,
} from "../../utils";

const fileEntry = fc.record<FileEntry>({
  id: fc.uuid(),
  title: fc.string({ minLength: 1, maxLength: 120 }),
  lastModified: fc.integer({ min: 0, max: Number.MAX_SAFE_INTEGER }),
  syncStatus: fc.constantFrom("synced", "pending-sync", "conflict"),
  isConflictCopy: fc.boolean(),
  parentFileId: fc.option(fc.uuid(), { nil: undefined }),
});

describe("cloud-sync file list properties", () => {
  it("Property 9: File List Sort Order", () => {
    fc.assert(
      fc.property(fc.uniqueArray(fileEntry, { selector: (f) => f.lastModified }), (files) => {
        const sorted = sortFilesByLastModifiedDesc(files);

        for (let index = 1; index < sorted.length; index++) {
          expect(sorted[index - 1].lastModified).toBeGreaterThan(
            sorted[index].lastModified,
          );
        }
      }),
    );
  });

  it("Property 10: Title Display Truncation", () => {
    fc.assert(
      fc.property(fc.string({ maxLength: 200 }), (title) => {
        const displayed = truncateFileTitle(title);

        if (title.length > 50) {
          expect(displayed).toBe(`${title.slice(0, 50)}...`);
        } else {
          expect(displayed).toBe(title);
        }
      }),
    );
  });

  it("Property 11: Title Validation", () => {
    fc.assert(
      fc.property(fc.string({ maxLength: 160 }), (title) => {
        const error = validateFileTitle(title);

        if (!title.trim() || title.length > 100) {
          expect(error).toBeTruthy();
        } else {
          expect(error).toBeNull();
        }
      }),
    );
  });
});
