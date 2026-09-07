// A folder shows what landed in it (E13, E24-S1).
//
// A transfer is the one way a share's contents change that the app itself does
// not perform: `listing://changed` is emitted for a rename, a delete, a new
// folder — this app mutating the share — and the queue is not one of those. So
// a file dragged in from Finder copied successfully, reached `done`, and simply
// never appeared, until the window was navigated away from and back (reported
// 2026-09-06).
//
// The window it has to reach is the one *showing that share*, which is why the
// mount is checked: queue progress is a deliberate broadcast — every window
// hears about every transfer, because the queue belongs to the app rather than
// to the window that started it — and relisting on all of them would be each
// window refreshing for shares it is not showing.

import { describe, expect, it, afterEach, beforeEach, vi } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Bookmark, Transfer, TransferStatus } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const MINE = "smb:nas.local:media";
const THEIRS = "sftp:orion.local:backups";

const saved: Bookmark[] = [
  {
    id: MINE,
    name: "media",
    protocol: "smb",
    host: "nas.local",
    share: "media",
    username: "",
    mountPreference: "rw",
    reachable: true,
    mounted: false,
  },
  {
    id: THEIRS,
    name: "backups",
    protocol: "sftp",
    host: "orion.local",
    share: "backups",
    username: "",
    mountPreference: "rw",
    reachable: true,
    mounted: false,
  },
];

/** One queue row, in whatever state the case is about. */
const row = (status: TransferStatus, mount: string | null): Transfer => ({
  id: "t1",
  direction: "up",
  name: "holiday.mov",
  sourcePath: "/Users/me/holiday.mov",
  destPath: "holiday.mov",
  bytesDone: status === "done" ? 4096 : 1024,
  bytesTotal: 4096,
  rate: null,
  etaSeconds: null,
  status,
  error: null,
  queuedAtMs: 1,
  mount,
});

/** A backend that records every listing and can push a queue event. */
function watchable(): Backend & { paths: string[]; moved: (transfer: Transfer) => void } {
  const inner = fixtureBackend();
  const paths: string[] = [];
  let announce: ((transfer: Transfer) => void) | null = null;
  return {
    ...inner,
    async listDir(path: string) {
      paths.push(path);
      return inner.listDir(path);
    },
    onTransferChanged(handler: (transfer: Transfer) => void) {
      announce = handler;
      return () => {
        announce = null;
      };
    },
    paths,
    moved: (transfer: Transfer) => announce?.(transfer),
  };
}

function open(backend: Backend, mount: string | null) {
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => saved, save: async () => undefined }}
        window={{ label: "browser-2", mount, path: "", view: "browse" }}
      />
    ),
    host,
  );
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
  vi.useRealTimers();
});

describe("a transfer that lands", () => {
  it("relists the folder it landed in", async () => {
    vi.useFakeTimers();
    const backend = watchable();
    open(backend, MINE);
    await vi.advanceTimersByTimeAsync(0);
    const before = backend.paths.length;

    backend.moved(row("done", MINE));
    await vi.advanceTimersByTimeAsync(0);

    // At once: the coalescer leads, so the first change is shown immediately
    // and only a burst behind it waits out the window.
    expect(backend.paths.length).toBe(before + 1);
  });

  it("leaves a window on another share alone", async () => {
    vi.useFakeTimers();
    const backend = watchable();
    open(backend, MINE);
    await vi.advanceTimersByTimeAsync(0);
    const before = backend.paths.length;

    backend.moved(row("done", THEIRS));
    // Past the coalescing window, so this is "never" rather than "not yet".
    await vi.advanceTimersByTimeAsync(500);

    expect(backend.paths.length).toBe(before);
  });

  it("does not relist on every progress tick", async () => {
    vi.useFakeTimers();
    const backend = watchable();
    open(backend, MINE);
    await vi.advanceTimersByTimeAsync(0);
    const before = backend.paths.length;

    // A running transfer emits one of these per chunk. Relisting on each would
    // put a listing round trip behind every progress bar update — on the share
    // that is already busy moving the bytes.
    backend.moved(row("running", MINE));
    backend.moved(row("running", MINE));
    await vi.advanceTimersByTimeAsync(500);

    expect(backend.paths.length).toBe(before);
  });
});

describe("the Refresh button", () => {
  const button = () => host.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');

  it("relists the folder when pressed", async () => {
    const backend = watchable();
    open(backend, MINE);
    await new Promise((done) => setTimeout(done, 0));
    const before = backend.paths.length;

    expect(button(), "⌘R has existed since E16-S1 with nothing on screen to say so").not.toBeNull();
    button()?.click();
    await new Promise((done) => setTimeout(done, 0));

    expect(backend.paths.length).toBe(before + 1);
  });

  it("is disabled with nothing mounted", async () => {
    // Not a permission — relisting is never refused — but there is no folder
    // to relist, and a live button that does nothing is the worse of the two.
    const backend = fixtureBackend({ disconnected: true });
    open(backend, null);
    await new Promise((done) => setTimeout(done, 0));

    expect(button()?.disabled).toBe(true);
  });
});
