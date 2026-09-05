import { describe, expect, it } from "vitest";
import { destinationOf, illegalDestination } from "./move";
import type { Entry } from "./types";

/** An entry at `path`, named by its last segment the way a listing's are. */
const at = (path: string, kind: Entry["kind"] = "dir"): Entry => ({
  name: path.slice(path.lastIndexOf("/") + 1),
  path,
  kind,
  size: 0,
  modifiedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: kind === "dir" ? "Folder" : "Document",
  kindCategory: kind === "dir" ? "folder" : "other",
  hidden: false,
});

describe("illegalDestination", () => {
  it("allows a folder that has nothing to do with the items", () => {
    expect(illegalDestination([at("docs/report.pdf", "file")], "docs", "archive")).toBeNull();
    expect(illegalDestination([at("docs/report.pdf", "file")], "docs", "archive/2024")).toBeNull();
  });

  it("allows the share root, which is a real destination and not a special case", () => {
    expect(illegalDestination([at("docs/report.pdf", "file")], "docs", "")).toBeNull();
  });

  it("refuses the folder the items are already in, and names the one item", () => {
    // Not an error so much as a no-op, but the button has to be disabled with
    // something on screen or the user clicks Move here and nothing happens.
    expect(illegalDestination([at("docs/report.pdf", "file")], "docs", "docs")).toBe(
      "“report.pdf” is already in this folder.",
    );
  });

  it("counts them when there are several", () => {
    const entries = [at("docs/a", "file"), at("docs/b", "file"), at("docs/c")];
    expect(illegalDestination(entries, "docs", "docs")).toBe(
      "These 3 items are already in this folder.",
    );
  });

  it("refuses the root when that is where the items already are", () => {
    // `""` is a path like any other here; the empty string must not read as
    // "no destination chosen".
    expect(illegalDestination([at("report.pdf", "file")], "", "")).toBe(
      "“report.pdf” is already in this folder.",
    );
  });

  it("refuses a folder being moved, as its own destination", () => {
    expect(illegalDestination([at("docs")], "", "docs")).toBe(
      "You cannot move “docs” into itself.",
    );
  });

  it("refuses anything inside a folder being moved, at any depth", () => {
    const inside = "You cannot move “docs” into a folder inside it.";
    expect(illegalDestination([at("docs")], "", "docs/2024")).toBe(inside);
    expect(illegalDestination([at("docs")], "", "docs/2024/q3/drafts")).toBe(inside);
  });

  it("does not mistake a sibling with a shared prefix for a child", () => {
    // The whole reason this is a module. `"docs-old".startsWith("docs")` is
    // true, and a picker that refused here would refuse a perfectly ordinary
    // move with a sentence claiming the folder was inside itself.
    for (const sibling of ["docs-old", "docs2", "docs.bak", "docsy/2024"]) {
      expect(illegalDestination([at("docs")], "", sibling), sibling).toBeNull();
    }
  });

  it("checks every entry, not just the first", () => {
    // A selection of a file and a folder, dragged around together: the file
    // makes the destination look fine and the folder is the one that is not.
    const entries = [at("work/notes.txt", "file"), at("work/docs")];
    expect(illegalDestination(entries, "work", "work/docs/2024")).toBe(
      "You cannot move “docs” into a folder inside it.",
    );
  });

  it("allows a folder to move into a sibling's subtree", () => {
    // The move that a too-eager containment check would also refuse: `docs`
    // is going *under* something, just not under itself.
    expect(illegalDestination([at("work/docs")], "work", "work/archive/2024")).toBeNull();
  });
});

describe("destinationOf", () => {
  it("keeps the leaf name and replaces everything above it", () => {
    expect(destinationOf(at("a/b/c.txt", "file"), "d")).toBe("d/c.txt");
  });

  it("joins without a leading slash at the root", () => {
    expect(destinationOf(at("a/b/c.txt", "file"), "")).toBe("c.txt");
  });

  it("does not move an item that is already there", () => {
    // The identity the same-folder refusal exists to protect: had it not
    // fired, this is the `rename(from, to)` we would have sent — from a path
    // to itself.
    const entry = at("docs/report.pdf", "file");
    expect(destinationOf(entry, "docs")).toBe(entry.path);
  });
});
