// What the context menu offers, decided once (E14-S2).
//
// The menu is the third place a verb can be reached from — after the toolbar
// and the keyboard — and the first two already share one rule, `gate()`. So
// this builds items *through* that call rather than beside it: a menu that
// decided for itself whether Delete was available would be a second answer to
// a question the design says has one, and the answers would diverge the first
// time a mount came back read-only.
//
// It is a pure function over what the window already knows, so the interesting
// cases — a read-only mount, a selection of twelve, a video where Edit makes no
// sense — are tested here rather than by opening a menu and looking at it.

import { gate, type Context } from "./permissions";
import { editable } from "./preview";
import type { Entry } from "./types";

/** Everything a context menu can ask the window to do. */
export type MenuAction =
  | "quickLook"
  | "edit"
  | "download"
  | "rename"
  | "move"
  | "delete"
  | "properties"
  | "newFolder"
  | "upload";

export interface MenuItem {
  action: MenuAction;
  label: string;
  /** Cannot be chosen. `reason` says why, and goes in the tooltip. */
  disabled: boolean;
  reason: string | null;
  /** A rule is drawn above this item. */
  separated?: boolean;
}

/** Verbs that act on exactly one thing, whatever else is selected. */
const ONE_ONLY: readonly MenuAction[] = ["quickLook", "edit", "rename"];

const SINGULAR = "Select one item";

/**
 * The items for a menu opened on `selection`.
 *
 * An empty selection is the list background: the menu is then about the folder
 * on screen, and the only things that make sense there are the two verbs that
 * put something *into* it.
 *
 * The caller is expected to have already resolved what the menu acts on —
 * right-clicking a row outside the selection selects it first — so this does
 * not know about a "row under the pointer" at all. That keeps the one rule
 * that would otherwise be easy to get wrong in one place: the menu acts on the
 * selection, always, and the selection is what you can see is highlighted.
 */
export function menuFor(selection: Entry[], context: Context): MenuItem[] {
  const count = selection.length;
  if (count === 0) {
    return [
      item("newFolder", "New Folder", context),
      item("upload", "Upload…", context),
    ];
  }

  const only = count === 1 ? selection[0] : undefined;
  // `gate` answers for the mount and the selection; the type is this menu's
  // own question, and it asks it by handing `gate` the answer.
  //
  // With several selected there is deliberately *no* opinion: the reason Edit
  // is unavailable is then the count, not the type, and the count is the one
  // the user can do something about. Saying "only text files can be edited" to
  // someone who selected three text files is both unhelpful and untrue.
  const withType: Context = { ...context, editable: only ? editable(only) : undefined };

  return [
    item("quickLook", "Quick Look", withType, count),
    item("edit", "Edit", withType, count),
    item("download", plural("Download", count), withType, count),
    { ...item("rename", "Rename", withType, count), separated: true },
    item("move", plural("Move", count, "to…"), withType, count),
    item("delete", plural("Delete", count), withType, count),
    { ...item("properties", "Properties", withType, count), separated: true },
  ];
}

/**
 * A verb's label with its count, when it acts on more than one thing.
 *
 * `Delete 3 items` rather than `Delete`, because the whole question a user has
 * when they right-click with several rows highlighted is whether the menu means
 * all of them or the one under the pointer.
 */
function plural(verb: string, count: number, tail?: string): string {
  const subject = count > 1 ? ` ${count} items` : "";
  return tail ? `${verb}${subject} ${tail}` : `${verb}${subject}`;
}

function item(action: MenuAction, label: string, context: Context, count = 0): MenuItem {
  const { disabled, reason } = gate(action, context);
  if (disabled) return { action, label, disabled, reason };
  // The mount allows it; this one cannot do several at once.
  if (count > 1 && ONE_ONLY.includes(action)) {
    return { action, label, disabled: true, reason: SINGULAR };
  }
  return { action, label, disabled: false, reason: null };
}
