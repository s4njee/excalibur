import { describe, expect, it } from "vitest";
import { OVERSCAN, cellsInBand, gridRange, itemsPerRow, rowsInBand, visibleRange } from "./virtual";

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

describe("a wrapping grid (E26-S4)", () => {
  it("fits as many cells across as the width allows, and never none", () => {
    expect(itemsPerRow(1000, 120)).toBe(8);
    expect(itemsPerRow(119, 120)).toBe(1);
    // The width the list holds before its first measurement.
    expect(itemsPerRow(Number.POSITIVE_INFINITY, 120)).toBe(1);
    expect(itemsPerRow(0, 120)).toBe(1);
  });

  it("starts and ends the window on a row boundary", () => {
    // A window that began mid-row would put its first cell in the wrong
    // column, because the grid places by order and not by index.
    const range = gridRange(0, 300, 100, 5, 1000, 0);
    expect(range.start % 5).toBe(0);
    expect(range.end % 5).toBe(0);
  });

  it("draws the rows the viewport covers, in cells", () => {
    // 3 rows of 4 visible, no overscan: 12 cells from the top.
    expect(gridRange(0, 300, 100, 4, 100, 0)).toEqual({ start: 0, end: 12 });
    // Scrolled two rows down: the window moves by whole rows.
    expect(gridRange(200, 300, 100, 4, 100, 0)).toEqual({ start: 8, end: 20 });
  });

  it("clamps to the last cell rather than past it", () => {
    const range = gridRange(0, 10_000, 100, 4, 6, 0);
    expect(range.end).toBe(6);
  });

  it("is empty for an empty listing", () => {
    expect(gridRange(0, 300, 100, 4, 0)).toEqual({ start: 0, end: 0 });
  });

  it("bounds a very large listing", () => {
    // The property the whole thing exists for.
    const range = gridRange(0, 800, 100, 6, 100_000);
    expect(range.end - range.start).toBeLessThan(200);
  });
});

describe("a rubber band over a grid (E26-S4)", () => {
  // A 4-across grid of 100px cells, 10 cells: two full rows and a short third.
  const band = (from: [number, number], to: [number, number], count = 10) =>
    cellsInBand({ x: from[0], y: from[1] }, { x: to[0], y: to[1] }, 100, 100, 4, count);

  it("covers a block, which is not a contiguous range of indices", () => {
    // Columns 1–2 of rows 0–1. `rowsInBand` would have answered 0..8.
    expect(band([110, 10], [290, 190])).toEqual([1, 2, 5, 6]);
  });

  it("selects a cell the band merely clips", () => {
    // Intersection, not containment: a drag across a corner selects it.
    expect(band([90, 90], [110, 110])).toEqual([0, 1, 4, 5]);
  });

  it("selects nothing for a band with no area", () => {
    // A click that never moved is a click, not a selection of everything under
    // one pixel.
    expect(band([50, 50], [50, 50])).toEqual([]);
    expect(band([50, 50], [50, 250])).toEqual([]);
  });

  it("reads the same dragged in any direction", () => {
    expect(band([290, 190], [110, 10])).toEqual(band([110, 10], [290, 190]));
  });

  it("stops at the edges rather than running past them", () => {
    // Far outside on every side: everything, and nothing that is not a cell.
    expect(band([-500, -500], [5000, 5000])).toEqual([
      0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
    ]);
  });

  it("does not invent cells in a short last row", () => {
    // 10 cells, 4 across: the last row holds two, not four.
    expect(band([0, 210], [400, 290])).toEqual([8, 9]);
  });

  it("is empty for an empty listing", () => {
    expect(band([0, 0], [400, 400], 0)).toEqual([]);
  });
});
