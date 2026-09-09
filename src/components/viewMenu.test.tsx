// The toolbar's View menu (E17-S1).
//
// Asked for directly, 2026-09-09. The window already had these settings — in
// Preferences and behind Cmd+1/2/3 — with nothing in the toolbar saying so.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend } from "../state/backend";
import { memoryPrefs, type Prefs } from "../state/prefs";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

async function open(prefs: Prefs = memoryPrefs()) {
  dispose = render(
    () => (
      <App
        backend={fixtureBackend()}
        prefs={prefs}
        bookmarks={{ load: async () => [], save: async () => undefined }}
      />
    ),
    host,
  );
  await settle();
  await settle();
}

const button = () => host.querySelector<HTMLElement>('[aria-label="View options"]');
const items = () => [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')];
const item = (text: string) =>
  items().find((candidate) => candidate.textContent?.includes(text)) ?? null;
const click = (element: HTMLElement | null) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));

async function openMenu() {
  click(button());
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

describe("the View menu", () => {
  it("is in the toolbar and opens on a click", async () => {
    await open();
    expect(button()).not.toBeNull();
    expect(items()).toHaveLength(0);
    await openMenu();
    expect(item("Show Hidden Files")).not.toBeNull();
    expect(item("Columns")).not.toBeNull();
  });

  it("closes when its own button is pressed again", async () => {
    // The reason the menu is anchored to the button: without that, the press
    // that closes it is dismissed and then re-opened by the button's click.
    await open();
    await openMenu();
    expect(items().length).toBeGreaterThan(0);
    await openMenu();
    expect(items()).toHaveLength(0);
  });

  it("hides dotfiles, and says so the next time it is opened", async () => {
    await open();
    // Hidden files are shown by default in this app.
    await openMenu();
    expect(item("Show Hidden Files")?.textContent).toContain("✓");
    click(item("Show Hidden Files"));
    await settle();
    await openMenu();
    expect(item("Show Hidden Files")?.textContent).not.toContain("✓");
  });

  it("switches the list off details, and ticks only the chosen layout", async () => {
    await open();
    await openMenu();
    expect(item("Columns")?.textContent).toContain("✓");
    click(item("Name-led"));
    await settle();
    await openMenu();
    expect(item("Name-led")?.textContent).toContain("✓");
    expect(item("Columns")?.textContent).not.toContain("✓");
  });

  it("remembers the choice, because these are preferences not signals", async () => {
    const prefs = memoryPrefs();
    await open(prefs);
    await openMenu();
    click(item("Mono grid"));
    await settle();
    dispose?.();
    host.remove();
    host = document.createElement("div");
    document.body.append(host);

    await open(prefs);
    await openMenu();
    expect(item("Mono grid")?.textContent).toContain("✓");
  });
});
