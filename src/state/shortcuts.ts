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
  | "preferences"
  | "transfers"
  | "layoutColumns"
  | "layoutNameLed"
  | "layoutMono"
  | "contextMenu";

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
  { id: "back", label: "Back", key: "[", mod: true },
  { id: "forward", label: "Forward", key: "]", mod: true },
  { id: "refresh", label: "Refresh", key: "r", mod: true },
  { id: "focusFilter", label: "Filter", key: "f", mod: true },
  { id: "escape", label: "Clear", key: "escape" },
  { id: "download", label: "Download", key: "d", mod: true },
  { id: "upload", label: "Upload", key: "u", mod: true },
  { id: "edit", label: "Edit", key: "e", mod: true },
  { id: "newFolder", label: "New Folder", key: "n", mod: true, shift: true },
  { id: "preferences", label: "Preferences", key: ",", mod: true },
  { id: "transfers", label: "Transfers", key: "t", mod: true, shift: true },
  // The three row treatments (E8-S3/S4). Bound only now that `1g` and `1h`
  // exist — until they did, two of the three would have done nothing visible.
  { id: "layoutColumns", label: "Columns", key: "1", mod: true },
  { id: "layoutNameLed", label: "Name-led", key: "2", mod: true },
  { id: "layoutMono", label: "Mono grid", key: "3", mod: true },
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
