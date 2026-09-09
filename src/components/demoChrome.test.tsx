// The demo share must not reach the app (E1-S6).
//
// `named()` used to fall back to `smb://nas.local/media` — the design's
// example share — whenever there was a listing but no bookmark behind it. That
// is a real state for a real window, so windows titled themselves after a
// share that does not exist. Reported directly, 2026-09-09.

import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
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
const settle = () => new Promise((done) => setTimeout(done, 0));

/** Pretend this webview is the desktop app rather than the browser preview. */
function asDesktop(on: boolean) {
  if (on) {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  } else {
    Reflect.deleteProperty(window as unknown as Record<string, unknown>, "__TAURI_INTERNALS__");
  }
}

async function open() {
  dispose = render(
    () => (
      <App
        // The fixture regardless, so the only thing under test is whether the
        // *chrome* invents a share — not which backend was chosen.
        backend={fixtureBackend()}
        prefs={memoryPrefs()}
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
  asDesktop(false);
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("a window with a listing but no bookmark behind it", () => {
  it("names no share in the app", async () => {
    asDesktop(true);
    await open();
    expect(host.textContent).not.toContain("nas.local");
    expect(host.textContent).not.toContain("media");
  });

  it("draws no path either, rather than a URI for a share that is not there", async () => {
    asDesktop(true);
    await open();
    const field = host.querySelector<HTMLElement>('[title="Click to type a path or URI"]');
    expect(field?.textContent ?? "").toBe("");
  });

  it("still shows the design's example share in the browser preview", async () => {
    // The preview has no engine, so without this the design's screens have no
    // share to name. Removing the fallback outright would have taken that too.
    asDesktop(false);
    await open();
    expect(host.textContent).toContain("nas.local");
  });
});

describe("the selection a window starts with", () => {
  const highlighted = () => [...host.querySelectorAll('[data-path][aria-selected="true"]')];

  it("is empty in the app", async () => {
    // The preview seeds one because the design's screens draw a selected row
    // and there is no way to click in a screenshot. That seed was ungated, so
    // every window began with `Arrival.2016.mkv` selected — invisible until a
    // folder actually contains a file by that name, and then a row highlighted
    // by nobody (swept 2026-09-09).
    asDesktop(true);
    await open();
    expect(highlighted()).toHaveLength(0);
  });

  it("still shows one in the browser preview", async () => {
    asDesktop(false);
    await open();
    expect(highlighted().length).toBeGreaterThan(0);
  });
});
