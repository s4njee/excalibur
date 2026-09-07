// Where the Downloads tab lives (XDL-E2-S1).
//
// A window can be on downloads the way it is on a mount: the sidebar's
// CATEGORIES section carries live counts off the queue, opening one switches
// the window onto the queue, and the toolbar's share verbs are absent there
// rather than greyed out. Mounting a share switches back.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";import { fixtureBackend } from "../state/backend";
import { memoryPrefs, type Prefs } from "../state/prefs";
import type { Bookmark } from "../state/types";
import type { Transfer, TransferStatus } from "../state/types";
import type { WindowParams } from "../state/window";

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
