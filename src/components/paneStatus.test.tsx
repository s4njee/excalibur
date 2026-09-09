// Each pane's status bar answers for its own share (XTP-E2-S2).
//
// With one pane the bar could read the window and be right by construction.
// With two it cannot: the window has one `listing()`, one `activeMountId()`
// and one banner, and every one of them describes the focused pane. A bar
// sourced from those would say the focused pane's badge, counts and capacity
// under *both* columns — which is exactly the failure a two-pane view cannot
// afford, because the whole point of the second column is comparing it with
// the first.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Bookmark, Listing, MountEvent, MountInfo } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??=
  NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
/** The window's mount-event subscriber, so a test can push one through. */
let announce: ((event: MountEvent) => void) | null = null;
/** Which share the window has told the shell it is working with, in order. */
let activated: string[] = [];
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

const REMOTE = "xsync:freya.local:/mnt/data";
const LOCAL = "local::/users/dorian/downloads";

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

const bookmark = (over: Partial<Bookmark>): Bookmark => ({
  id: REMOTE,
  name: "data",
  protocol: "xsync",
  host: "freya.local",
  share: "/mnt/data",
  mountPreference: "rw",
  mounted: false,
  reachable: true,
  ...over,
});

const local = bookmark({
  id: LOCAL,
  name: "Downloads",
  protocol: "local",
  host: "",
  share: "/Users/dorian/Downloads",
});

const entry = (name: string) => ({
  name,
  path: name,
  kind: "file" as const,
  size: 10,
  modifiedMs: 0,
  addedMs: null,
  mode: null,
  linkTarget: null,
  kindLabel: "Text",
  kindCategory: "text" as const,
  hidden: false,
});

/**
 * Two shares that disagree about everything the status bar reports.
 *
 * The remote one is read-only with two files; the local one is writable with
 * four and knows its capacity. A bar that read the window would show one of
 * these twice.
 */
async function open(
  options: {
    remote?: Partial<MountInfo>;
    localMount?: Partial<MountInfo>;
  } = {},
) {
  announce = null;
  activated = [];
  const base = fixtureBackend();
  const backend = {
    ...base,
    onMountChanged(handler: (event: MountEvent) => void) {
      announce = handler;
      return () => {
        announce = null;
      };
    },
    async listDir(_path: string, id?: string): Promise<Listing> {
      if (id === LOCAL) {
        return {
          entries: ["a", "b", "c", "d"].map(entry),
          mount: mount({
            capacity: { free: 4_000_000_000, total: 8_000_000_000 },
            ...options.localMount,
          }),
        };
      }
      return {
        entries: ["one", "two"].map(entry),
        mount: mount({
          writable: false,
          reason: "Read-only — ro,root_squash",
          ...options.remote,
        }),
      };
    },
    async mount() {},
    async setActive(id: string) {
      activated.push(id);
    },
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{
          load: async () => [bookmark({}), local],
          save: async () => undefined,
        }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const click = (element: HTMLElement | null) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
const viewButton = () =>
  host.querySelector<HTMLElement>('[aria-label="View options"]');
const menuItem = (text: string) =>
  [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
    (candidate) => candidate.textContent?.includes(text),
  ) ?? null;
const bars = () => [...host.querySelectorAll<HTMLElement>('[role="status"]')];
const badges = () =>
  bars().map(
    (bar) =>
      bar.querySelector<HTMLElement>("[class*='badge']")?.textContent ?? null,
  );
const lists = () => [...host.querySelectorAll<HTMLElement>('[role="grid"]')];
const failedDots = () =>
  bars().map((bar) => bar.querySelector("[class*='healthDotFailed']") !== null);

async function twoPanes() {
  click(viewButton());
  await settle();
  click(menuItem("Two Panes"));
  await settle();
  await settle();
}

/** Open a saved share by double-clicking its sidebar row, into the focused pane. */
async function openFromSidebar(name: string) {
  const row = [...host.querySelectorAll<HTMLElement>("[role='button']")].find(
    (candidate) => candidate.textContent?.includes(name),
  );
  row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  await settle();
}

/** Click a pane's list, which is how focus moves between them. */
async function focusPane(index: number) {
  lists()[index]?.dispatchEvent(
    new MouseEvent("pointerdown", { bubbles: true }),
  );
  await settle();
}

/** The two panes side by side on two different shares. */
async function splitAcrossShares() {
  await open();
  await openFromSidebar("data");
  await twoPanes();
  await focusPane(1);
  await openFromSidebar("Downloads");
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

describe("per-pane status bars", () => {
  it("gives each pane its own bar", async () => {
    await open();
    expect(bars()).toHaveLength(1);
    await twoPanes();
    expect(bars()).toHaveLength(2);
  });

  it("badges each pane from its own mount, not the focused one's", async () => {
    await splitAcrossShares();
    expect(badges()).toEqual(["RO", "LOCAL"]);
  });

  it("keeps them apart whichever pane is focused", async () => {
    // The bug this story removes is a bar that follows focus. Moving focus
    // back to the left must change nothing either bar says.
    await splitAcrossShares();
    await focusPane(0);
    expect(badges()).toEqual(["RO", "LOCAL"]);
  });

  it("counts each pane's own rows", async () => {
    await splitAcrossShares();
    expect(bars()[0]?.textContent).toContain("2 items");
    expect(bars()[1]?.textContent).toContain("4 items");
  });

  it("reports capacity per pane, so a share without it does not borrow one", async () => {
    await splitAcrossShares();
    expect(bars()[0]?.textContent).not.toContain("free of");
    expect(bars()[1]?.textContent).toContain("free of");
  });

  it("says why the read-only pane is read-only, beside a writable one", async () => {
    await splitAcrossShares();
    expect(bars()[0]?.textContent).toContain("Read-only — ro,root_squash");
    expect(bars()[1]?.textContent).not.toContain("Read-only");
  });

  it("badges a local folder LOCAL", async () => {
    await open();
    await openFromSidebar("Downloads");
    expect(badges()).toEqual(["LOCAL"]);
  });

  it("reddens only the pane whose share is in trouble", async () => {
    // The event stream and the banner are the window's, and the window has one
    // of each. Letting them paint both dots would have a healthy share look
    // like it was reconnecting because the *other* one is.
    await splitAcrossShares();
    expect(failedDots()).toEqual([false, false]);
    announce?.({
      id: LOCAL,
      status: "reconnecting",
      mount: null,
      reason: "lost",
    });
    await settle();
    expect(failedDots()).toEqual([false, true]);
  });

  it("still says RO for a local folder that cannot be written", async () => {
    // `LOCAL` is a convenience; the badge's job is the permission. Losing a
    // read-only warning to a label would be trading the one for the other —
    // a folder you cannot write is a warning, and which machine it is on is
    // not.
    await open({
      localMount: {
        writable: false,
        reason: "Read-only — no write permission",
      },
    });
    await openFromSidebar("Downloads");
    expect(badges()).toEqual(["RO"]);
  });
});

describe("which share the shell thinks this window is on", () => {
  it("follows the focused pane, not whichever was opened last", async () => {
    // The shell keeps one active mount per window and resolves `mkdir`,
    // `rename`, `remove` and the edit round trip through it. It was set when a
    // share was *connected* — so with two panes, a delete in the pane that was
    // not opened most recently went to a share the user was not looking at.
    await splitAcrossShares();
    expect(activated[activated.length - 1]).toBe(LOCAL);
    await focusPane(0);
    expect(activated[activated.length - 1]).toBe(REMOTE);
    await focusPane(1);
    expect(activated[activated.length - 1]).toBe(LOCAL);
  });
});
