// The transfer gutter (XTP-E3-S1).
//
// Direction is spatial with two panes: the arrows send the focused selection
// the way they point. What matters most here is what they *refuse* — the
// transfer surface is share↔local, so two remote shares is not expressible,
// and a blocked write must never be attempted.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Bookmark, Listing, MountInfo } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

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
  id: "xsync:freya.local:/mnt/data",
  name: "data",
  protocol: "xsync",
  host: "freya.local",
  share: "/mnt/data",
  mountPreference: "rw",
  mounted: false,
  reachable: true,
  ...over,
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

let uploads: unknown[][] = [];
let downloads: unknown[][] = [];

async function open(options: { saved?: Bookmark[]; mountInfo?: MountInfo } = {}) {
  uploads = [];
  downloads = [];
  const base = fixtureBackend();
  const backend = {
    ...base,
    async listDir(_path: string, _mount?: string): Promise<Listing> {
      return { entries: [entry("a.txt"), entry("b.txt")], mount: options.mountInfo ?? mount() };
    },
    async mount() {},
    async checkConflicts() {
      return [];
    },
    async enqueueUploads(...args: unknown[]) {
      uploads.push(args);
      return ["u1"];
    },
    async enqueueDownloads(...args: unknown[]) {
      downloads.push(args);
      return ["d1"];
    },
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => options.saved ?? [], save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const viewButton = () => host.querySelector<HTMLElement>('[aria-label="View options"]');
const menuItem = (text: string) =>
  [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((candidate) =>
    candidate.textContent?.includes(text),
  ) ?? null;
const click = (element: HTMLElement | null) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
const rightArrow = () =>
  host.querySelector<HTMLButtonElement>('[aria-label="Send to the right pane"]');
const rows = () => [...host.querySelectorAll<HTMLElement>("[data-path]")];

async function twoPanes() {
  click(viewButton());
  await settle();
  click(menuItem("Two Panes"));
  await settle();
  await settle();
}

/** Select the first row of the left pane, which also focuses it. */
async function selectOnTheLeft() {
  const row = rows()[0];
  row?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  row?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
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

describe("the transfer gutter", () => {
  it("draws an arrow each way", async () => {
    await open();
    await twoPanes();
    expect(rightArrow()).not.toBeNull();
    expect(host.querySelector('[aria-label="Send to the left pane"]')).not.toBeNull();
  });

  it("is refused with a reason while nothing is selected", async () => {
    await open();
    await twoPanes();
    expect(rightArrow()?.disabled).toBe(true);
    expect(rightArrow()?.title).toBe("Nothing selected");
  });

  it("refuses a read-only destination rather than attempting the write", async () => {
    // The same `writable` the status badge reads. A blocked write must never
    // be attempted and refused; it must not be attempted.
    await open({ mountInfo: mount({ writable: false, reason: "Read-only — ro,root_squash" }) });
    await twoPanes();
    await selectOnTheLeft();
    expect(rightArrow()?.disabled).toBe(true);
    expect(rightArrow()?.title).toBe("Read-only — ro,root_squash");
    click(rightArrow());
    await settle();
    expect(uploads).toHaveLength(0);
    expect(downloads).toHaveLength(0);
  });

  it("refuses two remote shares, and says that is what it is", async () => {
    // `enqueueUploads` takes local paths and `enqueueDownloads` a local
    // destination, so one side has to be local. Relaying between two remotes
    // is E22-S3 and blocked on xsync having no server-side copy.
    await open({ saved: [bookmark({ mounted: true })] });
    await twoPanes();
    await selectOnTheLeft();
    expect(rightArrow()?.disabled).toBe(true);
    expect(rightArrow()?.title).toContain("two remote shares");
  });

  it("uploads when the sending pane is a local folder", async () => {
    // The bookmark has to be *opened*, not merely stored as mounted: a stored
    // `mounted` flag is from a previous run and describes a share the shell is
    // not holding (E1-S6), so the window clears it on load.
    await open({
      saved: [
        bookmark({
          id: "local::/users/dorian/downloads",
          name: "Downloads",
          protocol: "local",
          host: "",
          share: "/Users/dorian/Downloads",
        }),
      ],
    });
    const row = [...host.querySelectorAll<HTMLElement>("[role='button']")].find((candidate) =>
      candidate.textContent?.includes("Downloads"),
    );
    row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    await settle();

    await twoPanes();
    await selectOnTheLeft();
    expect(rightArrow()?.disabled).toBe(false);
    click(rightArrow());
    await settle();
    await settle();
    // Absolute local paths, because that is what an upload's sources are: the
    // mount's own root with the row's path hung off it.
    expect(uploads).toHaveLength(1);
    expect(uploads[0]?.[0]).toEqual(["/Users/dorian/Downloads/a.txt"]);
  });
});

describe("double-clicking across the panes (XTP-E3-S2)", () => {
  /** A local mount on the left, so a transfer is expressible. */
  async function localTwoPanes() {
    await open({
      saved: [
        bookmark({
          id: "local::/users/dorian/downloads",
          name: "Downloads",
          protocol: "local",
          host: "",
          share: "/Users/dorian/Downloads",
        }),
      ],
    });
    const row = [...host.querySelectorAll<HTMLElement>("[role='button']")].find((candidate) =>
      candidate.textContent?.includes("Downloads"),
    );
    row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    await settle();
    await twoPanes();
  }

  it("sends a file to the other pane", async () => {
    await localTwoPanes();
    const row = rows()[0];
    row?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    row?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    await settle();
    expect(uploads).toHaveLength(1);
    expect(uploads[0]?.[0]).toEqual(["/Users/dorian/Downloads/a.txt"]);
  });

  it("does nothing to a file with one pane, where there is no other side", async () => {
    // Opening a file is Quick Look's and Edit's job; there is nowhere to send
    // it, and a double-click that quietly started a transfer would be a
    // surprise.
    await open();
    const row = rows()[0];
    row?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    await settle();
    expect(uploads).toHaveLength(0);
    expect(downloads).toHaveLength(0);
  });
});
