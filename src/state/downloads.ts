// The transfers pane's CATEGORIES tab strip (XDL-E2-S1): the queue, grouped
// the way the reference groups it, counted live from the window's transfer
// signal rather than a second store. The queue is the app's and every window
// sees the same rows; the *selected category* is the window's, like the path.

import { formatRate, formatSize } from "./format";
import type { Transfer, TransferDirection, TransferStatus } from "./types";

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
 * Whether the engine will accept this URL, said before Start is clicked
 * (XDL-E3-S1).
 *
 * This is the engine's `validate_http_url` rule mirrored, so a row the dialog
 * sends cannot be one the queue then refuses at enqueue time. The engine's
 * stance is deliberately strict and is quoted verbatim:
 *
 * - http/https only, and no whitespace or control characters;
 * - no credentials in the authority (`@`);
 * - no query or fragment at all — the engine treats them as a
 *   credential-smuggling vector and refuses the whole URL rather than a part.
 *
 * Returns the reason to show the user, or `null` when the URL is acceptable.
 */
export function validateDownloadUrl(url: string): string | null {
  const trimmed = url.trim();
  const lower = trimmed.toLowerCase();
  if (
    !(lower.startsWith("http://") || lower.startsWith("https://")) ||
    /[\s\u0000-\u001f\u007f-\u009f]/u.test(trimmed)
  ) {
    return "URL must start with http:// or https:// and contain no spaces";
  }
  const authority = trimmed.split("://")[1]?.split(/[/?#]/)[0] ?? "";
  if (authority === "" || authority.includes("@")) {
    return "download URL credentials are not supported";
  }
  if (trimmed.includes("?") || trimmed.includes("#")) {
    return "query and fragment URLs are not supported (the engine refuses them)";
  }
  return null;
}

/**
 * URL lines carried by a drop (XDL-E8-S1).
 *
 * `text/uri-list` permits comment lines beginning with `#`; plain text and
 * dropped `.txt` files use the same one-item-per-line shape. Preserve invalid
 * non-comment lines so the Add dialog can explain them instead of silently
 * discarding part of a dropped batch.
 */
export function downloadUrlLines(text: string): string[] {
  return text
    .split(/[\r\n]+/)
    .map((line) => line.trim())
    .filter((line) => line !== "" && !line.startsWith("#"));
}

/** A browser text drop is a download drop only when every line is a URL. */
export function validDownloadDrop(text: string): string[] {
  const urls = downloadUrlLines(text);
  return urls.length > 0 && urls.every((url) => validateDownloadUrl(url) === null)
    ? urls
    : [];
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

/**
 * What the drawer's progress cell reads (`twopane.md` XTP-E4-S2).
 *
 * The handoff's "67% · 112 MB/s", and the word `queued` for a row that has not
 * started — an empty track at 0% reads as a transfer that has stalled, which
 * is a different and worse thing to believe. A running row without a rate yet
 * shows the percentage alone rather than waiting for one.
 */
export function progressLabel(transfer: Transfer): string {
  if (transfer.status === "queued") return "queued";
  const fraction = fractionOf(transfer);
  // No total, so no percentage: the bytes that have landed are all this row
  // can honestly claim (XDL-E2-S3's rule, not a second one).
  if (fraction === null) return formatSize(transfer.bytesDone);
  const percent = `${Math.round(fraction * 100)}%`;
  if (transfer.status !== "running" || transfer.rate === null) return percent;
  return `${percent} · ${formatRate(transfer.rate)}`;
}

/**
 * A row's two ends, named (`twopane.md` XTP-E4-S2).
 *
 * The drawer's table gives a transfer a column per end, because with two panes
 * a transfer is *between* two places the user is looking at and "which file"
 * is only half the question.
 *
 * `Transfer` still carries `direction` and two bare paths for share rows —
 * `from`/`to` are set only by the HTTP route, and giving every row an endpoint
 * pair is E22-S3 (`backlog.md:107`), untouched. So the ends are read from the
 * endpoints where they exist and derived from the direction where they do not,
 * which is the same fallback `originHost` above already makes. Nothing is
 * invented: a row that names neither gets `null` and the cell draws a dash.
 */
export interface TransferEnds {
  /** The path on this machine. */
  local: string | null;
  /** The path on the other side: in the share, or the URL it came from. */
  remote: string | null;
  /** Which way the bytes move, for the direction cell. */
  direction: TransferDirection | null;
}

export function endsOf(transfer: Transfer): TransferEnds {
  const from = transfer.from;
  const to = transfer.to;
  if (from && to) {
    const direction: TransferDirection | null =
      from.kind === "local" ? "up" : to.kind === "local" ? "down" : null;
    return { local: pathOf(from.kind === "local" ? from : to), remote: pathOf(from.kind === "local" ? to : from), direction };
  }
  // Legacy share rows: the direction says which of the two paths is which.
  switch (transfer.direction) {
    case "up":
      return { local: transfer.sourcePath, remote: transfer.destPath, direction: "up" };
    case "down":
      return { local: transfer.destPath, remote: transfer.sourcePath, direction: "down" };
    default:
      return { local: null, remote: null, direction: null };
  }
}

/** One endpoint as a line of text: a path, or the URL an HTTP row came from. */
function pathOf(endpoint: NonNullable<Transfer["from"]>): string {
  return endpoint.kind === "http" ? endpoint.url : endpoint.path;
}

/**
 * Whether the queue still owes the user work.
 *
 * The same rule as the engine's `Status::active` — queued, running or paused —
 * and deliberately not "the queue is non-empty": a finished row is kept until
 * `Clear finished` takes it, so a queue that has ever been used is never empty
 * again. Opening the transfers pane on that meant it opened on every launch
 * for anyone who had ever downloaded anything (reported 2026-09-09).
 */
export function outstanding(transfers: Transfer[]): boolean {
  return transfers.some(
    (transfer) =>
      transfer.status === "queued" ||
      transfer.status === "running" ||
      transfer.status === "paused",
  );
}
