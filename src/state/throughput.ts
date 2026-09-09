// The downloads footer's numbers (XDL-E2-S4). Pure, so the exact strings the
// footer shows are testable without rendering anything. The one fact this
// module deliberately does not have is a connection count: the queue's
// `Transfer` carries no per-download connection state — that is the engine's
// (`xsync-download` XD-E2-S4), unplumbed to the row — so the footer says
// nothing rather than invent one.

import type { Transfer } from "./types";

/** Rows currently moving bytes. */
export function activeCount(transfers: Transfer[]): number {
  return transfers.filter((transfer) => transfer.status === "running").length;
}

/**
 * The aggregate speed, of what is *moving*. Paused rows at zero would drag it
 * down and misdescribe the link; only running rows contribute their rate.
 */
export function aggregateRate(transfers: Transfer[]): number {
  return transfers
    .filter((transfer) => transfer.status === "running")
    .reduce((total, transfer) => total + (transfer.rate ?? 0), 0);
}

/**
 * Bytes still to land, across running transfers with a known size. Rows that
 * cannot name a total contribute nothing — "remaining" is a claim about a
 * plan, and a sizeless row has none (XDL-E2-S3's rule, not a second one).
 */
export function remainingBytes(transfers: Transfer[]): number {
  return transfers
    .filter((transfer) => transfer.status === "running")
    .reduce(
      (total, transfer) =>
        total + (transfer.bytesTotal > 0 ? Math.max(0, transfer.bytesTotal - transfer.bytesDone) : 0),
      0,
    );
}

/** Rows waiting for a worker. */
export function queuedCount(transfers: Transfer[]): number {
  return transfers.filter((transfer) => transfer.status === "queued").length;
}

/** Rows that stopped with an error, and are still in the queue to say so. */
export function failedCount(transfers: Transfer[]): number {
  return transfers.filter((transfer) => transfer.status === "failed").length;
}

/**
 * How long the whole queue has left, in seconds (`twopane.md` XTP-E4-S2).
 *
 * The bytes still to move over the rate they are moving at. `null` when
 * nothing is moving or the link is reporting nothing: an ETA computed from a
 * rate of zero is infinite, and drawing that as a number would be a promise
 * the queue cannot keep.
 *
 * Deliberately not a sum of the rows' own `etaSeconds`: those run in parallel,
 * so adding them would over-report by the number of workers.
 */
export function aggregateEtaSeconds(transfers: Transfer[]): number | null {
  const rate = aggregateRate(transfers);
  if (rate <= 0) return null;
  const remaining = remainingBytes(transfers);
  return remaining <= 0 ? null : remaining / rate;
}

/**
 * A fixed-capacity ring of samples: push the newest value, drop the oldest.
 *
 * The reference's footer draws the last 60 samples; the caller samples the
 * aggregate rate on a timer and keeps this bounded, so the sparkline cannot
 * grow with the app's uptime.
 */
export function pushSample(samples: number[], value: number, cap = 60): number[] {
  const next = samples.length < cap ? [...samples, value] : [...samples.slice(1), value];
  return next;
}

/**
 * The sparkline's path across a `width`×`height` viewBox, oldest to newest.
 *
 * Decorative by design: a smooth-enough line drawn from the samples, with a
 * hair of padding so the peaks do not touch the frame. Returns `""` for
 * fewer than two samples, which draws nothing rather than a single dot.
 */
export function sparklinePath(samples: number[], width: number, height: number): string {
  if (samples.length < 2) return "";
  const max = Math.max(...samples, 1);
  const pad = 1;
  const innerWidth = width - pad * 2;
  const innerHeight = height - pad * 2;
  const step = samples.length > 1 ? innerWidth / (samples.length - 1) : 0;
  return samples
    .map((value, index) => {
      const x = pad + index * step;
      const y = pad + innerHeight - (value / max) * innerHeight;
      return `${index === 0 ? "M" : "L"}${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(" ");
}
