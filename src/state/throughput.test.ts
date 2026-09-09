import { describe, expect, it } from "vitest";
import {
  activeCount,
  aggregateEtaSeconds,
  aggregateRate,
  failedCount,
  queuedCount,
  pushSample,
  remainingBytes,
  sparklinePath,
} from "./throughput";
import type { Transfer, TransferStatus } from "./types";

function row(id: string, status: TransferStatus, overrides: Partial<Transfer> = {}): Transfer {
  return {
    id,
    direction: "down",
    from: { kind: "share", mount: "smb:nas.local:media", path: `remote/${id}` },
    to: { kind: "local", path: `/local/${id}` },
    name: id,
    sourcePath: `remote/${id}`,
    destPath: `/local/${id}`,
    bytesDone: 0,
    bytesTotal: 100,
    rate: null,
    etaSeconds: null,
    status,
    error: null,
    queuedAtMs: 1,
    mount: "smb:nas.local:media",
    ...overrides,
  };
}

describe("the footer's numbers (XDL-E2-S4)", () => {
  it("counts only what is moving", () => {
    const transfers = [
      row("a", "running"),
      row("b", "queued"),
      row("c", "paused"),
      row("d", "done"),
    ];
    expect(activeCount(transfers)).toBe(1);
  });

  it("sums the rate of running rows only, so paused rows cannot drag it down", () => {
    const transfers = [
      row("a", "running", { rate: 1_000_000 }),
      row("b", "running", { rate: 500_000 }),
      row("c", "paused", { rate: 9_000_000 }),
    ];
    expect(aggregateRate(transfers)).toBe(1_500_000);
  });

  it("sums remaining bytes only where the total is known", () => {
    const transfers = [
      row("a", "running", { bytesDone: 40, bytesTotal: 100 }),
      row("b", "running", { bytesDone: 10, bytesTotal: 10 }),
      // A sizeless row contributes nothing: "remaining" is a plan it has none
      // of (XDL-E2-S3's rule).
      row("c", "running", { bytesDone: 500, bytesTotal: 0 }),
      row("d", "queued", { bytesDone: 0, bytesTotal: 1000 }),
    ];
    expect(remainingBytes(transfers)).toBe(60);
  });
});

describe("the sparkline's ring and path", () => {
  it("keeps at most the cap, dropping the oldest", () => {
    let ring: number[] = [];
    for (let value = 1; value <= 70; value += 1) ring = pushSample(ring, value, 60);
    expect(ring.length).toBe(60);
    expect(ring[0]).toBe(11);
    expect(ring[ring.length - 1]).toBe(70);
  });

  it("is empty for fewer than two samples, so nothing draws", () => {
    expect(sparklinePath([], 120, 24)).toBe("");
    expect(sparklinePath([42], 120, 24)).toBe("");
  });

  it("scales the line to the samples' own peak", () => {
    // Oldest 10 MB/s then 20 MB/s: the newer, faster sample reaches the top,
    // so the last coordinate's y sits near the top of the viewBox.
    const path = sparklinePath([10_000_000, 20_000_000], 120, 24);
    const points = path.split(" ");
    expect(points[0]?.startsWith("M")).toBe(true);
    expect(points.some((point) => point.startsWith("L"))).toBe(true);
    const lastY = Number.parseFloat(points[points.length - 1] ?? "");
    expect(lastY).toBeLessThan(3);
  });
});

describe("the drawer's footer numbers (XTP-E4-S2)", () => {
  it("counts what is waiting and what went wrong", () => {
    const queue = [
      row("a", "running"),
      row("b", "queued"),
      row("c", "queued"),
      row("d", "failed"),
      row("e", "done"),
    ];
    expect(activeCount(queue)).toBe(1);
    expect(queuedCount(queue)).toBe(2);
    expect(failedCount(queue)).toBe(1);
  });

  it("divides what is left by how fast it is moving", () => {
    // 200 bytes to go at 50 B/s.
    const queue = [
      row("a", "running", { bytesDone: 0, bytesTotal: 100, rate: 25 }),
      row("b", "running", { bytesDone: 0, bytesTotal: 100, rate: 25 }),
    ];
    expect(aggregateEtaSeconds(queue)).toBe(4);
  });

  it("has no answer while nothing is moving", () => {
    // An ETA from a rate of zero is infinite, and drawing that as a number
    // would be a promise the queue cannot keep.
    expect(aggregateEtaSeconds([row("a", "queued")])).toBeNull();
    expect(aggregateEtaSeconds([row("a", "running", { rate: 0 })])).toBeNull();
  });

  it("does not add the rows' own ETAs together", () => {
    // They run in parallel; summing them over-reports by the number of
    // workers. Two rows of 100 bytes at 100 B/s each finish in a second.
    const queue = [
      row("a", "running", { bytesDone: 0, bytesTotal: 100, rate: 100, etaSeconds: 1 }),
      row("b", "running", { bytesDone: 0, bytesTotal: 100, rate: 100, etaSeconds: 1 }),
    ];
    expect(aggregateEtaSeconds(queue)).toBe(1);
  });
});
