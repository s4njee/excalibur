// The message log pane (XTP-E5-S1).
//
// The handoff calls the log "where protocol truth lives", which sets the bar
// for what is worth asserting: that a line's text is never paraphrased, that
// the pane says what resolution it is showing rather than implying frames it
// does not have, and that the recovery a line offers acts on the share the
// *line* is about — a log is read after the fact, and the pane has usually
// moved on.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { LogLine, MountInfo } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));
/** The window's log subscriber, so a test can push a line through. */
let announce: ((line: LogLine) => void) | null = null;
let cleared = 0;
let remounted: string[] = [];

const LINES: LogLine[] = [
  {
    at: Date.parse("2026-08-31T09:22:41Z"),
    category: "command",
    text: "mount -t nfs4 -o sec=krb5p 10.0.0.4:/export/backups",
    mount: "nfs:10.0.0.4",
    resolution: "verb",
  },
  {
    at: Date.parse("2026-08-31T09:22:41Z"),
    category: "response",
    text: "OK · export flags ro,root_squash,no_subtree_check",
    mount: "nfs:10.0.0.4",
    resolution: "verb",
  },
  {
    at: Date.parse("2026-08-31T09:22:44Z"),
    category: "error",
    text: "upload rejected — EROFS on /export/backups/patch-notes.txt",
    mount: "nfs:10.0.0.4",
    resolution: "verb",
  },
  {
    at: Date.parse("2026-08-31T09:22:44Z"),
    category: "hint",
    text: "export is mounted read-only",
    action: "remountRw",
    mount: "nfs:10.0.0.4",
    resolution: "verb",
  },
];

async function open(options: { lines?: LogLine[] } = {}) {
  announce = null;
  cleared = 0;
  remounted = [];
  const base = fixtureBackend();
  const backend = {
    ...base,
    async logLines(): Promise<LogLine[]> {
      return options.lines ?? LINES;
    },
    async clearLog(): Promise<void> {
      cleared += 1;
    },
    onLogLine(handler: (line: LogLine) => void) {
      announce = handler;
      return () => {
        announce = null;
      };
    },
    async remountReadWrite(id: string): Promise<MountInfo> {
      remounted.push(id);
      return (await base.listDir("")).mount;
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

const click = (element: Element | null | undefined) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
const toolbar = () => host.firstElementChild?.children[1] ?? null;
const verb = (label: string) =>
  [...(toolbar()?.querySelectorAll("button") ?? [])].find(
    (candidate) => candidate.textContent === label,
  ) ?? null;
const viewButton = () => host.querySelector<HTMLElement>('[aria-label="View options"]');
const menuItem = (text: string) =>
  [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((candidate) =>
    candidate.textContent?.includes(text),
  ) ?? null;
const pane = () => host.querySelector<HTMLElement>('[role="log"][aria-label="Message log"]');
const logTab = () => host.querySelector<HTMLElement>('[data-category="log"]');
const texts = () =>
  [...(pane()?.querySelectorAll<HTMLElement>("[data-category]") ?? [])].map(
    (row) => row.textContent ?? "",
  );

async function openQueue() {
  if (host.querySelector('[aria-label="Transfers"][role]') === null) {
    click(verb("Transfers"));
    await settle();
    await settle();
  }
}

async function openLog() {
  await openQueue();
  click(logTab());
  await settle();
}

async function twoPanes() {
  click(viewButton());
  await settle();
  click(menuItem("Two Panes"));
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

describe("the message log", () => {
  it("is a tab beside the categories, in both shapes", async () => {
    // "The log records in both views; only its surfacing differs."
    await open();
    await openQueue();
    expect(logTab()).not.toBeNull();
    await twoPanes();
    expect(logTab()).not.toBeNull();
  });

  it("shows the lines the engine already had", async () => {
    // The ring, not just what arrives next: a window opened after the failure
    // would otherwise be missing the line explaining why it was opened.
    await open();
    await openLog();
    expect(texts().some((text) => text.includes("EROFS"))).toBe(true);
  });

  it("says what resolution it is showing", async () => {
    // The design's stream is per protocol frame; these are engine verbs, and
    // a pane that let a reader believe otherwise would be worse than one that
    // says which it is.
    await open();
    await openLog();
    expect(pane()?.textContent).toContain("not protocol frames");
  });

  it("says so when the lines are frames off the wire", async () => {
    // An xsync session is tapped frame by frame (XTP-E5-S0), and the note is
    // what tells the reader the difference.
    await open({
      lines: [{ ...LINES[0]!, resolution: "frame" }],
    });
    await openLog();
    expect(pane()?.textContent).toContain("xsync protocol frames");
    expect(pane()?.textContent).not.toContain("not protocol frames");
  });

  it("quotes the server verbatim", async () => {
    await open();
    await openLog();
    expect(texts().join("\n")).toContain(
      "upload rejected — EROFS on /export/backups/patch-notes.txt",
    );
  });

  it("keeps the chatty half behind Verbose", async () => {
    await open();
    await openLog();
    expect(texts().some((text) => text.includes("export flags"))).toBe(false);
    const verbose = host.querySelector<HTMLInputElement>('input[type="checkbox"]');
    verbose?.click();
    await settle();
    expect(texts().some((text) => text.includes("export flags"))).toBe(true);
  });

  it("takes a line as it arrives", async () => {
    await open();
    await openLog();
    announce?.({
      at: Date.parse("2026-08-31T09:23:00Z"),
      category: "status",
      text: "mounted read-write",
      resolution: "verb",
    });
    await settle();
    expect(texts().some((text) => text.includes("mounted read-write"))).toBe(true);
  });

  it("empties on Clear, in the window and in the engine", async () => {
    await open();
    await openLog();
    click([...host.querySelectorAll("button")].find((b) => b.textContent === "Clear"));
    await settle();
    expect(texts()).toEqual([]);
    expect(cleared).toBe(1);
  });

  it("offers the recovery its hint line names, on that line's share", async () => {
    // The line names the mount, so this acts on the share the line is about
    // rather than on whatever the focused pane is holding now.
    await open();
    await openLog();
    click([...host.querySelectorAll("button")].find((b) => b.textContent === "Remount as RW"));
    await settle();
    await settle();
    expect(remounted).toEqual(["nfs:10.0.0.4"]);
  });

  it("offers nothing to recover from on an ordinary line", async () => {
    await open({ lines: [LINES[0]!] });
    await openLog();
    expect(
      [...host.querySelectorAll("button")].some((b) => b.textContent === "Remount as RW"),
    ).toBe(false);
  });

  it("goes back to the queue when a category is chosen", async () => {
    await open();
    await openLog();
    expect(pane()).not.toBeNull();
    click(host.querySelector<HTMLElement>('[data-category="all"]'));
    await settle();
    expect(pane()).toBeNull();
  });
});
