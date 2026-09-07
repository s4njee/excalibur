// Each favourite's row carries its own picture (E27-S1).
//
// The rule that picks the glyph is `state/favorites.test.ts`'s job — it is a
// pure function and the interesting cases are cheap there. What is left for
// here is the part that only exists once rows are on screen: that the glyph
// reaches the row at all, and that the *right* one does. Both fail silently in
// the same way — every row drawing the same folder still looks like a sidebar
// with icons, which is why "an svg is present" is not the assertion.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import App from "../App";
import { fixtureBackend } from "../state/backend";
import { memoryPrefs } from "../state/prefs";
import type { Favorite } from "../state/favorites";

class NoResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoResizeObserver as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = (ms = 0) => new Promise((done) => setTimeout(done, ms));

/** The glyph on each favourite row, in order. */
const glyphs = () =>
  [...host.querySelectorAll("[data-favorites] [data-glyph]")].map((svg) =>
    svg.getAttribute("data-glyph"),
  );

function open(saved: Favorite[]) {
  dispose = render(
    () => (
      <App
        backend={fixtureBackend()}
        prefs={memoryPrefs()}
        bookmarks={{ load: async () => [], save: async () => undefined }}
        favorites={{ load: async () => saved, save: async () => undefined }}
        window={{ label: "main", mount: null, path: "", view: "browse" }}
      />
    ),
    host,
  );
  return settle();
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

describe("the favourites' glyphs", () => {
  it("draws the platform's own folders as themselves", async () => {
    // The fixture's user folders are Dorian's, the same account the demo
    // bookmarks belong to.
    await open([
      { name: "dorian", path: "/Users/dorian" },
      { name: "Applications", path: "/Applications" },
      { name: "Desktop", path: "/Users/dorian/Desktop" },
      { name: "Movies", path: "/Users/dorian/Movies" },
    ]);
    await settle(10);

    expect(glyphs()).toEqual(["home", "applications", "desktop", "movies"]);
  });

  it("draws a folder the user added as a plain folder", async () => {
    await open([
      { name: "Documents", path: "/Users/dorian/Documents" },
      { name: "excalibur", path: "/Users/dorian/code/excalibur" },
    ]);
    await settle(10);

    // Side by side deliberately: a version that gave every row the same
    // picture would pass a test that only looked at one of them.
    expect(glyphs()).toEqual(["documents", "folder"]);
  });
});
