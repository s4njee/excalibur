// The per-pane directory tree (XTP-E2-S4).
//
// The first tree in this app, so what is asserted here is mostly what a tree
// has to get right rather than anything this codebase already does: that
// opening a folder is not the same gesture as going into it, that a folder
// listed once is not listed again, and that the tree follows the pane wherever
// the pane goes — including when it was the file list, not the tree, that
// moved it.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs, type Prefs } from "../state/prefs";
import type { Entry, Listing, MountInfo } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));
let listed: string[] = [];

const mount = (): MountInfo => ({
  status: "ready",
  writable: true,
  reason: "",
  protocolLabel: "xsync v3",
  securityLabel: "ssh",
  options: "",
  capacity: null,
  caps: {
    randomAccess: true,
    resumeWrite: true,
    atomicRename: true,
    symlinks: true,
    watch: false,
  },
});

const entry = (name: string, over: Partial<Entry> = {}): Entry => ({
  name: name.split("/").pop() ?? name,
  path: name,
  kind: "dir",
  size: 0,
  modifiedMs: 0,
  addedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: "Folder",
  kindCategory: "folder",
  hidden: (name.split("/").pop() ?? name).startsWith("."),
  ...over,
});

/** A three-level share: `Films/2016/…`, with a file at the root to ignore. */
const SHARE: Record<string, Entry[]> = {
  "": [entry("Films"), entry("Shows"), entry("notes.txt", { kind: "file" })],
  Films: [entry("Films/2016"), entry("Films/.cache")],
  "Films/2016": [entry("Films/2016/Arrival.mkv", { kind: "file" })],
  Shows: [],
};

async function open(prefs: Prefs = memoryPrefs()) {
  listed = [];
  const base = fixtureBackend();
  const backend = {
    ...base,
    async listDir(path: string): Promise<Listing> {
      listed.push(path);
      return { entries: SHARE[path] ?? [], mount: mount() };
    },
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={prefs}
        bookmarks={{ load: async () => [], save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const click = (element: Element | null | undefined) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
const viewButton = () => host.querySelector<HTMLElement>('[aria-label="View options"]');
const menuItem = (text: string) =>
  [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((candidate) =>
    candidate.textContent?.includes(text),
  ) ?? null;
const trees = () => [...host.querySelectorAll<HTMLElement>('[role="tree"]')];
const tree = (index = 0) => trees()[index] ?? null;
const rows = (index = 0) => [...(tree(index)?.querySelectorAll<HTMLElement>('[role="treeitem"]') ?? [])];
const row = (name: string, index = 0) =>
  rows(index).find((candidate) => candidate.textContent?.includes(name)) ?? null;
/** Row names without the disclosure glyph, which shares the row's text. */
const names = (index = 0) =>
  rows(index).map(
    (candidate) => candidate.querySelector("[class*='treeName']")?.textContent ?? "",
  );
const handles = () =>
  [...host.querySelectorAll<HTMLElement>('[aria-label^="Resize"][aria-orientation="horizontal"]')];
const paneUris = () =>
  [...host.querySelectorAll<HTMLElement>("[class*='paneTitleUri']")].map(
    (element) => element.textContent,
  );

async function twoPanes() {
  click(viewButton());
  await settle();
  click(menuItem("Two Panes"));
  await settle();
  await settle();
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("the directory tree", () => {
  it("belongs to the two-pane view, where the handoff draws it", async () => {
    // A single-pane window has the sidebar for this; a second navigator beside
    // it would be two places to click the same folder.
    await open();
    expect(trees()).toHaveLength(0);
    await twoPanes();
    expect(trees()).toHaveLength(2);
  });

  it("draws the share's folders, and no files", async () => {
    await open();
    await twoPanes();
    expect(names()).toEqual(["Films", "Shows"]);
  });

  it("takes the root from the pane's own listing rather than asking again", async () => {
    // The pane has just listed the folder it is in. A tree that fetched it
    // again would cost a round trip for an answer already on screen — and
    // could get a different one.
    await open();
    const before = listed.length;
    await twoPanes();
    expect(names()).toEqual(["Films", "Shows"]);
    // One listing: the second pane's own. The left pane already had its
    // folder, and neither tree asked for the root it was handed.
    expect(listed.length - before).toBe(1);
  });

  it("opens a folder from its disclosure, without going there", async () => {
    // Two gestures in one row. Expanding to see what is inside is not the same
    // as walking in, and a tree that navigated on every disclosure would make
    // looking around impossible.
    await open();
    await twoPanes();
    const before = paneUris()[0];
    click(row("Films")?.querySelector("[class*='treeDisclosure']"));
    await settle();
    await settle();
    expect(names()).toEqual(["Films", ".cache", "2016", "Shows"]);
    expect(paneUris()[0]).toBe(before);
  });

  it("indents what it opened", async () => {
    await open();
    await twoPanes();
    click(row("Films")?.querySelector("[class*='treeDisclosure']"));
    await settle();
    await settle();
    expect(row("2016")?.getAttribute("aria-level")).toBe("2");
    expect(row("Films")?.getAttribute("aria-expanded")).toBe("true");
  });

  it("lists a folder once, however often it is opened", async () => {
    await open();
    await twoPanes();
    const disclosure = () => row("Films")?.querySelector("[class*='treeDisclosure']");
    click(disclosure());
    await settle();
    await settle();
    const after = listed.length;
    click(disclosure());
    await settle();
    click(disclosure());
    await settle();
    await settle();
    expect(names()).toContain("2016");
    expect(listed.length).toBe(after);
  });

  it("goes to the folder a row names", async () => {
    await open();
    await twoPanes();
    click(row("Films"));
    await settle();
    await settle();
    expect(paneUris()[0]).toContain("Films");
    // The other pane stayed where it was: a tree belongs to its own pane.
    expect(paneUris()[1]).not.toContain("Films");
  });

  it("follows the pane, including when the file list is what moved it", async () => {
    // The selected row is the folder the pane is *in*, not a selection of the
    // tree's own — so double-clicking Films in the list has to move it.
    await open();
    await twoPanes();
    const listRow = host.querySelector<HTMLElement>('[data-path="Films"]');
    listRow?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    await settle();
    expect(row("Films")?.getAttribute("aria-selected")).toBe("true");
    expect(row("Shows")?.getAttribute("aria-selected")).toBe("false");
  });

  it("opens the ancestors of wherever the pane lands", async () => {
    // Two levels down, by the file list. Every folder on the way has to open
    // or the row for where you are would not be on screen at all.
    await open();
    await twoPanes();
    const openInList = async (path: string) => {
      host
        .querySelector<HTMLElement>(`[data-path="${path}"]`)
        ?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
      await settle();
      await settle();
    };
    await openInList("Films");
    await openInList("Films/2016");
    expect(row("Films")?.getAttribute("aria-expanded")).toBe("true");
    expect(row("2016")?.getAttribute("aria-selected")).toBe("true");
    // No disclosure at all: the pane listed `Films/2016` to draw it, the tree
    // took that listing, and it holds no folders. Announcing a closed
    // disclosure on a row that cannot open would be the tree claiming
    // something it now knows is untrue.
    expect(row("2016")?.hasAttribute("aria-expanded")).toBe(false);
  });

  it("keeps the disclosure of a folder it has never listed", async () => {
    // `Shows` is empty, but the tree does not know that until it asks.
    await open();
    await twoPanes();
    expect(row("Shows")?.getAttribute("aria-expanded")).toBe("false");
    click(row("Shows")?.querySelector("[class*='treeDisclosure']"));
    await settle();
    await settle();
    // Listed and empty: no disclosure to announce at all.
    expect(row("Shows")?.hasAttribute("aria-expanded")).toBe(false);
  });

  it("is put away and brought back from the View menu, one pane at a time", async () => {
    await open();
    await twoPanes();
    click(viewButton());
    await settle();
    click(menuItem("Directory Tree"));
    await settle();
    // The focused pane is the left one, so the right pane keeps its band.
    expect(trees()).toHaveLength(1);
    click(viewButton());
    await settle();
    click(menuItem("Directory Tree"));
    await settle();
    expect(trees()).toHaveLength(2);
  });

  it("greys the row where there is no second pane to put a tree in", async () => {
    await open();
    click(viewButton());
    await settle();
    const item = menuItem("Directory Tree");
    expect(item?.getAttribute("aria-disabled")).toBe("true");
    expect(item?.getAttribute("title")).toBe("Two panes only");
  });

  it("remembers each band's height across a relaunch", async () => {
    const prefs = memoryPrefs();
    await prefs.set("treeBand", { left: 0, right: 160 });
    await open(prefs);
    await twoPanes();
    // The left band was put away; the right one is the height it was left.
    expect(trees()).toHaveLength(1);
    expect(tree()?.parentElement?.style.height).toBe("160px");
  });

  it("resizes by its handle, and puts itself away when dragged shut", async () => {
    const prefs = memoryPrefs();
    await open(prefs);
    await twoPanes();
    const drag = (from: number, to: number) => {
      handles()[0]?.dispatchEvent(
        new MouseEvent("pointerdown", { bubbles: true, clientY: from, button: 0 }),
      );
      window.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientY: to }));
      window.dispatchEvent(new MouseEvent("pointerup", { bubbles: true }));
    };
    drag(200, 240);
    await settle();
    expect(tree()?.parentElement?.style.height).toBe("144px");
    expect(await prefs.get("treeBand")).toEqual({ left: 144, right: 104 });
    drag(240, 100);
    await settle();
    // Dragged shut is the same state the menu row produces, and it persists.
    expect(trees()).toHaveLength(1);
    expect(await prefs.get("treeBand")).toEqual({ left: 0, right: 104 });
  });
});
