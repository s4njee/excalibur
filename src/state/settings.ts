// The typed schema over the keyed preference store (E17-S1).
//
// `prefs.ts` stays deliberately untyped — it is the storage seam, and it is
// shared with the bookmark store. This is the layer that knows what a
// preference *is*: its name, its type, its default, and what to do when the
// stored value is none of those.
//
// Every field is validated independently. A preferences file is user-editable
// and outlives upgrades, so one bad value must cost that one value rather than
// the whole record — the same rule the column widths already followed, applied
// to everything.

import { DEFAULT_WIDTHS, sanitise as sanitiseWidths, type Widths } from "./columns";
import { DEFAULT_SORT, type Sort } from "./sort";
import type { Prefs } from "./prefs";
import type { RowLayout, SortColumn } from "./types";

/**
 * What the user picked, which is not the same as what is displayed.
 *
 * `system` is a choice, not the absence of one, and it has to be storable —
 * a window that follows the OS is doing something, and "no preference" could
 * not be told apart from "never asked".
 */
export type ThemeChoice = "system" | "light" | "dark";

/** What to do when a transfer's destination already exists. */
export type ConflictDefault = "ask" | "replace" | "skip" | "keepBoth";

export interface Settings {
  theme: ThemeChoice;
  rowLayout: RowLayout;
  sidebarCollapsed: boolean;
  showHidden: boolean;
  foldersFirst: boolean;
  /** `null` means the OS download folder, resolved by the engine. */
  downloadFolder: string | null;
  transferConcurrency: number;
  conflictDefault: ConflictDefault;
  columnWidths: Widths;
  sort: Sort;
}

export const DEFAULTS: Settings = {
  theme: "system",
  rowLayout: "columns",
  sidebarCollapsed: false,
  // Hidden files are shown by default because this is a tool for people who
  // deliberately went looking at a network share.
  showHidden: true,
  foldersFirst: true,
  downloadFolder: null,
  transferConcurrency: 3,
  // Never silently. A destination that already exists is the user's decision,
  // and E13-S4's prompt is what makes it one.
  conflictDefault: "ask",
  columnWidths: DEFAULT_WIDTHS,
  sort: DEFAULT_SORT,
};

/**
 * Where each field lives in the store.
 *
 * Named rather than derived, because two of these predate this schema and
 * changing their keys would silently discard what users already have.
 */
const KEYS: Record<keyof Settings, string> = {
  theme: "theme",
  rowLayout: "rowLayout",
  sidebarCollapsed: "sidebarCollapsed",
  showHidden: "showHidden",
  foldersFirst: "foldersFirst",
  downloadFolder: "downloadFolder",
  transferConcurrency: "transferConcurrency",
  conflictDefault: "conflictDefault",
  // Shaped for `1g`/`1h`, which have no columns to size, so those need no
  // migration when they land.
  columnWidths: "columnWidths.columns",
  sort: "sort",
};

const THEMES: readonly ThemeChoice[] = ["system", "light", "dark"];
const LAYOUTS: readonly RowLayout[] = ["columns", "name-led", "mono"];
const CONFLICTS: readonly ConflictDefault[] = ["ask", "replace", "skip", "keepBoth"];
const SORT_COLUMNS: readonly SortColumn[] = ["name", "size", "modified", "kind"];

/** The most transfers that may run at once, whatever the file says. */
export const MAX_CONCURRENCY = 8;

function oneOf<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value)
    ? (value as T)
    : fallback;
}

function bool(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function sanitiseSort(value: unknown): Sort {
  if (!value || typeof value !== "object") return DEFAULT_SORT;
  const { column, ascending } = value as Partial<Sort>;
  if (!SORT_COLUMNS.includes(column as SortColumn)) return DEFAULT_SORT;
  if (typeof ascending !== "boolean") return DEFAULT_SORT;
  return { column: column as SortColumn, ascending };
}

function sanitiseConcurrency(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return DEFAULTS.transferConcurrency;
  }
  // Clamped rather than rejected: someone who edited the file to 40 wanted
  // "more", and the nearest honest answer to that is the ceiling.
  return Math.min(MAX_CONCURRENCY, Math.max(1, Math.round(value)));
}

function sanitiseFolder(value: unknown): string | null {
  // Not checked for existence here — that is the engine's answer to give when
  // a transfer starts, and a folder can be unmounted between launch and use.
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

/** One stored value, coerced into the field it claims to be. */
export function sanitiseField<K extends keyof Settings>(key: K, value: unknown): Settings[K] {
  switch (key) {
    case "theme":
      return oneOf(value, THEMES, DEFAULTS.theme) as Settings[K];
    case "rowLayout":
      return oneOf(value, LAYOUTS, DEFAULTS.rowLayout) as Settings[K];
    case "conflictDefault":
      return oneOf(value, CONFLICTS, DEFAULTS.conflictDefault) as Settings[K];
    case "sidebarCollapsed":
      return bool(value, DEFAULTS.sidebarCollapsed) as Settings[K];
    case "showHidden":
      return bool(value, DEFAULTS.showHidden) as Settings[K];
    case "foldersFirst":
      return bool(value, DEFAULTS.foldersFirst) as Settings[K];
    case "downloadFolder":
      return sanitiseFolder(value) as Settings[K];
    case "transferConcurrency":
      return sanitiseConcurrency(value) as Settings[K];
    case "columnWidths":
      return sanitiseWidths(value) as Settings[K];
    case "sort":
      return sanitiseSort(value) as Settings[K];
    default: {
      // Exhaustive: a field added to `Settings` without a case here is a
      // compile error rather than a value that silently never loads.
      const unreachable: never = key;
      return unreachable;
    }
  }
}

/** Read every preference, falling back per field. */
export async function loadSettings(prefs: Prefs): Promise<Settings> {
  const keys = Object.keys(DEFAULTS) as (keyof Settings)[];
  const loaded = await Promise.all(
    keys.map(async (key) => {
      try {
        return [key, sanitiseField(key, await prefs.get(KEYS[key]))] as const;
      } catch {
        // A store that cannot be read is not worth failing a launch over.
        return [key, DEFAULTS[key]] as const;
      }
    }),
  );
  return Object.fromEntries(loaded) as unknown as Settings;
}

/** Write one preference. */
export async function saveSetting<K extends keyof Settings>(
  prefs: Prefs,
  key: K,
  value: Settings[K],
): Promise<void> {
  await prefs.set(KEYS[key], value);
}

/**
 * The theme to actually paint, given the choice and what the OS says.
 *
 * Separate from the choice because the two differ precisely when the choice is
 * `system`, and conflating them is how a window ends up unable to tell "the
 * user picked light" from "the OS is light today".
 */
export function resolveTheme(choice: ThemeChoice, prefersDark: boolean): "light" | "dark" {
  if (choice === "system") return prefersDark ? "dark" : "light";
  return choice;
}
