// Which rows are worth putting in the DOM.
//
// The design asks for 100k rows at 60fps, and 100k row elements is not that
// however they are styled. Every row is the same height, so the visible slice
// is arithmetic rather than measurement — no observers, no per-row bookkeeping.

/** Rows kept above and below the viewport so a fast scroll does not show gaps. */
export const OVERSCAN = 8;

export interface Range {
  /** First index to render. */
  start: number;
  /** One past the last index to render. */
  end: number;
}

/**
 * The slice to render for a scroll position.
 *
 * Clamped to the list at both ends, and never inverted, so a viewport measured
 * before layout (height 0) or a scroll position left over from a longer listing
 * both produce an empty or partial range rather than a negative one.
 */
export function visibleRange(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  count: number,
  overscan: number = OVERSCAN,
): Range {
  if (count <= 0 || rowHeight <= 0) return { start: 0, end: 0 };
  const first = Math.floor(Math.max(0, scrollTop) / rowHeight) - overscan;
  const last = Math.ceil((Math.max(0, scrollTop) + Math.max(0, viewportHeight)) / rowHeight) + overscan;
  const start = Math.min(count, Math.max(0, first));
  const end = Math.max(start, Math.min(count, last));
  return { start, end };
}

/**
 * The rows a rubber band covers.
 *
 * Takes content coordinates — scroll position already added — so a band that
 * was started before a scroll still covers what it visually covers. Works the
 * same whether the rows are in the DOM or not, which is the point: hit-testing
 * elements would only ever find the virtualised window.
 */
export function rowsInBand(
  fromY: number,
  toY: number,
  rowHeight: number,
  count: number,
): Range {
  if (count <= 0 || rowHeight <= 0) return { start: 0, end: 0 };
  const top = Math.min(fromY, toY);
  const bottom = Math.max(fromY, toY);
  // A band is a rectangle, so a row counts when the band overlaps it at all —
  // not when it covers the row's midpoint. Requiring the midpoint makes the
  // first row of a downward drag refuse to select until the pointer is 12px in.
  const start = Math.min(count, Math.max(0, Math.floor(top / rowHeight)));
  const end = Math.max(start, Math.min(count, Math.ceil(bottom / rowHeight)));
  return { start, end };
}
