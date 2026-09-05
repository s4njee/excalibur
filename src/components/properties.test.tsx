// The Properties sheet (E14-S4).
//
// Two things this has to get right and nothing else will catch. The first is
// that it never claims a size it did not measure: the AC forbids walking a
// folder, so a folder's size is an absence, and `0 B` is the failure mode —
// it is a plausible-looking number that says "empty" about a folder that may
// hold a terabyte. The second is that it stays read-only: it is reachable
// from the context menu on a read-only mount, and the way it becomes unsafe
// is somebody adding a field to it later.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import { Properties } from "./Properties";
import type { Entry, EntryKind } from "../state/types";

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const entry = (over: Partial<Entry> & { name: string; kind: EntryKind }): Entry => ({
  path: over.name,
  size: 0,
  modifiedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: over.kind === "dir" ? "Folder" : "Matroska Video",
  kindCategory: over.kind === "dir" ? "folder" : "video",
  hidden: false,
  ...over,
});

/** `31 Aug 2026, 18:24` in local time, the way the Modified column reads it. */
const AUG_31 = new Date(2026, 7, 31, 18, 24).getTime();
const AUG_30 = new Date(2026, 7, 30, 9, 5).getTime();

const show = (entries: Entry[], location = "Documentaries", shareName = "media") => {
  const closes: number[] = [];
  dispose = render(
    () => (
      <Properties
        entries={entries}
        location={location}
        shareName={shareName}
        onClose={() => closes.push(1)}
      />
    ),
    host,
  );
  return closes;
};

const text = () => host.textContent ?? "";
const sheet = () => host.querySelector('[role="dialog"]') as HTMLElement;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("one file", () => {
  const film = () =>
    entry({
      name: "Arrival.2016.mkv",
      path: "Documentaries/Arrival.2016.mkv",
      kind: "file",
      size: 48_234_496_000,
      modifiedMs: AUG_31,
    });

  it("names it, and says what the engine says it is", () => {
    show([film()]);
    expect(text()).toContain("Arrival.2016.mkv");
    // The engine's kind table, not the extension. `.mkv` mapped by this
    // component would be a second, divergent answer to the Kind column's.
    expect(text()).toContain("Matroska Video");
  });

  it("gives the size twice: the list's units and the exact count", () => {
    show([film()]);
    // Rounded, so it matches the Size column the user just came from...
    expect(text()).toContain("48.2 GB");
    // ...and exact, which is the whole reason to open this sheet on a file.
    expect(text()).toContain("48,234,496,000 bytes");
  });

  it("gives the modified time in full, not the list's short form", () => {
    show([film()]);
    expect(text()).toContain("31 Aug 2026, 18:24");
  });

  it("shows the path relative to the share root", () => {
    show([film()]);
    expect(text()).toContain("/Documentaries/Arrival.2016.mkv");
  });

  it("says nothing about a link when there is no link", () => {
    show([film()]);
    expect(text()).not.toContain("Links to");
  });
});

describe("a symlink", () => {
  it("shows the target verbatim", () => {
    show([
      entry({
        name: "latest.mkv",
        kind: "symlink",
        size: 24,
        linkTarget: "../archive/Arrival.2016.mkv",
        kindLabel: "Alias",
        kindCategory: "alias",
      }),
    ]);
    expect(text()).toContain("Links to");
    // Unresolved: relative, absolute or dangling is the server's business, and
    // a resolved path shown here would be this window's guess presented as
    // fact.
    expect(text()).toContain("../archive/Arrival.2016.mkv");
  });
});

describe("one folder", () => {
  it("shows a dash for its size rather than a number it did not measure", () => {
    show([entry({ name: "Documentaries", kind: "dir", modifiedMs: AUG_31 })]);
    // The bug this exists to prevent: a folder's `size` field is 0 (or the
    // directory inode's size), and printing it says "empty" about a folder
    // nobody counted.
    expect(text()).not.toContain("0 B");
    expect(text()).not.toContain("0 bytes");
    expect(text()).toContain("—");
    // And it says why, because a bare dash reads as a bug rather than a
    // decision.
    expect(text()).toContain("not measured");
  });

  it("still gives the things it does know", () => {
    show([entry({ name: "Documentaries", kind: "dir", modifiedMs: AUG_31 })]);
    expect(text()).toContain("Folder");
    expect(text()).toContain("31 Aug 2026, 18:24");
  });
});

describe("several entries", () => {
  const selection = () => [
    entry({ name: "a.mkv", kind: "file", size: 1_000_000_000, modifiedMs: AUG_30 }),
    entry({ name: "b.mkv", kind: "file", size: 500_000_000, modifiedMs: AUG_31 }),
    // A directory whose reported size is not zero, which is what several real
    // backends return. It must not reach the total.
    entry({ name: "Docs", kind: "dir", size: 4096 }),
  ];

  it("counts files and folders separately", () => {
    show(selection());
    expect(text()).toContain("2 files, 1 folder");
  });

  it("totals the files only, and says that is what it did", () => {
    show(selection());
    expect(text()).toContain("1.5 GB");
    expect(text()).toContain("1,500,000,000 bytes");
    // 1,500,004,096 would be the total with the directory's own size folded
    // in — plausible, wrong, and impossible to spot by eye.
    expect(text()).not.toContain("1,500,004,096");
    expect(text()).toContain("Folders are not included");
  });

  it("gives the newest change across the selection", () => {
    show(selection());
    expect(text()).toContain("31 Aug 2026, 18:24");
    expect(text()).not.toContain("30 Aug 2026");
  });

  it("says where they are", () => {
    show(selection(), "Documentaries/2024");
    expect(text()).toContain("media/Documentaries/2024");
  });

  it("names the share when they are at its root", () => {
    // The share root has no path to show, and an empty "Where" is not an
    // answer. This is the only thing `shareName` is for.
    show(selection(), "");
    expect(text()).toContain("media");
  });

  it("drops the folder caveat when nothing selected is a folder", () => {
    show([entry({ name: "a.mkv", kind: "file", size: 10 })]);
    expect(text()).not.toContain("Folders are not included");
  });
});

describe("it is read-only", () => {
  it("offers no control but the one that closes it", () => {
    // Enumerated rather than spot-checked: the failure is a field added here
    // later, on the reasonable-sounding grounds that Properties is where you
    // rename things. On a read-only mount that field would be a write verb
    // with no gate in front of it.
    show([entry({ name: "a.mkv", kind: "file", size: 10 })]);
    const buttons = [...host.querySelectorAll("button")];
    expect(buttons.map((button) => button.textContent?.trim())).toEqual(["Done"]);
    expect(host.querySelectorAll("input, textarea, select").length).toBe(0);
    expect(host.querySelectorAll("[contenteditable]").length).toBe(0);
  });

  it("takes no mount and so has nothing to gate", () => {
    // The sheet is reachable from a read-only mount by design. It renders the
    // same either way because it never had a write in it to disable — which
    // is why there is no `writable` prop to get wrong.
    const closes = show([entry({ name: "a.mkv", kind: "file", size: 10 })]);
    expect(closes.length).toBe(0);
    expect(sheet()).not.toBeNull();
  });
});

describe("closing it", () => {
  it("closes on Done", () => {
    const closes = show([entry({ name: "a.mkv", kind: "file", size: 10 })]);
    host.querySelector("button")?.click();
    expect(closes.length).toBe(1);
  });

  it("closes on a click outside, and not on a click inside", () => {
    const closes = show([entry({ name: "a.mkv", kind: "file", size: 10 })]);
    sheet().click();
    expect(closes.length).toBe(0);
    (host.firstElementChild as HTMLElement).click();
    expect(closes.length).toBe(1);
  });

  it("closes on Escape without letting it reach the window", () => {
    // The window's Escape clears the filter and then the selection. If this
    // key bubbled, dismissing the sheet would also empty the selection it was
    // describing — the user would close Properties and find their files
    // deselected.
    const closes = show([entry({ name: "a.mkv", kind: "file", size: 10 })]);
    const seen: string[] = [];
    const listener = () => seen.push("window");
    window.addEventListener("keydown", listener);
    try {
      sheet().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    } finally {
      window.removeEventListener("keydown", listener);
    }
    expect(closes.length).toBe(1);
    expect(seen).toEqual([]);
  });
});
