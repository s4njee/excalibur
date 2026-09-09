// Sync browsing (XTP-E6-S1).
//
// One pane's walk repeated in the other, by *relative* path — the two panes are
// usually two different roots, and following the subpath is the whole gesture.
// What is worth asserting is mostly the refusals: a mirror that cannot land
// leaves the other pane where it is and says so in that pane rather than in a
// modal, and a mirror that lands does not bounce back and forth forever.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs, type Prefs } from "../state/prefs";
import type { Bookmark, Entry, Listing, MountInfo } from "../state/types";

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
let created: string[] = [];

const OTHER = "xsync:freya.local:/mnt/backups";

const bookmark: Bookmark = {
  id: OTHER,
  name: "backups",
  protocol: "xsync",
  host: "freya.local",
  share: "/mnt/backups",
  mountPreference: "rw",
  mounted: false,
  reachable: true,
};

const mount = (over: Partial<MountInfo> = {}): MountInfo => ({
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
  ...over,
});

const dir = (path: string): Entry => ({
  name: path.split("/").pop() ?? path,
  path,
  kind: "dir",
  size: 0,
  modifiedMs: 0,
  addedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: "Folder",
  kindCategory: "folder",
  hidden: false,
});

const parentOf = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "");

/**
 * Two shares with different trees.
 *
 * The window's own mount has `Films/2016`; the one opened into the second pane
 * stops at `Films`. So walking down two levels in the first pane is a mirror
 * that lands and then one that cannot.
 */
function shares() {
  return {
    here: new Set(["", "Films", "Films/2016", "Shows"]),
    other: new Set(["", "Films"]),
  };
}

async function open(options: { prefs?: Prefs; writable?: boolean } = {}) {
  listed = [];
  created = [];
  const trees = shares();
  const base = fixtureBackend();
  const backend = {
    ...base,
    async listDir(path: string, id?: string): Promise<Listing> {
      listed.push(`${id ?? "here"}:${path}`);
      const tree = id === OTHER ? trees.other : trees.here;
      if (!tree.has(path)) throw { message: `no such directory: ${path}` };
      const entries = [...tree]
        .filter((candidate) => candidate !== "" && parentOf(candidate) === path)
        .map(dir);
      return {
        entries,
        mount: mount({ writable: id === OTHER ? (options.writable ?? true) : true }),
      };
    },
    async mkdir(path: string): Promise<void> {
      created.push(path);
      trees.other.add(path);
    },
    async mount() {},
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={options.prefs ?? memoryPrefs()}
        bookmarks={{ load: async () => [bookmark], save: async () => undefined }}
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
const lists = () => [...host.querySelectorAll<HTMLElement>('[role="grid"]')];
const paths = () =>
  [...host.querySelectorAll<HTMLElement>("[class*='paneTitleUri']")].map(
    (element) => element.textContent ?? "",
  );
const warnings = () =>
  [...host.querySelectorAll<HTMLElement>("[class*='paneWarning']")].filter(
    (element) => element.getAttribute("role") === "status",
  );
const button = (label: string) =>
  [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (candidate) => candidate.textContent === label,
  ) ?? null;

async function chooseView(label: string) {
  click(viewButton());
  await settle();
  click(menuItem(label));
  await settle();
  await settle();
}

/** Walk up, by the `..` row the list draws at the top. */
async function ascend(index: number) {
  const pane = lists()[index]?.closest("[class*='paneColumn']");
  const row = pane?.querySelector<HTMLElement>('[role="row"][title="Enclosing folder"]');
  expect(row, `a parent row in pane ${index}`).not.toBeNull();
  row?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  await settle();
  await settle();
}

/** Open a folder in one pane's list, which is how a walk actually happens. */
async function openIn(index: number, path: string) {
  const row = lists()[index]?.querySelector<HTMLElement>(`[data-path="${path}"]`);
  expect(row, `a row for ${path} in pane ${index}`).not.toBeNull();
  row?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  await settle();
  await settle();
}

/** Two panes, the second on the other share, with sync browsing on. */
async function twoShares(options: { sync?: boolean } = {}) {
  await chooseView("Two Panes");
  lists()[1]?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  await settle();
  const row = [...host.querySelectorAll<HTMLElement>("[role='button']")].find((candidate) =>
    candidate.textContent?.includes("backups"),
  );
  row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  await settle();
  // Back to the left pane, which is the one the tests walk.
  lists()[0]?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  await settle();
  if (options.sync !== false) await chooseView("Sync Browsing");
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

describe("sync browsing", () => {
  it("is off until it is asked for", async () => {
    // The handoff has it on; this window opens its second pane on the folder
    // the first is showing (XTP-E2-S1), and two panes in lockstep through one
    // share is a second pane that shows nothing the first does not.
    await open();
    await twoShares({ sync: false });
    await openIn(0, "Films");
    expect(paths()[0]).toContain("Films");
    expect(paths()[1]).not.toContain("Films");
  });

  it("names each pane after its own share", async () => {
    // Not sync browsing itself, but what made it untestable: every pane's
    // crumbs were built from *the first mounted bookmark*, so opening a share
    // into the second pane renamed the first one's title band too.
    await open();
    await twoShares({ sync: false });
    expect(paths()[0]).toBe("/media");
    expect(paths()[1]).toBe("/mnt/backups");
  });

  it("follows the other pane once it is on", async () => {
    await open();
    await twoShares();
    await openIn(0, "Films");
    expect(paths()[0]).toContain("Films");
    expect(paths()[1]).toContain("Films");
  });

  it("follows whichever pane moved", async () => {
    await open();
    await twoShares();
    await openIn(1, "Films");
    expect(paths()[1]).toContain("Films");
    expect(paths()[0]).toContain("Films");
  });

  it("does not bounce a mirrored pane back", async () => {
    // The loop this exists to avoid: the pane that followed fires its own
    // effect, which would follow the first one back, forever.
    await open();
    await twoShares();
    const before = listed.length;
    await openIn(0, "Films");
    expect(listed.length - before).toBeLessThan(6);
  });

  it("keeps the other pane put where the path is not there", async () => {
    // `Films/2016` is only on this window's own share.
    await open();
    await twoShares();
    await openIn(0, "Films");
    await openIn(0, "Films/2016");
    expect(paths()[0]).toContain("Films/2016");
    expect(paths()[1]).toContain("Films");
    expect(paths()[1]).not.toContain("2016");
  });

  it("says so in the pane that could not follow, and nowhere else", async () => {
    // Never a modal: the user is mid-walk in the *other* pane, and a dialog
    // would stop a gesture that succeeded.
    await open();
    await twoShares();
    await openIn(0, "Films");
    expect(warnings()).toHaveLength(0);
    await openIn(0, "Films/2016");
    expect(warnings()).toHaveLength(1);
    expect(warnings()[0]?.textContent).toContain("Films/2016");
    // In the right pane's own column, which is the one that is stuck.
    expect(lists()[1]?.closest("[class*='paneColumn']")?.contains(warnings()[0]!)).toBe(true);
  });

  it("offers to create the folder, and follows once it is there", async () => {
    await open();
    await twoShares();
    await openIn(0, "Films");
    await openIn(0, "Films/2016");
    click(button("Create folder"));
    await settle();
    await settle();
    await settle();
    expect(created).toEqual(["Films/2016"]);
    expect(paths()[1]).toContain("Films/2016");
    expect(warnings()).toHaveLength(0);
  });

  it("offers nothing to create on a share that cannot be written", async () => {
    await open({ writable: false });
    await twoShares();
    await openIn(0, "Films");
    await openIn(0, "Films/2016");
    expect(warnings()).toHaveLength(1);
    expect(button("Create folder")).toBeNull();
  });

  it("clears the warning when the panes are back in step", async () => {
    // Walking back up puts the first pane where the second already is. The
    // warning was about a path neither is on any more.
    await open();
    await twoShares();
    await openIn(0, "Films");
    await openIn(0, "Films/2016");
    expect(warnings()).toHaveLength(1);
    await ascend(0);
    expect(warnings()).toHaveLength(0);
  });

  it("stops following when it is turned off", async () => {
    await open();
    await twoShares();
    await chooseView("Sync Browsing");
    await openIn(0, "Films");
    expect(paths()[0]).toContain("Films");
    expect(paths()[1]).not.toContain("Films");
  });
});
