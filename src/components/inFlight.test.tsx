// A file appears where it is going before it gets there (XTP-E3-S3).
//
// The alternative is a folder that stays empty while something is obviously
// happening. The row is a promise, not a file: it carries the bytes that have
// arrived and says so, and it becomes an ordinary row the moment the listing
// knows about it — no relist.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Listing, MountInfo, Transfer } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

const MOUNT: MountInfo = {
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
};

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

const arriving = (over: Partial<Transfer> = {}): Transfer => ({
  id: "t1",
  direction: "up",
  name: "landing.iso",
  sourcePath: "/Users/dorian/landing.iso",
  // An upload's destination is share-relative on its mount.
  destPath: "landing.iso",
  bytesDone: 4096,
  bytesTotal: 8192,
  rate: null,
  etaSeconds: null,
  status: "running",
  error: null,
  queuedAtMs: 1,
  mount: null,
  ...over,
});

async function open(queue: Transfer[], listed = [entry("already.txt")]) {
  const base = fixtureBackend();
  const backend = {
    ...base,
    async listDir(): Promise<Listing> {
      return { entries: listed, mount: MOUNT };
    },
    async transfers() {
      return queue;
    },
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => [], save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const rowFor = (path: string) => host.querySelector<HTMLElement>(`[data-path="${path}"]`);

// jsdom lays nothing out, so the list measures zero width and drops the
// Modified and Kind columns — which is correct, and would make the assertion
// about the Modified cell vacuous.
beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get: () => 1200,
  });
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  Reflect.deleteProperty(HTMLElement.prototype, "clientWidth");
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("a file on its way", () => {
  it("is drawn in the folder it is going to", async () => {
    await open([arriving()]);
    expect(rowFor("landing.iso")).not.toBeNull();
  });

  it("says it is transferring rather than showing a date it has not got", async () => {
    await open([arriving()]);
    expect(rowFor("landing.iso")?.textContent).toContain("Transferring…");
  });

  it("is drawn as arriving, not as an ordinary file", async () => {
    await open([arriving()]);
    expect(rowFor("landing.iso")?.className).toMatch(/rowInFlight/);
    expect(rowFor("already.txt")?.className).not.toMatch(/rowInFlight/);
  });

  it("stops being a promise once the listing knows about it", async () => {
    // The row the server has published wins: no relist is needed for the
    // change, because the real entry simply takes its place.
    await open([arriving()], [entry("already.txt"), entry("landing.iso")]);
    expect(host.querySelectorAll('[data-path="landing.iso"]')).toHaveLength(1);
    expect(rowFor("landing.iso")?.className).not.toMatch(/rowInFlight/);
  });

  it("stays out of folders it is not going to", async () => {
    // The same queue is app-wide: a transfer into another share, or another
    // folder, must not appear here.
    await open([arriving({ destPath: "elsewhere/landing.iso" })]);
    expect(rowFor("landing.iso")).toBeNull();
  });

  it("is gone once it has finished", async () => {
    await open([arriving({ status: "done" })]);
    expect(rowFor("landing.iso")).toBeNull();
  });
});
