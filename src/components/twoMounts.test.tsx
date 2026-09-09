// The sidebar with two shares open (XTP-E8-S2).
//
// MOUNTED has listed both since a bookmark learned to open into the focused
// pane; what this adds is which of them is *marked*, and a way to give one
// back. The question the handoff never had to ask — its two-pane window has no
// sidebar — is which pane a row acts on, and the answer everywhere in this
// window is the focused one.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Bookmark, Listing, MountInfo } from "../state/types";
import type { WindowDragEvent } from "../state/backend/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));
let unmounted: string[] = [];
let saved: Bookmark[][] = [];
let downloads: unknown[][] = [];
let uploads: unknown[][] = [];
/** The window's drag subscriber, so a test can push a drag over it. */
let drag: ((event: WindowDragEvent) => void) | null = null;

// A local folder beside a remote share, which is the pair a transfer is
// actually expressible between — `enqueueUploads` takes local paths and
// `enqueueDownloads` a local destination, so one side has to be this machine.
const MEDIA = "local::/users/dorian/media";
const BACKUPS = "xsync:freya.local:/mnt/backups";

const bookmark = (id: string, name: string): Bookmark =>
  id === MEDIA
    ? {
        id,
        name,
        protocol: "local",
        host: "",
        share: "/Users/dorian/Media",
        mountPreference: "rw",
        mounted: false,
        reachable: true,
      }
    : {
        id,
        name,
        protocol: "xsync",
        host: "freya.local",
        share: `/mnt/${name}`,
        mountPreference: "rw",
        mounted: false,
        reachable: true,
      };

const mount = (writable = true): MountInfo => ({
  status: "ready",
  writable,
  reason: writable ? "" : "Read-only — export mapped ro,root_squash",
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

async function open(options: { writable?: boolean } = {}) {
  unmounted = [];
  saved = [];
  downloads = [];
  uploads = [];
  drag = null;
  const base = fixtureBackend();
  const backend = {
    ...base,
    async listDir(_path: string, id?: string): Promise<Listing> {
      // Only the second share is ever read-only, so a refusal is about the
      // pane that is not focused — which is the case this story is for.
      return { entries: [], mount: mount(id === BACKUPS ? (options.writable ?? true) : true) };
    },
    onWindowDrag(handler: (event: WindowDragEvent) => void) {
      drag = handler;
      return () => {
        drag = null;
      };
    },
    async checkConflicts() {
      return [];
    },
    async enqueueDownloads(...args: unknown[]) {
      downloads.push(args);
      return ["d1"];
    },
    async enqueueUploads(...args: unknown[]) {
      uploads.push(args);
      return ["u1"];
    },
    async mount() {},
    async unmount(id: string) {
      unmounted.push(id);
    },
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{
          load: async () => [bookmark(MEDIA, "media"), bookmark(BACKUPS, "backups")],
          save: async (next: Bookmark[]) => {
            saved.push(next);
          },
        }}
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
const sidebarRow = (name: string) =>
  [...host.querySelectorAll<HTMLElement>("[role='button']")].find((candidate) =>
    candidate.textContent?.includes(name),
  ) ?? null;
/** The rows under MOUNTED, which carry the drop target attribute. */
const mountedRows = () =>
  [...host.querySelectorAll<HTMLElement>("[data-bookmark]")].map(
    (row) => row.getAttribute("data-bookmark") ?? "",
  );
const activeRow = () =>
  [...host.querySelectorAll<HTMLElement>("[data-bookmark]")].find((row) =>
    row.className.includes("sidebarRowActive"),
  ) ?? null;

async function focusPane(index: number) {
  lists()[index]?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  await settle();
}

async function openInto(index: number, name: string) {
  await focusPane(index);
  sidebarRow(name)?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  await settle();
}

/** Two panes, one share each. */
async function twoShares() {
  click(viewButton());
  await settle();
  click(menuItem("Two Panes"));
  await settle();
  await settle();
  await openInto(0, "media");
  await openInto(1, "backups");
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

describe("MOUNTED with two mounts", () => {
  it("lists both, because the window holds both", async () => {
    await open();
    await twoShares();
    expect(mountedRows().sort()).toEqual([BACKUPS, MEDIA].sort());
  });

  it("marks the focused pane's share, and moves the mark with focus", async () => {
    // It was "the one mounted bookmark", which with two is whichever comes
    // first in the list — so the mark named a share the window's verbs would
    // not act on.
    await open();
    await twoShares();
    expect(activeRow()?.getAttribute("data-bookmark")).toBe(BACKUPS);
    await focusPane(0);
    expect(activeRow()?.getAttribute("data-bookmark")).toBe(MEDIA);
  });
});

describe("unmounting one of two shares", () => {
  const rightClick = (element: Element | null | undefined) =>
    element?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));

  it("is offered only on a share this window is holding", async () => {
    await open();
    await twoShares();
    rightClick(sidebarRow("media"));
    await settle();
    expect(menuItem("Unmount")).not.toBeNull();
  });

  it("disconnects that share and leaves the other pane alone", async () => {
    await open();
    await twoShares();
    await focusPane(0);
    rightClick(sidebarRow("media"));
    await settle();
    click(menuItem("Unmount"));
    await settle();
    await settle();

    expect(unmounted).toEqual([MEDIA]);
    // The pane that was on it has no share now — the first-run screen, with
    // the way out of it — and the other pane is untouched.
    expect(mountedRows()).toEqual([BACKUPS]);
    expect(activeRow()?.getAttribute("data-bookmark")).toBeUndefined();
  });

  it("keeps the bookmark, which is a row and not a session", async () => {
    await open();
    await twoShares();
    await focusPane(0);
    rightClick(sidebarRow("media"));
    await settle();
    click(menuItem("Unmount"));
    await settle();
    await settle();
    const last = saved[saved.length - 1] ?? [];
    expect(last.map((entry) => entry.id).sort()).toEqual([BACKUPS, MEDIA].sort());
    expect(last.find((entry) => entry.id === MEDIA)?.mounted).toBe(false);
    expect(last.find((entry) => entry.id === BACKUPS)?.mounted).toBe(true);
  });
});

describe("dropping onto a sidebar row (XTP-E8-S3)", () => {
  /**
   * A drag from the other pane, over the sidebar row for `id`.
   *
   * `elementFromPoint` is the seam the window reads a drop's target through,
   * and jsdom has no layout to answer it with — so the row is handed over
   * directly. What is being tested is the decision, not the hit test.
   */
  async function dragOver(id: string, phase: "over" | "drop", from: string) {
    const row = [...host.querySelectorAll<HTMLElement>("[data-bookmark]")].find(
      (candidate) => candidate.getAttribute("data-bookmark") === id,
    );
    expect(row, `a sidebar row for ${id}`).not.toBeNull();
    const original = document.elementFromPoint;
    document.elementFromPoint = () => row ?? null;
    drag?.({
      phase,
      position: { x: 10, y: 10 },
      modifiers: { alt: false, shift: false, command: false, control: false },
      selection: { mount: from, paths: ["Films/a.mkv"], window: "other" },
    });
    await settle();
    await settle();
    document.elementFromPoint = original;
  }

  const caption = () =>
    host.querySelector<HTMLElement>("[class*='dropCaption']")?.textContent ?? "";
  const refusing = () => host.querySelector("[class*='dropCaptionRefused']") !== null;

  it("copies onto a share the other pane is holding", async () => {
    // The drop that E24-S2 built for another *window* now has an obvious
    // source: the selection in the pane beside it.
    await open();
    await twoShares();
    // Dragging *from* the pane you are in, which is the gesture — and which
    // makes the target the share the other pane holds.
    await focusPane(0);
    await dragOver(BACKUPS, "over", MEDIA);
    expect(refusing()).toBe(false);
    expect(caption()).toContain("backups");

    await dragOver(BACKUPS, "drop", MEDIA);
    expect(downloads.length + uploads.length).toBe(1);
  });

  it("refuses a read-only share, with the reason", async () => {
    // The same `mount.writable` everything else reads. It consulted the
    // *focused* pane's mount alone, so a drop onto the other pane's read-only
    // share was accepted here and failed later.
    await open({ writable: false });
    await twoShares();
    await focusPane(0);
    await dragOver(BACKUPS, "over", MEDIA);
    expect(refusing()).toBe(true);
    expect(caption()).toContain("read-only");

    await dragOver(BACKUPS, "drop", MEDIA);
    expect(downloads.length + uploads.length).toBe(0);
  });
});
