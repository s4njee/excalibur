// Where the Downloads tab lives (XDL-E2-S1).
//
// A window can be on downloads the way it is on a mount: the sidebar's
// CATEGORIES section carries live counts off the queue, opening one switches
// the window onto the queue, and the toolbar's share verbs are absent there
// rather than greyed out. Mounting a share switches back.

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
        window={options.window ?? { label: "main", mount: null, path: "", view: "browse" }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const categoryRow = (id: string) => host.querySelector<HTMLElement>(`[data-category="${id}"]`);
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
const bookmarkRow = (name: string) =>
  [...host.querySelectorAll<HTMLElement>("[role='button']")].find((row) =>
    row.textContent?.includes(name),
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

describe("the sidebar's CATEGORIES section", () => {
  it("counts every group live off the queue", async () => {
    await open();
    expect(categoryRow("all")?.textContent).toContain("6");
    expect(categoryRow("downloading")?.textContent).toContain("2");
    expect(categoryRow("paused")?.textContent).toContain("1");
    expect(categoryRow("completed")?.textContent).toContain("1");
    expect(categoryRow("failed")?.textContent).toContain("1");
  });

  it("keeps FAVORITES, MOUNTED and BOOKMARKS where they were", async () => {
    await open();
    expect(host.textContent).toContain("Favorites");
    expect(host.textContent).toContain("Mounted");
    expect(host.textContent).toContain("Bookmarks");
    expect(host.textContent).toContain("Categories");
  });
});

describe("a window on downloads", () => {
  it("drops the share verbs instead of disabling them, and shows the queue", async () => {
    await open();
    // Sanity: the verbs are there while browsing.
    expect(verb("Quick Look")).not.toBeNull();
    click(categoryRow("downloading"));
    await settle();

    expect(verb("Quick Look")).toBeNull();
    expect(verb("Edit")).toBeNull();
    expect(verb("Download")).toBeNull();
    expect(verb("Upload")).toBeNull();
    // The queue, not a sheet over the file list.
    expect(host.querySelector('[aria-label="Downloads"]')).not.toBeNull();
    expect(host.querySelector('[aria-label="Transfers"]')).toBeNull();
  });

  it("shows only the chosen category's rows", async () => {
    await open();
    click(categoryRow("paused"));
    await settle();
    const pane = host.querySelector('[aria-label="Downloads"]');
    expect(pane?.textContent).toContain("paused-file");
    expect(pane?.textContent).not.toContain("done-file");
    expect(pane?.textContent).not.toContain("running-file");
  });

  it("titles the window after the view, not the mount behind it", async () => {
    await open();
    click(categoryRow("all"));
    await settle();
    // TitleBar names Downloads; the native title follows in the app.
    const titlebar = host.firstElementChild?.children[0];
    expect(titlebar?.textContent).toContain("Downloads");
  });

  it("switches back to the file list when a share is mounted", async () => {
    // The fixture refuses mounts the way the preview does, so this window
    // gets a backend that connects: the point is what mounting does to the
    // view, not the connection itself.
    const backend = { ...fixtureBackend({ queue }), async mount() {} };
    dispose = render(
      () => (
        <App
          backend={backend}
          prefs={memoryPrefs()}
          bookmarks={store()}
          window={{ label: "main", mount: null, path: "", view: "browse" }}
        />
      ),
      host,
    );
    await settle();
    await settle();
    click(categoryRow("all"));
    await settle();
    expect(verb("Quick Look")).toBeNull();

    const bookmark = bookmarkRow("media");
    expect(bookmark).not.toBeNull();
    bookmark?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    await settle();

    expect(verb("Quick Look")).not.toBeNull();
    expect(host.querySelector('[aria-label="Downloads"]')).toBeNull();
  });
});

describe("adding a download (XDL-E3-S1)", () => {
  it("offers Add URL only where the queue is, and opens the dialog from it", async () => {
    await open();
    // The share verbs are gone on downloads, and the reference's primary
    // control takes the toolbar's primary slot there instead.
    expect(verb("＋ Add URL")).toBeNull();
    click(categoryRow("all"));
    await settle();
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
          window={{ label: "main", mount: null, path: "", view: "downloads" }}
        />
      ),
      host,
    );
    await settle();
    await settle();
    click(categoryRow("all"));
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
          window={{ label: "main", mount: null, path: "", view: "downloads" }}
        />
      ),
      host,
    );
    await settle();
    await settle();
    click(categoryRow("all"));
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
    dispose = render(() => <App backend={backend} prefs={memoryPrefs()} bookmarks={store()} window={{ label: "main", mount: null, path: "", view: "downloads" }} />, host);
    await settle(); await settle();
    click(categoryRow("all")); await settle();
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
    dispose = render(() => <App backend={backend} prefs={memoryPrefs()} bookmarks={store()} window={{ label: "main", mount: null, path: "", view: "downloads" }} />, host);
    await settle(); await settle(); click(categoryRow("all")); await settle();
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
    await open({ window: { label: "main", mount: null, path: "", view: "downloads" } });
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
          window={{ label: "main", mount: null, path: "", view: "downloads" }}
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

describe("remembering the view", () => {
  it("opens on downloads when the shell says so", async () => {
    // What a restored window will carry in its URL once E23-S2 persists
    // window records: no store read, no mount needed.
    await open({
      prefs: memoryPrefs(),
      window: { label: "main", mount: null, path: "", view: "downloads" },
    });
    expect(host.querySelector('[aria-label="Downloads"]')).not.toBeNull();
    expect(verb("Quick Look")).toBeNull();
  });
  it("reopens on downloads after a restart, per window", async () => {
    const prefs = memoryPrefs();
    await open({ prefs, window: { label: "main", mount: null, path: "", view: "browse" } });
    click(categoryRow("paused"));
    await settle();
    dispose?.();
    dispose = undefined;
    host.remove();
    host = document.createElement("div");
    document.body.append(host);

    // A fresh window with the same label and the same prefs: what a relaunch
    // is, minus the shell.
    await open({ prefs, window: { label: "main", mount: null, path: "", view: "browse" } });
    expect(host.querySelector('[aria-label="Downloads"]')).not.toBeNull();
    expect(verb("Quick Look")).toBeNull();
  });

  it("restores the remembered view only for windows opened onto nothing", async () => {
    const prefs = memoryPrefs();
    await prefs.set("window-view:main", { view: "downloads", category: "paused" });
    // The shell opened this window with no view and no mount: the remembered
    // answer applies, category included.
    await open({ prefs, window: { label: "main", mount: null, path: "", view: "browse" } });
    expect(host.querySelector('[aria-label="Downloads"]')).not.toBeNull();
    const pane = host.querySelector('[aria-label="Downloads"]');
    expect(pane?.textContent).toContain("paused-file");
    expect(pane?.textContent).not.toContain("done-file");
  });

  it("does not mistake a ⌘N window for a restored one", async () => {
    const prefs = memoryPrefs();
    await prefs.set("window-view:browser-2", { view: "downloads", category: "all" });
    // ⌘N carries the mount it opened on. That explicit answer wins over
    // whatever a previous window with a recycled label chose.
    await open({
      prefs,
      queue: [],
      window: { label: "browser-2", mount: "smb:nas.local:media", path: "", view: "browse" },
    });
    expect(host.querySelector('[aria-label="Downloads"]')).toBeNull();
  });
});
