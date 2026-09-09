// The shape of the directory tree (`twopane.md` XTP-E2-S4).
//
// The first tree in this app — `MoveTo.tsx:18` declined one, and column view
// draws ancestors rather than a hierarchy — so what a tree *is* is written
// here, as a rule over a cache and a set of open folders, rather than emerging
// from a component's markup.
//
// Two things are worth stating plainly, because both are places a tree usually
// goes wrong:
//
//   1. **Open and loaded are different facts.** A folder the user has just
//      opened has no children yet; its disclosure must still turn, or a click
//      that takes a network round trip looks like a click that did nothing.
//   2. **Not yet listed is not "no children".** An unlisted folder keeps its
//      `▶` — the tree does not know, and drawing a leaf would be a claim it
//      cannot support. `hasChildren` is `null` for exactly that case.

import type { Entry } from "./types";

/** A folder the tree can draw. Share-relative; `""` is the root, never a row. */
export interface TreeNode {
  path: string;
  name: string;
  hidden: boolean;
}

export interface TreeRow extends TreeNode {
  /** `0` for a child of the share root, which is where the tree starts. */
  depth: number;
  /** The user has opened it, whether or not its children have arrived. */
  expanded: boolean;
  /** `null` while the folder has not been listed: unknown, not empty. */
  hasChildren: boolean | null;
}

/** What one listing contributes to the tree: its directories, by name. */
export function foldersOf(entries: Entry[]): TreeNode[] {
  return entries
    .filter((entry) => entry.kind === "dir")
    .map((entry) => ({ path: entry.path, name: entry.name, hidden: entry.hidden }))
    .sort((left, right) =>
      // The list's own name comparison, so `Films 10` and `Films 9` fall the
      // same way in both. A tree is a navigator and keeps name order whatever
      // the list is sorted by — sorting folders by size is not a hierarchy.
      left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: "base" }),
    );
}

/**
 * Every folder between the share root and `path`, outermost first.
 *
 * `["Films", "Films/2016"]` for `Films/2016/Arrival` — the folders that have
 * to be open for that row to be on screen. The path itself is not one of them:
 * a folder is revealed by its ancestors, and opening it as well would expand a
 * folder the user only navigated into.
 */
export function ancestorsOf(path: string): string[] {
  if (path === "") return [];
  const parts = path.split("/").filter((part) => part !== "");
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join("/"));
}

/**
 * The rows to draw, top to bottom.
 *
 * `loaded` is the session cache: a folder's key is present once it has been
 * listed, and its value is every directory in it. `expanded` is what the user
 * has opened. A folder that is open but not yet in the cache draws its own row
 * and no children, which is the loading state — there is nothing else honest
 * to draw, and a spinner per row would be more chrome than the band is tall.
 */
export function treeRows(
  loaded: ReadonlyMap<string, TreeNode[]>,
  expanded: ReadonlySet<string>,
  showHidden: boolean,
): TreeRow[] {
  const visible = (node: TreeNode) => showHidden || !node.hidden;
  const rows: TreeRow[] = [];
  const walk = (parent: string, depth: number) => {
    for (const node of (loaded.get(parent) ?? []).filter(visible)) {
      const children = loaded.get(node.path);
      const open = expanded.has(node.path);
      rows.push({
        ...node,
        depth,
        expanded: open,
        hasChildren: children === undefined ? null : children.some(visible),
      });
      if (open && children !== undefined) walk(node.path, depth + 1);
    }
  };
  walk("", 0);
  return rows;
}

/** The band's height when it is opened, per the handoff's 104px. */
export const DEFAULT_TREE_HEIGHT = 104;
/**
 * Under this a drag means "put it away".
 *
 * Two rows and the padding around them. A band shorter than that shows one
 * folder of a hierarchy, which is not a tree; and having the drag collapse it
 * is what makes "collapsible to zero" a gesture rather than only a menu row.
 */
export const MIN_TREE_HEIGHT = 44;
/** Half of a small window's pane. The band is a navigator, not the view. */
export const MAX_TREE_HEIGHT = 320;

/** Where a drag on the band's bottom edge lands, `0` meaning collapsed. */
export function resizedBand(start: number, delta: number): number {
  const height = Math.round(start + delta);
  if (height < MIN_TREE_HEIGHT) return 0;
  return Math.min(MAX_TREE_HEIGHT, height);
}

/** One band's remembered height, read back from a user-editable file. */
export function sanitiseHeight(raw: unknown): number {
  if (typeof raw !== "number" || !Number.isFinite(raw)) return DEFAULT_TREE_HEIGHT;
  // `0` is a real answer — the band was put away — so it survives the clamp
  // that would otherwise round it up to the minimum.
  if (raw <= 0) return 0;
  return Math.min(MAX_TREE_HEIGHT, Math.max(MIN_TREE_HEIGHT, Math.round(raw)));
}
