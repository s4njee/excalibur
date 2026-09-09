// How big the transfers pane is, and how a drag changes it (E13-S1).
//
// The pane is anchored to the top-right, under the toolbar, so it grows to the
// left and downwards: dragging its left edge left makes it wider, dragging its
// bottom edge down makes it taller. The arithmetic lives here rather than in
// the component because it is the part worth testing — the component's job is
// to measure the panel and the window and hand those numbers over.

export interface PaneSize {
  width: number;
  /**
   * `null` until the user drags the height.
   *
   * The pane is content-height by default, capped by its own `max-height`: a
   * queue of two rows should not draw a half-screen box of empty list. An
   * explicit height is the user having said otherwise, so it is remembered
   * separately rather than by writing today's content height to the store.
   */
  height: number | null;
}

/** Wide enough for the table's columns; the handoff's 560px sketch is narrower. */
export const DEFAULT_PANE_WIDTH = 720;

/** Below this the Host and Size columns collapse into each other. */
export const MIN_PANE_WIDTH = 460;

/** Two rows and the chrome around them; less is not a list. */
export const MIN_PANE_HEIGHT = 180;

export const DEFAULT_PANE_SIZE: PaneSize = { width: DEFAULT_PANE_WIDTH, height: null };

/** Room the pane may occupy: the window, less the chrome above and a margin. */
export interface Available {
  width: number;
  height: number;
}

const clamp = (value: number, low: number, high: number) =>
  Math.round(Math.min(Math.max(value, low), Math.max(low, high)));

/**
 * The size a drag lands on.
 *
 * `start` is what the panel measured when the drag began, so a height that was
 * `null` becomes a number the moment the user takes hold of the bottom edge —
 * the pane keeps the size it was already drawing rather than jumping.
 *
 * A drag that only moves horizontally leaves the height alone, `null`
 * included: taking hold of the left edge should not silently freeze a height
 * the user never asked to fix.
 */
export function resized(
  start: { width: number; height: number },
  delta: { x: number; y: number },
  edges: { left: boolean; bottom: boolean },
  available: Available,
): PaneSize {
  return {
    width: edges.left
      ? clamp(start.width - delta.x, MIN_PANE_WIDTH, available.width)
      : clamp(start.width, MIN_PANE_WIDTH, available.width),
    height: edges.bottom
      ? clamp(start.height + delta.y, MIN_PANE_HEIGHT, available.height)
      : null,
  };
}

/**
 * Bring a remembered size back inside a window that may have changed.
 *
 * A pane sized on a large display and reopened on a small one would otherwise
 * be wider than the window it is anchored in, with its left edge — and the
 * handle on it — off screen and unreachable.
 */
export function clampToWindow(size: PaneSize, available: Available): PaneSize {
  return {
    width: clamp(size.width, MIN_PANE_WIDTH, available.width),
    height: size.height === null ? null : clamp(size.height, MIN_PANE_HEIGHT, available.height),
  };
}

/**
 * Read a size back from storage.
 *
 * The file is user-editable and survives upgrades, so a missing, non-numeric
 * or absurd value falls back per field rather than failing the load. `height`
 * is the one field whose `null` is meaningful, so it is kept.
 */
export function sanitise(raw: unknown): PaneSize {
  const record = typeof raw === "object" && raw !== null ? (raw as Record<string, unknown>) : {};
  const width = record.width;
  const height = record.height;
  return {
    width:
      typeof width === "number" && Number.isFinite(width) && width >= MIN_PANE_WIDTH
        ? Math.round(width)
        : DEFAULT_PANE_WIDTH,
    height:
      typeof height === "number" && Number.isFinite(height) && height >= MIN_PANE_HEIGHT
        ? Math.round(height)
        : null,
  };
}

/**
 * The docked drawer's content height, in pixels (`twopane.md` XTP-E4-S1).
 *
 * The same pane, docked rather than floating: with two panes a sheet over the
 * window covers the very columns the transfer is between, so the two-pane view
 * puts the queue below them and gives it the width instead.
 *
 * `0` is collapsed — the tab strip stays, which is what makes it recoverable
 * without a menu row, and is why this is one number rather than a height and a
 * flag that could disagree.
 */
export const DEFAULT_DRAWER_HEIGHT = 176;

/** Under this a drag means "put it away": a header, and no rows worth reading. */
export const MIN_DRAWER_HEIGHT = 88;

/** The drawer never takes more than this share of the room above it. */
export const MAX_DRAWER_FRACTION = 0.75;

/**
 * Where a drag on the drawer's top edge lands.
 *
 * The drawer is anchored to the bottom and grows upward, so a *negative*
 * delta — the pointer moving up the screen — makes it taller. `available` is
 * the room its column has; without it a drag can push the panes off the top,
 * and there is no edge left to grab.
 */
export function resizedDrawer(start: number, delta: number, available: number): number {
  const height = Math.round(start - delta);
  if (height < MIN_DRAWER_HEIGHT) return 0;
  return Math.min(height, Math.max(0, Math.round(available * MAX_DRAWER_FRACTION)));
}

/** One remembered drawer height, read back from a user-editable file. */
export function sanitiseDrawerHeight(raw: unknown): number {
  if (typeof raw !== "number" || !Number.isFinite(raw)) return DEFAULT_DRAWER_HEIGHT;
  // `0` is the user having collapsed it, so it survives the clamp that would
  // otherwise round it up to the minimum and reopen the drawer every launch.
  if (raw <= 0) return 0;
  return Math.max(MIN_DRAWER_HEIGHT, Math.round(raw));
}
