// What the transfers panel's overflow menu offers.
//
// *Asked for directly, 2026-09-07: "there should probably be a hamburger menu
// or similar that has clear failed, retry failed".*
//
// A pure function over the queue, for the same reason `menu.ts` is one: what a
// menu offers is a rule, and a rule is far cheaper to state and to test here
// than by opening a menu and reading it. The panel's header then has nothing to
// decide — it draws these rows through the same `ContextMenu` the file list
// uses, which is where Escape, the arrow keys and the focus ring already live.
//
// Two rules the counts encode, and both are about the difference between the
// verbs rather than about tidiness:
//
//   1. **Failed and finished are cleared separately.** A failed row carries a
//      `Retry`, so sweeping it away with the done ones would throw away a
//      choice the user has not made yet — which is why `Status::finished()` in
//      the engine still means done-or-cancelled and this menu has two items
//      instead of one.
//   2. **An item whose count is zero is greyed, never hidden.** A menu whose
//      rows move about between openings has to be re-read every time; one with
//      a greyed row can be learned once. The count is the whole answer to "will
//      this do anything", so it is in the label rather than in a tooltip.

import type { MenuItem } from "./menu";
import type { Transfer } from "./types";

/** Everything the transfers panel's menu can ask the window to do. */
export type TransferMenuAction =
  | "retryFailed"
  | "clearFailed"
  | "clearFinished"
  | "pauseAll";

/** How many rows each item would act on. */
export interface TransferCounts {
  failed: number;
  finished: number;
  unfinished: number;
}

/**
 * What each verb would take, counted once.
 *
 * `unfinished` is queued *and* running, matching `pause_all` in the engine: a
 * queued transfer that is paused stays paused, so it is as much a thing the
 * verb acts on as one that is moving.
 */
export function transferCounts(transfers: Transfer[]): TransferCounts {
  return {
    failed: transfers.filter((transfer) => transfer.status === "failed").length,
    finished: transfers.filter(
      (transfer) => transfer.status === "done" || transfer.status === "cancelled",
    ).length,
    unfinished: transfers.filter(
      (transfer) => transfer.status === "queued" || transfer.status === "running",
    ).length,
  };
}

/**
 * The menu for a queue in this state.
 *
 * The order is deliberate: the two failure verbs first, because a queue with
 * failures in it is the only reason anyone opens this menu — the panel already
 * says how many there are in its footer — and the destructive one second, so
 * that `Retry` is what a pointer travelling down the list reaches first.
 */
export function transferMenu(transfers: Transfer[]): MenuItem<TransferMenuAction>[] {
  const counts = transferCounts(transfers);
  return [
    item("retryFailed", "Retry failed", counts.failed, "Nothing has failed"),
    item("clearFailed", "Clear failed", counts.failed, "Nothing has failed"),
    {
      ...item("clearFinished", "Clear finished", counts.finished, "Nothing has finished"),
      separated: true,
    },
    {
      ...item("pauseAll", "Pause all", counts.unfinished, "Nothing is transferring"),
      separated: true,
    },
  ];
}

/**
 * One row, refused when it would act on nothing.
 *
 * The count goes in parentheses rather than into the sentence — `Retry failed
 * (260)` — because the four labels are read as a column and a verb that moved
 * about to make room for its number would be harder to scan than the number is
 * worth. A refused row shows the plain verb: there is no count to give.
 */
function item(
  action: TransferMenuAction,
  label: string,
  count: number,
  refusal: string,
): MenuItem<TransferMenuAction> {
  if (count === 0) return { action, label, disabled: true, reason: refusal };
  return { action, label: `${label} (${count})`, disabled: false, reason: null };
}
