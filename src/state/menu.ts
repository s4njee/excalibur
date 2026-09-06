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

import { gate, type Affordance, type Context } from "./permissions";
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
  | "upload"
  | "addFavorite"
  | "removeFavorite";

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
 * What the menu needs to know about *where* the selection is (E27-S1).
 *
 * Separate from `Context` on purpose. `Context` is the argument `gate()`
 * reads, and every field in it is about whether the **mount** allows a write;
 * neither of these is. Adding a favourite writes the app's own sidebar list —
 * it is allowed on a read-only mount, the same way removing a bookmark is —
 * so putting these next to `writable` would invite the next reader to gate
 * them on it.
 */
export interface MenuTarget {
  /**
   * Whether the mount is a folder on this machine.
   *
   * A favourite is an absolute path, so only a local mount has one to give: a
   * folder on a share exists at a share-relative path that means nothing
   * without the mount behind it.
   */
  local?: boolean;
  /** Whether the one selected folder is already in FAVORITES. */
  favorite?: boolean;
}

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
export function menuFor(
  selection: Entry[],
  context: Context,
  target: MenuTarget = {},
): MenuItem[] {
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
    { ...favoriteItem(only, target), separated: true },
  ];
}

/**
 * `Add to Favorites`, and the three reasons it is not offered.
 *
 * Built beside `gate()` rather than through it, because it is not a fact about
 * the mount: the sidebar's list is the app's own, and adding to it is as
 * available on a read-only share as removing a bookmark is. What it *is* gated
 * on is whether the thing selected can be a favourite at all.
 *
 * The mount's reason comes first when more than one applies, on the same
 * principle `Gate` states: say the one the user cannot fix by clicking a
 * different row.
 */
function favoriteItem(only: Entry | undefined, target: MenuTarget): MenuItem {
  const label = "Add to Favorites";
  const refuse = (reason: string): MenuItem => ({
    action: "addFavorite",
    label,
    disabled: true,
    reason,
  });
  if (!target.local) return refuse("Only folders on this computer can be favorites");
  if (!only) return refuse(SINGULAR);
  if (only.kind !== "dir") return refuse("Only folders can be favorites");
  if (target.favorite) return refuse("Already in Favorites");
  return { action: "addFavorite", label, disabled: false, reason: null };
}

/**
 * The menu on a row in the FAVORITES section.
 *
 * One item, and it is here rather than assembled in the sidebar so that every
 * menu this app opens is built by the same module — the accretion guard in
 * `menu.test.ts` then covers this one too, and a `MenuAction` that no menu
 * offers stays impossible.
 */
export function favoriteMenu(): MenuItem[] {
  return [
    {
      action: "removeFavorite",
      label: "Remove from Favorites",
      disabled: false,
      reason: null,
    },
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

/**
 * A menu item built from `gate()`, which is the whole point of this module.
 *
 * Typed to the actions that *are* affordances rather than to `MenuAction`: the
 * two favourites verbs act on the app's own sidebar list and have no answer in
 * `gate`, and the type is what stops the next one being routed through here by
 * accident and silently gated on `writable`.
 */
function item(
  action: Extract<MenuAction, Affordance>,
  label: string,
  context: Context,
  count = 0,
): MenuItem {
  const { disabled, reason } = gate(action, context);
  if (disabled) return { action, label, disabled, reason };
  // The mount allows it; this one cannot do several at once.
  if (count > 1 && ONE_ONLY.includes(action)) {
    return { action, label, disabled: true, reason: SINGULAR };
  }
  return { action, label, disabled: false, reason: null };
}
