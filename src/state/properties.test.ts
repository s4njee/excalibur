import { describe, expect, it } from "vitest";
import {
  formatCountLine,
  formatExactBytes,
  locationLabel,
  newestModified,
  sharePath,
  summarise,
} from "./properties";
import type { Entry, EntryKind } from "./types";

const entry = (name: string, kind: EntryKind, size: number, modifiedMs: number | null): Entry => ({
  name,
  path: name,
  kind,
  size,
  addedMs: null,
  modifiedMs,
  mode: null,
  linkTarget: null,
  kindLabel: kind === "dir" ? "Folder" : "Matroska Video",
  kindCategory: kind === "dir" ? "folder" : "video",
  hidden: false,
});

const file = (name: string, size: number, modifiedMs: number | null = null) =>
  entry(name, "file", size, modifiedMs);
const folder = (name: string, modifiedMs: number | null = null) =>
  entry(name, "dir", 0, modifiedMs);

describe("summarise", () => {
  it("counts folders apart from files and sizes only the files", () => {
    const summary = summarise([file("a.mkv", 100), folder("Docs"), file("b.srt", 23)]);
    expect(summary).toEqual({ files: 2, folders: 1, bytes: 123 });
  });

  it("never adds a folder's own size to the total", () => {
    // The interesting case is a backend that reports a non-zero size for a
    // directory — many do, it is the size of the directory *inode*, and it is
    // not the size of what is inside. Adding it would produce a total that is
    // wrong by an arbitrary amount and looks plausible.
    const inflated = { ...folder("Docs"), size: 4096 };
    expect(summarise([file("a.mkv", 1000), inflated]).bytes).toBe(1000);
  });

  it("counts a symlink as a file rather than inventing a third kind", () => {
    const link = entry("latest.mkv", "symlink", 12, null);
    const summary = summarise([link, folder("Docs")]);
    expect(summary.files).toBe(1);
    expect(summary.folders).toBe(1);
  });

  it("answers for a single entry as well as a selection", () => {
    // The sheet uses the same function on the single-entry path, so a summary
    // of one has to be right rather than merely unused.
    expect(summarise([file("a.mkv", 48_234_496_000)])).toEqual({
      files: 1,
      folders: 0,
      bytes: 48_234_496_000,
    });
  });

  it("is total on an empty list", () => {
    expect(summarise([])).toEqual({ files: 0, folders: 0, bytes: 0 });
  });
});

describe("newestModified", () => {
  it("takes the newest, not the first or the last", () => {
    const times = [1_700_000_000_000, 1_800_000_000_000, 1_750_000_000_000];
    const entries = times.map((ms, index) => file(`f${index}`, 0, ms));
    expect(newestModified(entries)).toBe(1_800_000_000_000);
  });

  it("skips entries with no time instead of ranking them as ancient", () => {
    // A `null` coerced to 0 would sort as 1970 and be silently ignored here,
    // but the same coercion elsewhere shows up as a file modified in 1970.
    // Skipping is the behaviour, and it is the behaviour that is asserted.
    expect(newestModified([file("a", 0, null), file("b", 0, 1_700_000_000_000)])).toBe(
      1_700_000_000_000,
    );
  });

  it("has no answer when nothing does", () => {
    expect(newestModified([file("a", 0, null)])).toBeNull();
    expect(newestModified([])).toBeNull();
  });
});

describe("formatExactBytes", () => {
  it("groups thousands the same way whatever the machine's locale is", () => {
    expect(formatExactBytes(48_234_496_000)).toBe("48,234,496,000 bytes");
    expect(formatExactBytes(999)).toBe("999 bytes");
    expect(formatExactBytes(1000)).toBe("1,000 bytes");
  });

  it("says byte, singular, for one", () => {
    expect(formatExactBytes(1)).toBe("1 byte");
    expect(formatExactBytes(0)).toBe("0 bytes");
  });
});

describe("locationLabel", () => {
  it("names the share root by the share rather than leaving it blank", () => {
    expect(locationLabel("", "media")).toBe("media");
  });

  it("hangs a subfolder off the share name", () => {
    expect(locationLabel("Documentaries/2024", "media")).toBe("media/Documentaries/2024");
  });

  it("does not double a slash if the caller supplied one", () => {
    expect(locationLabel("/Documentaries/", "media")).toBe("media/Documentaries");
  });
});

describe("sharePath", () => {
  it("marks the path as being relative to the share root", () => {
    expect(sharePath("Documentaries/Cosmos.mkv")).toBe("/Documentaries/Cosmos.mkv");
    expect(sharePath("Arrival.2016.mkv")).toBe("/Arrival.2016.mkv");
  });

  it("does not double the root slash", () => {
    expect(sharePath("/Arrival.2016.mkv")).toBe("/Arrival.2016.mkv");
  });
});

describe("formatCountLine", () => {
  it("reads as a sentence, singular and plural", () => {
    expect(formatCountLine({ files: 3, folders: 2, bytes: 0 })).toBe("3 files, 2 folders");
    expect(formatCountLine({ files: 1, folders: 1, bytes: 0 })).toBe("1 file, 1 folder");
  });

  it("leaves out the half that is zero", () => {
    // "4 files, 0 folders" makes a reader look for folders that are not there.
    expect(formatCountLine({ files: 4, folders: 0, bytes: 0 })).toBe("4 files");
    expect(formatCountLine({ files: 0, folders: 2, bytes: 0 })).toBe("2 folders");
  });
});
