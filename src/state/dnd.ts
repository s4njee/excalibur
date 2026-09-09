// What a drop between two Excalibur windows actually does (E24-S2).
//
// A module rather than branches inside the drop handler, for the reason
// `move.ts` gives: this is the part of the feature that can be wrong
// invisibly. The visible half — a ring, a highlighted row — is a component
// test's business. The half that matters is *which verb runs*, and getting
// that wrong is a file moved off a share instead of copied onto one, or a
// folder moved inside itself. Both sit behind a gesture no unit test of a
// component can perform.
//
// Three rules, and only three:
//
//   1. **Same mount is a move.** `rename` on the share, E14-S3's rules
//      unchanged — including the one that refuses a folder into itself.
//   2. **Different mounts is a copy**, through the transfer queue.
//   3. **⌥ forces a copy**, even on the same mount.
//
// What makes this longer than three rules is that the queue can only do a copy
// with **the local filesystem on one side** (E22-S1/S2 are what change that),
// so rule 2 has a refusal in it. The refusal is a sentence rather than a
// silence: a drop that lands and does nothing is indistinguishable from a
// broken app, which is the same argument E13-S5 made about the sidebar.

import { illegalDestination } from "./move";
import { join, parent } from "./navigation";
import type { Entry, Protocol } from "./types";

/** How long a folder has to be hovered before a drag descends into it. */
export const SPRING_DELAY_MS = 600;

/** What one drag is carrying, as the pasteboard spells it. */
export interface DragSelection {
  /** The mount the paths are on. */
  mount: string;
  /** Share-relative paths, `navigation.ts`'s shape. */
  paths: string[];
  /** The Tauri label of the window the drag started in. */
  window: string;
}

/**
 * A mount, as a drop decision needs to know it.
 *
 * `root` is only meaningful for `local`, where it is the absolute folder the
 * share is rooted at — which is what turns a share-relative path into an OS
 * path, and an OS path is the only thing the transfer queue's two verbs take.
 */
export interface DropMount {
  id: string;
  protocol: Protocol;
  root: string;
}

/** Everything the answer depends on. */
export interface DropRequest {
  selection: DragSelection;
  /** The mount the drag came from, or `null` if this app no longer holds it. */
  from: DropMount | null;
  /** The mount being dropped onto. */
  to: DropMount;
  /** The folder being dropped into, share-relative on `to`. */
  path: string;
  /** Whether `to` accepts writes at all. */
  writable: boolean;
  /** ⌥ held at the moment of the drop. */
  alt: boolean;
}

/** One item, with the name it will land as. */
export interface DropItem {
  /** Share-relative source path. */
  path: string;
  name: string;
}

/**
 * What a drop would do.
 *
 * `none` is a drop that is legitimately nothing — released back in the folder
 * it started in — and draws no ring at all. `refused` is a drop that *would*
 * have meant something and cannot happen; it draws the ring with its reason,
 * because that is the case a user needs told about.
 */
export type DropPlan =
  | { verb: "move"; mount: string; destination: string; items: DropItem[] }
  | {
      verb: "copy";
      /**
       * Which of the queue's two verbs runs.
       *
       * `download` reads the share and writes the local filesystem, so it is
       * chosen whenever the *destination* is local — including for a copy
       * within one local share, and including local→local across two of them.
       *
       * It used to be the only one of the two that walked a folder, which was
       * a second reason to prefer it. Since E22-S2 both do: an upload is
       * planned by xsync rather than pre-walked here, so a dropped folder is
       * one row and the planner descends it.
       */
      direction: "download" | "upload";
      /** The share the bytes come from. */
      sourceMount: string;
      /** The share they go to. */
      destMount: string;
      /**
       * For `download`, the absolute local folder. For `upload`, the
       * share-relative folder on `destMount`.
       */
      destination: string;
      /**
       * For `download`, share-relative paths on `sourceMount`. For `upload`,
       * absolute local paths.
       */
      sources: string[];
      items: DropItem[];
    }
  | { verb: "none" }
  | { verb: "refused"; reason: string };

/** The leaf of a share-relative path, which is the name it keeps on a move. */
function leaf(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** A share-relative path on a local mount, as an absolute OS path. */
export function osPath(mount: DropMount, path: string): string {
  const root = mount.root.replace(/\/+$/, "");
  return path === "" ? root : `${root}/${path}`;
}

/**
 * `illegalDestination` speaks in `Entry`s; a pasteboard carries paths.
 *
 * Only `name` and `path` are read there, but the fields are filled honestly
 * rather than cast, so a future rule that looks at `kind` gets a value it can
 * see is unknown instead of a confident lie. `kind` is genuinely unknowable
 * here: the destination window has never listed the source folder.
 */
function asEntries(paths: string[]): Entry[] {
  return paths.map((path) => ({
    name: leaf(path),
    path,
    kind: "other",
    size: 0,
    modifiedMs: null,
    addedMs: null,
    mode: null,
    linkTarget: null,
    kindLabel: "",
    kindCategory: "other",
    hidden: false,
  }));
}

/**
 * The one sentence for everything E22 has not built yet.
 *
 * Named rather than repeated so the two places that refuse cannot drift into
 * two different explanations of the same missing feature.
 */
const NEEDS_E22 = "Copying between two shares arrives with E22.";

/** What this drop does, or why it does not. */
export function resolveDrop(request: DropRequest): DropPlan {
  const { selection, from, to, path, alt } = request;
  const items = selection.paths.map((source) => ({ path: source, name: leaf(source) }));
  if (items.length === 0) return { verb: "none" };

  if (!request.writable) {
    // The same gate the toolbar uses, said out loud. A ring that appears and
    // then refuses is worse than no ring, so the *list* never gets this far
    // (E13-S5); a sidebar row for a read-only share can, and has to explain.
    return { verb: "refused", reason: "That share is mounted read-only." };
  }

  const sameMount = selection.mount === to.id;

  // A drop back where the drag started. Not an error and not a refusal — the
  // user changed their mind, which is what releasing over the same folder
  // means — so no ring at all rather than a red one.
  //
  // The source folder is the parent of any one path: a selection is always
  // made inside a single folder, which is what makes this knowable in a window
  // that has never listed it.
  const sourceFolder = parent(selection.paths[0] ?? "");
  if (sameMount && sourceFolder === path && !alt) return { verb: "none" };

  if (sameMount && !alt) {
    const refusal = illegalDestination(asEntries(selection.paths), sourceFolder, path);
    if (refusal) return { verb: "refused", reason: refusal };
    return { verb: "move", mount: to.id, destination: path, items };
  }

  // Everything below is a copy: a different mount, or ⌥ on the same one.
  if (!from) {
    // The payload names a mount this app is not holding. Possible after the
    // source window closed its share mid-drag, and the honest answer is that
    // there is nothing to read the bytes from.
    return { verb: "refused", reason: "That share is no longer open." };
  }

  if (to.protocol === "local") {
    return {
      verb: "copy",
      direction: "download",
      sourceMount: from.id,
      destMount: to.id,
      destination: osPath(to, path),
      sources: selection.paths,
      items,
    };
  }
  if (from.protocol === "local") {
    return {
      verb: "copy",
      direction: "upload",
      sourceMount: from.id,
      destMount: to.id,
      destination: path,
      sources: selection.paths.map((source) => osPath(from, source)),
      items,
    };
  }
  // Neither side is the local filesystem, and the queue has no verb that holds
  // two backends at once. Refused with the reason rather than half-built: a
  // row that queues and then fails with an IO error is a worse answer than a
  // sentence saying it is not here yet.
  return { verb: "refused", reason: NEEDS_E22 };
}

/**
 * The drop ring's line.
 *
 * `where` is what the user should recognise: the folder's name when the drop
 * is onto a row, the share's when it is onto the list background or a sidebar
 * bookmark. A count only when there is more than one — "Copy 1 item" is a
 * sentence nobody writes.
 */
export function describeDrop(plan: DropPlan, where: string): string {
  switch (plan.verb) {
    case "none":
      return "";
    case "refused":
      return plan.reason;
    case "move":
      return plan.items.length === 1
        ? `Move to ${where}`
        : `Move ${plan.items.length} items to ${where}`;
    case "copy":
      return plan.items.length === 1
        ? `Copy to ${where}`
        : `Copy ${plan.items.length} items to ${where}`;
  }
}

/**
 * Where each item lands, for the move that runs one `rename` per item.
 *
 * The same rule `move.ts` states: a move keeps the leaf and replaces
 * everything above it.
 */
export function moveTargets(plan: Extract<DropPlan, { verb: "move" }>): {
  from: string;
  to: string;
}[] {
  return plan.items.map((item) => ({
    from: item.path,
    to: join(plan.destination, item.name),
  }));
}
