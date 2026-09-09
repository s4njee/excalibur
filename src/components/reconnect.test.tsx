// `Reconnect` in the two-pane toolbar (XTP-E2-S3).
//
// The button the handoff puts first in its two-pane toolbar, and the
// distinction it forced: `Reconnect` is not `Remount as RW`. They were one
// call until now, which meant the banner's Reconnect quietly handed write
// access to a share somebody had deliberately opened read-only.

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
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

const ID = "xsync:freya.local:/mnt/data";
let reconnected: string[] = [];
let remounted: string[] = [];
let listed = 0;
/** The window's mount-event subscriber, so a test can push one through. */
let announce: ((event: MountEvent) => void) | null = null;

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

const bookmark: Bookmark = {
  id: ID,
  name: "data",
  protocol: "xsync",
  host: "freya.local",
  share: "/mnt/data",
  mountPreference: "ro",
  mounted: false,
  reachable: true,
};

async function open(options: { saved?: Bookmark[]; mountInfo?: MountInfo } = {}) {
  reconnected = [];
  remounted = [];
  listed = 0;
  announce = null;
  const base = fixtureBackend();
  const backend = {
    ...base,
    async listDir(): Promise<Listing> {
      listed += 1;
      return { entries: [], mount: options.mountInfo ?? mount() };
    },
    async mount() {},
    onMountChanged(handler: (event: MountEvent) => void) {
      announce = handler;
      return () => {
        announce = null;
      };
    },
    async reconnect(id: string): Promise<MountInfo> {
      reconnected.push(id);
      return options.mountInfo ?? mount();
    },
    async remountReadWrite(id: string): Promise<MountInfo> {
      remounted.push(id);
      return mount();
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

const click = (element: Element | null | undefined) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
const viewButton = () => host.querySelector<HTMLElement>('[aria-label="View options"]');
const menuItem = (text: string) =>
  [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((candidate) =>
    candidate.textContent?.includes(text),
  ) ?? null;
/** Scoped to the toolbar, which is the window's second row. */
const toolbar = () => host.firstElementChild?.children[1] ?? null;
const button = (label: string) =>
  [...(toolbar()?.querySelectorAll("button") ?? [])].find(
    (candidate) => candidate.textContent === label,
  ) ?? null;
const bannerButton = () =>
  [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (candidate) => candidate.textContent === "Reconnect" && candidate !== button("Reconnect"),
  ) ?? null;

async function twoPanes() {
  click(viewButton());
  await settle();
  click(menuItem("Two Panes"));
  await settle();
  await settle();
}

/** Open the saved share, so the window has a mount id to act on. */
async function openShare() {
  const row = [...host.querySelectorAll<HTMLElement>("[role='button']")].find((candidate) =>
    candidate.textContent?.includes("data"),
  );
  row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
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

describe("the two-pane toolbar's Reconnect", () => {
  it("is drawn only with two panes, where the handoff puts it", async () => {
    // In single pane the banner that appears when a connection drops already
    // offers exactly this, and the bar is full.
    await open({ saved: [bookmark] });
    expect(button("Reconnect")).toBeNull();
    await twoPanes();
    expect(button("Reconnect")).not.toBeNull();
  });

  it("reconnects the share the focused pane is holding", async () => {
    await open({ saved: [bookmark] });
    await openShare();
    await twoPanes();
    click(button("Reconnect"));
    await settle();
    await settle();
    expect(reconnected).toEqual([ID]);
  });

  it("does not ask for read-write, whatever the share was opened as", async () => {
    // The bug this story found: one call served both buttons, so recovering a
    // dropped connection silently upgraded a share opened read-only.
    await open({ saved: [bookmark] });
    await openShare();
    await twoPanes();
    click(button("Reconnect"));
    await settle();
    await settle();
    expect(remounted).toEqual([]);
  });

  it("re-lists afterwards, because the mount facts may have changed", async () => {
    await open({ saved: [bookmark] });
    await openShare();
    await twoPanes();
    const before = listed;
    click(button("Reconnect"));
    await settle();
    await settle();
    expect(listed).toBeGreaterThan(before);
  });

  it("is refused, with the reason, when there is no share", async () => {
    await open();
    await twoPanes();
    expect(button("Reconnect")?.disabled).toBe(true);
    expect(button("Reconnect")?.title).toBe("No share connected");
  });

  it("keeps Remount as RW as the upgrade it is", async () => {
    // The status bar's button is the recovery path for a read-only mount and
    // still asks for the upgrade. Two buttons, two calls, two labels.
    await open({
      saved: [bookmark],
      mountInfo: mount({ writable: false, reason: "Read-only — ro,root_squash" }),
    });
    await openShare();
    const remount = [...host.querySelectorAll<HTMLButtonElement>("button")].find(
      (candidate) => candidate.textContent === "Remount as RW",
    );
    click(remount);
    await settle();
    await settle();
    expect(remounted).toEqual([ID]);
    expect(reconnected).toEqual([]);
  });

  it("puts the banner's Reconnect on the same call", async () => {
    await open({ saved: [bookmark] });
    await openShare();
    announce?.({ id: ID, status: "error", mount: null, reason: "connection lost" });
    await settle();
    click(bannerButton());
    await settle();
    await settle();
    expect(reconnected).toEqual([ID]);
    expect(remounted).toEqual([]);
  });
});

describe("the toolbar's tooltips (XTP-E2-S3)", () => {
  it("names the shortcut on the controls that have one", async () => {
    await open({ saved: [bookmark] });
    const refresh = toolbar()?.querySelector<HTMLButtonElement>('[aria-label="Refresh"]');
    expect(refresh?.title).toContain("Refresh (");
  });

  it("gives a refused verb its reason instead", async () => {
    // The rule the `Verb` comment states: teaching a shortcut for something
    // the user cannot do would be worse than saying nothing.
    await open({ saved: [bookmark] });
    expect(button("Quick Look")?.title).toBe("Nothing selected");
  });

  it("leaves no control without one", async () => {
    // The AC's "gains a tooltip naming it". A control whose only label is a
    // glyph is the one that most needs it.
    await open({ saved: [bookmark] });
    const bare = [...(toolbar()?.querySelectorAll("button") ?? [])].filter(
      // Breadcrumbs are not controls with shortcuts: a crumb's tooltip would
      // repeat the name already on it, and the field it sits in carries the
      // one tooltip that says what clicking there does.
      (candidate) => candidate.title === "" && candidate.closest("[class*='pathField']") === null,
    );
    expect(bare.map((candidate) => candidate.textContent)).toEqual([]);
  });
});
