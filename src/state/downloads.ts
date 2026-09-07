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
