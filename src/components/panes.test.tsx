// A window holds two panes and reads the focused one (XTP-E1-S2).
//
// This step is deliberately invisible: with one pane drawn, focus never moves
// and every accessor resolves to the same pane it always did. What is worth
// asserting is the two ways that could have gone wrong — a second pane that
// quietly loads a second listing, and a Tab key taken away from the toolbar
// for a gesture that has nowhere to go.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;
const settle = () => new Promise((done) => setTimeout(done, 0));

let listed: string[] = [];

async function open(prefs = memoryPrefs()) {
  listed = [];
  const base = fixtureBackend();
  const backend = {
    ...base,
    async listDir(path: string) {
      listed.push(path);
      return base.listDir(path);
    },
  } as unknown as Backend;
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={prefs}
        bookmarks={{ load: async () => [], save: async () => undefined }}
      />
    ),
    host,
  );
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

describe("a window with two panes held", () => {
  it("still makes one listing request, because the second pane is inert", async () => {
    // The second pane has no mount of its own until XTP-E2 draws it. If it
    // loaded anyway it would ask the *window's* mount for a path it does not
    // own, and every window would cost two round trips instead of one.
    await open();
    expect(listed).toHaveLength(1);
  });

  it("leaves Tab to the browser while one pane is drawn", async () => {
    // Tab is how the toolbar's controls are reached. Taking it for a gesture
    // with nowhere to go would cost that.
    await open();
    const event = new KeyboardEvent("keydown", {
      key: "Tab",
      bubbles: true,
      cancelable: true,
    });
    window.dispatchEvent(event);
    await settle();
    expect(event.defaultPrevented).toBe(false);
  });

  it("still draws exactly one file list", async () => {
    // Two panes are held, not drawn. XTP-E2 is what puts the second on screen.
    await open();
    expect(host.querySelectorAll('[role="grid"]')).toHaveLength(1);
  });
});

describe("two panes, drawn", () => {
  const viewButton = () => host.querySelector<HTMLElement>('[aria-label="View options"]');
  const menuItem = (text: string) =>
    [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((candidate) =>
      candidate.textContent?.includes(text),
    ) ?? null;
  const lists = () => [...host.querySelectorAll<HTMLElement>('[role="grid"]')];
  const gutter = () => host.querySelector<HTMLElement>('[aria-label="Resize panes"]');
  const click = (element: HTMLElement | null) =>
    element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));

  async function turnOn() {
    click(viewButton());
    await settle();
    click(menuItem("Two Panes"));
    await settle();
    await settle();
  }

  it("draws a second list and a gutter between them", async () => {
    await open();
    expect(lists()).toHaveLength(1);
    await turnOn();
    expect(lists()).toHaveLength(2);
    expect(gutter()).not.toBeNull();
  });

  it("opens the second pane where the first one is, not on nothing", async () => {
    // An empty column with no way to tell whether it is broken is a worse
    // first sight than the folder you were already looking at.
    await open();
    await turnOn();
    // Both panes asked for the same path: the one the window was showing.
    expect(listed.filter((path) => path === "")).toHaveLength(2);
  });

  it("moves focus to the pane that was clicked", async () => {
    await open();
    await turnOn();
    const [left, right] = lists();
    const columnOf = (list: HTMLElement) => list.closest("[class*='paneColumn']");
    // The left one starts focused, because that is the pane the window was.
    expect(columnOf(left!)?.className).toMatch(/paneColumnFocused/);
    right?.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    await settle();
    expect(columnOf(right!)?.className).toMatch(/paneColumnFocused/);
    expect(columnOf(left!)?.className).not.toMatch(/paneColumnFocused/);
  });

  it("remembers the split across a relaunch", async () => {
    const prefs = memoryPrefs();
    await prefs.set("splitRatio", 0.7);
    await open(prefs);
    await turnOn();
    const slot = lists()[0]?.closest("[class*='paneSlot']") as HTMLElement | null;
    expect(slot?.style.flexGrow).toBe("0.7");
  });

  it("keeps one file list when the second pane is put away", async () => {
    await open();
    await turnOn();
    expect(lists()).toHaveLength(2);
    await turnOn();
    expect(lists()).toHaveLength(1);
  });
});
