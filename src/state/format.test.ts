import { describe, expect, it } from "vitest";
import { formatCapacity, formatCounts, formatModified, formatSize } from "./format";

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
