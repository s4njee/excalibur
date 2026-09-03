// Where the window is looking, and how it got there.
//
// Paths here are share-relative and slash-separated, with `""` for the share
// root — the same shape `ShareBackend::list_dir` takes, so nothing has to be
// translated on the way to the engine. There is no leading slash: `""` and
// `"/"` would otherwise be two spellings of the root, and every comparison
// would have to normalise first.
//
// The history is a stack and an index rather than two stacks. Back and forward
// are then just moves along one list, and the invariant that matters — going
// somewhere new discards the forward entries — is a truncation rather than a
// pair of pushes and clears that have to be kept in step.

/** A per-window browsing history. */
export interface History {
  /** Every path visited on this branch, oldest first. */
  stack: string[];
  /** Which entry of `stack` is on screen. */
  index: number;
}

/** A history sitting at one path with nothing behind or ahead of it. */
export function initial(path = ""): History {
  return { stack: [path], index: 0 };
}

/** The path currently on screen. */
export function current(history: History): string {
  return history.stack[history.index] ?? "";
}

export function canBack(history: History): boolean {
  return history.index > 0;
}

export function canForward(history: History): boolean {
  return history.index < history.stack.length - 1;
}

/**
 * Navigate somewhere, discarding anything that was ahead.
 *
 * Going to the path already on screen is a no-op rather than a new entry:
 * otherwise clicking the current breadcrumb, or re-selecting the folder you are
 * in, would fill the history with duplicates and make Back appear broken.
 */
export function go(history: History, path: string): History {
  if (current(history) === path) return history;
  const stack = [...history.stack.slice(0, history.index + 1), path];
  return { stack, index: stack.length - 1 };
}

export function back(history: History): History {
  return canBack(history) ? { ...history, index: history.index - 1 } : history;
}

export function forward(history: History): History {
  return canForward(history) ? { ...history, index: history.index + 1 } : history;
}

/** The parent of a path; the root is its own parent. */
export function parent(path: string): string {
  const cut = path.lastIndexOf("/");
  if (cut === -1) return "";
  return path.slice(0, cut);
}

/** Whether there is anywhere above this path to go. */
export function canAscend(path: string): boolean {
  return path !== "";
}

/** A path's components, for the breadcrumb. The root has none. */
export function segments(path: string): string[] {
  return path === "" ? [] : path.split("/");
}

/** The path of a child of `path`. */
export function join(path: string, name: string): string {
  return path === "" ? name : `${path}/${name}`;
}

/** The path formed by the first `count` segments, for a breadcrumb click. */
export function ancestor(path: string, count: number): string {
  return segments(path).slice(0, count).join("/");
}
