// Which destinations a move may not have, and where each item lands (E14-S3).
//
// A module rather than conditions inside the picker, because these are the
// part of the feature that can be wrong invisibly. A component test can only
// ask whether the button was disabled; it cannot say *why*, and the case that
// actually corrupts a share — a folder moved inside itself — sits three clicks
// down a tree that the test would have to build to reach. Here it is one call.
//
// Paths are share-relative, slash-separated, with no leading slash and `""`
// for the share root: `navigation.ts`'s shape, whose helpers do the splitting
// so there is one definition of what a segment is.

import { join, segments } from "./navigation";
import type { Entry } from "./types";

/**
 * Whether `path` is `folder` itself, or anything below it.
 *
 * Compared segment by segment, because the obvious spelling is wrong:
 * `path.startsWith(folder)` calls `"docs-old"` a child of `"docs"`, and the
 * move that check was meant to refuse would go through. Comparing whole
 * segments has no such neighbourhood.
 *
 * The root contains everything, which falls out of this rather than being a
 * case: it has no segments, so every path matches all of them.
 */
function isAtOrUnder(path: string, folder: string): boolean {
  const inner = segments(path);
  const outer = segments(folder);
  if (inner.length < outer.length) return false;
  return outer.every((segment, index) => inner[index] === segment);
}

/**
 * Why `destination` cannot take these entries, or null when it can.
 *
 * The string is shown to the user verbatim, so it is a sentence and it names
 * the folder that is the problem — "that folder is invalid" tells someone
 * looking at a tree of folders nothing at all.
 *
 * Entries are checked whatever their `kind`. Only a directory can really
 * contain the destination, but a rule that first decides what counts as a
 * directory is a rule with a second way to be wrong; if a path we are moving
 * ever does turn out to sit above the destination, the answer is no
 * regardless of what the engine called it.
 */
export function illegalDestination(
  entries: Entry[],
  from: string,
  destination: string,
): string | null {
  if (destination === from) {
    const only = entries.length === 1 ? entries[0] : undefined;
    return only
      ? `“${only.name}” is already in this folder.`
      : `These ${entries.length} items are already in this folder.`;
  }

  for (const entry of entries) {
    if (destination === entry.path) {
      return `You cannot move “${entry.name}” into itself.`;
    }
    if (isAtOrUnder(destination, entry.path)) {
      return `You cannot move “${entry.name}” into a folder inside it.`;
    }
  }

  return null;
}

/**
 * Where one entry lands under `destination`.
 *
 * By name, not by path: a move keeps the leaf and replaces everything above
 * it. Carrying the old path's parents along is how `a/b/c.txt` moved to `d`
 * becomes `d/a/b/c.txt`.
 */
export function destinationOf(entry: Entry, destination: string): string {
  return join(destination, entry.name);
}
