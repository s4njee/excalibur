// The keyboard map (E16-S1).
//
// One table, read twice: once to decide what a keystroke means, and once to
// label the toolbar. Two lists would drift — a tooltip promising ⌘D while the
// handler listens for something else is worse than no tooltip, because the
// user believes it.
//
// The table says only what a key *means*. Whether the verb is allowed right
// now is the caller's question: a shortcut that silently did nothing because
// the mount is read-only would be indistinguishable from a broken binding, so
// `App` runs the same permission gate the buttons do and shows the same
// refusal.

/** Every verb reachable from the keyboard. */
export type ShortcutId =
  | "quickLook"
  | "rename"
  | "delete"
  | "descend"
  | "ascend"
  | "back"
  | "forward"
  | "refresh"
  | "focusFilter"
  | "escape"
  | "download"
  | "upload"
  | "edit"
  | "newFolder"
  | "newWindow"
  | "preferences"
  | "transfers"
  | "layoutColumns"
  | "layoutNameLed"
  | "layoutMono"
  | "layoutIcons"
  | "layoutColumnView"
  | "switchPane"
  | "togglePanes"
  | "contextMenu"
  // Moving through the list without a pointer (E26-S2). The four arrows are
  // two verbs each — move, and extend from the anchor — because they differ in
  // Shift and `match` compares modifiers exactly.
  | "moveDown"
  | "moveUp"
  | "extendDown"
  | "extendUp"
  | "first"
  | "last"
  | "pageDown"
  | "pageUp"
  | "open"
  | "goToFolder"
  | "toggleHidden";

export interface Shortcut {
  id: ShortcutId;
  /** The verb's name, so a tooltip reads "Download (⌘D)". */
  label: string;
  /** Matched against `event.key`, lowercased for letters. */
  key: string;
  /** Command on a Mac, Control elsewhere. */
  mod?: boolean;
  shift?: boolean;
  /** A second key for the same verb, where the design gives one. */
  alias?: string;
}

/**
 * The map.
 *
 * Deliberately flat and literal — it is meant to be read against the design's
 * §Keyboard table and disagreed with, which a cleverer structure would make
 * harder.
 */
export const SHORTCUTS: readonly Shortcut[] = [
  { id: "quickLook", label: "Quick Look", key: " " },
  // The two keys every desktop gives for "show me what I can do with this":
  // the dedicated Menu key, and Shift+F10 for the keyboards without one. They
  // are separate rows because they differ in a modifier, and `match` compares
  // modifiers exactly — an `alias` would have to share Shift, which the Menu
  // key does not have. `hint()` reports the second, which is the one a user
  // without a Menu key needs told (E14-S2).
  { id: "contextMenu", label: "Show actions", key: "contextmenu" },
  { id: "contextMenu", label: "Show actions", key: "f10", shift: true },
  { id: "rename", label: "Rename", key: "enter" },
  { id: "delete", label: "Delete", key: "delete", alias: "backspace" },
  { id: "descend", label: "Open", key: "arrowdown", mod: true },
  { id: "ascend", label: "Enclosing folder", key: "arrowup", mod: true },
  // Moving through the list (E26-S2). A bare arrow moves the selection and
  // takes the anchor with it; Shift extends the range *from* the anchor, which
  // stays where it is (E8-S8) — so ⇧↓ then ⇧↑ shrinks the range it just grew
  // rather than walking down the list. Two rows rather than one with a
  // modifier flag, because `match` compares modifiers exactly.
  { id: "moveDown", label: "Next row", key: "arrowdown" },
  { id: "moveUp", label: "Previous row", key: "arrowup" },
  { id: "extendDown", label: "Extend down", key: "arrowdown", shift: true },
  { id: "extendUp", label: "Extend up", key: "arrowup", shift: true },
  { id: "first", label: "First row", key: "home" },
  { id: "last", label: "Last row", key: "end" },
  { id: "pageDown", label: "Down a page", key: "pagedown" },
  { id: "pageUp", label: "Up a page", key: "pageup" },
  // The same verb ⌘↓ is, under the name every file manager gives it. A folder
  // is descended into; a file has no "open" in this window yet, so it gets
  // Quick Look — see the `open` case in `App`.
  { id: "open", label: "Open", key: "o", mod: true },
  { id: "goToFolder", label: "Go to Folder", key: "g", mod: true, shift: true },
  // Shift changes the reported key, so ⌘⇧. arrives as `>` on a US layout and
  // as `.` on the layouts where it does not. Both spellings are the same
  // keystroke; `hint` reports the first, which is what the key is labelled.
  { id: "toggleHidden", label: "Show hidden files", key: ".", mod: true, shift: true, alias: ">" },
  { id: "back", label: "Back", key: "[", mod: true },
  { id: "forward", label: "Forward", key: "]", mod: true },
  { id: "refresh", label: "Refresh", key: "r", mod: true },
  { id: "focusFilter", label: "Filter", key: "f", mod: true },
  { id: "escape", label: "Clear", key: "escape" },
  { id: "download", label: "Download", key: "d", mod: true },
  { id: "upload", label: "Upload", key: "u", mod: true },
  { id: "edit", label: "Edit", key: "e", mod: true },
  { id: "newFolder", label: "New Folder", key: "n", mod: true, shift: true },
  // ⌘N, and ⌘⇧N is New Folder. `match` compares modifiers exactly for this
  // reason: the two differ only in Shift, and "at least these modifiers" would
  // make ⌘⇧N open a window as well as create a folder (E23-S1).
  { id: "newWindow", label: "New Window", key: "n", mod: true },
  { id: "preferences", label: "Preferences", key: ",", mod: true },
  { id: "transfers", label: "Transfers", key: "t", mod: true, shift: true },
  // The three row treatments (E8-S3/S4). Bound only now that `1g` and `1h`
  // exist — until they did, two of the three would have done nothing visible.
  { id: "layoutColumns", label: "Columns", key: "1", mod: true },
  { id: "layoutNameLed", label: "Name-led", key: "2", mod: true },
  { id: "layoutMono", label: "Mono grid", key: "3", mod: true },
  { id: "layoutIcons", label: "Icons", key: "4", mod: true },
  { id: "layoutColumnView", label: "Columns view", key: "5", mod: true },
  { id: "togglePanes", label: "Two panes", key: "6", mod: true },
  // Only bound while two panes are drawn; with one it is left to the
  // browser, which uses it to move between the toolbar's controls.
  { id: "switchPane", label: "Switch pane", key: "tab" },
] as const;

const BY_ID = new Map(SHORTCUTS.map((shortcut) => [shortcut.id, shortcut]));

/** Whether the platform names the modifier ⌘ or Ctrl. */
export function onMac(): boolean {
  if (typeof navigator === "undefined") return false;
  return /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent);
}

/**
 * Which verb a keystroke means, or `null`.
 *
 * Command and Control are both accepted whatever the platform. A Mac user with
 * a PC keyboard reaching for Ctrl is asking for the same thing, and refusing
 * them buys nothing.
 */
export function match(event: KeyboardEvent): ShortcutId | null {
  const pressed = event.key.length === 1 ? event.key.toLowerCase() : event.key.toLowerCase();
  const mod = event.metaKey || event.ctrlKey;
  for (const shortcut of SHORTCUTS) {
    if (pressed !== shortcut.key && pressed !== shortcut.alias) continue;
    // Matched exactly, not merely "at least": ⌘⇧N is New Folder and ⌘N must
    // not fall through to it.
    if (mod !== Boolean(shortcut.mod)) continue;
    if (event.shiftKey !== Boolean(shortcut.shift)) continue;
    return shortcut.id;
  }
  return null;
}

const NAMES: Record<string, string> = {
  " ": "Space",
  enter: "Enter",
  escape: "Esc",
  delete: "Del",
  arrowdown: "↓",
  arrowup: "↑",
  home: "Home",
  end: "End",
  pagedown: "Page Down",
  pageup: "Page Up",
};

/** How a shortcut is written, e.g. `⌘D` or `Ctrl+Shift+T`. */
export function hint(id: ShortcutId, mac = onMac()): string {
  const shortcut = BY_ID.get(id);
  if (!shortcut) return "";
  const key = NAMES[shortcut.key] ?? shortcut.key.toUpperCase();
  const parts: string[] = [];
  if (shortcut.mod) parts.push(mac ? "⌘" : "Ctrl");
  if (shortcut.shift) parts.push(mac ? "⇧" : "Shift");
  parts.push(key);
  // A Mac writes them run together; everywhere else joins with `+`.
  return mac ? parts.join("") : parts.join("+");
}

/** A tooltip: the verb, then how to reach it. */
export function tooltip(id: ShortcutId, mac = onMac()): string {
  const shortcut = BY_ID.get(id);
  if (!shortcut) return "";
  return `${shortcut.label} (${hint(id, mac)})`;
}
