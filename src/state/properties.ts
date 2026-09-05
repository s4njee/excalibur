// What the Properties sheet states about a selection (E14-S4).
//
// Kept out of the component for the same reason `format.ts` is: the sheet's
// whole job is to be *correct*, and a number that is arrived at inside JSX can
// only be checked by rendering the sheet and reading it back.
//
// The one formatter here that is not in `format.ts` is `formatExactBytes`.
// Nothing else in the window shows an unrounded byte count — the Size column
// deliberately rounds — so it has no business being in the shared formatter
// module until a second caller wants it.

import type { Entry } from "./types";

export interface Summary {
  files: number;
  folders: number;
  /** Total size of the *files*. A folder's own contents are not walked. */
  bytes: number;
}

/**
 * What a selection adds up to.
 *
 * `bytes` is the sum over everything that is not a directory, which is the
 * same set the list shows a size for. Symlinks are counted as files rather
 * than given a third category: the list does not distinguish them either, and
 * a "2 files, 1 folder, 1 link" summary invents a taxonomy the design never
 * draws.
 *
 * A directory contributes `0`, and the sheet has to say so out loud. Sizing a
 * folder means walking it — a transfer's worth of round trips over SMB or NFS
 * — and the AC rules that out. The honest presentation of "we did not measure
 * this" is not `0 B`, which reads as an empty folder.
 */
export function summarise(entries: Entry[]): Summary {
  let files = 0;
  let folders = 0;
  let bytes = 0;
  for (const entry of entries) {
    if (entry.kind === "dir") {
      folders += 1;
      continue;
    }
    files += 1;
    bytes += entry.size;
  }
  return { files, folders, bytes };
}

/**
 * The newest modification time in a selection, or `null` when not one entry
 * carries a time.
 *
 * The newest rather than a range: a selection's "modified" answers "when did
 * any of this last change", which is the question a user asks before they
 * overwrite or delete it. An entry with no time is skipped rather than treated
 * as the epoch, which would make an unknown time win the comparison as the
 * *oldest* and silently drop out of a "newest" answer either way.
 */
export function newestModified(entries: Entry[]): number | null {
  let newest: number | null = null;
  for (const entry of entries) {
    if (entry.modifiedMs === null) continue;
    if (newest === null || entry.modifiedMs > newest) newest = entry.modifiedMs;
  }
  return newest;
}

/**
 * An exact byte count, grouped: `48,234,496,000 bytes`.
 *
 * Grouped by hand rather than with `toLocaleString`, because the grouping
 * character would then follow the machine's locale while every other number in
 * this window is fixed. Two formats for one figure in one sheet is worse than
 * either format.
 */
export function formatExactBytes(bytes: number): string {
  const grouped = `${Math.trunc(bytes)}`.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${grouped} ${bytes === 1 ? "byte" : "bytes"}`;
}

/**
 * Where a folder is, named the way the user would say it: `media/Documentaries`.
 *
 * The share root is the share's own name and not an empty string or a bare
 * `/`: "where is it?" answered with nothing is not an answer, and the path
 * field above the list names the root the same way.
 */
export function locationLabel(location: string, shareName: string): string {
  const trimmed = location.replace(/^\/+|\/+$/g, "");
  return trimmed === "" ? shareName : `${shareName}/${trimmed}`;
}

/**
 * An entry's share-relative path, marked as being relative to the share root.
 *
 * The leading `/` is the share root, not the server's. Without it a root-level
 * file reads as a bare name — indistinguishable from the Name row directly
 * above it, which is how a "Path" row comes to look broken.
 */
export function sharePath(path: string): string {
  return `/${path.replace(/^\/+/, "")}`;
}

/**
 * The count line for a multi-selection: `3 files, 2 folders`.
 *
 * A part with a zero count is dropped rather than shown as `0 folders`, which
 * invites the reader to wonder which folders. Both zero cannot happen — the
 * sheet never opens on an empty selection — but it is answered anyway so the
 * function is total.
 */
export function formatCountLine(summary: Summary): string {
  const parts: string[] = [];
  if (summary.files > 0) parts.push(`${summary.files} file${summary.files === 1 ? "" : "s"}`);
  if (summary.folders > 0) {
    parts.push(`${summary.folders} folder${summary.folders === 1 ? "" : "s"}`);
  }
  return parts.length === 0 ? "Nothing selected" : parts.join(", ");
}
