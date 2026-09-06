// Which window an event reaches (E23-S1, E24-S2).
//
// The shell addresses almost every event it sends: `emit_to(label, …)` for a
// mount whose audience the engine computed, for a listing that changed, for an
// edit that saved, and for a drag that is over one particular window. The
// frontend then subscribed with `listen`, which registers for
// `EventTarget::Any` and receives an event whatever window it was sent to — so
// each address was computed, sent, and then thrown away, and every window acted
// on every window's events.
//
// With one window open that is invisible, which is how it survived review. With
// two it is destructive: on 2026-09-06 one ISO dragged from Finder onto one
// window started *two* uploads, one per window, each to that window's own
// mount. The second window happened to be showing the file's own folder, so its
// upload copied the file onto itself and rewrote it in place.
//
// Checked against the source rather than by driving the real subscriptions:
// mocking `@tauri-apps/api` here raced with the real module and the test passed
// or failed with the scheduler, which is worse than no test. This is the same
// shape as `components/overlays.test.ts` — a rule that is invisible in review
// stated once, next to the reason it exists.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const SOURCE = readFileSync(resolve(process.cwd(), "src/state/backend.ts"), "utf8");

/** Events the shell sends with `emit_to`, so they belong to one window. */
const ADDRESSED = [
  "mount://changed",
  "listing://changed",
  "edit://saved",
  "drag://over",
  "drag://drop",
  "drag://leave",
];

/** The three the window-drag subscription registers through one `bind`. */
const VIA_BIND = ["drag://over", "drag://drop", "drag://leave"];

describe("the shell backend's subscriptions", () => {
  it("reads the backend's source", () => {
    // Guards the test itself: a path that stopped resolving would pass forever.
    expect(SOURCE).toContain("function tauriBackend()");
  });

  for (const event of ADDRESSED) {
    it(`takes ${event} through this window, not the global bus`, () => {
      expect(
        SOURCE,
        `${event} is addressed to one window by the shell, so subscribing with ` +
          `\`listen\` hands it to every window instead`,
      ).not.toMatch(new RegExp(`listen(<[^>]*>)?\\(\\s*"${event}"`));
      // The window-drag events share one `bind`, so the name reaches
      // `listenHere` as its argument rather than as a literal at the call.
      if (VIA_BIND.includes(event)) {
        expect(SOURCE).toMatch(new RegExp(`bind\\(\\s*"${event}"`));
        expect(SOURCE).toMatch(/const bind = async \([\s\S]*?await listenHere</);
      } else {
        expect(SOURCE).toMatch(new RegExp(`listenHere(<[^>]*>)?\\(\\s*"${event}"`));
      }
    });
  }

  it("takes a file drop from this webview's own subscription", () => {
    // `listen("tauri://drag-drop")` also *works* — it just also fires in every
    // other window, which is the bug. The webview's own subscription is the
    // only form that cannot.
    expect(SOURCE).toContain("getCurrentWebview().onDragDropEvent(");
    expect(SOURCE).not.toContain("tauri://drag-");
  });

  it("leaves the one real broadcast on the global bus", () => {
    // Queue progress is emitted with `emit`, not `emit_to`: a transfer belongs
    // to the app rather than to the window that started it, and every window
    // shows the same queue. Scoping this one would be a regression, so it is
    // stated here next to the rule it is the exception to.
    expect(SOURCE).toMatch(/listen<[^>]*>\(\s*"transfer:\/\/progress"/);
  });

  it("keeps the window-scoped helper going through the window", () => {
    // The helper is the single place the scoping is decided; if it ever loses
    // `getCurrentWebviewWindow` every rule above becomes vacuous while still
    // passing, because they only check which helper is called.
    expect(SOURCE).toContain(
      'const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow")',
    );
    expect(SOURCE).toMatch(/getCurrentWebviewWindow\(\)\.listen</);
  });
});
