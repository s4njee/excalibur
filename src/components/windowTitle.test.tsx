// A window tells the platform its name (E23-S1, E26).
//
// The title the design draws is painted by the webview — it is centred over the
// toolbar, and the native title is hidden to make room for it. That leaves the
// *real* window title at whatever it was created with, which is the app's name
// for every window. Nothing in the app looks wrong; everything outside it does,
// because the macOS Window menu, Mission Control and the app switcher all read
// the native title and nothing else. Two windows were two entries both reading
// "Excalibur", which is no more use for switching between them than no list.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend, type Backend } from "../state/backend";
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

const MOUNT = "smb:nas.local:media";

const saved: Bookmark[] = [
  {
    id: MOUNT,
    name: "media",
    protocol: "smb",
    host: "nas.local",
    share: "media",
    username: "",
    mountPreference: "rw",
    reachable: true,
    mounted: false,
  },
];

/** A backend that remembers what this window called itself. */
function named(inner: Backend): Backend & { titles: string[] } {
  const titles: string[] = [];
  return {
    ...inner,
    async setWindowTitle(title: string) {
      titles.push(title);
    },
    titles,
  };
}

function open(backend: Backend, mount: string | null) {
  dispose = render(
    () => (
      <App
        backend={backend}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => saved, save: async () => undefined }}
        window={{ label: "browser-2", mount, path: "", view: "browse" }}
      />
    ),
    host,
  );
  return new Promise((done) => setTimeout(done, 0));
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

describe("a window's native title", () => {
  it("names the share it is showing", async () => {
    const backend = named(fixtureBackend());
    await open(backend, MOUNT);

    // The same string the title bar draws. Two windows on two shares are two
    // distinguishable rows in the Window menu only if this is the share's name
    // and not the app's.
    expect(backend.titles.at(-1)).toBe("media — nas.local");
  });

  it("falls back to the app's name with nothing mounted", async () => {
    // Read outside the app — in the Window menu and the app switcher — where a
    // sentence about this app's internal state would be noise. "Excalibur" is
    // the only thing that means anything there.
    const backend = named(fixtureBackend({ disconnected: true }));
    await open(backend, null);

    expect(backend.titles.at(-1)).toBe("Excalibur");
  });
});
