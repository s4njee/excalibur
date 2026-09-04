import type { RowLayout } from "./types";
// Column widths for the `1f` layout.
//
// Name is `1fr` and absorbs whatever the other three do not take, so only
// three widths are stored. A divider sits between each pair of columns and
// moving it right grows the column on its left — the convention every table
// with draggable columns uses.

/** The three fixed columns. Name is `1fr` and has no stored width. */
export interface Widths {
  size: number;
  modified: number;
  kind: number;
}

/** The design's metrics, and what an unrecognised stored value falls back to. */
export const DEFAULT_WIDTHS: Widths = { size: 96, modified: 156, kind: 128 };

/**
 * Narrower than this and a column cannot show even a truncated value, so
 * dragging past it is treated as dragging to it.
 */
export const MIN_WIDTH = 48;

/** Which divider is being dragged, named for the column to its right. */
export type Divider = "size" | "modified" | "kind";

const clamp = (value: number) => Math.max(MIN_WIDTH, Math.round(value));

/**
 * Move one divider by `delta` pixels.
 *
 * Each divider adjusts at most the two columns it sits between, so a drag
 * cannot ripple across the whole header. The first one has only one fixed
 * column beside it — Name is `1fr` and takes up the slack by itself.
 */
export function resize(widths: Widths, divider: Divider, delta: number): Widths {
  switch (divider) {
    // Name | Size. Dragging right grows Name, which means Size shrinks.
    case "size":
      return { ...widths, size: clamp(widths.size - delta) };
    // Size | Modified.
    case "modified": {
      const size = clamp(widths.size + delta);
      // Take from Modified exactly what Size actually gained, so a clamped
      // column does not silently steal width from its neighbour.
      const applied = size - widths.size;
      return { ...widths, size, modified: clamp(widths.modified - applied) };
    }
    // Modified | Kind.
    case "kind": {
      const modified = clamp(widths.modified + delta);
      const applied = modified - widths.modified;
      return { ...widths, modified, kind: clamp(widths.kind - applied) };
    }
  }
}

/** Which optional columns a given list width has room for (E1-S4). */
export type VisibleColumns = { modified: boolean; kind: boolean };

/**
 * The width below which Kind is dropped.
 *
 * The handoff's number. It is a *list* width, not a window width, because the
 * sidebar collapsing gives the list ~200px back and the columns should take
 * it.
 */
export const KIND_DROPS_BELOW = 900;

/**
 * And Modified.
 *
 * The handoff says Kind drops "then Modified" without giving a second number.
 *
 * 560 is chosen so that a date survives at the handoff's "minimum useful
 * window" of ~820px. That is a *window* width and this is a *list* width: the
 * sidebar takes about 200px, so an 820px window leaves the list around 620px.
 * An earlier 700 here was picked by comparing the two directly, and dropped
 * Modified at exactly the size the handoff calls usable.
 *
 * At 560 the fixed columns and gaps take 284px, so Name still gets 276px —
 * enough to read a name by. Below that Name is the thing suffering, and it is
 * the column nobody would trade away.
 */
export const MODIFIED_DROPS_BELOW = 560;

/**
 * Which columns to draw at a given list width.
 *
 * Dropping rather than shrinking, per the handoff: a Modified column narrow
 * enough to fit at 700px shows a truncated date, which is worse than no date
 * because it looks like information.
 */
export function visibleColumns(listWidth: number): VisibleColumns {
  return {
    kind: listWidth >= KIND_DROPS_BELOW,
    modified: listWidth >= MODIFIED_DROPS_BELOW,
  };
}

/**
 * The CSS `grid-template-columns` these widths describe.
 *
 * The header and the rows both call this with the same arguments, which is
 * what keeps them aligned — a dropped column has to disappear from both or the
 * two grids stop agreeing about which cell is which.
 */
export function template(widths: Widths, visible: VisibleColumns = { modified: true, kind: true }): string {
  const columns = [`1fr`, `${widths.size}px`];
  if (visible.modified) columns.push(`${widths.modified}px`);
  if (visible.kind) columns.push(`${widths.kind}px`);
  return columns.join(" ");
}

/**
 * Read widths back from storage.
 *
 * Anything missing, non-numeric or below the minimum falls back to the design's
 * value for that column alone. A persisted file is user-editable and survives
 * upgrades, so it is not a trustworthy source of layout numbers.
 */
export function sanitise(raw: unknown): Widths {
  const record = typeof raw === "object" && raw !== null ? (raw as Record<string, unknown>) : {};
  const one = (key: keyof Widths) => {
    const value = record[key];
    return typeof value === "number" && Number.isFinite(value) && value >= MIN_WIDTH
      ? Math.round(value)
      : DEFAULT_WIDTHS[key];
  };
  return { size: one("size"), modified: one("modified"), kind: one("kind") };
}

/**
 * How tall one row is, per layout (E8-S3/S4).
 *
 * The single source: the virtual window does arithmetic on it, and the row
 * itself is sized from it inline. Previously this was a constant in
 * `Shell.tsx` that had to be kept in step with a `--height-row` token by hand
 * — two places to change, and three layouts would have made it six.
 *
 * `1g` is two lines: 6px padding, a 12.5px name and a 10px metadata line.
 */
export const ROW_HEIGHTS: Record<RowLayout, number> = {
  columns: 24,
  "name-led": 42,
  mono: 22,
};
