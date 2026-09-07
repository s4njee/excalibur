// The sidebar's CATEGORIES section (XDL-E2-S1): the download queue, grouped
// the way the reference groups it, counted live from the window's transfer
// signal rather than a second store. The queue is the app's and every window
// sees the same rows; the *selected category* is the window's, like the path.

import type { Transfer, TransferStatus } from "./types";

/** The reference's five groups, in its order. */
export type DownloadCategory = "all" | "downloading" | "paused" | "completed" | "failed";

export const DOWNLOAD_CATEGORIES: { id: DownloadCategory; label: string }[] = [
  { id: "all", label: "All" },
  { id: "downloading", label: "Downloading" },
  { id: "paused", label: "Paused" },
  { id: "completed", label: "Completed" },
  { id: "failed", label: "Failed" },
];

/**
 * Which category a row belongs in.
 *
 * Downloading is queued *or* running: a row waiting for a worker is still a
 * download in flight, and splitting the two would double the section for a
 * distinction the footer already draws. Cancelled rows belong to no category
 * but All — they are neither finished work nor work awaiting an answer, only
 * a record until cleared.
 */
export function categorize(status: TransferStatus): DownloadCategory | null {
  switch (status) {
    case "queued":
    case "running":
      return "downloading";
    case "paused":
      return "paused";
    case "done":
      return "completed";
    case "failed":
      return "failed";
    default:
      return null;
  }
}

/** Live counts, one per category, for the sidebar rows. */
export function countByCategory(transfers: Transfer[]): Record<DownloadCategory, number> {
  const counts: Record<DownloadCategory, number> = {
    all: transfers.length,
    downloading: 0,
    paused: 0,
    completed: 0,
    failed: 0,
  };
  for (const transfer of transfers) {
    const category = categorize(transfer.status);
    if (category !== null && category !== "all") counts[category] += 1;
  }
  return counts;
}

/** The rows a category shows. `all` is the whole queue. */
export function filterByCategory(
  transfers: Transfer[],
  category: DownloadCategory,
): Transfer[] {
  if (category === "all") return transfers;
  return transfers.filter((transfer) => categorize(transfer.status) === category);
}

/**
 * How far through a row is, or `null` when the size is unknown (XDL-E2-S3).
 *
 * This is the engine's `Transfer::fraction` rule rather than a second one:
 * `None` when `bytes_total` is 0, clamped to `0.0..=1.0` otherwise, because a
 * bar that sits at zero for a whole transfer looks broken where an
 * indeterminate one is honest about not knowing. Both the sheet rows and the
 * downloads table read this, so the two cannot disagree about what counts as
 * knowing.
 */
export function fractionOf(transfer: { bytesDone: number; bytesTotal: number }): number | null {
  if (transfer.bytesTotal === 0) return null;
  return Math.min(1, Math.max(0, transfer.bytesDone / transfer.bytesTotal));
}

/**
 * The hostname of a URL with any credentials removed, for display under the
 * row's filename (XDL-E2-S2). `URL.hostname` strips userinfo by construction;
 * the manual path is for strings it refuses, and strips the same way — the
 * last `@` ends the credentials, never the host.
 */
export function redactUrlHost(url: string): string | null {
  try {
    const hostname = new URL(url).hostname;
    return hostname === "" ? null : hostname;
  } catch {
    const authority = url.split("://")[1]?.split(/[/:?#]/)[0] ?? "";
    if (authority === "") return null;
    const host = authority.includes("@") ? (authority.split("@").pop() ?? "") : authority;
    return host === "" ? null : host;
  }
}

/**
 * Where a row comes from, in one line for the table's file cell (XDL-E2-S2):
 * the origin host for downloads, the share's host for share rows, This Mac
 * for local origins. Legacy rows without endpoints resolve through the same
 * direction and mount the worker routes on.
 *
 * `null` only when nothing names an origin at all, which the cell draws as
 * "—" rather than inventing one.
 */
export function originHost(transfer: Transfer): string | null {
  const from = transfer.from;
  if (from !== undefined && from !== null) {
    switch (from.kind) {
      case "http":
        return redactUrlHost(from.url);
      case "share": {
        const host = from.mount?.split(":")[1];
        return host === undefined || host === "" ? "Share" : host;
      }
      case "local":
        return "This Mac";
    }
  }
  switch (transfer.direction) {
    case "down": {
      const host = transfer.mount?.split(":")[1];
      return host === undefined || host === "" ? "Share" : host;
    }
    case "up":
      return "This Mac";
    default:
      return null;
  }
}
