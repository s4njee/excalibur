import { describe, expect, it } from "vitest";
import { decodeText, looksBinary, previewFor, streamUrl } from "./preview";
import type { Caps, Entry } from "./types";

const caps = (randomAccess: boolean): Caps => ({
  randomAccess,
  resumeWrite: true,
  atomicRename: true,
  symlinks: true,
  watch: false,
});

const entry = (over: Partial<Entry>): Entry => ({
  name: "x",
  path: "x",
  kind: "file",
  size: 1,
  modifiedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: "Document",
  kindCategory: "other",
  hidden: false,
  ...over,
});

describe("previewFor", () => {
  it("routes on the engine's category, not on the label", () => {
    // The label is prose for a person. Two video formats with different
    // labels must reach the same viewer.
    expect(previewFor(entry({ kindCategory: "video", kindLabel: "Matroska Video" }), caps(true))).toEqual({ kind: "video" });
    expect(previewFor(entry({ kindCategory: "video", kindLabel: "MPEG-4 Video" }), caps(true))).toEqual({ kind: "video" });
  });

  it("previews code as text", () => {
    expect(previewFor(entry({ kindCategory: "code" }), caps(true))).toEqual({ kind: "text" });
  });

  it("refuses media when the share cannot seek, but still shows text and images", () => {
    // A scrub bar that cannot scrub is worse than no viewer. Text and images
    // are read from the start, so they are unaffected.
    expect(previewFor(entry({ kindCategory: "video" }), caps(false)).kind).toBe("none");
    expect(previewFor(entry({ kindCategory: "audio" }), caps(false)).kind).toBe("none");
    expect(previewFor(entry({ kindCategory: "text" }), caps(false)).kind).toBe("text");
    expect(previewFor(entry({ kindCategory: "image" }), caps(false)).kind).toBe("image");
  });

  it("names the kind when there is nothing to show", () => {
    const preview = previewFor(entry({ kindCategory: "archive", kindLabel: "ZIP Archive" }), caps(true));
    expect(preview).toEqual({ kind: "none", reason: "ZIP Archive has no preview" });
  });

  it("has no preview for a folder", () => {
    expect(previewFor(entry({ kind: "dir", kindCategory: "folder" }), caps(true)).kind).toBe("none");
  });
});

describe("streamUrl", () => {
  it("encodes each segment separately", () => {
    // A `/` inside a name must not become a directory separator, and a space
    // or `#` must survive the trip.
    expect(streamUrl("a b/c#d.mkv")).toBe("excalibur-stream://localhost/a%20b/c%23d.mkv");
  });
});

describe("decodeText", () => {
  const utf8 = (text: string) => new TextEncoder().encode(text);

  it("reads UTF-8", () => {
    expect(decodeText(utf8("héllo — ok"))).toBe("héllo — ok");
  });

  it("reads UTF-16 by its byte-order mark", () => {
    // Decoded as UTF-8 this is not slightly wrong, it is unreadable.
    const bytes = new Uint8Array([0xff, 0xfe, 0x68, 0x00, 0x69, 0x00]);
    expect(decodeText(bytes)).toBe("hi");
  });

  it("falls back to Latin-1 rather than failing", () => {
    // 0xe9 is é in Latin-1 and invalid on its own in UTF-8.
    expect(decodeText(new Uint8Array([0x63, 0x61, 0x66, 0xe9]))).toBe("café");
  });
});

describe("looksBinary", () => {
  it("calls a NUL byte binary", () => {
    expect(looksBinary(new Uint8Array([0x68, 0x00, 0x69]))).toBe(true);
  });

  it("leaves text alone", () => {
    expect(looksBinary(new TextEncoder().encode("plain text\nwith lines\n"))).toBe(false);
  });

  it("only looks at the start", () => {
    // A NUL a megabyte in does not change the answer for a preview, and
    // scanning the whole window costs more than it settles.
    const bytes = new Uint8Array(200_000);
    bytes.fill(0x61);
    bytes[100_000] = 0;
    expect(looksBinary(bytes)).toBe(false);
  });
});
