// The toolbar's Quick Look button (E3-S5).
//
// It rendered enabled, carried a tooltip promising Space, and did nothing:
// the `Verb` had no `onClick`, so the panel could only be opened from the
// keyboard. The property worth holding is not "the button opens the panel" —
// it is that the button and the key are the *same verb*, which is what a
// second handler with its own copy of the one-selected-row rule would break.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";

// jsdom has no layout, so nothing the virtualised list observes can resize.
class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = (ms = 0) => new Promise((done) => setTimeout(done, ms));

/** The panel, while it is on screen. It outlives its own close by 140ms. */
const panel = () => host.querySelector('[role="dialog"][aria-label^="Preview of"]');

const verb = (label: string) =>
  [...host.querySelectorAll("button")].find(
    (button) => button.textContent?.trim() === label,
  );

/** What Space does, as the window sees it. */
const pressSpace = () =>
  window.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));

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
  // The window opens with one row selected, which is the state the verb needs.
  await settle();
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("the Quick Look verb", () => {
  it("opens the panel from the toolbar", async () => {
    expect(verb("Quick Look")?.disabled).toBe(false);
    verb("Quick Look")?.click();
    await settle();
    expect(panel()).not.toBeNull();
  });

  it("is the same verb as Space, not a second one beside it", async () => {
    // Opened by the button, closed by the key, and back again. Two handlers
    // holding their own state would each toggle their own panel, and this is
    // the assertion that catches it.
    verb("Quick Look")?.click();
    await settle();
    expect(panel()).not.toBeNull();

    pressSpace();
    // The panel stays mounted for the length of its exit animation (E1-S5),
    // so this waits it out rather than asserting on the frame after the key.
    await settle(200);
    expect(panel()).toBeNull();

    pressSpace();
    await settle();
    expect(panel()).not.toBeNull();

    verb("Quick Look")?.click();
    await settle(200);
    expect(panel()).toBeNull();
  });

  it("keeps the one-selected-row rule", async () => {
    // Two rows selected has no single thing to preview, and the design says
    // the panel follows one row. The button must not have its own opinion
    // about that — this is the rule Space obeys, checked through the click.
    const rows = [...host.querySelectorAll<HTMLElement>("[data-path]")];
    const other = rows.find((row) => row.getAttribute("aria-selected") !== "true");
    expect(other, "the list rendered no second row to select").toBeDefined();
    other?.dispatchEvent(new MouseEvent("click", { bubbles: true, metaKey: true }));
    await settle();
    // Stated rather than assumed: a test that silently selected nothing would
    // pass by never reaching the rule it claims to check.
    expect(host.querySelectorAll('[aria-selected="true"]')).toHaveLength(2);

    verb("Quick Look")?.click();
    await settle();
    expect(panel()).toBeNull();
  });
});
