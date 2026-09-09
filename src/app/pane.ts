// One pane: a place in a share, and what is selected there (XTP-E1-S1).
//
// The window used to hold exactly one of each of these as loose signals, which
// is why it could only ever show one directory. Two panes needs two of them,
// so they are gathered into something a window can have more than one of.
//
// This ships no visible change: with one pane the window behaves exactly as it
// did, and the suite that says so is unchanged. That is the point — the value
// is in what it makes possible, and the way to be sure nothing was broken on
// the way is that nothing had to be adjusted to fit.
//
// A composable rather than a pure module, so it lives beside the others in
// `src/app/` rather than in `src/state/`, where every file is pure and has a
// sibling test. `twopane.md` says `state/pane.ts`; it was written before
// `src/app/` existed.
//
// What is deliberately *not* here: sort, the column widths, hidden files and
// folders-first. Those are persisted settings and they belong to the window —
// two panes sorted differently would be two answers to one question the
// toolbar can only ask once (`twopane.md` XTP-E7-S1).

import { createSignal, onCleanup } from "solid-js";
import { createListing } from "../state/listing";
import { coalesce } from "../state/refresh";
import { EMPTY_SELECTION, type Selection } from "../state/selection";
import * as nav from "../state/navigation";
import type { Listing } from "../state/types";

export interface PaneOptions {
  /** Where the pane opens. A ⌘N window inherits the folder, not the trail. */
  path?: string;
  listDir: (path: string) => Promise<Listing>;
  /** The browser preview opens with a row selected; the app does not. */
  selection?: Selection;
}

export type Pane = ReturnType<typeof createPane>;

/**
 * Which pane, spatially.
 *
 * Not `local`/`remote`, which is the handoff's framing: it draws a local tree
 * beside a remote one, but either pane here can hold either — a window with two
 * shares open, or two local folders, is an ordinary thing to want. Naming them
 * by their contents would be wrong the first time somebody did that.
 */
export type PaneSide = "left" | "right";

export function createPane(options: PaneOptions) {
  // A window opened by ⌘N starts in the folder the window that opened it was
  // showing, with no history behind it: Back in a new window has nowhere to go,
  // and inheriting somebody else's trail would give it somewhere wrong.
  const [history, setHistory] = createSignal<nav.History>(nav.initial(options.path ?? ""));
  const path = () => nav.current(history());

  const listingState = createListing(options.listDir);
  const listing = listingState.listing;

  const refresh = coalesce(() => listingState.load(path()));
  onCleanup(() => refresh.dispose());

  /**
   * Nothing is mounted: the engine refused the listing rather than returning
   * a directory (E1-S6).
   *
   * Deliberately not "there is no listing yet", which is also true for the
   * frame before the first request settles — an empty state that flashed
   * before every window would be worse than none. `error` is set only once a
   * request has come back, and a *failed navigation* on a live mount leaves
   * the previous listing in place, so it does not land here either.
   */
  const disconnected = () => listing() === null && listingState.error() !== null;

  const goTo = (target: string) => setHistory((current) => nav.go(current, target));
  const ascend = () => goTo(nav.parent(path()));

  const [filter, setFilter] = createSignal("");
  const [selection, setSelection] = createSignal<Selection>(
    options.selection ?? EMPTY_SELECTION,
  );
  /** The paths a row asks about, which is all the list needs of a selection. */
  const selected = () => selection().paths as Set<string>;

  return {
    history,
    setHistory,
    path,
    listingState,
    listing,
    refresh,
    disconnected,
    goTo,
    ascend,
    filter,
    setFilter,
    selection,
    setSelection,
    selected,
  };
}
