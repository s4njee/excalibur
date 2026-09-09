// Where the queue lives (XDL-E2-S1).
//
// The window always browses; the queue is a pane over it, opened from the
// toolbar's Transfers button or the View menu's item. CATEGORIES is that
// pane's tab strip, carrying live counts off the queue, and the file list's
// verbs stay live behind it — the pane is a sheet, not somewhere the window
// goes. Superseded the browse/downloads view on 2026-09-09.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend } from "../state/backend";
import { memoryPrefs, type Prefs } from "../state/prefs";
import type { Bookmark } from "../state/types";
import type { Transfer, TransferStatus } from "../state/types";
import type { WindowParams } from "../state/window";

const DEFAULT_RETRY = {
  maxAttempts: 5,
  initialWaitSeconds: 1,
  maxWaitSeconds: 30,
};

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = () => new Promise((done) => setTimeout(done, 0));

function row(id: string, status: TransferStatus): Transfer {
  return {
    id,
    direction: "down",
    from: { kind: "share", mount: "smb:nas.local:media", path: `remote/${id}` },
    to: { kind: "local", path: `/local/${id}` },
    name: id,
    sourcePath: `remote/${id}`,
    destPath: `/local/${id}`,
    bytesDone: 0,
    bytesTotal: 10,
    rate: null,
    etaSeconds: null,
    status,
    error: null,
    queuedAtMs: 1,
    mount: "smb:nas.local:media",
  };
}

const queue: Transfer[] = [
  row("queued-file", "queued"),
  row("running-file", "running"),
  row("paused-file", "paused"),
  row("done-file", "done"),
  row("failed-file", "failed"),
  row("cancelled-file", "cancelled"),
];

const saved: Bookmark[] = [
  {
    id: "smb:nas.local:media",
    name: "media",
    protocol: "smb",
    host: "nas.local",
    share: "media",
    username: "",
    mountPreference: "rw",
    reachable: true,
    mounted: false,
  },
];

const store = () => ({ load: async () => saved, save: async () => undefined });

async function open(options: {
  prefs?: Prefs;
  window?: WindowParams;
  queue?: Transfer[];
} = {}) {
  dispose = render(
    () => (
      <App
        backend={fixtureBackend({ queue: options.queue ?? queue })}
        prefs={options.prefs ?? memoryPrefs()}
        bookmarks={store()}
        window={options.window ?? { label: "main", mount: null, path: "" }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const categoryRow = (id: string) => host.querySelector<HTMLElement>(`[data-category="${id}"]`);
// Scoped to the dialog: the toolbar button that opens it carries the same
// label, and it comes first in the DOM.
const pane = () => host.querySelector('[role="dialog"][aria-label="Transfers"]');
/**
 * Make sure the transfers pane is open.
 *
 * It opens itself when the queue is not empty, which most of these fixtures
 * are; going through the button anyway means no test silently depends on that
 * seed, and the ones with an empty queue take the same path.
 */
async function openPane() {
  if (pane() === null) {
    click(verb("Transfers"));
    await settle();
  }
}
const click = (element: HTMLElement | null) => {
  expect(element).not.toBeNull();
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
};
// The toolbar is the window's second row, after the title bar (multiWindow
// pins the same indexing). Scoped because the sidebar footer has its own
// Edit button, which must not read as a share verb.
const toolbar = () => host.firstElementChild?.children[1] ?? null;
const verb = (label: string) =>
  [...(toolbar()?.querySelectorAll("button") ?? [])].find(
    (button) => button.textContent === label,
  ) ?? null;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("the transfers pane's category tabs", () => {
  it("counts every group live off the queue", async () => {
    await open();
    await openPane();
    expect(categoryRow("all")?.textContent).toContain("6");
    expect(categoryRow("downloading")?.textContent).toContain("2");
    expect(categoryRow("paused")?.textContent).toContain("1");
    expect(categoryRow("completed")?.textContent).toContain("1");
    expect(categoryRow("failed")?.textContent).toContain("1");
  });

  it("leaves the sidebar its own sections and takes CATEGORIES out of it", async () => {
    await open();
    await openPane();
    // The sidebar's scroll region, which is what holds the sections.
    const sections = host.querySelector("[data-favorites]")?.parentElement;
    expect(sections?.textContent).toContain("Favorites");
    expect(sections?.textContent).toContain("Mounted");
    expect(sections?.textContent).toContain("Bookmarks");
    // The rows moved into the pane, so the section header must not still be
    // in the sidebar drawing a second, stale copy of the same choice.
    expect(sections?.textContent).not.toContain("Categories");
    expect(categoryRow("all")?.closest("[data-favorites]")).toBeNull();
  });

  it("shows only the chosen category's rows", async () => {
    await open();
    await openPane();
    click(categoryRow("paused"));
    await settle();
    expect(pane()?.textContent).toContain("paused-file");
    expect(pane()?.textContent).not.toContain("done-file");
    expect(pane()?.textContent).not.toContain("running-file");
  });

  it("opens and closes from the toolbar without disturbing the file list", async () => {
    await open({ queue: [] });
    // An empty queue does not open itself, so this is the button's own doing.
    expect(pane()).toBeNull();
    click(verb("Transfers"));
    await settle();
    expect(pane()).not.toBeNull();
    click(verb("Transfers"));
    await settle();
    expect(pane()).toBeNull();
  });

  it("keeps the share verbs while the pane is open", async () => {
    // The pane is a sheet over the window, not a place the window goes: the
    // file list is still behind it and its verbs still apply. The old
    // downloads view answered this the other way, and dropped them.
    await open();
    await openPane();
    expect(pane()).not.toBeNull();
    expect(verb("Quick Look")).not.toBeNull();
    expect(verb("Edit")).not.toBeNull();
    expect(verb("Download")).not.toBeNull();
    expect(verb("Upload")).not.toBeNull();
  });

  it("titles the window after the mount, never after the queue", async () => {
    await open();
    await openPane();
    const titlebar = host.firstElementChild?.children[0];
    expect(titlebar?.textContent).not.toContain("Downloads");
  });
});

describe("adding a download (XDL-E3-S1)", () => {
  it("offers Add URL in the toolbar, without the pane being open", async () => {
    await open({ queue: [] });
    // The HTTP downloader needs a route that does not start with "open the
    // transfers pane and notice a button in its header", which is where this
    // briefly lived. The reference draws it as a toolbar control too.
    expect(pane()).toBeNull();
    const add = verb("＋ Add URL");
    expect(add).not.toBeNull();
    add?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(host.querySelector('[aria-label="Add Download"]')).not.toBeNull();
  });

  it("seeds the destination from the preference, and starts what the dialog sends", async () => {
    const started: { url: string; folder: string }[] = [];
    const backend = {
      ...fixtureBackend({ queue }),
      // The fixture refuses enqueues the way the preview does; a window that
      // can add a download is the thing under test, so the queue answers.
      async enqueueHttpDownload(url: string, folder: string) {
        started.push({ url, folder });
        return `row-${started.length}`;
      },
    };
    const prefs = memoryPrefs();
    await prefs.set("downloadFolder", "/Users/dorian/Downloads");
    dispose = render(
      () => (
        <App
          backend={backend}
          prefs={prefs}
          bookmarks={store()}
          window={{ label: "main", mount: null, path: "" }}
        />
      ),
      host,
    );
    await settle();
    await settle();
    verb("＋ Add URL")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await settle();
    const box = host.querySelector<HTMLTextAreaElement>("#download-urls")!;
    box.value = "https://cdn.test/arrival.mkv";
    box.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const saveTo = host.querySelector<HTMLInputElement>("#download-dest")!;
    expect(saveTo.value).toBe("/Users/dorian/Downloads");
    const start = [...host.querySelectorAll("button")].find((b) =>
      b.textContent?.includes("Start download"),
    ) as HTMLButtonElement;
    start.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await settle();
    expect(started).toEqual([
      { url: "https://cdn.test/arrival.mkv", folder: "/Users/dorian/Downloads" },
    ]);
    expect(host.querySelector('[aria-label="Add Download"]')).toBeNull();
  });

  it("remembers the destination per window for the next dialog", async () => {
    const prefs = memoryPrefs();
    const backend = {
      ...fixtureBackend({ queue }),
      async enqueueHttpDownload() {
        return "remembered";
      },
    };
    dispose = render(
      () => (
        <App
          backend={backend}
          prefs={prefs}
          bookmarks={store()}
          window={{ label: "main", mount: null, path: "" }}
        />
      ),
      host,
    );
    await settle();
    await settle();
    verb("＋ Add URL")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await settle();
    const saveTo = host.querySelector<HTMLInputElement>("#download-dest")!;
    saveTo.value = "/Volumes/Media";
    saveTo.dispatchEvent(new Event("input", { bubbles: true }));
    const box = host.querySelector<HTMLTextAreaElement>("#download-urls")!;
    box.value = "https://cdn.test/file.iso";
    box.dispatchEvent(new Event("input", { bubbles: true }));
    const start = [...host.querySelectorAll("button")].find((button) =>
      button.textContent?.includes("Start download"),
    );
    start?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await settle();
    // Reopen after using it: the last destination for this window wins over
    // both the configured and OS folders.
    verb("＋ Add URL")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await settle();
    expect(host.querySelector<HTMLInputElement>("#download-dest")?.value).toBe("/Volumes/Media");
  });

  it("passes per URL connection overrides to the backend", async () => {
    const calls: { url: string; options?: { connections?: number | null; retry?: typeof DEFAULT_RETRY } }[] = [];
    const backend = {
      ...fixtureBackend({ queue }),
      async downloadLimits() { return { maxConnections: 8, maxPerHost: 4 }; },
      async probeHttpDownload() { return { length: 10, ranges: true, filename: null, finalUrl: null, warning: null }; },
      async enqueueHttpDownload(url: string, _folder: string, options?: { connections?: number | null; retry?: typeof DEFAULT_RETRY }) {
        calls.push({ url, options });
        return `row-${calls.length}`;
      },
    };
    dispose = render(() => <App backend={backend} prefs={memoryPrefs()} bookmarks={store()} window={{ label: "main", mount: null, path: "" }} />, host);
    await settle(); await settle();
    verb("＋ Add URL")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    const box = host.querySelector<HTMLTextAreaElement>("#download-urls")!;
    box.value = "https://a.test/one.iso\nhttps://b.test/two.iso";
    box.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise((done) => setTimeout(done, 300));
    const choice = [...host.querySelectorAll<HTMLInputElement>("input[name='download-connections']")].find((input) => input.nextElementSibling?.textContent?.trim() === "3");
    choice?.click();
    const start = [...host.querySelectorAll("button")].find((b) => b.textContent?.includes("Start download"));
    start?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle(); await settle();
    expect(calls).toEqual([
      { url: "https://a.test/one.iso", options: { connections: 3, retry: DEFAULT_RETRY } },
      { url: "https://b.test/two.iso", options: { connections: 3, retry: DEFAULT_RETRY } },
    ]);
  });

  it("queues mirror mode as one job with ordered failover URLs", async () => {
    const calls: { url: string; options?: { mirrors?: string[]; preferredMirror?: number | null; retry?: typeof DEFAULT_RETRY } }[] = [];
    const backend = {
      ...fixtureBackend({ queue }),
      async downloadLimits() { return { maxConnections: 4, maxPerHost: 4 }; },
      async probeHttpDownload() { return { length: 10, ranges: true, filename: null, finalUrl: null, warning: null }; },
      async enqueueHttpDownload(url: string, _folder: string, options?: { mirrors?: string[]; preferredMirror?: number | null; retry?: typeof DEFAULT_RETRY }) {
        calls.push({ url, options });
        return "row-1";
      },
    };
    dispose = render(() => <App backend={backend} prefs={memoryPrefs()} bookmarks={store()} window={{ label: "main", mount: null, path: "" }} />, host);
    await settle(); await settle(); await openPane();
    verb("＋ Add URL")?.dispatchEvent(new MouseEvent("click", { bubbles: true })); await settle();
    const box = host.querySelector<HTMLTextAreaElement>("#download-urls")!;
    box.value = "https://a.test/one.iso\nhttps://b.test/one.iso\nhttps://c.test/one.iso";
    box.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    const mirror = [...host.querySelectorAll<HTMLInputElement>("input[name='download-mode']")].find((input) => input.nextElementSibling?.textContent?.includes("Mirrors"));
    mirror?.click();
    const start = [...host.querySelectorAll("button")].find((b) => b.textContent?.includes("Start download"));
    start?.dispatchEvent(new MouseEvent("click", { bubbles: true })); await settle(); await settle();
    expect(calls).toEqual([{ url: "https://a.test/one.iso", options: { mirrors: ["https://b.test/one.iso", "https://c.test/one.iso"], preferredMirror: 0, retry: DEFAULT_RETRY } }]);
  });
});

describe("dropping URLs (XDL-E8-S1)", () => {
  it("opens a prefilled dialog for a browser URL pasteboard", async () => {
    await open({ window: { label: "main", mount: null, path: "" } });
    const dropped = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(dropped, "dataTransfer", {
      value: {
        getData: (type: string) =>
          type === "text/uri-list"
            ? "# browser drag\nhttps://a.test/one.iso\nhttps://b.test/two.iso"
            : "",
      },
    });
    host.firstElementChild?.dispatchEvent(dropped);
    await settle();
    await settle();
    expect(host.querySelector<HTMLTextAreaElement>("#download-urls")?.value).toBe(
      "https://a.test/one.iso\nhttps://b.test/two.iso",
    );
  });

  it("reads a native .txt drop and opens the same dialog", async () => {
    let drop:
      | ((event: {
          phase: "over" | "drop" | "leave";
          paths: string[];
          position: { x: number; y: number } | null;
        }) => void)
      | undefined;
    const backend = {
      ...fixtureBackend({ queue }),
      onFileDrop(handler: NonNullable<typeof drop>) {
        drop = handler;
        return () => undefined;
      },
      async downloadUrlsFromFiles(paths: string[]) {
        expect(paths).toEqual(["/tmp/downloads.txt"]);
        return ["https://cdn.test/arrival.mkv"];
      },
    };
    dispose = render(
      () => (
        <App
          backend={backend}
          prefs={memoryPrefs()}
          bookmarks={store()}
          window={{ label: "main", mount: null, path: "" }}
        />
      ),
      host,
    );
    await settle();
    await settle();
    drop?.({ phase: "drop", paths: ["/tmp/downloads.txt"], position: null });
    await settle();
    await settle();
    expect(host.querySelector<HTMLTextAreaElement>("#download-urls")?.value).toBe(
      "https://cdn.test/arrival.mkv",
    );
  });
});

describe("remembering the chosen tab", () => {
  it("reopens on the same tab after a restart, per window", async () => {
    const prefs = memoryPrefs();
    await open({ prefs });
    await openPane();
    click(categoryRow("paused"));
    await settle();
    dispose?.();
    dispose = undefined;
    host.remove();
    host = document.createElement("div");
    document.body.append(host);

    // A fresh window with the same label and the same prefs: what a relaunch
    // is, minus the shell.
    await open({ prefs });
    await openPane();
    expect(pane()?.textContent).toContain("paused-file");
    expect(pane()?.textContent).not.toContain("done-file");
  });

  it("keeps the tab per window label", async () => {
    const prefs = memoryPrefs();
    await prefs.set("transfers-category:browser-2", "failed");
    // This window is `main`, so the other window's tab is not its own.
    await open({ prefs });
    await openPane();
    expect(pane()?.textContent).toContain("done-file");
  });

  it("ignores a stale record from when the queue was a view", async () => {
    // `window-view:` is what the removed downloads view wrote, and it carried
    // a view beside the category. Reading it here is exactly how a deleted
    // view comes back: the pane would be suppressed on launch by a record
    // written months ago. A different key means it can only be ignored.
    const prefs = memoryPrefs();
    await prefs.set("window-view:main", { view: "downloads", category: "paused" });
    await open({ prefs });
    // The queue is not empty, so the pane opened itself and stayed open.
    expect(pane()).not.toBeNull();
    // And on All, not on the stale record's `paused`.
    expect(pane()?.textContent).toContain("done-file");
  });
});

describe("remembering the pane's size", () => {
  const grip = () => pane()?.querySelector<HTMLElement>('[aria-label="Resize pane width"]') ?? null;

  it("opens at the size it was left at", async () => {
    const prefs = memoryPrefs();
    await prefs.set("transfersPane", { width: 880, height: 420 });
    await open({ prefs });
    await openPane();
    expect((pane() as HTMLElement).style.width).toBe("880px");
    expect((pane() as HTMLElement).style.height).toBe("420px");
  });

  it("draws no explicit height until one has been dragged", async () => {
    // `null` is content-height: a queue of two rows should not open as a
    // half-screen box of empty list.
    const prefs = memoryPrefs();
    await prefs.set("transfersPane", { width: 800, height: null });
    await open({ prefs });
    await openPane();
    expect((pane() as HTMLElement).style.width).toBe("800px");
    expect((pane() as HTMLElement).style.height).toBe("");
  });

  it("writes the size when the drag ends, not on every frame", async () => {
    // A file write per pointermove is the thing this split exists to avoid,
    // so the assertion is as much about the silence during the drag.
    const prefs = memoryPrefs();
    await open({ prefs });
    await openPane();
    const handle = grip();
    expect(handle).not.toBeNull();

    handle?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, clientX: 400 }));
    window.dispatchEvent(new PointerEvent("pointermove", { clientX: 300 }));
    await settle();
    expect(await prefs.get("transfersPane")).toBeUndefined();

    window.dispatchEvent(new PointerEvent("pointerup", {}));
    await settle();
    expect(await prefs.get("transfersPane")).toEqual({
      width: expect.any(Number),
      height: null,
    });
  });

  it("pulls a size from a bigger display back inside this window", async () => {
    const prefs = memoryPrefs();
    await prefs.set("transfersPane", { width: 5000, height: null });
    await open({ prefs });
    await openPane();
    // jsdom's window is 1024 wide; the clamp leaves a margin for the grip.
    expect(Number.parseInt((pane() as HTMLElement).style.width, 10)).toBeLessThanOrEqual(
      window.innerWidth,
    );
  });
});

describe("what the pane does on launch", () => {
  it("stays shut when the queue is only history", async () => {
    // The bug: finished rows are kept until `Clear finished` takes them, so a
    // queue that has ever been used is never empty again. Opening on
    // "non-empty" meant the pane opened on every launch, forever, for anyone
    // who had ever downloaded anything (reported 2026-09-09).
    await open({ queue: [row("done-file", "done"), row("failed-file", "failed")] });
    expect(pane()).toBeNull();
  });

  it("opens when something is still queued, running or paused", async () => {
    // The other half: a transfer that outlived the last session is worth
    // surfacing without being asked.
    await open({ queue: [row("done-file", "done"), row("paused-file", "paused")] });
    expect(pane()).not.toBeNull();
  });

  it("stays shut on an empty queue", async () => {
    await open({ queue: [] });
    expect(pane()).toBeNull();
  });
});
