import { describe, expect, it } from "vitest";
import {
  ancestor,
  back,
  canAscend,
  canBack,
  canForward,
  current,
  forward,
  go,
  initial,
  join,
  parent,
  segments,
} from "./navigation";

describe("history", () => {
  it("starts at the root with nowhere to go", () => {
    const history = initial();
    expect(current(history)).toBe("");
    expect(canBack(history)).toBe(false);
    expect(canForward(history)).toBe(false);
  });

  it("moves back and forward over the same trail", () => {
    let history = go(go(initial(), "a"), "a/b");
    expect(current(history)).toBe("a/b");

    history = back(history);
    expect(current(history)).toBe("a");
    expect(canForward(history)).toBe(true);

    history = forward(history);
    expect(current(history)).toBe("a/b");
  });

  it("discards the forward trail when you go somewhere new", () => {
    let history = go(go(initial(), "a"), "a/b");
    history = back(history); // at "a", with "a/b" ahead
    history = go(history, "c");

    expect(current(history)).toBe("c");
    expect(canForward(history)).toBe(false);
    expect(history.stack).toEqual(["", "a", "c"]);
  });

  it("does not record going where you already are", () => {
    // Clicking the last breadcrumb, or the folder you are already in, would
    // otherwise stack duplicates and make Back look broken.
    const history = go(initial(), "a");
    expect(go(history, "a")).toBe(history);
  });

  it("refuses to move past either end", () => {
    const history = initial();
    expect(back(history)).toBe(history);
    expect(forward(history)).toBe(history);
  });
});

describe("paths", () => {
  it("treats the root as its own parent so ascending cannot escape", () => {
    expect(parent("")).toBe("");
    expect(canAscend("")).toBe(false);
    expect(parent("a")).toBe("");
    expect(parent("a/b/c")).toBe("a/b");
  });

  it("has no segments at the root", () => {
    expect(segments("")).toEqual([]);
    expect(segments("a/b")).toEqual(["a", "b"]);
  });

  it("joins without a leading slash at the root", () => {
    // "" and "/" would otherwise be two spellings of the same directory.
    expect(join("", "media")).toBe("media");
    expect(join("a", "b")).toBe("a/b");
  });

  it("builds the path a breadcrumb click should go to", () => {
    expect(ancestor("a/b/c", 0)).toBe("");
    expect(ancestor("a/b/c", 2)).toBe("a/b");
    expect(ancestor("a/b/c", 3)).toBe("a/b/c");
  });
});
