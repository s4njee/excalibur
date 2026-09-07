// The sidebar's FAVORITES section, as the window wires it (E27-S1).
//
// The rules live in `state/favorites.test.ts`, where they are cheap. What is
// left for here is the part that only exists once the sidebar is on screen:
// that the section is **above** MOUNTED, that a favourite whose folder is
// missing is never drawn, that clicking one connects the same local mount the
// dialog's Local tab would, and that the two ways of adding a folder — the
// row menu and the section's own context menu for removing — reach the store.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
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

const settle = (ms = 0) => new Promise((done) => setTimeout(done, ms));

/** The sidebar's own headers and rows, in the order they are painted. */
const sidebar = () => host.querySelector<HTMLElement>("[data-favorites]")?.parentElement;
const sidebarText = () => sidebar()?.textContent ?? "";
const favoriteRows = () =>
  [...(host.querySelectorAll<HTMLElement>("[data-favorites] [role='button']") ?? [])];
const favoriteNames = () =>
  favoriteRows().map((row) => row.textContent?.trim() ?? "");
const rows = () => [...host.querySelectorAll<HTMLElement>("[data-path]")];
const menuItem = (label: string) =>
  [...host.querySelectorAll('[role="menuitem"]')].find((entry) =>
    entry.textContent?.startsWith(label),
  );
const rightClick = (element: Element) =>
  element.dispatchEvent(
    new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }),
  );

/** The folder this machine is pretending to have, matching the fixture user. */
const HOME = "/Users/dorian";
const PRESENT = [
  HOME,
  "/Applications",
  `${HOME}/Desktop`,
  `${HOME}/Documents`,
  `${HOME}/Pictures`,
  `${HOME}/Movies`,
];

/**
 * A backend with a filesystem behind it, so the section has something to
 * check against and `mount` can succeed.
 *
 * `existingDirectories` answers from `present`, which is how a favourite
 * whose folder is gone is staged: the entry is in the store, the folder is
 * not in this set.
 */
function machine(
  present: string[] = PRESENT,
): Backend & { mounts: unknown[]; preconnected: string[] } {
  const inner = fixtureBackend();
  const mounts: unknown[] = [];
  const preconnected: string[] = [];
  return {
    ...inner,
    async mount(spec) {
      mounts.push(spec);
    },
    async preconnect(spec) {
      const id = (spec as { id: string }).id;
      // The ejected disk: a favourite the section knows about whose folder is
      // not on this machine. It must fail, and the window must not notice.
      if (!present.some((path) => `local::${path.toLowerCase()}` === id)) {
        throw new Error("no such file or directory");
      }
      preconnected.push(id);
    },
    async existingDirectories(paths: string[]) {
      return paths.filter((path) => present.includes(path));
    },
    mounts,
    preconnected,
  };
}

/** A local mount, so the file list's rows are folders on this machine. */
const LOCAL: Bookmark = {
  id: `local::${HOME.toLowerCase()}/media`,
  name: "media",
  protocol: "local",
  host: "",
  share: `${HOME}/media`,
  mountPreference: "rw",
  mounted: false,
  reachable: true,
};

function open(
  backend: Backend,
  favorites: { load: () => Promise<Favorite[] | null>; save: (f: Favorite[]) => Promise<void> },
  options: { bookmarks?: Bookmark[]; mount?: string | null } = {},
) {
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{
          load: async () => options.bookmarks ?? [],
          save: async () => undefined,
        }}
        favorites={favorites}
        window={{ label: "main", mount: options.mount ?? null, path: "" }}
      />
    ),
    host,
  );
  return settle();
}

/** A store that records what the window wrote, and what it started with. */
function store(initial: Favorite[] | null) {
  const saved: Favorite[][] = [];
  return {
    saved,
    latest: () => saved[saved.length - 1],
    load: async () => (saved.length > 0 ? (saved[saved.length - 1] ?? null) : initial),
    save: async (favorites: Favorite[]) => {
      saved.push(favorites);
    },
  };
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

describe("the FAVORITES section", () => {
  it("is seeded on a first run and sits above MOUNTED", async () => {
    const saving = store(null);
    await open(machine(), saving);

    expect(favoriteNames()).toEqual([
      "Home",
      "Applications",
      "Desktop",
      "Documents",
      "Pictures",
      "Movies",
    ]);
    // Order, not merely presence: "above Mounted" is the requirement, and a
    // section rendered after it would satisfy every other assertion here.
    const text = sidebarText();
    expect(text.indexOf("Favorites")).toBeLessThan(text.indexOf("Mounted"));
    expect(text.indexOf("Mounted")).toBeLessThan(text.indexOf("Bookmarks"));
    // Written once, so the next launch reads a list rather than seeding again.
    expect(saving.latest()?.map((favorite) => favorite.name)).toContain("Home");
  });

  it("does not seed over a saved list, however short", async () => {
    // The user removed everything but Documents on a previous run. Seeding
    // "because the list looks wrong" is what this forbids.
    const saving = store([{ name: "Documents", path: `${HOME}/Documents` }]);
    await open(machine(), saving);
    expect(favoriteNames()).toEqual(["Documents"]);
    expect(saving.saved).toEqual([]);
  });

  it("does not draw a favourite whose folder is not there", async () => {
    const saving = store([
      { name: "Documents", path: `${HOME}/Documents` },
      { name: "Backups", path: "/Volumes/Archive/Backups" },
    ]);
    // The external disk is unplugged: `existingDirectories` does not list it.
    await open(machine([`${HOME}/Documents`]), saving);

    expect(favoriteNames()).toEqual(["Documents"]);
    // Hidden, not forgotten. Nothing was written, so plugging the disk back in
    // brings the row back.
    expect(saving.saved).toEqual([]);
  });

  it("opens a favourite as a local mount, the way a local bookmark does", async () => {
    const backend = machine();
    await open(backend, store([{ name: "Documents", path: `${HOME}/Documents` }]));

    favoriteRows()[0]?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    // The spec the engine receives: `local`, no host, the folder as the share.
    // A second shape for the same folder is the thing this forbids.
    expect(backend.mounts).toHaveLength(1);
    expect(backend.mounts[0]).toMatchObject({
      protocol: "local",
      host: "",
      share: `${HOME}/Documents`,
    });
  });

  it("removes a favourite from its own context menu, and saves that", async () => {
    const saving = store([
      { name: "Documents", path: `${HOME}/Documents` },
      { name: "Pictures", path: `${HOME}/Pictures` },
    ]);
    await open(machine(), saving);
    expect(favoriteNames()).toEqual(["Documents", "Pictures"]);

    rightClick(favoriteRows()[0]!);
    await settle();
    const remove = menuItem("Remove from Favorites");
    expect(remove).not.toBeUndefined();
    remove?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(favoriteNames()).toEqual(["Pictures"]);
    expect(saving.latest()).toEqual([{ name: "Pictures", path: `${HOME}/Pictures` }]);
  });
});

describe("Add to Favorites, from a row in the list", () => {
  const openOnLocal = (saving: ReturnType<typeof store>) =>
    open(machine(), saving, { bookmarks: [LOCAL], mount: LOCAL.id });

  const rowNamed = (name: string) =>
    rows().find((row) => row.textContent?.includes(name));

  it("adds the folder's absolute path on this machine", async () => {
    const saving = store([]);
    await openOnLocal(saving);

    rightClick(rowNamed("Documentaries")!);
    await settle();
    const add = menuItem("Add to Favorites");
    expect(add?.getAttribute("aria-disabled")).toBe("false");
    add?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    // The share-relative `Documentaries` turned into a path that means
    // something without the mount — which is the only kind a favourite can be.
    expect(saving.latest()).toEqual([
      { name: "Documentaries", path: `${HOME}/media/Documentaries` },
    ]);
    expect(favoriteNames()).toEqual(["Documentaries"]);
  });

  it("is refused for a file, and says so", async () => {
    await openOnLocal(store([]));
    rightClick(rowNamed("Arrival.2016.mkv")!);
    await settle();
    const add = menuItem("Add to Favorites");
    expect(add?.getAttribute("aria-disabled")).toBe("true");
    expect(add?.getAttribute("title")).toMatch(/folders/i);
  });

  it("is refused on a share, where a folder has no path on this machine", async () => {
    // The fixture's mount with no local bookmark behind it: an SMB share.
    await open(machine(), store([]));
    rightClick(rows()[1]!);
    await settle();
    expect(menuItem("Add to Favorites")?.getAttribute("aria-disabled")).toBe("true");
    expect(menuItem("Add to Favorites")?.getAttribute("title")).toMatch(/this computer/i);
  });
});

describe("preconnecting the favourites at startup", () => {
  // *Asked for directly, 2026-09-07: "all of the local favorites should be
  // preconnected on startup".*
  //
  // The point is the transfer queue rather than the browsing: a queue restored
  // from the last run names shares nobody has mounted yet, its rows now wait
  // for them rather than failing, and this is the arrival they wait for.

  it("holds every favourite whose folder is there, without opening one", async () => {
    const engine = machine();
    await open(engine, store([{ name: "Documents", path: `${HOME}/Documents` }]));

    expect(engine.preconnected).toEqual([`local::${HOME.toLowerCase()}/documents`]);
    // Preconnecting is not opening: no window is put on the mount, so the
    // sidebar shows nothing under MOUNTED and the listing is untouched.
    expect(engine.mounts).toEqual([]);
    expect(sidebarText()).not.toContain("Documents · local");
  });

  it("holds the seeded defaults on a first run too", async () => {
    const engine = machine();
    await open(engine, store(null));
    expect(engine.preconnected).toEqual(
      PRESENT.map((path) => `local::${path.toLowerCase()}`),
    );
  });

  it("says nothing at all about a favourite whose folder has gone", async () => {
    // An ejected disk. The row is not drawn, nothing is preconnected, and —
    // the part that matters — no banner appears and no failed row is queued.
    const engine = machine([`${HOME}/Documents`]);
    await open(
      engine,
      store([
        { name: "Documents", path: `${HOME}/Documents` },
        { name: "Backups", path: "/Volumes/Archive/Backups" },
      ]),
    );

    expect(engine.preconnected).toEqual([`local::${HOME.toLowerCase()}/documents`]);
    expect(favoriteNames()).toEqual(["Documents"]);
    expect(host.textContent).not.toMatch(/no such file/i);
  });
});
