// The context menu, as the window wires it (E14-S2).
//
// What the menu *offers* is `state/menu.test.ts`'s job — it is a pure function
// and the interesting cases are cheap there. What is left for here is the part
// that only exists once a real menu is on screen: which rows it ends up acting
// on, and the fact that while it is open the window's own keyboard shortcuts
// are not also firing behind it.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = (ms = 0) => new Promise((done) => setTimeout(done, ms));

const rows = () => [...host.querySelectorAll<HTMLElement>("[data-path]")];
const menu = () => host.querySelector('[role="menu"]');
const items = () =>
  [...host.querySelectorAll('[role="menuitem"]')].map((item) => item.textContent ?? "");
const selection = () =>
  [...host.querySelectorAll('[aria-selected="true"]')].map((row) => row.getAttribute("data-path"));

const rightClick = (element: Element) =>
  element.dispatchEvent(
    new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }),
  );

const item = (label: string) =>
  [...host.querySelectorAll('[role="menuitem"]')].find((entry) =>
    entry.textContent?.startsWith(label),
  );

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

describe("the context menu", () => {
  it("acts on the row it was opened over when that row was not selected", async () => {
    const target = rows()[6];
    expect(selection()).not.toContain(target?.getAttribute("data-path"));

    rightClick(target!);
    await settle();
    expect(menu()).not.toBeNull();
    // Selected first, so what the menu will act on is what is highlighted.
    // A menu acting on rows the user cannot see are chosen is the failure
    // this ordering exists to prevent.
    expect(selection()).toEqual([target?.getAttribute("data-path")]);
  });

  it("keeps a multiple selection when opened over one of its rows", async () => {
    rows()[4]?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    rows()[6]?.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
    await settle();
    const chosen = selection();
    expect(chosen.length).toBe(3);

    rightClick(rows()[5]!);
    await settle();
    expect(selection()).toEqual(chosen);
    // And it says so, rather than leaving the user to guess whether the verb
    // means all three or the one under the pointer.
    expect(items()).toContain("Delete 3 items");
  });

  it("offers the folder's verbs on the background, not a file's", async () => {
    rightClick(host.querySelector('[role="rowgroup"]')!);
    await settle();
    expect(items()).toEqual(["New Folder", "Upload…"]);
  });

  it("closes on Escape", async () => {
    rightClick(rows()[4]!);
    await settle();
    menu()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    expect(menu()).toBeNull();
  });

  it("does not let the window's shortcuts fire behind it", async () => {
    rightClick(rows()[4]!);
    await settle();
    const before = rows().length;

    // Delete is both a menu item and the window's own shortcut. With a menu
    // open it must mean neither until something is chosen — a delete that
    // happened *behind* an open menu offering Delete would be indefensible.
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true }));
    await settle();
    expect(rows().length).toBe(before);
    expect(menu()).not.toBeNull();
  });

  it("opens from the keyboard, on the selection", async () => {
    // Shift+F10 and the Menu key: the two keys a desktop gives for "what can I
    // do with this". Without them the menu is a pointer-only feature, and
    // every verb in it that has no toolbar button — Move to…, Properties —
    // would be unreachable without a mouse.
    rows()[4]?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: "F10", shiftKey: true, bubbles: true }),
    );
    await settle();
    expect(menu()).not.toBeNull();
    expect(items()).toContain("Properties");

    menu()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "ContextMenu", bubbles: true }));
    await settle();
    expect(menu()).not.toBeNull();
  });

  it("hands the selection to Properties", async () => {
    rightClick(rows()[4]!);
    await settle();
    item("Properties")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(menu(), "the menu closes behind the sheet it opened").toBeNull();
    const sheet = host.querySelector('[role="dialog"][aria-label="Properties"]');
    expect(sheet).not.toBeNull();
    expect(sheet?.textContent).toContain(rows()[4]?.getAttribute("data-path"));
  });
});
