import { describe, expect, it } from "vitest";
import { OVERSCAN, visibleRange } from "./virtual";

describe("visibleRange", () => {
  it("renders a viewport's worth plus overscan, not the whole list", () => {
    const { start, end } = visibleRange(0, 240, 24, 100_000);
    expect(start).toBe(0);
    // 10 rows visible, plus overscan below. Nowhere near 100k.
    expect(end).toBe(10 + OVERSCAN);
  });

  it("moves the window as the list scrolls", () => {
    const { start, end } = visibleRange(24_000, 240, 24, 100_000);
    expect(start).toBe(1000 - OVERSCAN);
    expect(end).toBe(1010 + OVERSCAN);
  });

  it("clamps to the end of the list", () => {
    const { start, end } = visibleRange(10_000_000, 240, 24, 100);
    expect(end).toBe(100);
    expect(start).toBeLessThanOrEqual(end);
  });

  it("is empty for an empty list", () => {
    expect(visibleRange(0, 240, 24, 0)).toEqual({ start: 0, end: 0 });
  });

  it("never inverts when measured before layout", () => {
    // A container measured before it has been laid out reports height 0, and a
    // scroll position can be left over from a longer listing.
    const { start, end } = visibleRange(5000, 0, 24, 10);
    expect(start).toBeLessThanOrEqual(end);
    expect(end).toBeLessThanOrEqual(10);
  });

  it("treats a zero row height as nothing to render rather than dividing by it", () => {
    expect(visibleRange(0, 240, 0, 100)).toEqual({ start: 0, end: 0 });
  });
});
