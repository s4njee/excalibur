// What the context menu offers (E14-S2).
//
// The property these hold is the one the design stakes everything on: the menu
// is a third way to reach the same verbs, and it must not become a second
// opinion about whether they are allowed. So the read-only case is asserted as
// a *partition* — exactly the write items disabled, exactly the read ones not —
// rather than item by item, because a new verb quietly added to the wrong side
// is the failure that would otherwise slip through.

import { describe, expect, it } from "vitest";
import { menuFor, type MenuAction } from "./menu";
import { READ_AFFORDANCES, WRITE_AFFORDANCES, type Context } from "./permissions";
import type { Category, Entry } from "./types";

function entry(name: string, kindCategory: Category = "text", kind: "file" | "dir" = "file"): Entry {
  return {
    name,
    path: name,
    kind,
    size: 10,
    modifiedMs: 0,
    mode: null,
    linkTarget: null,
    kindLabel: kindCategory,
    kindCategory,
    hidden: false,
  };
}

const RW: Context = { writable: true, reason: "", hasSelection: true };
const RO: Context = { writable: false, reason: "Exported read-only", hasSelection: true };

const byAction = (items: { action: MenuAction }[]) => items.map((item) => item.action);
const find = (items: ReturnType<typeof menuFor>, action: MenuAction) =>
  items.find((item) => item.action === action);

describe("the context menu's items", () => {
  it("offers the folder's own verbs on the list background", () => {
    const background = menuFor([], { ...RW, hasSelection: false });
    expect(byAction(background)).toEqual(["newFolder", "upload"]);
    expect(background.every((item) => !item.disabled)).toBe(true);
  });

  it("offers every verb for one editable file on a writable mount", () => {
    const items = menuFor([entry("notes.txt")], RW);
    expect(byAction(items)).toEqual([
      "quickLook",
      "edit",
      "download",
      "rename",
      "move",
      "delete",
      "properties",
    ]);
    expect(items.filter((item) => item.disabled)).toEqual([]);
  });

  it("disables exactly the writes on a read-only mount, and says the server's reason", () => {
    const items = menuFor([entry("notes.txt")], RO);
    const disabled = new Set(items.filter((item) => item.disabled).map((item) => item.action));
    const offered = new Set(byAction(items));

    for (const affordance of WRITE_AFFORDANCES) {
      if (!offered.has(affordance as MenuAction)) continue;
      expect(disabled.has(affordance as MenuAction), `${affordance} must be refused`).toBe(true);
    }
    for (const affordance of READ_AFFORDANCES) {
      if (!offered.has(affordance as MenuAction)) continue;
      expect(disabled.has(affordance as MenuAction), `${affordance} must stay`).toBe(false);
    }
    // Verbatim, not paraphrased: the server said why and the user can act on it.
    expect(find(items, "delete")?.reason).toBe("Exported read-only");
  });

  it("keeps the one-at-a-time verbs for one item, and says which", () => {
    const many = menuFor([entry("a.txt"), entry("b.txt"), entry("c.txt")], RW);
    for (const action of ["quickLook", "edit", "rename"] as const) {
      expect(find(many, action)?.disabled, action).toBe(true);
      expect(find(many, action)?.reason, action).toBe("Select one item");
    }
    // The verbs that do act on all of them stay, and say so.
    for (const action of ["download", "move", "delete"] as const) {
      expect(find(many, action)?.disabled, action).toBe(false);
    }
    expect(find(many, "delete")?.label).toBe("Delete 3 items");
    expect(find(many, "move")?.label).toBe("Move 3 items to…");
    expect(find(many, "properties")?.disabled).toBe(false);
  });

  it("counts only when there is more than one", () => {
    const one = menuFor([entry("a.txt")], RW);
    expect(find(one, "delete")?.label).toBe("Delete");
    expect(find(one, "move")?.label).toBe("Move to…");
  });

  it("offers Edit only for something this app can edit", () => {
    const video = menuFor([entry("film.mkv", "video")], RW);
    expect(find(video, "edit")?.disabled).toBe(true);
    expect(find(video, "edit")?.reason).toMatch(/text/i);
    // The rest of the menu is unaffected: a video is still previewable,
    // downloadable and deletable.
    expect(find(video, "quickLook")?.disabled).toBe(false);
    expect(find(video, "download")?.disabled).toBe(false);

    const folder = menuFor([entry("Series", "folder", "dir")], RW);
    expect(find(folder, "edit")?.disabled).toBe(true);
  });

  it("offers every action the type allows, somewhere", () => {
    // The same accretion guard the affordance list has: an action added to the
    // union and to the window's switch, but to no menu, is a verb nobody can
    // reach and a branch nobody runs.
    const all: Record<MenuAction, true> = {
      quickLook: true,
      edit: true,
      download: true,
      rename: true,
      move: true,
      delete: true,
      properties: true,
      newFolder: true,
      upload: true,
    };
    const offered = new Set([
      ...byAction(menuFor([entry("a.txt")], RW)),
      ...byAction(menuFor([], { ...RW, hasSelection: false })),
    ]);
    expect([...offered].sort()).toEqual(Object.keys(all).sort());
  });
});
