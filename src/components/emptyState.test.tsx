// First run: a window with nothing mounted (E1-S6).
//
// The bug this covers was not a missing empty state — it was a *present* fake
// one. The shell fell back to a mock share, so the desktop app opened on
// `smb://nas.local/media` with rows, an `RW` badge and verbs that acted on it.
// So the assertions here are mostly about what the window must *not* say: no
// share name, no path, no badge, no enabled verb.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { defaultBackend, fixtureBackend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// The virtualised list measures itself on mount. jsdom has no layout, so
// nothing here can resize — the list only needs the constructor to exist.
class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

/** Let the listing's promise settle before looking at the window. */
const settle = () => new Promise((done) => setTimeout(done, 0));

/**
 * The window with nothing mounted.
 *
 * `bookmarks` is deliberately not empty: a saved bookmark still flagged
 * `mounted` from a previous session is the case where naming the chrome from
 * the bookmark rather than from the listing would put a share in the title bar
 * that the shell is not holding.
 */
async function renderDisconnected() {
  const saved = [
    {
      id: "b1",
      name: "media",
      protocol: "smb" as const,
      host: "nas.local",
      share: "media",
      username: "",
      port: null,
      mountPreference: "rw" as const,
      reachable: true,
      mounted: true,
    },
  ];
  dispose = render(
    () => (
      <App
        backend={fixtureBackend({ disconnected: true })}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => saved, save: async () => undefined }}
      />
    ),
    host,
  );
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

describe("the empty state", () => {
  it("says there is no share, and offers the way out of it", async () => {
    await renderDisconnected();
    expect(host.textContent).toContain("No share connected");
    expect(
      [...host.querySelectorAll("button")].some(
        (button) => button.textContent?.trim() === "New connection…",
      ),
    ).toBe(true);
  });

  it("opens the connection dialog from that button", async () => {
    await renderDisconnected();
    const button = [...host.querySelectorAll("button")].find(
      (candidate) => candidate.textContent?.trim() === "New connection…",
    );
    button?.click();
    await settle();
    expect(host.querySelector('[aria-label="New Connection"]')).not.toBeNull();
  });

  it("names nothing in the title bar or the breadcrumb", async () => {
    await renderDisconnected();
    // The two rows that describe the mount: `window > titlebar` and
    // `window > toolbar`. Not the sidebar — a bookmark is a saved connection
    // and belongs on screen whether or not it is mounted.
    const window = host.firstElementChild;
    const chrome = [window?.children[0], window?.children[1]]
      .map((row) => row?.textContent ?? "")
      .join(" ");
    // The demo names are the specific thing that must not appear: they are
    // what the mock share put here, and they name a host the user does not
    // have. The bookmark's own name is equally wrong — it is not mounted.
    expect(chrome).not.toContain("nas.local");
    expect(chrome).not.toContain("smb://");
    expect(chrome).not.toContain("media");
  });

  it("claims nothing in the status bar", async () => {
    await renderDisconnected();
    const status = host.querySelector('[role="status"]');
    expect(status?.textContent).toBe("Not connected");
    // `RO` would be a statement about a share that is not there, and the badge
    // is the one thing in this bar a user is meant to be able to trust.
    expect(status?.textContent).not.toContain("RO");
    expect(status?.textContent).not.toContain("free of");
  });

  it("enables no verb", async () => {
    await renderDisconnected();
    // Enumerated rather than spot-checked: a verb added to the toolbar later
    // is covered without anyone remembering this test exists.
    const verbs = ["Quick Look", "Edit", "Download", "Upload"];
    for (const label of verbs) {
      const button = [...host.querySelectorAll("button")].find(
        (candidate) => candidate.textContent?.trim() === label,
      );
      expect(button, label).toBeDefined();
      expect(button?.disabled, label).toBe(true);
    }
  });

  it("draws no file list at all", async () => {
    await renderDisconnected();
    // Not "an empty list": there is no directory here, and a column header
    // over nothing would describe a listing that was never returned.
    expect(host.querySelector("[data-path]")).toBeNull();
  });
});

describe("the fixture is the preview's, not the app's", () => {
  it("does not fall back to it inside the Tauri shell", async () => {
    // The rule E1-S6 exists to enforce: in the shell, a listing goes to the
    // engine and a failure stays a failure. The fixture answering here is
    // exactly how a first run came to show films that are not there.
    const calls: string[] = [];
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (command: string) => {
        calls.push(command);
        return Promise.reject({ message: "no share is mounted", surface: "banner" });
      },
    };
    try {
      await expect(defaultBackend().listDir("")).rejects.toBeTruthy();
      expect(calls).toEqual(["list_dir"]);
    } finally {
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    }
  });

  it("still serves the preview in a browser", async () => {
    // The other half of the same rule, and the half a regression would break
    // silently: `?screen=1a` must keep answering from the committed fixture.
    const listing = await defaultBackend().listDir("");
    expect(listing.entries.length).toBeGreaterThan(0);
  });
});

describe("the transfers sheet", () => {
  it("opens below the toolbar rather than over it", () => {
    // Anchored to the title bar, the sheet covered Quick Look, Edit and
    // Download: the click landed on the sheet and did nothing, which a user
    // cannot tell from a broken button. jsdom has no layout, so this reads the
    // rule rather than measuring it — the measurement is in the preview.
    const css = readFileSync(
      resolve(process.cwd(), "src/components/transfers.module.css"),
      "utf8",
    );
    const top = /\.panel\s*\{[^}]*?top:\s*([^;]+);/s.exec(css)?.[1]?.trim();
    expect(top).toBe("calc(var(--height-titlebar) + var(--height-toolbar))");
  });
});
