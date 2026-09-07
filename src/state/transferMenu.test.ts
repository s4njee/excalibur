// What the transfers panel's overflow menu offers.
//
// The property worth holding is that the menu's shape does not move: the same
// four rows in the same order however the queue is doing, with a count when
// there is something to act on and a greyed row and a reason when there is not.
// So the empty queue is asserted as the whole list rather than item by item —
// an item that started hiding itself is the failure that would otherwise pass.

import { describe, expect, it } from "vitest";
import { transferCounts, transferMenu, type TransferMenuAction } from "./transferMenu";
import type { Transfer, TransferStatus } from "./types";

function transfer(id: string, status: TransferStatus): Transfer {
  return {
    id,
    direction: "down",
    name: id,
    sourcePath: `remote/${id}`,
    destPath: `/local/${id}`,
    bytesDone: 0,
    bytesTotal: 100,
    rate: null,
    etaSeconds: null,
    status,
    error: status === "failed" ? "connection lost: not connected" : null,
    queuedAtMs: 0,
  };
}

const byAction = (items: { action: TransferMenuAction }[]) => items.map((item) => item.action);
const find = (items: ReturnType<typeof transferMenu>, action: TransferMenuAction) =>
  items.find((item) => item.action === action);

const SHAPE: TransferMenuAction[] = ["retryFailed", "clearFailed", "clearFinished", "pauseAll"];

describe("the transfers panel's menu", () => {
  it("offers the same four items in the same order, whatever the queue holds", () => {
    // The rule the design argument rests on: a menu whose rows appear and
    // disappear has to be re-read on every opening. This one can be learned.
    expect(byAction(transferMenu([]))).toEqual(SHAPE);
    expect(
      byAction(
        transferMenu([
          transfer("a", "failed"),
          transfer("b", "done"),
          transfer("c", "running"),
        ]),
      ),
    ).toEqual(SHAPE);
  });

  it("refuses every item on an empty queue, and says why", () => {
    const items = transferMenu([]);
    expect(items.every((item) => item.disabled)).toBe(true);
    expect(find(items, "retryFailed")?.reason).toBe("Nothing has failed");
    expect(find(items, "clearFailed")?.reason).toBe("Nothing has failed");
    expect(find(items, "clearFinished")?.reason).toBe("Nothing has finished");
    expect(find(items, "pauseAll")?.reason).toBe("Nothing is transferring");
    // A refused row shows the plain verb: there is no count to give.
    expect(find(items, "retryFailed")?.label).toBe("Retry failed");
  });

  it("counts what each verb would take", () => {
    // The user's own queue, in miniature: a wall of failures, one success, and
    // something still moving.
    const queue = [
      ...Array.from({ length: 260 }, (_, index) => transfer(`bad-${index}`, "failed")),
      transfer("good", "done"),
      transfer("gone", "cancelled"),
      transfer("moving", "running"),
      transfer("next", "queued"),
      transfer("held", "paused"),
    ];
    const items = transferMenu(queue);

    expect(find(items, "retryFailed")?.label).toBe("Retry failed (260)");
    expect(find(items, "clearFailed")?.label).toBe("Clear failed (260)");
    // Done *and* cancelled, which is what the engine's `clear_finished` takes.
    expect(find(items, "clearFinished")?.label).toBe("Clear finished (2)");
    // Queued and running. A paused row stays paused, and is not something
    // `Pause all` acts on.
    expect(find(items, "pauseAll")?.label).toBe("Pause all (2)");
    expect(items.every((item) => item.disabled)).toBe(false);
  });

  it("keeps failed and finished apart", () => {
    // The reason there are two clearing verbs rather than one. A failed row
    // carries a Retry button; clearing it with the done ones would throw away
    // the thing the user is about to act on, which is why the engine's
    // `Status::finished()` does not include it either.
    const items = transferMenu([transfer("a", "failed")]);
    expect(find(items, "clearFailed")?.disabled).toBe(false);
    expect(find(items, "clearFinished")?.disabled).toBe(true);

    const done = transferMenu([transfer("b", "done")]);
    expect(find(done, "clearFailed")?.disabled).toBe(true);
    expect(find(done, "clearFinished")?.disabled).toBe(false);
  });

  it("counts the queue once, for the panel and the menu to agree", () => {
    const counts = transferCounts([
      transfer("a", "failed"),
      transfer("b", "done"),
      transfer("c", "cancelled"),
      transfer("d", "running"),
      transfer("e", "queued"),
      transfer("f", "paused"),
    ]);
    expect(counts).toEqual({ failed: 1, finished: 2, unfinished: 2 });
  });

  it("offers every action the type allows, somewhere", () => {
    // The accretion guard `menu.test.ts` carries, for the same reason: an
    // action added to the union and to the panel's switch but to no menu is a
    // verb nobody can reach and a branch nobody runs.
    const all: Record<TransferMenuAction, true> = {
      retryFailed: true,
      clearFailed: true,
      clearFinished: true,
      pauseAll: true,
    };
    expect(byAction(transferMenu([])).sort()).toEqual(Object.keys(all).sort());
  });
});
