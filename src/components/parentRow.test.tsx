// The `..` row (E8-S10).
//
// The property worth holding is not "there is a row saying `..`" — it is that
// the row is *not an entry*. It carries no path, so nothing that walks the
// listing can pick it up: not the selection, not the status bar's count, not
// Download. And because it sits above the entries in a virtualised list whose
// every calculation is `index * rowHeight`, the second property is that the
// indices still line up — an off-by-one there aims a rubber band, a
// type-to-select scroll, or a screen reader's row number one row wrong.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";

// The virtualised list measures itself on mount; jsdom has no layout.
class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = () => new Promise((done) => setTimeout(done, 0));

const parentRow = () => host.querySelector('[role="row"][title="Enclosing folder"]');
const entryRows = () => [...host.querySelectorAll<HTMLElement>("[data-path]")];
/**
 * Where the window is, read off the listing itself.
 *
 * A share-relative path with no `/` in it is a child of the root, so this is
 * the observable that says whether navigating actually happened — the
 * breadcrumb has no test hook and its class names are hashed away.
 */
const firstPath = () => entryRows()[0]?.getAttribute("data-path") ?? "";

beforeEach(async () => {
  host = document.createElement("div");
  document.body.append(host);
  dispose = render(
    () => (
      <App
        backend={fixtureBackend()}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => [], save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

/** Descend into the first folder the fixture offers. */
async function descend() {
  const folder = entryRows().find((row) => row.textContent?.includes("Folder"))
    ?? entryRows()[0];
  folder?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  return folder;
}

describe("the parent row", () => {
  it("is withheld at the share root", () => {
    expect(firstPath()).not.toBe("");
    expect(firstPath()).not.toContain("/");
    expect(parentRow()).toBeNull();
  });

  it("appears once there is somewhere above to go, and goes there", async () => {
    await descend();
    expect(firstPath(), "descended into a folder").toContain("/");
    const row = parentRow();
    expect(row, "a folder below the root has a parent row").not.toBeNull();

    row?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    // Back at the root: its children have no `/` in their paths, and the row
    // withdraws because there is nowhere above to go.
    expect(firstPath()).not.toContain("/");
    expect(parentRow()).toBeNull();
  });

  it("is not an entry: no path, so nothing that walks the listing sees it", async () => {
    await descend();
    expect(parentRow()?.hasAttribute("data-path")).toBe(false);
    // The drop hit test, the selection and the band all key on `data-path`.
    expect(entryRows().some((row) => row.textContent?.trim().startsWith(".."))).toBe(false);
  });

  it("does nothing on a single click, as clicking past the last row does", async () => {
    await descend();
    const before = host.querySelectorAll('[aria-selected="true"]').length;
    const where = firstPath();
    parentRow()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(host.querySelectorAll('[aria-selected="true"]').length).toBe(before);
    expect(firstPath()).toBe(where);
  });

  it("shifts every entry's row index down by one, and only when it is drawn", async () => {
    // At the root the first entry is row 1 of `aria-rowcount`.
    const grid = () => host.querySelector('[role="grid"]');
    const rootRows = Number(grid()?.getAttribute("aria-rowcount"));
    expect(entryRows()[0]?.getAttribute("aria-rowindex")).toBe("1");

    await descend();
    expect(parentRow()?.getAttribute("aria-rowindex")).toBe("1");
    expect(entryRows()[0]?.getAttribute("aria-rowindex")).toBe("2");
    // And the count includes it: a grid whose rowcount omits a row it draws
    // tells a screen reader the list ends one row early.
    expect(Number(grid()?.getAttribute("aria-rowcount"))).toBe(entryRows().length + 1);
    expect(rootRows).toBeGreaterThan(0);
  });
});
