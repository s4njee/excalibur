import { describe, expect, it } from "vitest";
import { OVERSCAN, rowsInBand, visibleRange } from "./virtual";

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

describe("rowsInBand", () => {
  it("covers a row the band touches at all", () => {
    // Requiring the midpoint would make the first row of a downward drag
    // refuse to select until the pointer is 12px in.
    expect(rowsInBand(0, 1, 24, 100)).toEqual({ start: 0, end: 1 });
  });

  it("works in either drag direction", () => {
    expect(rowsInBand(100, 20, 24, 100)).toEqual(rowsInBand(20, 100, 24, 100));
  });

  it("covers exactly the rows a tall band spans", () => {
    // 24..96 touches rows 1, 2 and 3.
    expect(rowsInBand(24, 96, 24, 100)).toEqual({ start: 1, end: 4 });
  });

  it("clamps to the list rather than running past it", () => {
    expect(rowsInBand(0, 999_999, 24, 5)).toEqual({ start: 0, end: 5 });
  });

  it("is empty for an empty list", () => {
    expect(rowsInBand(0, 100, 24, 0)).toEqual({ start: 0, end: 0 });
  });
});
