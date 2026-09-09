// One pane's directory tree: what has been listed, and what is open
// (`twopane.md` XTP-E2-S4).
//
// The cache is the reason this is a composable rather than a resource per node
// the way `ColumnView` does it. A tree is walked: folders are opened, closed
// and opened again, and re-listing a folder because the user collapsed it and
// changed their mind would put a network round trip behind a gesture that has
// none. So the listings live for as long as the pane is on the share, and the
// component below is left with nothing to remember.
//
// It hangs off the pane rather than the window because it *is* pane state, in
// exactly the way `history` and `selection` are: two panes on two shares have
// two trees, and a window has no answer for "which folders are open".

import { createSignal, untrack } from "solid-js";
import { engineFailure } from "../state/errors";
import { ancestorsOf, foldersOf, treeRows, type TreeNode, type TreeRow } from "../state/tree";
import type { Listing } from "../state/types";

export interface TreeOptions {
  /** Lists one folder of *this pane's* share. */
  list: (path: string) => Promise<Listing>;
  /**
   * A folder would not open, in the engine's words.
   *
   * A tree that failed in silence is a click that did nothing; a permission
   * that stops an expansion is worth the same banner a permission that stops
   * anything else gets.
   */
  onProblem?: (message: string) => void;
}

export type Tree = ReturnType<typeof createTree>;

export function createTree(options: TreeOptions) {
  const [loaded, setLoaded] = createSignal<ReadonlyMap<string, TreeNode[]>>(new Map());
  const [expanded, setExpanded] = createSignal<ReadonlySet<string>>(new Set());
  /** In flight, so a second click while a folder opens does not list it twice. */
  const pending = new Set<string>();
  /**
   * Which share the cache belongs to, as a number.
   *
   * A listing that comes back after the pane has moved to another share would
   * otherwise be filed under the new one, and the tree would show a folder
   * from somewhere else entirely.
   */
  let generation = 0;

  const forget = (path: string) =>
    setExpanded((current) => {
      const next = new Set(current);
      next.delete(path);
      return next;
    });

  const load = async (path: string) => {
    if (untrack(loaded).has(path) || pending.has(path)) return;
    pending.add(path);
    const mine = generation;
    try {
      // Untracked: `load` is reached from an effect on the pane's path, and
      // the list call reads the pane's mount — a dependency that would have
      // that effect re-run on a share change it is not about.
      const listing = await untrack(() => options.list(path));
      if (mine !== generation) return;
      setLoaded((current) => new Map(current).set(path, foldersOf(listing.entries)));
    } catch (error) {
      if (mine !== generation) return;
      // Left unlisted rather than cached as empty: the tree still does not
      // know what is in there, and `▶` says exactly that.
      forget(path);
      options.onProblem?.(engineFailure(error).message);
    } finally {
      pending.delete(path);
    }
  };

  /**
   * Take the folders of a listing the pane already has.
   *
   * The pane lists the folder it is in; the tree needs exactly that folder's
   * children too. Fetching them again would be a second round trip for an
   * answer already on screen — and worse, a second answer, which is how a tree
   * comes to disagree with the list beside it. Overwrites, because a refresh
   * is the newer truth.
   */
  const absorb = (path: string, entries: Listing["entries"]) =>
    setLoaded((current) => new Map(current).set(path, foldersOf(entries)));

  /** Open a folder, listing it if this is the first time. */
  const open = (path: string) => {
    setExpanded((current) => new Set(current).add(path));
    void load(path);
  };

  const toggle = (path: string) => {
    if (untrack(expanded).has(path)) forget(path);
    else open(path);
  };

  /**
   * Make `path` visible: its ancestors open, and the root listed.
   *
   * Called whenever the pane navigates, including by a click in the file list,
   * so the tree follows the pane rather than the pane following the tree. It
   * does not open `path` itself — walking into a folder is not the same
   * gesture as opening it, and unfolding it would push its siblings off a band
   * five rows tall.
   *
   * Reads through `untrack` because it is called from an effect on the pane's
   * path: the folders it opens are the folders it would otherwise re-read.
   */
  const reveal = (path: string) => {
    // The root is the one folder the tree may already have from the pane: when
    // the pane is *at* the root, `absorb` has it, and asking again would be a
    // request for something on screen.
    if (path !== "") void load("");
    for (const ancestor of ancestorsOf(path)) {
      if (!untrack(expanded).has(ancestor)) open(ancestor);
      else void load(ancestor);
    }
  };

  /** A different share: everything the tree knows was about the old one. */
  const reset = () => {
    generation += 1;
    pending.clear();
    setLoaded(new Map());
    setExpanded(new Set<string>());
  };

  const rows = (showHidden: boolean): TreeRow[] => treeRows(loaded(), expanded(), showHidden);

  return { rows, toggle, reveal, absorb, reset, expanded, loaded };
}
