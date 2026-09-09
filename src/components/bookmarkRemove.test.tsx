// Removing a bookmark from the sidebar (E3-S1).
//
// The footer's − acts on the *selected* bookmark, and a single click selects
// where a double opens. Untested until now, which is how a mounted row's
// missing selection state went unnoticed.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
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

const bookmark = (over: Partial<Bookmark>): Bookmark => ({
  id: "xsync:freya.local:/mnt/data",
  name: "data on freya.local",
  protocol: "xsync",
  host: "freya.local",
  share: "/mnt/data",
  mountPreference: "rw",
  mounted: false,
  reachable: true,
  ...over,
});

async function open(saved: Bookmark[]) {
  dispose = render(
    () => (
      <App
        backend={fixtureBackend()}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => saved, save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const remove = () =>
  host.querySelector<HTMLButtonElement>('[aria-label="Remove bookmark"]');
const row = (name: string) =>
  [...host.querySelectorAll<HTMLElement>("[role='button']")].find((candidate) =>
    candidate.textContent?.includes(name),
  ) ?? null;
const click = (element: HTMLElement | null) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("removing a bookmark", () => {
  it("offers − only once something is selected", async () => {
    await open([bookmark({})]);
    expect(remove()?.disabled).toBe(true);
    click(row("data on freya.local"));
    await settle();
    expect(remove()?.disabled).toBe(false);
  });

  it("takes the row away when − is pressed", async () => {
    await open([bookmark({})]);
    click(row("data on freya.local"));
    await settle();
    click(remove());
    await settle();
    expect(row("data on freya.local")).toBeNull();
  });

  it("can remove a share that is mounted", async () => {
    // A mounted bookmark is in MOUNTED, not BOOKMARKS. Its row selects the
    // same way — and if it did not, a share could never be forgotten without
    // unmounting it first, which there is no button for.
    await open([bookmark({ mounted: true })]);
    click(row("data on freya.local"));
    await settle();
    expect(remove()?.disabled).toBe(false);
  });

  it("offers Remove Bookmark on a right-click, which is the gesture people try", async () => {
    // The footer's − was the only route, and a bare glyph acting on a
    // selection with no visible state is not one anyone finds. FAVORITES
    // already answered a right-click; MOUNTED and BOOKMARKS did not.
    await open([bookmark({})]);
    const target = row("data on freya.local");
    target?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    await settle();
    const item = [...host.querySelectorAll('[role="menuitem"]')].find(
      (candidate) => candidate.textContent?.trim() === "Remove Bookmark",
    );
    expect(item).toBeDefined();
    item?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(row("data on freya.local")).toBeNull();
  });

  it("works the same on a mounted share, without unmounting it first", async () => {
    // There is no unmount button, so if this needed one a mounted share could
    // never be forgotten.
    await open([bookmark({ mounted: true })]);
    const target = row("data on freya.local");
    target?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    await settle();
    const item = [...host.querySelectorAll('[role="menuitem"]')].find(
      (candidate) => candidate.textContent?.trim() === "Remove Bookmark",
    );
    item?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(row("data on freya.local")).toBeNull();
  });
});
