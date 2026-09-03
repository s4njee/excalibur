import { describe, expect, it } from "vitest";
import {
  formatCapacity,
  formatCounts,
  formatDuration,
  formatModified,
  formatSize,
  formatTransferStatus,
} from "./format";

describe("the strings the design specifies", () => {
  it("renders the mock's own figures", () => {
    // These are the literal strings in the handoff, so a change to them is a
    // change to the design rather than a refactor.
    expect(formatSize(48_234_496_000)).toBe("48.2 GB");
    expect(formatCapacity(2_400_000_000_000, 16_000_000_000_000)).toBe("2.4 TB free of 16 TB");
    expect(formatCounts(13, 1, 48_234_496_000)).toBe("13 items · 1 selected · 48.2 GB");
  });

  it("omits the selection clause when nothing is selected", () => {
    expect(formatCounts(13, 0, 0)).toBe("13 items");
  });

  it("drops a trailing .0 so a round number reads as one", () => {
    // "2.4 TB free of 16 TB" appears in one sentence in the design, so the
    // decimal cannot be fixed at one place or at zero.
    expect(formatSize(16_000_000_000_000)).toBe("16 TB");
    expect(formatSize(2_400_000_000_000)).toBe("2.4 TB");
    // Not exactly 45, so it keeps the decimal rather than claiming to be.
    expect(formatSize(44_982_075_392)).toBe("45.0 GB");
  });

  it("keeps small sizes exact rather than rounding to zero", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(999)).toBe("999 B");
    expect(formatSize(6148)).toBe("6.1 KB");
  });

  it("drops the decimal once three digits are shown", () => {
    expect(formatSize(999_000_000_000)).toBe("999 GB");
  });

  it("formats a modification time the way the Modified column does", () => {
    // Constructed locally, because the column shows local time.
    const when = new Date(2026, 7, 31, 18, 24).getTime();
    expect(formatModified(when)).toBe("31 Aug 2026, 18:24");
    expect(formatModified(null)).toBe("—");
  });
});

describe("formatCounts with a filter", () => {
  it("says so when rows are hidden, rather than just showing a smaller number", () => {
    // "3 items" reads exactly like a directory containing three files, which
    // is the one thing a user must not conclude while a filter is on.
    expect(formatCounts(3, 0, 0, 13)).toBe("3 of 13 items");
  });

  it("stays plain when the filter matches everything", () => {
    expect(formatCounts(13, 0, 0, 13)).toBe("13 items");
  });

  it("keeps the selection parts alongside a filtered count", () => {
    expect(formatCounts(3, 1, 48_200_000_000, 13)).toBe("3 of 13 items · 1 selected · 48.2 GB");
  });

  it("is unchanged when no total is given", () => {
    expect(formatCounts(13, 0, 0)).toBe("13 items");
  });
});

describe("transfer row strings", () => {
  const running = {
    status: "running",
    bytesDone: 41_200_000_000,
    bytesTotal: 61_700_000_000,
    rate: 112_000_000,
    etaSeconds: 184,
  };

  it("produces the design's running string", () => {
    expect(formatTransferStatus(running)).toBe("41.2 / 61.7 GB · 112 MB/s · 3m 04s");
  });

  it("shows the unit once, on the total", () => {
    // "41.2 GB / 61.7 GB" is what a naive version produces and is not what
    // the design shows.
    expect(formatTransferStatus(running)).not.toContain("GB /");
  });

  it("leaves out what it cannot say rather than filling it in", () => {
    // A row reading "· — · —" is noise.
    expect(
      formatTransferStatus({ ...running, rate: null, etaSeconds: null }),
    ).toBe("41.2 / 61.7 GB");
  });

  it("says queued and done exactly as the design does", () => {
    expect(formatTransferStatus({ ...running, status: "queued" })).toBe("queued");
    expect(
      formatTransferStatus({ ...running, status: "done", bytesDone: 84_000 }),
    ).toBe("done · 84 KB");
  });

  it("falls back to a plain count when the size is unknown", () => {
    expect(
      formatTransferStatus({ ...running, bytesTotal: 0, rate: null, etaSeconds: null }),
    ).toBe("41.2 GB");
  });
});

describe("formatDuration", () => {
  it("uses at most two units", () => {
    expect(formatDuration(12)).toBe("12s");
    expect(formatDuration(184)).toBe("3m 04s");
    expect(formatDuration(3720)).toBe("1h 02m");
  });

  it("pads the second field, so a column of them lines up", () => {
    expect(formatDuration(65)).toBe("1m 05s");
  });

  it("does not report a negative remaining time", () => {
    expect(formatDuration(-5)).toBe("0s");
  });
});
