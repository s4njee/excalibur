// Opening a favourite must not leave a bookmark behind (E27-S1).
//
// The window's model is one mount at a time and a mount is a bookmark, so
// opening a folder from FAVORITES has to produce one. It should not outlive
// the mount: the favourite is already the permanent row for that folder.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Favorite } from "../state/favorites";
import type { Bookmark } from "../state/types";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

const FAVOURITE: Favorite = { path: "/Users/dorian/Downloads", name: "Downloads" };

/** What reached the bookmark store, which is what survives a relaunch. */
let written: Bookmark[][] = [];

async function open() {
  written = [];
  const backend = {
    ...fixtureBackend(),
    async mount() {},
    async existingDirectories(paths: string[]) {
      return paths;
    },
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{
          load: async () => [],
          save: async (next: Bookmark[]) => {
            written.push(next);
          },
        }}
        favorites={{ load: async () => [FAVOURITE], save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const row = (name: string) =>
  [...host.querySelectorAll<HTMLElement>("[role='button']")].find((candidate) =>
    candidate.textContent?.includes(name),
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

describe("opening a folder from FAVORITES", () => {
  it("never writes it to the bookmark store", async () => {
    await open();
    row("Downloads")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await settle();
    // The favourite is the permanent row for this folder; a saved bookmark
    // saying the same thing is the redundancy (asked for directly).
    const saved = written.flat();
    expect(saved.some((bookmark) => bookmark.share === FAVOURITE.path)).toBe(false);
  });

  it("shows it under MOUNTED but never under BOOKMARKS", async () => {
    await open();
    row("Downloads")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await settle();
    const sidebar = host.querySelector("[data-favorites]")?.parentElement;
    const text = sidebar?.textContent ?? "";
    // MOUNTED has to draw it: the window needs to say what it is on.
    const mounted = text.indexOf("Mounted");
    const bookmarks = text.indexOf("Bookmarks");
    expect(mounted).toBeGreaterThan(-1);
    expect(bookmarks).toBeGreaterThan(mounted);
    expect(text.slice(mounted, bookmarks)).toContain("Downloads");
    // BOOKMARKS must not gain a second row naming the same folder.
    expect(text.slice(bookmarks)).not.toContain("Downloads");
  });

  it("leaves a bookmark the user saved for that folder alone", async () => {
    // The OS folders are seeded as favourites, so a path test would hide most
    // deliberately-saved local bookmarks. Only a bookmark this window created
    // by opening a favourite is transient.
    written = [];
    const backend = {
      ...fixtureBackend(),
      async mount() {},
      async existingDirectories(paths: string[]) {
        return paths;
      },
    } as unknown as Backend;
    const saved: Bookmark = {
      id: "local::/users/dorian/downloads",
      name: "Downloads",
      protocol: "local",
      host: "",
      share: "/Users/dorian/Downloads",
      mountPreference: "rw",
      mounted: false,
      reachable: true,
    };
    dispose = render(
      () => (
        <App
          backend={backend}
          prefs={memoryPrefs()}
          bookmarks={{
            load: async () => [saved],
            save: async (next: Bookmark[]) => {
              written.push(next);
            },
          }}
          favorites={{ load: async () => [FAVOURITE], save: async () => undefined }}
        />
      ),
      host,
    );
    await settle();
    await settle();
    row("Downloads")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    await settle();
    // Every write still carries it: it was theirs before the favourite was
    // clicked and it is theirs after.
    for (const write of written) {
      expect(write.some((bookmark) => bookmark.id === saved.id)).toBe(true);
    }
  });
});
