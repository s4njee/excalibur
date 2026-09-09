import { describe, expect, it } from "vitest";
import { DEFAULT_SORT, sortEntries, toggle } from "./sort";
import type { Entry } from "./types";

const entry = (over: Partial<Entry> & { name: string }): Entry => ({
  path: over.name,
  kind: "file",
  size: 0,
  modifiedMs: null,
  addedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: "Document",
  kindCategory: "other",
  hidden: false,
  ...over,
});

const names = (entries: Entry[]) => entries.map((e) => e.name);

describe("toggle", () => {
  it("flips the direction when the same column is clicked again", () => {
    expect(toggle(DEFAULT_SORT, "name")).toEqual({ column: "name", ascending: false });
  });

  it("starts names and kinds ascending", () => {
    expect(toggle({ column: "size", ascending: true }, "name").ascending).toBe(true);
    expect(toggle({ column: "size", ascending: true }, "kind").ascending).toBe(true);
  });

  it("starts size and modified descending", () => {
    // A first click on Size means "biggest"; on Modified it means "newest".
    // Starting these ascending makes every user click twice.
    expect(toggle(DEFAULT_SORT, "size").ascending).toBe(false);
    expect(toggle(DEFAULT_SORT, "modified").ascending).toBe(false);
  });
});

describe("sortEntries", () => {
  it("sorts names naturally, so 10 follows 9", () => {
    const entries = [entry({ name: "File 10" }), entry({ name: "File 9" }), entry({ name: "File 1" })];
    expect(names(sortEntries(entries, DEFAULT_SORT, false))).toEqual(["File 1", "File 9", "File 10"]);
  });

  it("keeps folders first in both directions", () => {
    // "Folders first" is a preference about grouping, not a claim that folders
    // sort before files — reversing must not sink them to the bottom.
    const entries = [
      entry({ name: "b.txt" }),
      entry({ name: "a-dir", kind: "dir" }),
      entry({ name: "a.txt" }),
    ];
    expect(names(sortEntries(entries, { column: "name", ascending: true }, true))[0]).toBe("a-dir");
    expect(names(sortEntries(entries, { column: "name", ascending: false }, true))[0]).toBe("a-dir");
  });

  it("breaks ties by name rather than by the order the backend returned", () => {
    // Every folder has size 0. Without a tiebreak they would shuffle between
    // two listings of an unchanged directory.
    const entries = [entry({ name: "c" }), entry({ name: "a" }), entry({ name: "b" })];
    const sorted = sortEntries(entries, { column: "size", ascending: true }, false);
    expect(names(sorted)).toEqual(["a", "b", "c"]);
    // And the same answer for the same input, whatever order it arrives in.
    const shuffled = [entry({ name: "b" }), entry({ name: "c" }), entry({ name: "a" })];
    expect(names(sortEntries(shuffled, { column: "size", ascending: true }, false))).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("sorts sizes numerically, not as strings", () => {
    const entries = [entry({ name: "big", size: 1000 }), entry({ name: "small", size: 9 })];
    expect(names(sortEntries(entries, { column: "size", ascending: true }, false))).toEqual([
      "small",
      "big",
    ]);
  });

  it("puts a file with no timestamp oldest rather than at the epoch", () => {
    // 1970 is a date a real file could have; "unknown" is not the same thing.
    const entries = [
      entry({ name: "known", modifiedMs: 0 }),
      entry({ name: "unknown", modifiedMs: null }),
    ];
    expect(names(sortEntries(entries, { column: "modified", ascending: true }, false))).toEqual([
      "unknown",
      "known",
    ]);
  });

  it("does not mutate the listing it was given", () => {
    const entries = [entry({ name: "b" }), entry({ name: "a" })];
    sortEntries(entries, DEFAULT_SORT, false);
    expect(names(entries)).toEqual(["b", "a"]);
  });
});
