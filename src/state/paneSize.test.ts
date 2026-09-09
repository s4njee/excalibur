import { describe, expect, it } from "vitest";
import {
  DEFAULT_PANE_SIZE,
  DEFAULT_PANE_WIDTH,
  MIN_PANE_HEIGHT,
  MIN_PANE_WIDTH,
  clampToWindow,
  resized,
  sanitise,
} from "./paneSize";

const room = { width: 1200, height: 700 };
const start = { width: 720, height: 400 };

describe("resizing the transfers pane", () => {
  it("grows to the left, because the pane is anchored to the right", () => {
    // Dragging the left edge 100px left is a wider pane, not a narrower one.
    const wider = resized(start, { x: -100, y: 0 }, { left: true, bottom: false }, room);
    expect(wider.width).toBe(820);
    const narrower = resized(start, { x: 100, y: 0 }, { left: true, bottom: false }, room);
    expect(narrower.width).toBe(620);
  });

  it("grows downwards from the height it was already drawing", () => {
    const taller = resized(start, { x: 0, y: 120 }, { left: false, bottom: true }, room);
    expect(taller.height).toBe(520);
  });

  it("takes both at once from the corner", () => {
    const both = resized(start, { x: -60, y: 60 }, { left: true, bottom: true }, room);
    expect(both).toEqual({ width: 780, height: 460 });
  });

  it("leaves the height auto when only the width is dragged", () => {
    // Taking hold of the left edge must not silently fix a height the user
    // never asked to fix — the pane stays as tall as the queue needs.
    expect(resized(start, { x: -40, y: 0 }, { left: true, bottom: false }, room).height).toBeNull();
  });

  it("refuses to shrink past the point where the table stops being a table", () => {
    const tiny = resized(start, { x: 900, y: -900 }, { left: true, bottom: true }, room);
    expect(tiny.width).toBe(MIN_PANE_WIDTH);
    expect(tiny.height).toBe(MIN_PANE_HEIGHT);
  });

  it("cannot be dragged wider or taller than the window it sits in", () => {
    const huge = resized(start, { x: -5000, y: 5000 }, { left: true, bottom: true }, room);
    expect(huge.width).toBe(room.width);
    expect(huge.height).toBe(room.height);
  });

  it("survives a window smaller than the minimum without inverting", () => {
    // A clamp whose high is below its low would otherwise return the high and
    // hand back a negative-ish layout.
    const cramped = resized(start, { x: 0, y: 0 }, { left: true, bottom: true }, { width: 100, height: 50 });
    expect(cramped.width).toBe(MIN_PANE_WIDTH);
    expect(cramped.height).toBe(MIN_PANE_HEIGHT);
  });
});

describe("bringing a remembered size back", () => {
  it("pulls a pane sized on a bigger display back inside this one", () => {
    // Otherwise the left edge — and the handle on it — is off screen, and the
    // pane can never be made smaller again.
    expect(clampToWindow({ width: 1600, height: 900 }, room)).toEqual({ width: 1200, height: 700 });
  });

  it("leaves an auto height auto", () => {
    expect(clampToWindow({ width: 800, height: null }, room).height).toBeNull();
  });
});

describe("reading a remembered size", () => {
  it("falls back per field, not wholesale", () => {
    expect(sanitise({ width: 900, height: "tall" })).toEqual({ width: 900, height: null });
    expect(sanitise({ width: null, height: 300 })).toEqual({
      width: DEFAULT_PANE_WIDTH,
      height: 300,
    });
  });

  it("treats a stored size below the minimum as no answer at all", () => {
    expect(sanitise({ width: 10, height: 10 })).toEqual(DEFAULT_PANE_SIZE);
  });

  it("keeps a null height, which is the meaningful default", () => {
    expect(sanitise({ width: 800, height: null })).toEqual({ width: 800, height: null });
  });

  it("survives anything at all in the file", () => {
    expect(sanitise(undefined)).toEqual(DEFAULT_PANE_SIZE);
    expect(sanitise("720x400")).toEqual(DEFAULT_PANE_SIZE);
    expect(sanitise([])).toEqual(DEFAULT_PANE_SIZE);
    expect(sanitise({ width: Number.NaN, height: Number.POSITIVE_INFINITY })).toEqual(
      DEFAULT_PANE_SIZE,
    );
  });
});
