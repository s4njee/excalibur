// What is selected, and how a click changes it.
//
// Pure functions over an explicit state, because selection is where the
// interesting bugs are: the anchor a shift-click extends from is invisible, so
// a wrong one is only noticeable three clicks later. Kept out of the component
// so the sequences can be written down as tests.
//
// Paths, not indices. The list re-sorts and re-filters underneath a selection,
// and an index would then point at a different file.

import { fold } from "./filter";

export interface Selection {
  /** The selected paths. `verbs and status counts derive from it`. */
  paths: ReadonlySet<string>;
  /**
   * The row a shift-click extends from.
   *
   * Not the same as "the last path clicked": a shift-click moves the far end of
   * the range without moving the anchor, so shift-clicking twice from one
   * starting row grows and shrinks the same range rather than walking away.
   */
  anchor: string | null;
}

export const EMPTY_SELECTION: Selection = { paths: new Set(), anchor: null };

export interface Modifiers {
  /** Extend from the anchor. */
  shift?: boolean;
  /** Add or remove one row (Cmd on macOS, Ctrl elsewhere). */
  toggle?: boolean;
}

/** The paths between two rows inclusive, in the order they are displayed. */
export function between(order: readonly string[], from: string, to: string): string[] {
  const start = order.indexOf(from);
  const end = order.indexOf(to);
  if (start === -1 || end === -1) return [to];
  return order.slice(Math.min(start, end), Math.max(start, end) + 1);
}

/** Apply a click to a selection. `order` is the paths as displayed. */
export function click(
  current: Selection,
  order: readonly string[],
  path: string,
  modifiers: Modifiers = {},
): Selection {
  if (modifiers.toggle) {
    const paths = new Set(current.paths);
    if (paths.has(path)) paths.delete(path);
    else paths.add(path);
    // The anchor follows a toggle, so a following shift-click extends from the
    // row the user last touched rather than from wherever they were before.
    return { paths, anchor: path };
  }

  if (modifiers.shift && current.anchor !== null) {
    // Replaces rather than unions: a shift-click means "the range is this",
    // so shrinking it by shift-clicking closer to the anchor has to work.
    return { paths: new Set(between(order, current.anchor, path)), anchor: current.anchor };
  }

  return { paths: new Set([path]), anchor: path };
}

/** Replace the selection with a rubber band's contents. */
export function band(
  current: Selection,
  covered: readonly string[],
  additive: boolean,
): Selection {
  // Holding the toggle modifier while banding adds to what was already there,
  // which is how a second group gets picked up without losing the first.
  const paths = additive ? new Set([...current.paths, ...covered]) : new Set(covered);
  return { paths, anchor: current.anchor ?? covered[0] ?? null };
}

/**
 * Where a keyboard move leaves the list.
 *
 * The row to scroll into view comes back with the selection because the two
 * answers differ: a plain move reveals the row it landed on, an extend reveals
 * the *far end* it just moved, and neither is the anchor. Deriving it in the
 * caller would mean re-deciding there what this module already decided.
 */
export interface Move {
  selection: Selection;
  /** The row index to reveal, or `null` when nothing moved. */
  reveal: number | null;
}

/**
 * The row the keyboard is standing on.
 *
 * The anchor, normally. Two cases are not: nothing is selected, which is `-1`
 * and lets ↓ mean "the first row" and ↑ "the last"; and an anchor that a
 * filter or a sort has taken off screen, where the last still-visible selected
 * row is the nearest honest answer — moving from a row the user cannot see
 * would look like the keys were ignored.
 */
function focusIndex(order: readonly string[], selection: Selection): number {
  if (selection.paths.size === 0) return -1;
  if (selection.anchor !== null) {
    const at = order.indexOf(selection.anchor);
    if (at !== -1) return at;
  }
  let last = -1;
  for (let index = 0; index < order.length; index += 1) {
    const path = order[index];
    if (path !== undefined && selection.paths.has(path)) last = index;
  }
  return last;
}

/**
 * The end of the selection a Shift-arrow moves.
 *
 * The selected row furthest from the anchor, which for a range built by
 * extending *is* the end that was last moved — so ⇧↓⇧↓⇧↑ grows twice and
 * shrinks once rather than jumping. A tie (a range that straddles the anchor
 * evenly, which only a click and a band can produce) takes the later row.
 */
export function farIndex(order: readonly string[], selection: Selection): number {
  const anchor = focusIndex(order, selection);
  if (anchor === -1) return -1;
  let far = anchor;
  let distance = 0;
  for (let index = 0; index < order.length; index += 1) {
    const path = order[index];
    if (path === undefined || !selection.paths.has(path)) continue;
    const away = Math.abs(index - anchor);
    if (away >= distance) {
      distance = away;
      far = index;
    }
  }
  return far;
}

function clamp(index: number, count: number): number {
  return Math.min(count - 1, Math.max(0, index));
}

/** Select one row by index, taking the anchor with it. Clamps at both ends. */
export function moveTo(current: Selection, order: readonly string[], index: number): Move {
  if (order.length === 0) return { selection: current, reveal: null };
  const at = clamp(index, order.length);
  const path = order[at];
  if (path === undefined) return { selection: current, reveal: null };
  return { selection: { paths: new Set([path]), anchor: path }, reveal: at };
}

/**
 * Extend the range to a row, leaving the anchor where it is.
 *
 * With nothing selected there is nothing to extend *from*, so Shift-arrow
 * behaves as the bare arrow does and selects one row — which is what makes the
 * first keystroke into a list do something rather than nothing.
 */
export function extendTo(current: Selection, order: readonly string[], index: number): Move {
  if (order.length === 0) return { selection: current, reveal: null };
  const at = clamp(index, order.length);
  const path = order[at];
  if (path === undefined) return { selection: current, reveal: null };
  const anchorIndex = focusIndex(order, current);
  const anchor = anchorIndex === -1 ? null : (order[anchorIndex] ?? null);
  if (anchor === null) return moveTo(current, order, at);
  return {
    selection: { paths: new Set(between(order, anchor, path)), anchor },
    reveal: at,
  };
}

/**
 * Move the selection by `delta` rows.
 *
 * With nothing selected, down selects the first row and up the last — the same
 * rule whether the step is one row or a page, because "there is nowhere to
 * move from, so enter the list at the near end" is one idea.
 */
export function step(current: Selection, order: readonly string[], delta: number): Move {
  if (order.length === 0) return { selection: current, reveal: null };
  const at = focusIndex(order, current);
  if (at === -1) return moveTo(current, order, delta > 0 ? 0 : order.length - 1);
  return moveTo(current, order, at + delta);
}

/** Extend the range by `delta` rows from the end that is not the anchor. */
export function extendBy(current: Selection, order: readonly string[], delta: number): Move {
  if (order.length === 0) return { selection: current, reveal: null };
  const far = farIndex(order, current);
  if (far === -1) return moveTo(current, order, delta > 0 ? 0 : order.length - 1);
  return extendTo(current, order, far + delta);
}

/** Where a type-ahead search should start from. */
export interface TypeAheadFrom {
  /** The row selected right now. */
  current: string | null;
  /**
   * The row that was selected when this typing burst began.
   *
   * Extending a buffer must *refine* the match, not step past it. Typing `b`
   * and then `l` means "the first thing starting with bl" — searching onward
   * from what `b` just landed on skips it and finds the second `bl` instead.
   */
  origin: string | null;
}

/**
 * Find the row type-to-select should jump to.
 *
 * Two behaviours share one function because a user does not distinguish them:
 *
 * - **Repeating a letter cycles.** `s`, `s`, `s` inside the typing window means
 *   "next thing starting with s", not "something starting with sss". So a
 *   buffer of one repeated character matches on a single instance and searches
 *   on from the current row.
 * - **Adding a letter refines.** `b` then `l` searches from where the burst
 *   began, so it lands on the first `bl` rather than the one after `b`'s match.
 *
 * Both wrap around the end of the list.
 */
export function typeAhead(
  order: readonly string[],
  names: readonly string[],
  buffer: string,
  from: TypeAheadFrom,
): string | null {
  if (buffer === "") return null;
  const repeated = [...buffer].every((character) => character === buffer[0]);
  const needle = fold(repeated ? buffer.slice(0, 1) : buffer);
  if (needle === "") return null;

  const anchor = repeated ? from.current : from.origin;
  const start = anchor === null ? -1 : order.indexOf(anchor);
  for (let step = 1; step <= order.length; step += 1) {
    const index = (start + step + order.length) % order.length;
    const name = names[index];
    if (name !== undefined && fold(name).startsWith(needle)) return order[index] ?? null;
  }
  return null;
}
