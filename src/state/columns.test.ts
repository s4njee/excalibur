import { describe, expect, it } from "vitest";
import { DEFAULT_WIDTHS, MIN_WIDTH, resize, sanitise, template, visibleColumns } from "./columns";

describe("template", () => {
  it("leaves Name as the flexible column", () => {
    expect(template(DEFAULT_WIDTHS)).toBe("1fr 96px 156px 156px 128px");
  });
});

describe("resize", () => {
  it("shrinks Size when the Name divider is dragged right", () => {
    // Name is 1fr, so it takes the slack by itself.
    expect(resize(DEFAULT_WIDTHS, "size", 20).size).toBe(76);
  });

  it("moves width between exactly the two columns a divider sits between", () => {
    // Added sits between Size and Modified now, so this divider is theirs.
    const next = resize(DEFAULT_WIDTHS, "modified", 20);
    expect(next.added).toBe(176);
    expect(next.modified).toBe(136);
    // Neither neighbour of that pair is on this divider.
    expect(next.size).toBe(DEFAULT_WIDTHS.size);
    expect(next.kind).toBe(DEFAULT_WIDTHS.kind);
  });

  it("pairs a divider with whatever is actually drawn beside it", () => {
    // With Added dropped, the divider left of Modified has Size on its other
    // side. Hardcoded pairs meant it resized a column that was not on screen.
    const next = resize(DEFAULT_WIDTHS, "modified", 20, {
      added: false,
      modified: true,
      kind: true,
    });
    expect(next.size).toBe(116);
    expect(next.modified).toBe(136);
    expect(next.added).toBe(DEFAULT_WIDTHS.added);
  });

  it("ignores a divider for a column that is not drawn", () => {
    expect(resize(DEFAULT_WIDTHS, "kind", 20, { added: true, modified: true, kind: false })).toEqual(
      DEFAULT_WIDTHS,
    );
  });

  it("stops at the minimum instead of collapsing a column", () => {
    expect(resize(DEFAULT_WIDTHS, "size", 10_000).size).toBe(MIN_WIDTH);
  });

  it("does not let a clamped column steal width from its neighbour", () => {
    // Dragging far left would take Size below the minimum. Modified should
    // only give up what Size actually took.
    const start = { size: 96, added: MIN_WIDTH + 10, modified: 200, kind: 128 };
    const next = resize(start, "modified", -1000);
    expect(next.added).toBe(MIN_WIDTH);
    expect(next.modified).toBe(start.modified + 10);
  });
});

describe("sanitise", () => {
  it("accepts what it wrote", () => {
    expect(sanitise({ size: 120, added: 140, modified: 200, kind: 90 })).toEqual({
      size: 120,
      added: 140,
      modified: 200,
      kind: 90,
    });
  });

  it("falls back per column rather than discarding the whole record", () => {
    const widths = sanitise({ size: 120, added: 140, modified: "wide", kind: 4 });
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

describe("responsive column dropping", () => {
  it("keeps every column at a comfortable width", () => {
    expect(visibleColumns(1200)).toEqual({ added: true, modified: true, kind: true });
  });

  it("drops Kind first, then Added, then Modified", () => {
    // The handoff gives the first and last of those; Added is ours, and it
    // outlives Kind because "what just arrived" is the question this list is
    // usually open for.
    expect(visibleColumns(899)).toEqual({ added: true, modified: true, kind: false });
    expect(visibleColumns(759)).toEqual({ added: false, modified: true, kind: false });
    expect(visibleColumns(559)).toEqual({ added: false, modified: false, kind: false });
  });

  it("still shows a date at the minimum useful window", () => {
    // The handoff calls ~820×480 the minimum useful *window*. The sidebar
    // takes about 200px of that, so the list is around 620px — and a date
    // should survive there. Comparing the two numbers directly is how an
    // earlier threshold came to drop Modified at exactly that size.
    expect(visibleColumns(620).modified).toBe(true);
  });

  it("builds a template with only the columns that are drawn", () => {
    // Header and rows share this call; a dropped column that vanished from one
    // and not the other would put every cell in the wrong place.
    const widths = { size: 96, added: 156, modified: 156, kind: 128 };
    expect(template(widths, { added: true, modified: true, kind: true })).toBe(
      "1fr 96px 156px 156px 128px",
    );
    expect(template(widths, { added: false, modified: true, kind: true })).toBe(
      "1fr 96px 156px 128px",
    );
    expect(template(widths, { added: false, modified: true, kind: false })).toBe("1fr 96px 156px");
    expect(template(widths, { added: false, modified: false, kind: false })).toBe("1fr 96px");
  });

  it("defaults to every column, so existing callers are unchanged", () => {
    expect(template({ size: 96, added: 156, modified: 156, kind: 128 })).toBe(
      "1fr 96px 156px 156px 128px",
    );
  });
});
