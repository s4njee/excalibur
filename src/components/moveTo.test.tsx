// The Move to… picker (E14-S3).
//
// `move.ts` already holds the rules and proves them; what is left to test here
// is the wiring, and the wiring has its own ways of being wrong: opening at
// the root instead of where the items are, listing files the user cannot move
// into, handing the caller a folder's *name* where it wanted a path, or
// letting Escape close the sheet and clear the window's filter behind it.

import { describe, expect, it, afterEach, beforeEach, vi } from "vitest";
import { render } from "solid-js/web";
import { MoveTo } from "./MoveTo";
import type { Entry, Listing, MountInfo } from "../state/types";

const mount: MountInfo = {
  status: "ready",
  writable: true,
  reason: "",
  protocolLabel: "SMB",
  securityLabel: "signed",
  options: "",
  capacity: null,
  caps: {
    randomAccess: true,
    resumeWrite: true,
    atomicRename: true,
    symlinks: true,
    watch: false,
  },
};

const entry = (path: string, kind: Entry["kind"]): Entry => ({
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

/**
 * A share with the shapes the rules care about.
 *
 * `archive/docs-old` is the prefix trap, on screen: it is a legal destination
 * for `docs`, and a picker that refused it would refuse it here.
 */
const TREE: Record<string, Entry[]> = {
  "": [entry("docs", "dir"), entry("archive", "dir"), entry("notes.txt", "file")],
  docs: [entry("docs/2024", "dir"), entry("docs/report.pdf", "file")],
  "docs/2024": [],
  archive: [entry("archive/docs-old", "dir")],
  "archive/docs-old": [],
};

let listed: string[] = [];
const listDir = async (path: string): Promise<Listing> => {
  listed.push(path);
  const entries = TREE[path];
  if (!entries) throw new Error(`no such folder: ${path}`);
  return { entries, mount };
};

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

const rows = () => [...host.querySelectorAll<HTMLElement>("[data-destination]")];
const rowNamed = (name: string) => rows().find((row) => row.textContent?.trim() === name);
const button = (label: string) =>
  [...host.querySelectorAll("button")].find((candidate) => candidate.textContent?.trim() === label);
const moveHere = () => button("Move here") as HTMLButtonElement | undefined;
const text = () => host.textContent ?? "";

/** Click a folder row and let its listing arrive. */
async function open(name: string) {
  rowNamed(name)?.click();
  await settle();
}

async function show(options: {
  entries: Entry[];
  from: string;
  onMove?: (destination: string) => void;
  onCancel?: () => void;
}) {
  dispose = render(
    () => (
      <MoveTo
        entries={options.entries}
        from={options.from}
        shareName="media"
        listDir={listDir}
        onMove={options.onMove ?? (() => undefined)}
        onCancel={options.onCancel ?? (() => undefined)}
      />
    ),
    host,
  );
  await settle();
}

beforeEach(() => {
  listed = [];
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("where it opens", () => {
  it("starts in the folder the items are in, not at the share root", async () => {
    await show({ entries: [entry("docs/report.pdf", "file")], from: "docs" });
    expect(listed).toEqual(["docs"]);
    // And it says so, rather than offering a Move here that would do nothing.
    expect(moveHere()?.disabled).toBe(true);
    expect(text()).toContain("“report.pdf” is already in this folder.");
  });

  it("offers a way up from there, and none at the root", async () => {
    await show({ entries: [entry("docs/report.pdf", "file")], from: "docs" });
    expect(rowNamed("..")).toBeDefined();

    await open("..");
    expect(listed).toEqual(["docs", ""]);
    expect(rowNamed("..")).toBeUndefined();
  });
});

describe("what it lists", () => {
  it("lists only directories", async () => {
    // A file cannot be a destination, and drawing one is an invitation to
    // click it and wonder why nothing happened.
    await show({ entries: [entry("docs/2024", "dir")], from: "docs" });
    expect(rowNamed("report.pdf")).toBeUndefined();
    expect(rowNamed("2024")).toBeDefined();
  });

  it("lists a folder that is itself being moved", async () => {
    // Hiding it would leave a hole where the user can see a folder should be.
    // The refusal that names it is more use than its absence.
    await show({ entries: [entry("docs", "dir")], from: "" });
    expect(rowNamed("docs")).toBeDefined();
  });

  it("does not label its rows the way the file list does", async () => {
    // The sheet renders inside the window, and everything that hit-tests the
    // listing — the selection, the drop target, the status bar's count — keys
    // on `data-path`. A picker row carrying one would join the listing.
    await show({ entries: [entry("docs", "dir")], from: "" });
    expect(host.querySelector("[data-path]")).toBeNull();
  });

  it("says so when a folder is empty, and still offers it", async () => {
    await show({ entries: [entry("docs/report.pdf", "file")], from: "docs" });
    await open("2024");
    expect(text()).toContain("No folders here.");
    expect(moveHere()?.disabled).toBe(false);
  });

  it("reports a listing that failed rather than showing an empty folder", async () => {
    await show({ entries: [entry("gone/a", "file")], from: "gone" });
    expect(text()).toContain("no such folder: gone");
    expect(text()).not.toContain("No folders here.");
  });
});

describe("choosing a destination", () => {
  it("hands back the path it walked to, not the folder's name", async () => {
    // `archive/docs-old`, not `docs-old`: the caller renames straight into
    // this string, and a bare name would move the items to the share root.
    const moved = vi.fn();
    await show({ entries: [entry("docs", "dir")], from: "", onMove: moved });
    await open("archive");
    await open("docs-old");
    expect(moveHere()?.disabled).toBe(false);
    moveHere()?.click();
    expect(moved).toHaveBeenCalledWith("archive/docs-old");
  });

  it("moves nothing itself", async () => {
    // The picker has one backend call and it is `listDir`. Everything else —
    // the renames, the partial-failure report, the re-list — is the caller's.
    const moved = vi.fn();
    await show({ entries: [entry("docs", "dir")], from: "", onMove: moved });
    await open("archive");
    moveHere()?.click();
    expect(moved).toHaveBeenCalledWith("archive");
    expect(listed).toEqual(["", "archive"]);
  });

  it("offers the share root as a destination", async () => {
    const moved = vi.fn();
    await show({ entries: [entry("docs/report.pdf", "file")], from: "docs", onMove: moved });
    await open("..");
    moveHere()?.click();
    expect(moved).toHaveBeenCalledWith("");
  });

  it("goes back up the breadcrumb", async () => {
    await show({ entries: [entry("docs/report.pdf", "file")], from: "docs" });
    await open("..");
    await open("archive");
    button("media")?.click();
    await settle();
    expect(rowNamed("notes.txt")).toBeUndefined();
    expect(rowNamed("docs")).toBeDefined();
    expect(rowNamed("..")).toBeUndefined();
  });
});

describe("refusals", () => {
  it("disables Move here inside a folder being moved, with the reason on screen", async () => {
    await show({ entries: [entry("docs", "dir")], from: "" });

    await open("docs");
    expect(moveHere()?.disabled).toBe(true);
    expect(text()).toContain("You cannot move “docs” into itself.");

    await open("2024");
    expect(moveHere()?.disabled).toBe(true);
    expect(text()).toContain("You cannot move “docs” into a folder inside it.");
  });

  it("points the disabled button at the reason", async () => {
    // A disabled control and a sentence elsewhere in the sheet are two facts
    // a screen reader user has to connect for themselves.
    await show({ entries: [entry("docs", "dir")], from: "" });
    await open("docs");
    const described = moveHere()?.getAttribute("aria-describedby");
    expect(described).toBeTruthy();
    expect(host.querySelector(`#${described}`)?.textContent).toContain("into itself");
  });

  it("allows a sibling whose name merely starts the same way", async () => {
    await show({ entries: [entry("docs", "dir")], from: "" });
    await open("archive");
    await open("docs-old");
    expect(moveHere()?.disabled).toBe(false);
  });
});

describe("escape", () => {
  it("cancels, and does not reach the window behind it", async () => {
    // The window's own Escape handler clears the filter and then the
    // selection. One Escape closes one thing, so the sheet takes it in the
    // capture phase and stops it there.
    const cancelled = vi.fn();
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    try {
      await show({ entries: [entry("docs", "dir")], from: "", onCancel: cancelled });
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      expect(cancelled).toHaveBeenCalledTimes(1);
      expect(behind).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", behind);
    }
  });

  it("lets every other key through", async () => {
    const cancelled = vi.fn();
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    try {
      await show({ entries: [entry("docs", "dir")], from: "", onCancel: cancelled });
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
      expect(cancelled).not.toHaveBeenCalled();
      expect(behind).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("keydown", behind);
    }
  });

  it("stops listening once it is gone", async () => {
    // A sheet that keeps its window listener after unmounting cancels a move
    // the user started afterwards.
    const cancelled = vi.fn();
    await show({ entries: [entry("docs", "dir")], from: "", onCancel: cancelled });
    dispose?.();
    dispose = undefined;
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(cancelled).not.toHaveBeenCalled();
  });
});
