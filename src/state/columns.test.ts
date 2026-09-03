import { describe, expect, it } from "vitest";
import { DEFAULT_WIDTHS, MIN_WIDTH, resize, sanitise, template } from "./columns";

describe("template", () => {
  it("leaves Name as the flexible column", () => {
    expect(template(DEFAULT_WIDTHS)).toBe("1fr 96px 156px 128px");
  });
});

describe("resize", () => {
  it("shrinks Size when the Name divider is dragged right", () => {
    // Name is 1fr, so it takes the slack by itself.
    expect(resize(DEFAULT_WIDTHS, "size", 20).size).toBe(76);
  });

  it("moves width between exactly the two columns a divider sits between", () => {
    const next = resize(DEFAULT_WIDTHS, "modified", 20);
    expect(next.size).toBe(116);
    expect(next.modified).toBe(136);
    // Kind is not on this divider and must not move.
    expect(next.kind).toBe(DEFAULT_WIDTHS.kind);
  });

  it("stops at the minimum instead of collapsing a column", () => {
    expect(resize(DEFAULT_WIDTHS, "size", 10_000).size).toBe(MIN_WIDTH);
  });

  it("does not let a clamped column steal width from its neighbour", () => {
    // Dragging far left would take Size below the minimum. Modified should
    // only give up what Size actually took.
    const start = { size: MIN_WIDTH + 10, modified: 200, kind: 128 };
    const next = resize(start, "modified", -1000);
    expect(next.size).toBe(MIN_WIDTH);
    expect(next.modified).toBe(start.modified + 10);
  });
});

describe("sanitise", () => {
  it("accepts what it wrote", () => {
    expect(sanitise({ size: 120, modified: 200, kind: 90 })).toEqual({
      size: 120,
      modified: 200,
      kind: 90,
    });
  });

  it("falls back per column rather than discarding the whole record", () => {
    const widths = sanitise({ size: 120, modified: "wide", kind: 4 });
    expect(widths.size).toBe(120);
    expect(widths.modified).toBe(DEFAULT_WIDTHS.modified);
    // Below the minimum is as unusable as a missing value.
    expect(widths.kind).toBe(DEFAULT_WIDTHS.kind);
  });

  it("survives junk, because the file is user-editable and outlives upgrades", () => {
    expect(sanitise(null)).toEqual(DEFAULT_WIDTHS);
    expect(sanitise("nonsense")).toEqual(DEFAULT_WIDTHS);
    expect(sanitise(undefined)).toEqual(DEFAULT_WIDTHS);
  });
});
