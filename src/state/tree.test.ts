import { describe, expect, it } from "vitest";
import {
  DEFAULT_TREE_HEIGHT,
  MAX_TREE_HEIGHT,
  MIN_TREE_HEIGHT,
  ancestorsOf,
  foldersOf,
  resizedBand,
  sanitiseHeight,
  treeRows,
  type TreeNode,
} from "./tree";
import type { Entry } from "./types";

const entry = (name: string, over: Partial<Entry> = {}): Entry => ({
  name,
  path: name,
  kind: "dir",
  size: 0,
  modifiedMs: 0,
  addedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: "Folder",
  kindCategory: "folder",
  hidden: name.startsWith("."),
  ...over,
});

const node = (path: string, hidden = false): TreeNode => ({
  path,
  name: path.split("/").pop() ?? path,
  hidden,
});

describe("foldersOf", () => {
  it("keeps the directories and drops the files", () => {
    const listing = [entry("Films"), entry("notes.txt", { kind: "file" }), entry("Shows")];
    expect(foldersOf(listing).map((folder) => folder.name)).toEqual(["Films", "Shows"]);
  });

  it("orders by name the way the list does, digits included", () => {
    // `numeric` is the list's own rule (`sort.ts:31`). A tree that ordered
    // `Season 10` before `Season 9` would disagree with the rows below it.
    const listing = [entry("Season 10"), entry("Season 9"), entry("season 1")];
    expect(foldersOf(listing).map((folder) => folder.name)).toEqual([
      "season 1",
      "Season 9",
      "Season 10",
    ]);
  });

  it("keeps hidden folders, and says which they are", () => {
    // Whether they are drawn is the window's `Show hidden`, decided at row
    // time — dropping them here would make the tree unable to follow it
    // without re-listing every folder.
    expect(foldersOf([entry(".git")])).toEqual([{ path: ".git", name: ".git", hidden: true }]);
  });
});

describe("ancestorsOf", () => {
  it("names the folders that have to be open for a path to be on screen", () => {
    expect(ancestorsOf("Films/2016/Arrival")).toEqual(["Films", "Films/2016"]);
  });

  it("stops short of the path itself", () => {
    // Navigating into a folder should reveal it, not open it: expanding it
    // would show its children unasked, which is a different gesture.
    expect(ancestorsOf("Films")).toEqual([]);
  });

  it("has nothing to open at the share root", () => {
    expect(ancestorsOf("")).toEqual([]);
  });
});

describe("treeRows", () => {
  const loaded = new Map<string, TreeNode[]>([
    ["", [node("Films"), node("Shows")]],
    ["Films", [node("Films/2016")]],
  ]);

  it("draws the root's folders when nothing is open", () => {
    const rows = treeRows(loaded, new Set(), true);
    expect(rows.map((row) => [row.name, row.depth])).toEqual([
      ["Films", 0],
      ["Shows", 0],
    ]);
  });

  it("indents the children of an open folder under it", () => {
    const rows = treeRows(loaded, new Set(["Films"]), true);
    expect(rows.map((row) => [row.name, row.depth])).toEqual([
      ["Films", 0],
      ["2016", 1],
      ["Shows", 0],
    ]);
  });

  it("turns the disclosure of a folder that is open but still loading", () => {
    // The click took a network round trip. A row that kept its `▶` until the
    // children arrived would look like a click that did nothing.
    const rows = treeRows(loaded, new Set(["Shows"]), true);
    const shows = rows.find((row) => row.name === "Shows");
    expect(shows?.expanded).toBe(true);
    expect(rows).toHaveLength(2);
  });

  it("says it does not know whether an unlisted folder has children", () => {
    // `null`, not `false`: the tree has never asked, and drawing a leaf would
    // be a claim it cannot support.
    const rows = treeRows(loaded, new Set(), true);
    expect(rows.find((row) => row.name === "Shows")?.hasChildren).toBeNull();
    expect(rows.find((row) => row.name === "Films")?.hasChildren).toBe(true);
  });

  it("knows an empty folder is empty once it has been listed", () => {
    const listed = new Map(loaded).set("Shows", []);
    const rows = treeRows(listed, new Set(), true);
    expect(rows.find((row) => row.name === "Shows")?.hasChildren).toBe(false);
  });

  it("follows Show hidden, at every level", () => {
    const withDots = new Map<string, TreeNode[]>([
      ["", [node("Films"), node(".Trashes", true)]],
      ["Films", [node("Films/.cache", true), node("Films/2016")]],
    ]);
    expect(treeRows(withDots, new Set(["Films"]), true).map((row) => row.name)).toEqual([
      "Films",
      ".cache",
      "2016",
      ".Trashes",
    ]);
    expect(treeRows(withDots, new Set(["Films"]), false).map((row) => row.name)).toEqual([
      "Films",
      "2016",
    ]);
  });

  it("hides a folder whose only children are hidden, disclosure and all", () => {
    // Otherwise a `▶` opens onto nothing, which reads as a broken tree rather
    // than as a setting.
    const withDots = new Map<string, TreeNode[]>([
      ["", [node("Films")]],
      ["Films", [node("Films/.cache", true)]],
    ]);
    const rows = treeRows(withDots, new Set(["Films"]), false);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.hasChildren).toBe(false);
  });
});

describe("resizedBand", () => {
  it("follows the drag", () => {
    expect(resizedBand(104, 40)).toBe(144);
  });

  it("collapses rather than leaving a band too short to be a tree", () => {
    expect(resizedBand(104, -80)).toBe(0);
  });

  it("stops growing before the band becomes the view", () => {
    expect(resizedBand(300, 400)).toBe(MAX_TREE_HEIGHT);
  });
});

describe("sanitiseHeight", () => {
  it("keeps a collapsed band collapsed", () => {
    // `0` is the user having put it away, not a missing value: rounding it up
    // to the minimum would reopen the band on every launch.
    expect(sanitiseHeight(0)).toBe(0);
  });

  it("falls back for anything that is not a height", () => {
    expect(sanitiseHeight(undefined)).toBe(DEFAULT_TREE_HEIGHT);
    expect(sanitiseHeight("104")).toBe(DEFAULT_TREE_HEIGHT);
    expect(sanitiseHeight(Number.NaN)).toBe(DEFAULT_TREE_HEIGHT);
  });

  it("brings an absurd stored height back into range", () => {
    expect(sanitiseHeight(4000)).toBe(MAX_TREE_HEIGHT);
    expect(sanitiseHeight(12)).toBe(MIN_TREE_HEIGHT);
  });
});
