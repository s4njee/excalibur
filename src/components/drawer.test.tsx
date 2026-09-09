// The transfer queue, docked (XTP-E4-S1).
//
// One queue in two shapes: a sheet over the window with one pane, a drawer
// along the bottom with two. The properties worth holding are that it is the
// *same* surface either way — same rows, same tabs, same handlers — and that
// the drawer never takes the window over on its own.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs, type Prefs } from "../state/prefs";
import type { Transfer, TransferStatus } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));
/** The window's transfer subscriber, so a test can push a row through. */
let announce: ((transfer: Transfer) => void) | null = null;

function row(id: string, status: TransferStatus, over: Partial<Transfer> = {}): Transfer {
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
    ...over,
  };
}

/** A share transfer as the engine actually makes one: no endpoint pair. */
function share(id: string, status: TransferStatus, over: Partial<Transfer> = {}): Transfer {
  return row(id, status, { from: undefined, to: undefined, ...over });
}

async function open(options: { prefs?: Prefs; queue?: Transfer[] } = {}) {
  announce = null;
  const base = fixtureBackend({ queue: options.queue ?? [] });
  const backend = {
    ...base,
    onTransferChanged(handler: (transfer: Transfer) => void) {
      announce = handler;
      return () => {
        announce = null;
      };
    },
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={options.prefs ?? memoryPrefs()}
        bookmarks={{ load: async () => [], save: async () => undefined }}
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
const toolbar = () => host.firstElementChild?.children[1] ?? null;
const verb = (label: string) =>
  [...(toolbar()?.querySelectorAll("button") ?? [])].find(
    (candidate) => candidate.textContent === label,
  ) ?? null;
const sheet = () => host.querySelector('[role="dialog"][aria-label="Transfers"]');
const drawer = () => host.querySelector('[role="region"][aria-label="Transfers"]');
const table = () => host.querySelector('[role="grid"][aria-label="Download list"]');
const tab = (id: string) => host.querySelector<HTMLElement>(`[data-category="${id}"]`);
const collapse = () =>
  host.querySelector<HTMLElement>('[aria-label="Collapse transfers"]') ??
  host.querySelector<HTMLElement>('[aria-label="Expand transfers"]');

async function twoPanes() {
  click(viewButton());
  await settle();
  click(menuItem("Two Panes"));
  await settle();
  await settle();
}

/**
 * Make sure the queue is on screen.
 *
 * It opens itself at launch when the queue still owes work, so clicking
 * unconditionally would *close* it for exactly the fixtures that have rows.
 */
async function openQueue() {
  if (sheet() === null && drawer() === null) {
    click(verb("Transfers"));
    await settle();
    await settle();
  }
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

describe("the transfer queue's two shapes", () => {
  it("floats over a window with one pane, as it always did", async () => {
    await open();
    await openQueue();
    expect(sheet()).not.toBeNull();
    expect(drawer()).toBeNull();
  });

  it("docks along the bottom with two", async () => {
    // A sheet anchored under the toolbar covers one of the two columns the
    // transfer is between, which is the pair the user is watching.
    await open();
    await twoPanes();
    await openQueue();
    expect(drawer()).not.toBeNull();
    expect(sheet()).toBeNull();
  });

  it("is the same queue either way", async () => {
    // Same rows, same tabs: only the chrome differs, which is what stops the
    // two shapes coming to disagree about what is in the queue.
    await open({ queue: [row("a", "running"), row("b", "failed")] });
    await openQueue();
    expect(tab("failed")?.textContent).toContain("1");
    await twoPanes();
    expect(drawer()).not.toBeNull();
    expect(tab("failed")?.textContent).toContain("1");
    expect(table()).not.toBeNull();
  });

  it("changes shape without closing", async () => {
    await open();
    await openQueue();
    await twoPanes();
    expect(drawer()).not.toBeNull();
    await twoPanes();
    expect(sheet()).not.toBeNull();
  });
});

describe("the drawer's collapse", () => {
  it("leaves the tab strip and takes the rows", async () => {
    await open({ queue: [row("a", "running")] });
    await twoPanes();
    await openQueue();
    expect(table()).not.toBeNull();
    click(collapse());
    await settle();
    // The counts stay readable, which is what makes the strip worth keeping.
    expect(drawer()).not.toBeNull();
    expect(tab("all")).not.toBeNull();
    expect(table()).toBeNull();
  });

  it("comes back to the height it was, not to the default", async () => {
    const prefs = memoryPrefs();
    await prefs.set("drawerHeight", 240);
    await open({ prefs });
    await twoPanes();
    await openQueue();
    click(collapse());
    await settle();
    expect(await prefs.get("drawerHeight")).toBe(0);
    click(collapse());
    await settle();
    expect(await prefs.get("drawerHeight")).toBe(240);
  });

  it("opens collapsed when that is how it was left", async () => {
    const prefs = memoryPrefs();
    await prefs.set("drawerHeight", 0);
    await open({ prefs, queue: [row("a", "running")] });
    await twoPanes();
    await openQueue();
    expect(drawer()).not.toBeNull();
    expect(table()).toBeNull();
  });
});

describe("a failure while the queue is on screen", () => {
  it("selects the Failed tab", async () => {
    await open({ queue: [row("a", "running")] });
    await twoPanes();
    await openQueue();
    expect(tab("all")?.getAttribute("aria-selected")).toBe("true");
    announce?.(row("a", "failed"));
    await settle();
    expect(tab("failed")?.getAttribute("aria-selected")).toBe("true");
  });

  it("never opens the queue on its own", async () => {
    // A transfer failing is not a reason to take the window over. An empty
    // queue, so nothing has opened the drawer for its own reasons.
    await open();
    await twoPanes();
    expect(drawer()).toBeNull();
    announce?.(row("a", "failed"));
    await settle();
    expect(drawer()).toBeNull();
    // And it did not quietly change the tab behind the user's back either.
    await openQueue();
    expect(tab("all")?.getAttribute("aria-selected")).toBe("true");
  });

  it("stays put when a failure is only re-reported", async () => {
    // Keyed on which rows have failed, not on how many: a row updating again
    // must not drag the user back to a tab they navigated away from.
    await open({ queue: [row("a", "failed")] });
    await twoPanes();
    await openQueue();
    click(tab("completed"));
    await settle();
    announce?.(row("a", "failed"));
    await settle();
    expect(tab("completed")?.getAttribute("aria-selected")).toBe("true");
  });

  it("pills a failure count, and only a real one", async () => {
    await open({ queue: [row("a", "failed")] });
    await twoPanes();
    await openQueue();
    const pill = (id: string) =>
      tab(id)?.querySelector("[class*='tabCountFailed']") !== null &&
      tab(id)?.querySelector("[class*='tabCountFailed']") !== undefined;
    expect(pill("failed")).toBe(true);
    expect(pill("paused")).toBe(false);
  });
});

describe("the drawer's queue table (XTP-E4-S2)", () => {
  /** The queue table's own headers; the file lists above have theirs too. */
  const headers = () =>
    [
      ...(host
        .querySelector('[role="grid"][aria-label="Download list"]')
        ?.querySelectorAll<HTMLElement>('[role="columnheader"]') ?? []),
    ].map((cell) => cell.textContent ?? "");
  const cells = (id: string) =>
    [...(host.querySelector(`[data-transfer="${id}"]`)?.children ?? [])].map(
      (cell) => cell.textContent ?? "",
    );

  it("names both ends of a transfer, which the sheet's columns do not", async () => {
    await open({ queue: [share("a", "running")] });
    await openQueue();
    expect(headers()).toContain("Speed");
    expect(headers()).not.toContain("Remote file");
    await twoPanes();
    expect(headers()).toEqual(["Dir", "Local file", "Remote file", "Size", "Progress"]);
  });

  it("says which way, and which file at each end", async () => {
    // A share row carries no endpoint pair — that is E22-S3 — so the two
    // columns are derived from the direction the row does carry.
    await open({
      queue: [
        share("up", "running", {
          direction: "up",
          sourcePath: "/Users/dorian/notes.txt",
          destPath: "Movies/notes.txt",
        }),
      ],
    });
    await twoPanes();
    await openQueue();
    expect(cells("up")[0]).toBe("↑ UP");
    expect(cells("up")[1]).toBe("/Users/dorian/notes.txt");
    expect(cells("up")[2]).toBe("Movies/notes.txt");
  });

  it("turns the arrow round for a download", async () => {
    await open({
      queue: [
        share("down", "running", {
          direction: "down",
          sourcePath: "Movies/a.mkv",
          destPath: "/Users/dorian/a.mkv",
        }),
      ],
    });
    await twoPanes();
    await openQueue();
    expect(cells("down")[0]).toBe("↓ DOWN");
    expect(cells("down")[1]).toBe("/Users/dorian/a.mkv");
    expect(cells("down")[2]).toBe("Movies/a.mkv");
  });

  it("reads the rate beside the percentage", async () => {
    await open({
      queue: [share("a", "running", { bytesDone: 67, bytesTotal: 100, rate: 112_000_000 })],
    });
    await twoPanes();
    await openQueue();
    expect(cells("a")[4]).toContain("67% · 112 MB/s");
  });

  it("says a queued row is queued rather than drawing it at nought", async () => {
    // An empty track reading "0%" is a transfer that has stalled, which is a
    // different and worse thing to believe.
    await open({ queue: [share("a", "queued")] });
    await twoPanes();
    await openQueue();
    expect(cells("a")[4]).toContain("queued");
    expect(cells("a")[4]).not.toContain("0%");
  });

  it("counts the queue and how long it has left, in the footer", async () => {
    await open({
      queue: [
        share("a", "running", { bytesDone: 0, bytesTotal: 100, rate: 25 }),
        share("b", "queued"),
        share("c", "failed"),
      ],
    });
    await twoPanes();
    await openQueue();
    const footer = host.querySelector("[class*='footer']")?.textContent ?? "";
    expect(footer).toContain("1 active · 1 queued · 1 failed");
    expect(footer).toContain("ETA 4s");
  });

  it("has no ETA to give while nothing is moving", async () => {
    await open({ queue: [share("a", "queued")] });
    await twoPanes();
    await openQueue();
    expect(host.querySelector("[class*='footer']")?.textContent ?? "").not.toContain("ETA");
  });
});
