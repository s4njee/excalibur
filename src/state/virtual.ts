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

/**
 * How many cells fit across a wrapping grid (E26-S4).
 *
 * At least one, always: a list narrower than a cell still has to draw them,
 * and a zero would make the row count infinite. An unmeasured width — the
 * `Infinity` the list starts at before its first layout — answers one for the
 * same reason, and the real number arrives with the resize observer.
 */
export function itemsPerRow(width: number, cellWidth: number): number {
  if (!Number.isFinite(width) || width <= 0 || cellWidth <= 0) return 1;
  return Math.max(1, Math.floor(width / cellWidth));
}

/**
 * Which cells to render for a wrapping grid, as cell indices.
 *
 * The same arithmetic as `visibleRange`, done in grid rows and then multiplied
 * back out: a grid scrolls by row, so the window has to start and end on a row
 * boundary or the first drawn cell lands in the wrong column.
 */
export function gridRange(
  scrollTop: number,
  viewportHeight: number,
  cellHeight: number,
  perRow: number,
  cells: number,
  overscan: number = OVERSCAN,
): Range {
  if (cells <= 0 || perRow <= 0 || cellHeight <= 0) return { start: 0, end: 0 };
  const rows = visibleRange(
    scrollTop,
    viewportHeight,
    cellHeight,
    Math.ceil(cells / perRow),
    overscan,
  );
  return {
    start: Math.min(cells, rows.start * perRow),
    end: Math.min(cells, rows.end * perRow),
  };
}

/**
 * Which cells a rubber band covers in a wrapping grid (E26-S4).
 *
 * `rowsInBand` cannot answer this. A band over rows is a vertical extent and
 * so a contiguous range of indices; a band over a grid is a *rectangle*, and
 * the cells it covers are a block — rows 2 to 4, columns 1 to 3 — which is not
 * contiguous in index order at all.
 *
 * Intersection, not containment: dragging across the corner of a cell selects
 * it, the way it does in every file manager. A band with no area selects
 * nothing, so a click that never moved stays a click.
 */
export function cellsInBand(
  from: { x: number; y: number },
  to: { x: number; y: number },
  cellWidth: number,
  cellHeight: number,
  perRow: number,
  count: number,
): number[] {
  if (count <= 0 || perRow <= 0 || cellWidth <= 0 || cellHeight <= 0) return [];
  const left = Math.min(from.x, to.x);
  const right = Math.max(from.x, to.x);
  const top = Math.min(from.y, to.y);
  const bottom = Math.max(from.y, to.y);
  if (right === left || bottom === top) return [];

  const rows = Math.ceil(count / perRow);
  // `ceil(edge) - 1` rather than `floor(edge)`: a band whose edge lands exactly
  // on a boundary touches the cell before it, not the one after.
  const firstRow = Math.max(0, Math.floor(top / cellHeight));
  const lastRow = Math.min(rows - 1, Math.ceil(bottom / cellHeight) - 1);
  const firstColumn = Math.max(0, Math.floor(left / cellWidth));
  const lastColumn = Math.min(perRow - 1, Math.ceil(right / cellWidth) - 1);

  const covered: number[] = [];
  for (let row = firstRow; row <= lastRow; row += 1) {
    for (let column = firstColumn; column <= lastColumn; column += 1) {
      const index = row * perRow + column;
      // The last row is usually short; it has no cells past the count.
      if (index < count) covered.push(index);
    }
  }
  return covered;
}
