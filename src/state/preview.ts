// What Quick Look should show for a file, and where to read it from.
//
// The routing decision is the engine's, not this file's: `kindCategory` comes
// from the one table that also names the Kind column and picks the row glyph.
// Choosing a viewer by matching on the *label* — "Matroska Video" — is how two
// video formats end up handled differently by accident.

import type { Caps, Entry } from "./types";

/** How a file is previewed, or why it is not. */
export type Preview =
  | { kind: "text" }
  | { kind: "image" }
  | { kind: "video" }
  | { kind: "audio" }
  | { kind: "pdf" }
  /** Nothing can show it; the panel offers a download instead. */
  | { kind: "none"; reason: string };

/**
 * The URL the webview reads a file's bytes from.
 *
 * A custom protocol, never a command: file bytes must not cross IPC, and a
 * command's return value would mean holding the whole file in memory to show a
 * frame of it. Each path segment is encoded separately so a `/` in a name
 * cannot become a directory separator.
 */
export function streamUrl(path: string): string {
  const encoded = path.split("/").map(encodeURIComponent).join("/");
  return `excalibur-stream://localhost/${encoded}`;
}

/** Bytes of a text file read for the preview before scrolling continues it. */
export const TEXT_WINDOW = 1024 * 1024;

/**
 * Decide what to show.
 *
 * `caps.randomAccess` gates the media viewers rather than the whole panel: a
 * backend that cannot seek can still show text and an image, which it reads
 * from the start. Offering a scrub bar that cannot scrub would be worse than
 * not offering the viewer.
 */
export function previewFor(entry: Entry, caps: Caps): Preview {
  if (entry.kind === "dir") return { kind: "none", reason: "Folders have no preview" };
  switch (entry.kindCategory) {
    case "text":
    case "code":
      return { kind: "text" };
    case "image":
      return { kind: "image" };
    case "pdf":
      return { kind: "pdf" };
    case "video":
      return caps.randomAccess
        ? { kind: "video" }
        : { kind: "none", reason: "This share cannot seek, so video cannot play" };
    case "audio":
      return caps.randomAccess
        ? { kind: "audio" }
        : { kind: "none", reason: "This share cannot seek, so audio cannot play" };
    default:
      return { kind: "none", reason: `${entry.kindLabel} has no preview` };
  }
}

/**
 * Whether this app can hand the file to an editor (E12, E14-S2).
 *
 * The engine's category, never the extension and never the label — the same
 * table `previewFor` above routes on, so "what can be shown as text" and "what
 * can be edited as text" cannot drift apart.
 *
 * Editing means writing the file back afterwards, so a folder, a symlink or a
 * video is not a candidate: `editFile` would put 48 GB of container in front
 * of `$EDITOR` and then try to save it.
 */
export function editable(entry: Entry): boolean {
  if (entry.kind !== "file") return false;
  return entry.kindCategory === "text" || entry.kindCategory === "code";
}

/**
 * Decode bytes as text, sniffing the encoding.
 *
 * UTF-16 is detected by its byte-order mark, because a UTF-16 file decoded as
 * UTF-8 is not slightly wrong — it is unreadable, with a NUL between every
 * character. Everything else is tried as UTF-8 and falls back to Latin-1,
 * which never fails and is right often enough for a preview.
 */
export function decodeText(bytes: Uint8Array): string {
  if (bytes.length >= 2) {
    if (bytes[0] === 0xff && bytes[1] === 0xfe) {
      return new TextDecoder("utf-16le").decode(bytes.subarray(2));
    }
    if (bytes[0] === 0xfe && bytes[1] === 0xff) {
      return new TextDecoder("utf-16be").decode(bytes.subarray(2));
    }
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return new TextDecoder("latin1").decode(bytes);
  }
}

/**
 * Whether a byte range looks like something no text viewer should show.
 *
 * A NUL byte is the signal, as it is for `file(1)` and `grep`: text almost
 * never contains one and binaries almost always do. Only the first few
 * kilobytes are examined — a NUL a megabyte in does not change the answer for
 * a preview, and scanning the whole window costs more than it settles.
 */
export function looksBinary(bytes: Uint8Array): boolean {
  const window = bytes.subarray(0, 8192);
  return window.includes(0);
}
