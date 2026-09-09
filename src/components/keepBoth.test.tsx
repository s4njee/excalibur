// `Keep both` keeps both (backlog.md:117).
//
// It computed a free name, showed it as "Keep both would call it …", and then
// re-sent the original paths — so the answer behaved as Replace and overwrote
// the file it had just promised to keep. Found while reading the conflict path
// that XTP-E3-S2's drags go through, which is why it is fixed here.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Conflict, Listing, MountInfo } from "../state/types";

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

let downloads: { paths: string[]; rename?: Record<string, string> }[] = [];

async function open() {
  downloads = [];
  const base = fixtureBackend();
  const backend = {
    ...base,
    async listDir(): Promise<Listing> {
      return { entries: [entry("a.txt")], mount: MOUNT };
    },
    async checkConflicts(): Promise<Conflict[]> {
      return [
        {
          transferId: "",
          name: "a.txt",
          destPath: "/Users/dorian/Downloads/a.txt",
          keepBothAs: "a 2.txt",
        } as Conflict,
      ];
    },
    async downloadFolder() {
      return "/Users/dorian/Downloads";
    },
    async enqueueDownloads(
      paths: string[],
      _destination: string,
      _mount?: string,
      rename?: Record<string, string>,
    ) {
      downloads.push({ paths, rename });
      return ["d1"];
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

const click = (element: HTMLElement | null) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
const button = (text: string) =>
  [...host.querySelectorAll<HTMLElement>("button")].find(
    (candidate) => candidate.textContent?.trim() === text,
  ) ?? null;

/** Select a row and ask to download it, which is what raises the prompt. */
async function downloadWithAClash() {
  const row = host.querySelector<HTMLElement>("[data-path]");
  row?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await settle();
  click(button("Download"));
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

describe("Keep both", () => {
  it("offers the free name the destination suggested", async () => {
    await open();
    await downloadWithAClash();
    expect(host.textContent).toContain("a 2.txt");
  });

  it("sends that name, rather than overwriting under the old one", async () => {
    // The bug: the name was shown and then thrown away, so the request went
    // out under the original path and replaced the file.
    await open();
    await downloadWithAClash();
    click(button("Keep both"));
    await settle();
    await settle();
    expect(downloads).toHaveLength(1);
    expect(downloads[0]?.rename).toEqual({ "a.txt": "a 2.txt" });
  });

  it("sends no rename for Replace, which is what keeping the name means", async () => {
    await open();
    await downloadWithAClash();
    click(button("Replace"));
    await settle();
    await settle();
    expect(downloads[0]?.rename).toBeUndefined();
  });
});
