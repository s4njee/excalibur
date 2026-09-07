// The transfers panel does not render an unbounded queue.
//
// This is a boot bug, not a scrolling one. The panel opens by itself when the
// queue is not empty, every row carries a progress bar and a caption, and the
// list is a plain `For` — so a queue of a few thousand rows was a window that
// stayed blank for seconds before its first paint. Bounded rather than
// virtualised because these rows are not one height: a finished row drops its
// bar, so there is no single row height to do the arithmetic with.

import { describe, expect, it, afterEach, beforeEach } from "vitest";
import { render } from "solid-js/web";
import { Transfers } from "./Transfers";
import type { Transfer } from "../state/types";

let host: HTMLDivElement;
let dispose: (() => void) | undefined;

const settle = () => new Promise((done) => setTimeout(done, 0));

function queue(count: number): Transfer[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `row${index}`,
    direction: "down" as const,
    name: `file-${index}.bin`,
    sourcePath: `Show/file-${index}.bin`,
    destPath: `/Users/dorian/Downloads/file-${index}.bin`,
    bytesDone: 0,
    bytesTotal: 1024,
    rate: null,
    etaSeconds: null,
    status: "queued" as const,
    error: null,
    queuedAtMs: index,
  }));
}

function open(transfers: Transfer[]) {
  dispose = render(
    () => (
      <Transfers
        transfers={transfers}
        onPause={() => undefined}
        onCancel={() => undefined}
        onRetry={() => undefined}
        onClearFinished={() => undefined}
        onPauseAll={() => undefined}
        onClose={() => undefined}
        onRemount={() => undefined}
      />
    ),
    host,
  );
  return settle();
}

const drawn = () => host.querySelectorAll("[data-transfer]").length;
const showAll = () =>
  [...host.querySelectorAll("button")].find((b) => b.textContent?.startsWith("Show "));

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  host.remove();
});

describe("a very long queue", () => {
  it("draws a bounded number of rows and says how many it withheld", async () => {
    await open(queue(2253));

    // The exact cap is not the property — that it is bounded well below the
    // queue is. A panel that renders two thousand rows with two thousand
    // progress bars is the blank window this test exists to prevent.
    expect(drawn()).toBeLessThan(500);
    expect(showAll()?.textContent).toContain("2053");
  });

  it("draws all of them once the user asks", async () => {
    await open(queue(2253));
    const bounded = drawn();

    showAll()?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await settle();

    // The click is the user accepting the cost, so nothing is withheld after
    // it — and the offer goes away rather than staying to be clicked again.
    expect(drawn()).toBeGreaterThan(bounded);
    expect(showAll()).toBeUndefined();
  });

  it("leaves an ordinary queue completely alone", async () => {
    // The common case must not pay for the pathological one: no cap row, and
    // every transfer drawn.
    await open(queue(12));

    expect(showAll()).toBeUndefined();
    expect(host.textContent).toContain("file-11.bin");
  });
});
