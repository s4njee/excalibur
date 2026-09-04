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

/** The CSS `grid-template-columns` these widths describe. */
export function template(widths: Widths): string {
  return `1fr ${widths.size}px ${widths.modified}px ${widths.kind}px`;
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
