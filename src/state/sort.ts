// How the list is ordered.
//
// Kept apart from the component because ordering is the kind of thing that
// looks obviously right and is quietly wrong: a comparator that is not a total
// order makes rows jump between renders, and it is far easier to see that in a
// test than in a window.

import type { Entry, SortColumn } from "./types";

export interface Sort {
  column: SortColumn;
  ascending: boolean;
}

export const DEFAULT_SORT: Sort = { column: "name", ascending: true };

/**
 * What clicking a column header should do.
 *
 * Clicking the current column flips it. Clicking a new one starts from the
 * direction that column is usually wanted in: names and kinds read A–Z, but a
 * first click on Size or Modified almost always means "biggest" or "newest",
 * and starting those ascending makes every user click twice.
 */
export function toggle(current: Sort, column: SortColumn): Sort {
  if (current.column === column) return { column, ascending: !current.ascending };
  return { column, ascending: column === "name" || column === "kind" };
}

/** Natural order, so `File 10` follows `File 9` rather than `File 1`. */
function byName(left: Entry, right: Entry): number {
  return left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: "base" });
}

function value(entry: Entry, column: SortColumn): number | string {
  switch (column) {
    case "size":
      return entry.size;
    case "modified":
      // A missing timestamp sorts as the oldest thing there is rather than
      // as 1970, which is a real date some file could legitimately have.
      return entry.modifiedMs ?? Number.NEGATIVE_INFINITY;
    case "kind":
      return entry.kindLabel;
    case "name":
      return entry.name;
  }
}

/**
 * Order a listing.
 *
 * Ties break by name rather than by the order the backend happened to return.
 * The AC calls for a *stable* sort, and this is stable in the sense that
 * matters — sorting the same listing twice gives the same answer — but it does
 * not preserve the backend's order on ties, because that order is arbitrary and
 * can differ between two listings of an unchanged directory. Sorting by Size
 * with every folder at zero would otherwise shuffle the folders on each
 * refresh.
 */
export function sortEntries(entries: Entry[], sort: Sort, foldersFirst: boolean): Entry[] {
  const direction = sort.ascending ? 1 : -1;
  return entries.slice().sort((left, right) => {
    if (foldersFirst) {
      const leftIsDir = left.kind === "dir";
      const rightIsDir = right.kind === "dir";
      // Folders lead in both directions: the preference is "folders first",
      // not "folders sort before files", so reversing must not sink them.
      if (leftIsDir !== rightIsDir) return leftIsDir ? -1 : 1;
    }
    if (sort.column === "name") return direction * byName(left, right);

    const a = value(left, sort.column);
    const b = value(right, sort.column);
    if (a !== b) {
      const order = typeof a === "number" && typeof b === "number"
        ? a - b
        : String(a).localeCompare(String(b), undefined, { sensitivity: "base" });
      return direction * order;
    }
    return byName(left, right);
  });
}
