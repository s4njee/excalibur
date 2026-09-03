import { describe, expect, it } from "vitest";
import { EMPTY_SELECTION, band, between, click, typeAhead } from "./selection";

const order = ["a", "b", "c", "d", "e"];
const paths = (selection: { paths: ReadonlySet<string> }) => [...selection.paths].sort();

describe("click", () => {
  it("replaces the selection and sets the anchor", () => {
    const selection = click(EMPTY_SELECTION, order, "c");
    expect(paths(selection)).toEqual(["c"]);
    expect(selection.anchor).toBe("c");
  });

  it("toggles one row without disturbing the rest", () => {
    let selection = click(EMPTY_SELECTION, order, "b");
    selection = click(selection, order, "d", { toggle: true });
    expect(paths(selection)).toEqual(["b", "d"]);
    selection = click(selection, order, "b", { toggle: true });
    expect(paths(selection)).toEqual(["d"]);
  });

  it("extends from the anchor, in either direction", () => {
    const from = click(EMPTY_SELECTION, order, "c");
    expect(paths(click(from, order, "e", { shift: true }))).toEqual(["c", "d", "e"]);
    expect(paths(click(from, order, "a", { shift: true }))).toEqual(["a", "b", "c"]);
  });

  it("keeps the anchor still while the far end moves", () => {
    // Two shift-clicks from one starting row must grow and shrink the same
    // range, not walk the anchor along behind the pointer.
    let selection = click(EMPTY_SELECTION, order, "b");
    selection = click(selection, order, "e", { shift: true });
    expect(paths(selection)).toEqual(["b", "c", "d", "e"]);
    selection = click(selection, order, "c", { shift: true });
    expect(paths(selection)).toEqual(["b", "c"]);
    expect(selection.anchor).toBe("b");
  });

  it("treats a shift-click with no anchor as a plain click", () => {
    expect(paths(click(EMPTY_SELECTION, order, "c", { shift: true }))).toEqual(["c"]);
  });

  it("moves the anchor on a toggle", () => {
    let selection = click(EMPTY_SELECTION, order, "a");
    selection = click(selection, order, "c", { toggle: true });
    // A following shift-click extends from the row last touched.
    expect(paths(click(selection, order, "e", { shift: true }))).toEqual(["c", "d", "e"]);
  });
});

describe("between", () => {
  it("is inclusive and order-independent", () => {
    expect(between(order, "b", "d")).toEqual(["b", "c", "d"]);
    expect(between(order, "d", "b")).toEqual(["b", "c", "d"]);
  });

  it("falls back to the clicked row when the anchor has gone", () => {
    // The anchor can name a row a filter has since hidden.
    expect(between(order, "zz", "c")).toEqual(["c"]);
  });
});

describe("band", () => {
  it("replaces by default", () => {
    const start = click(EMPTY_SELECTION, order, "a");
    expect(paths(band(start, ["c", "d"], false))).toEqual(["c", "d"]);
  });

  it("adds to the selection when the modifier is held", () => {
    const start = click(EMPTY_SELECTION, order, "a");
    expect(paths(band(start, ["c", "d"], true))).toEqual(["a", "c", "d"]);
  });
});

describe("typeAhead", () => {
  const names = ["Arrival.mkv", "Blade.mkv", "Alien.mkv", "Solaris.mkv"];
  const rows = ["p0", "p1", "p2", "p3"];

  const nowhere = { current: null, origin: null };

  it("jumps to the first match", () => {
    expect(typeAhead(rows, names, "bl", nowhere)).toBe("p1");
  });

  it("ignores case and diacritics, like the filter", () => {
    expect(typeAhead(rows, ["Amélie.mkv"], "ame", nowhere)).toBe("p0");
  });

  it("matches a prefix, not a substring", () => {
    // "rival" is inside "Arrival" but type-to-select is a prefix jump.
    expect(typeAhead(rows, names, "rival", nowhere)).toBeNull();
  });

  it("walks through matches when the same letter repeats", () => {
    // `a`, `a`, `a` inside the typing window means "next thing starting with
    // a", not "something starting with aaa".
    expect(typeAhead(rows, names, "a", nowhere)).toBe("p0");
    expect(typeAhead(rows, names, "aa", { current: "p0", origin: null })).toBe("p2");
    // ...and wraps.
    expect(typeAhead(rows, names, "aaa", { current: "p2", origin: null })).toBe("p0");
  });

  it("refines rather than advancing when the buffer grows", () => {
    // Typing `b` lands on Blade.mkv. Adding `l` must stay on it — searching
    // onward from `b`'s match would skip to the next thing starting with bl.
    const withSrt = ["Blade.mkv", "Blade.srt"];
    const two = ["p0", "p1"];
    expect(typeAhead(two, withSrt, "b", { current: null, origin: null })).toBe("p0");
    expect(typeAhead(two, withSrt, "bl", { current: "p0", origin: null })).toBe("p0");
  });

  it("searches from after the current row when cycling", () => {
    expect(typeAhead(rows, names, "a", { current: "p0", origin: "p0" })).toBe("p2");
  });

  it("answers nothing when nothing matches", () => {
    expect(typeAhead(rows, names, "zzz", nowhere)).toBeNull();
    expect(typeAhead(rows, names, "", nowhere)).toBeNull();
  });
});
