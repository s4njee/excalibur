import { describe, expect, it } from "vitest";
import {
  EMPTY_SELECTION,
  band,
  between,
  click,
  extendBy,
  extendTo,
  farIndex,
  moveTo,
  step,
  typeAhead,
} from "./selection";

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

describe("keyboard moves", () => {
  it("enters the list from the near end when nothing is selected", () => {
    // ↓ into an untouched list means the first row, ↑ the last. The rule is
    // the same for a page, because "there is nowhere to move from" is one idea
    // however far the step would have been.
    expect(paths(step(EMPTY_SELECTION, order, 1).selection)).toEqual(["a"]);
    expect(paths(step(EMPTY_SELECTION, order, -1).selection)).toEqual(["e"]);
    expect(paths(step(EMPTY_SELECTION, order, 10).selection)).toEqual(["a"]);
    expect(step(EMPTY_SELECTION, order, -1).reveal).toBe(4);
  });

  it("moves one row and takes the anchor with it", () => {
    const from = click(EMPTY_SELECTION, order, "c");
    const down = step(from, order, 1);
    expect(paths(down.selection)).toEqual(["d"]);
    expect(down.selection.anchor).toBe("d");
    expect(down.reveal).toBe(3);
  });

  it("stops at the ends rather than wrapping", () => {
    // Wrapping would move the eye from the bottom of a listing to the top,
    // which reads as the list having jumped rather than the key having worked.
    const last = click(EMPTY_SELECTION, order, "e");
    expect(paths(step(last, order, 1).selection)).toEqual(["e"]);
    const first = click(EMPTY_SELECTION, order, "a");
    expect(paths(step(first, order, -1).selection)).toEqual(["a"]);
  });

  it("clamps a page to the list", () => {
    const from = click(EMPTY_SELECTION, order, "b");
    expect(paths(step(from, order, 20).selection)).toEqual(["e"]);
    expect(paths(step(from, order, -20).selection)).toEqual(["a"]);
  });

  it("goes to the first and last rows by index", () => {
    const from = click(EMPTY_SELECTION, order, "c");
    expect(paths(moveTo(from, order, 0).selection)).toEqual(["a"]);
    expect(paths(moveTo(from, order, order.length - 1).selection)).toEqual(["e"]);
  });

  it("does nothing at all in an empty list", () => {
    expect(step(EMPTY_SELECTION, [], 1).reveal).toBeNull();
    expect(moveTo(EMPTY_SELECTION, [], 0).reveal).toBeNull();
    expect(extendBy(EMPTY_SELECTION, [], 1).reveal).toBeNull();
  });

  it("extends from the anchor, which stays where it is", () => {
    // E8-S8's rule, reached by keyboard: ⇧↓⇧↓ grows the range and ⇧↑ shrinks
    // the one it grew, rather than walking the anchor down the list.
    let selection = click(EMPTY_SELECTION, order, "b");
    let move = extendBy(selection, order, 1);
    expect(paths(move.selection)).toEqual(["b", "c"]);
    move = extendBy(move.selection, order, 1);
    expect(paths(move.selection)).toEqual(["b", "c", "d"]);
    expect(move.selection.anchor).toBe("b");
    move = extendBy(move.selection, order, -1);
    expect(paths(move.selection)).toEqual(["b", "c"]);
    expect(move.selection.anchor).toBe("b");
  });

  it("shrinks past the anchor and out the other side", () => {
    // Three ⇧↑ from a two-row range: b..c becomes b, then a..b.
    let move = extendBy(click(EMPTY_SELECTION, order, "b"), order, 1);
    move = extendBy(move.selection, order, -1);
    expect(paths(move.selection)).toEqual(["b"]);
    move = extendBy(move.selection, order, -1);
    expect(paths(move.selection)).toEqual(["a", "b"]);
    expect(move.selection.anchor).toBe("b");
  });

  it("reveals the end that moved, not the anchor", () => {
    const from = click(EMPTY_SELECTION, order, "b");
    expect(extendBy(from, order, 2).reveal).toBe(3);
  });

  it("extends by a page, clamped", () => {
    const from = click(EMPTY_SELECTION, order, "b");
    const move = extendBy(from, order, 20);
    expect(paths(move.selection)).toEqual(["b", "c", "d", "e"]);
    expect(move.selection.anchor).toBe("b");
  });

  it("treats a shift-arrow with nothing selected as a plain move", () => {
    // There is nothing to extend from, and doing nothing would make the first
    // keystroke into a list a dead one.
    expect(paths(extendBy(EMPTY_SELECTION, order, 1).selection)).toEqual(["a"]);
    expect(paths(extendTo(EMPTY_SELECTION, order, 2).selection)).toEqual(["c"]);
  });

  it("moves on from the last visible row when the anchor was filtered away", () => {
    // The anchor is a path, and a filter can take it off screen. Moving from a
    // row the user cannot see would look like the key was ignored.
    const selection = { paths: new Set(["b", "d"]), anchor: "z" };
    expect(paths(step(selection, order, 1).selection)).toEqual(["e"]);
  });

  it("finds the end of a range that a click built", () => {
    const selection = click(click(EMPTY_SELECTION, order, "b"), order, "d", { shift: true });
    expect(farIndex(order, selection)).toBe(3);
    expect(farIndex(order, EMPTY_SELECTION)).toBe(-1);
  });
});
