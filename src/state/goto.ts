// Typing a path instead of clicking to it (E26-S2's ⌘⇧G).
//
// The sheet is three lines of JSX around this module: a field, a completion and
// a navigation. What is actually interesting is here, because it is the part
// that can be wrong in ways a screenshot cannot show — a path that escapes the
// share, a completion that eats a component the user had already typed, or a
// second Tab that undoes the first.
//
// Paths are `navigation.ts`'s: share-relative, slash-separated, `""` for the
// root, no leading slash. A *typed* path is looser than that — a user may lead
// with `/` because the path field shows one, may leave a trailing slash because
// they are still typing, and may type `..` because every other path box takes
// it. `resolve` is the funnel between the two, and it is the only place that
// knows the difference.

import { fold } from "./filter";

/**
 * A typed path, as a share-relative one.
 *
 * A leading slash means the share root rather than the machine's — this window
 * has no way to name anything outside the mount, so `/etc` is `etc` inside the
 * share and not a second filesystem. `..` above the root stays at the root for
 * the same reason: there is nowhere above it to go, and silently producing
 * `../etc` would hand the engine a path it would have to refuse.
 */
export function resolve(input: string): string {
  const out: string[] = [];
  for (const segment of input.trim().split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      out.pop();
      continue;
    }
    out.push(segment);
  }
  return out.join("/");
}

/** The part of the input a completion replaces: the last component typed. */
export function stemOf(input: string): string {
  const text = input.trim();
  const cut = text.lastIndexOf("/");
  return cut === -1 ? text : text.slice(cut + 1);
}

/**
 * The folder whose listing can complete the stem.
 *
 * `docs/re` completes from `docs`, so completion works at any depth rather than
 * only in the directory the window happens to be showing — the sheet lists that
 * folder itself rather than reusing the window's entries.
 */
export function parentOf(input: string): string {
  const text = input.trim();
  const cut = text.lastIndexOf("/");
  return cut === -1 ? "" : resolve(text.slice(0, cut));
}

/** Everything before the stem, kept exactly as typed so a leading `/` survives. */
function prefixOf(input: string): string {
  const text = input.trim();
  const cut = text.lastIndexOf("/");
  return cut === -1 ? "" : text.slice(0, cut + 1);
}

/**
 * The folder names a stem could still become.
 *
 * Folded, so completion is case- and diacritic-insensitive on exactly the same
 * terms as the filter and type-to-select. One rule, one implementation.
 */
export function candidates(input: string, names: readonly string[]): string[] {
  const stem = fold(stemOf(input));
  return names.filter((name) => fold(name).startsWith(stem));
}

/** The longest start every candidate shares, compared literally. */
function commonPrefix(names: readonly string[]): string {
  if (names.length === 0) return "";
  let prefix = names[0] ?? "";
  for (const name of names.slice(1)) {
    let length = 0;
    while (length < prefix.length && length < name.length && prefix[length] === name[length]) {
      length += 1;
    }
    prefix = prefix.slice(0, length);
  }
  return prefix;
}

/**
 * What Tab should put in the field, or `null` when it can add nothing.
 *
 * One candidate completes the whole component and adds the separator, because
 * the next thing a user does after completing a folder is descend into it. More
 * than one extends as far as they agree and stops — the shell rule, and the one
 * that never throws away a keystroke: a completion that picked the first match
 * would make Tab a gamble rather than a shortcut.
 *
 * `null` rather than the unchanged input, so the caller can tell "nothing
 * happened" from "something did" without comparing strings.
 */
export function complete(input: string, names: readonly string[]): string | null {
  const matching = candidates(input, names);
  if (matching.length === 0) return null;
  const stem = stemOf(input);
  if (matching.length === 1) {
    const only = matching[0] as string;
    return `${prefixOf(input)}${only}/`;
  }
  const shared = commonPrefix(matching);
  // Shorter than what is typed only when the candidates disagree in case; the
  // typed text is then already as far as agreement goes.
  if (shared === stem || shared.length < stem.length) return null;
  return `${prefixOf(input)}${shared}`;
}
