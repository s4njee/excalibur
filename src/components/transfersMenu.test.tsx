// The transfers panel's overflow menu, as the panel wires it.
//
// *Asked for directly, 2026-09-07: "there should probably be a hamburger menu
// or similar that has clear failed, retry failed".*
//
// What the menu *offers* is `state/transferMenu.test.ts`'s job. What is left
// for here is the part that only exists once it is on screen: that it is the
// shared `ContextMenu` rather than a second one, that a keyboard can reach it,
// and that a refused row is inert rather than absent.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { DEFAULT_PANE_SIZE } from "../state/paneSize";
import { render } from "solid-js/web";
import { Transfers } from "./Transfers";
import type { Transfer, TransferStatus } from "../state/types";

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = () => new Promise((done) => setTimeout(done, 0));

function row(id: string, status: TransferStatus): Transfer {
  return {
    id,
    direction: "down",
    name: `${id}.bin`,
    sourcePath: `Show/${id}.bin`,
    destPath: `/Users/dorian/Downloads/${id}.bin`,
    bytesDone: 0,
    bytesTotal: 1024,
    rate: null,
    etaSeconds: null,
    status,
    error: status === "failed" ? "connection lost: not connected" : null,
    queuedAtMs: 0,
  };
}

const chosen: string[] = [];

function open(transfers: Transfer[]) {
  chosen.length = 0;
  dispose = render(
    () => (
      <Transfers
        size={DEFAULT_PANE_SIZE}
        onResize={() => undefined}
        onResizeEnd={() => undefined}
        categories={[{ id: "all", label: "All", count: 0 }]}
        category="all"
        onCategory={() => undefined}
        transfers={transfers}
        onPause={() => undefined}
        onCancel={() => undefined}
        onRetry={() => undefined}
        onClearFinished={() => chosen.push("clearFinished")}
        onClearFailed={() => chosen.push("clearFailed")}
        onRetryFailed={() => chosen.push("retryFailed")}
        onPauseAll={() => chosen.push("pauseAll")}
        onClose={() => chosen.push("close")}
        onRemount={() => undefined}
      />
    ),
    host,
  );
  return settle();
}

const hamburger = () => host.querySelector<HTMLElement>('[aria-label="Queue actions"]');
const menu = () => host.querySelector('[role="menu"]');
const items = () =>
  [...host.querySelectorAll('[role="menuitem"]')].map((item) => item.textContent ?? "");
const item = (label: string) =>
  [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((entry) =>
    entry.textContent?.startsWith(label),
  );

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("the transfers panel's overflow menu", () => {
  it("shows an HTTP origin without presenting it as a share download", async () => {
    await open([{
      ...row("http", "queued"),
      direction: null,
      from: { kind: "http", url: "https://example.com/http.bin" },
      to: { kind: "local", path: "/Users/dorian/Downloads" },
    }]);
    // The panel draws the reference's table now, which says where a row came
    // from by naming its origin host rather than by stamping a direction chip
    // on it. `This Mac` is what a local origin reads as, so its absence is the
    // half of this that says the HTTP row was not mistaken for a share copy.
    expect(host.textContent).toContain("example.com");
    expect(host.textContent).not.toContain("This Mac");
  });

  it("keeps only Close in the header and puts the queue's verbs behind one control", async () => {
    // The panel's own verb stays a button; everything that acts on the *queue*
    // moved. A header of five buttons is what this replaced. Add URL is the
    // toolbar's, not the panel's — putting it here meant the only route to the
    // HTTP downloader was to open the pane and spot a button in its header.
    await open([row("a", "failed")]);

    // The header's own: not a row's — a failed row has its own `Retry` — and
    // not the category strip's, which chooses what to look at rather than
    // acting on anything.
    const buttons = [...host.querySelectorAll("button")]
      .filter((button) => button.closest("[data-transfer]") === null)
      .filter((button) => button.getAttribute("role") !== "tab")
      .map((button) => button.getAttribute("aria-label") ?? button.textContent ?? "");
    expect(buttons).toEqual(["Queue actions", "Close"]);
    expect(menu()).toBeNull();
  });

  it("opens the shared menu, labelled, with every item present", async () => {
    await open([row("a", "failed"), row("b", "done")]);

    hamburger()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(menu()?.getAttribute("aria-label")).toBe("Queue actions");
    expect(hamburger()?.getAttribute("aria-expanded")).toBe("true");
    expect(items()).toEqual([
      "Retry failed (1)",
      "Clear failed (1)",
      "Clear finished (1)",
      "Pause all",
    ]);
  });

  it("runs the verb that was chosen and closes", async () => {
    await open([row("a", "failed")]);
    hamburger()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    item("Retry failed")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(chosen).toEqual(["retryFailed"]);
    expect(menu()).toBeNull();
  });

  it("greys a verb with nothing to act on rather than hiding it", async () => {
    // A menu whose rows move about between openings has to be re-read every
    // time; one with a greyed row can be learned once.
    await open([row("a", "done")]);
    hamburger()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    const retry = item("Retry failed");
    expect(retry).toBeDefined();
    expect(retry?.getAttribute("aria-disabled")).toBe("true");
    expect(retry?.getAttribute("title")).toBe("Nothing has failed");

    retry?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(chosen).toEqual([]);
    expect(menu()).not.toBeNull();
  });

  it("closes on a second press of the control that opened it", async () => {
    // The window's own dismissal fires on `pointerdown` anywhere outside the
    // menu, and the button is outside it — so without the menu knowing what it
    // hangs off, the press that should close it closes it and the button's own
    // click opens it straight back.
    await open([row("a", "failed")]);
    hamburger()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    expect(menu()).not.toBeNull();

    hamburger()?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    hamburger()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    expect(menu()).toBeNull();
    expect(hamburger()?.getAttribute("aria-expanded")).toBe("false");
    expect(chosen).toEqual([]);
  });

  it("is reachable and dismissable from the keyboard (E16-S2)", async () => {
    await open([row("a", "failed"), row("b", "done")]);
    hamburger()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    // Arrows skip nothing here, because everything is available — the point is
    // that the shared menu's own key handling is what runs.
    menu()?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    await settle();
    expect(menu()?.getAttribute("aria-activedescendant")).toBe("context-menu-item-0");
    menu()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await settle();
    expect(chosen).toEqual(["retryFailed"]);

    hamburger()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();
    menu()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    expect(menu()).toBeNull();
    expect(chosen).toEqual(["retryFailed"]);
  });
});
