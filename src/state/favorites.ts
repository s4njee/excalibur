// The sidebar's FAVORITES section: folders on this machine, one click away.
//
// A favourite is a *path*, and nothing else. It is not a bookmark with the
// network fields left blank: it has no host, no credential, no reachability
// and no mount preference, and giving it those fields would be four columns
// that are always the same value. What it shares with a bookmark is the way it
// opens — `Protocol::Local`, absolute path, the `file:` scheme — so opening
// one goes through the connection the dialog's Local tab already builds
// (`toDraft`/`toSpec`) rather than a second route to the same mount.
//
// Two rules live here rather than in the window, because both are the kind
// that is wrong invisibly:
//
//   1. **The defaults are seeded once.** Only when nothing has ever been
//      saved — `load()` answering `null`, which is why it is nullable at all.
//      A user who removes Pictures has removed it; re-seeding on the next
//      launch because "the list looks like it is missing one" would make the
//      section impossible to curate.
//   2. **A favourite whose folder is gone is not shown.** Clicking it could
//      only fail, and a row that always fails is worse than no row. It stays
//      in the saved list: an unplugged disk comes back, and forgetting the
//      favourite the moment it was unmounted would delete the user's list by
//      accident.

import type { UserFolders } from "./backend";
import type { DragSelection, DropMount } from "./dnd";
import { osPath } from "./dnd";
import { emptyDraft, type Draft } from "./connection";

/** A folder in the sidebar's FAVORITES section. */
export interface Favorite {
  /** What the row reads. Derived once, on the way in, and then stored. */
  name: string;
  /** Absolute, and the row's identity: two favourites are the same folder. */
  path: string;
}

/** Where favourites live, and how they get there. */
export interface FavoriteStore {
  /**
   * The saved list, or `null` when nothing has ever been saved.
   *
   * The `null` is the whole point: an empty array means *the user emptied it*,
   * and only the absence of a record means *seed the defaults*. Collapsing the
   * two would put Home back in the sidebar every launch after it was removed.
   */
  load(): Promise<Favorite[] | null>;
  save(favorites: Favorite[]): Promise<void>;
}

const KEY = "favorites";

/** The Tauri store plugin: JSON under the app's data directory, as bookmarks. */
function tauriStore(): FavoriteStore {
  const open = async () => {
    const { load } = await import("@tauri-apps/plugin-store");
    return load("favorites.json", { autoSave: true });
  };
  return {
    async load() {
      const store = await open();
      return (await store.get<Favorite[]>(KEY)) ?? null;
    },
    async save(favorites) {
      const store = await open();
      await store.set(KEY, favorites);
      await store.save();
    },
  };
}

/** `localStorage`, so the browser preview keeps its favourites too. */
function browserStore(): FavoriteStore {
  return {
    async load() {
      try {
        const raw = window.localStorage.getItem(KEY);
        return raw ? (JSON.parse(raw) as Favorite[]) : null;
      } catch {
        // Corrupt or blocked storage. `null` rather than `[]` for the same
        // reason as a first run: an unreadable store is not evidence that the
        // user emptied the section, so the defaults come back.
        return null;
      }
    },
    async save(favorites) {
      try {
        window.localStorage.setItem(KEY, JSON.stringify(favorites));
      } catch {
        // Full or blocked storage: the section still works for this session.
      }
    },
  };
}

export function defaultFavoriteStore(): FavoriteStore {
  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  return inTauri ? tauriStore() : browserStore();
}

/**
 * A path in the one form the list compares.
 *
 * Trailing slashes only, and deliberately **not** lower-cased — unlike
 * `bookmarkId`, which has a protocol and a host to keep apart and can afford
 * to. A favourite is a folder and nothing else, so `/data/Foo` and `/data/foo`
 * on a case-sensitive filesystem are two different ones, and folding them
 * would refuse to favourite the second.
 */
export function normalisePath(path: string): string {
  const trimmed = path.trim().replace(/\/+$/, "");
  return trimmed === "" ? "/" : trimmed;
}

/**
 * What the row calls a folder.
 *
 * The last path segment, because that is the name the user gave it — except
 * for the home directory, which by that rule would read as the account name.
 * "Home" is what Finder and every file manager call it, and it is the one
 * folder whose leaf tells the user nothing they were asking.
 */
export function favoriteName(path: string, home: string): string {
  const normalised = normalisePath(path);
  if (home !== "" && normalised === normalisePath(home)) return "Home";
  return normalised.split("/").filter(Boolean).pop() ?? "/";
}

/** The favourite for a folder, named by the rule above. */
export function toFavorite(path: string, home: string): Favorite {
  return { name: favoriteName(path, home), path: normalisePath(path) };
}

/**
 * The section as a machine that has never been curated should show it.
 *
 * Finder's own set, in Finder's order. Empty fields are dropped rather than
 * seeded: an empty string is the platform saying it has no such folder — no
 * Applications directory on Linux, no media folder the user never made — and
 * a row for it could only fail.
 *
 * The video folder is seeded from `videos` and labelled by its own leaf, so
 * the row reads `Movies` on macOS and `Videos` (or whatever `XDG_VIDEOS_DIR`
 * is set to) on Linux. Labelling it "Videos" everywhere would name a folder
 * that, on a Mac, does not exist.
 */
export function defaultFavorites(folders: UserFolders): Favorite[] {
  const home = folders.home;
  return [
    folders.home,
    folders.applications,
    folders.desktop,
    folders.documents,
    folders.pictures,
    folders.videos,
  ]
    .filter((path) => path !== "")
    .map((path) => toFavorite(path, home));
}

/**
 * No answer from the OS yet.
 *
 * Every field empty, and empty never matches a real path — so before the first
 * `userFolders()` reply every favourite draws the plain folder, and the special
 * glyphs appear together when the answer lands. The alternative, guessing from
 * names in the meantime, would flicker Documents from folder to document.
 */
export const NO_USER_FOLDERS: UserFolders = {
  home: "",
  applications: "",
  desktop: "",
  documents: "",
  pictures: "",
  videos: "",
};

/** Which picture a favourite's row carries. */
export type Glyph =
  | "home"
  | "applications"
  | "desktop"
  | "documents"
  | "pictures"
  | "movies"
  | "folder";

/**
 * The glyph for one favourite, by *path* rather than by name.
 *
 * Finder gives its special folders their own icons and everything else a plain
 * folder, and it decides by which folder a thing actually is. Matching on the
 * row's label instead would hand the document icon to any folder a user
 * happens to have called `Documents`, and would miss home entirely — home's
 * label is the account name, which is a different word on every machine.
 */
export function favoriteGlyph(path: string, folders: UserFolders): Glyph {
  const target = normalisePath(path);
  const named: [string, Glyph][] = [
    [folders.home, "home"],
    [folders.applications, "applications"],
    [folders.desktop, "desktop"],
    [folders.documents, "documents"],
    [folders.pictures, "pictures"],
    [folders.videos, "movies"],
  ];
  for (const [candidate, glyph] of named) {
    // The empty check is load-bearing: a platform without an Applications
    // folder reports `""`, and `normalisePath("")` would otherwise match a
    // favourite whose path normalised to the same nothing.
    if (candidate !== "" && normalisePath(candidate) === target) return glyph;
  }
  return "folder";
}

/** Whether this folder is already in the section. */
export function isFavorite(favorites: Favorite[], path: string): boolean {
  const target = normalisePath(path);
  return favorites.some((favorite) => normalisePath(favorite.path) === target);
}

/**
 * Add a folder to the end of the section.
 *
 * Adding one that is already there is not an error and not a second row: the
 * gesture that reaches this — a menu item, a drop — is one a user can repeat
 * without noticing, and two identical rows is the worse outcome.
 */
export function addFavorite(favorites: Favorite[], favorite: Favorite): Favorite[] {
  if (isFavorite(favorites, favorite.path)) return favorites;
  return [...favorites, { ...favorite, path: normalisePath(favorite.path) }];
}

export function removeFavorite(favorites: Favorite[], path: string): Favorite[] {
  const target = normalisePath(path);
  return favorites.filter((favorite) => normalisePath(favorite.path) !== target);
}

/** Put a removed favourite back where it was, for undo. */
export function restoreFavorite(
  favorites: Favorite[],
  favorite: Favorite,
  index: number,
): Favorite[] {
  const next = favorites.slice();
  next.splice(Math.min(Math.max(index, 0), next.length), 0, favorite);
  return next;
}

/**
 * The rows the sidebar actually draws: the saved list minus what is not there.
 *
 * `existing` is the answer to one batched `existingDirectories` call, so the
 * order of the section is the saved order and never the order the filesystem
 * answered in.
 */
export function visibleFavorites(favorites: Favorite[], existing: Set<string>): Favorite[] {
  return favorites.filter((favorite) => existing.has(normalisePath(favorite.path)));
}

/**
 * The connection that opens a favourite.
 *
 * Exactly what the dialog's Local tab produces for the same folder, so a
 * favourite and a hand-typed local bookmark reach the engine as one shape —
 * and `bookmarkId` then gives them the same id, which is what stops opening
 * Documents twice from growing two rows under MOUNTED.
 */
export function favoriteDraft(favorite: Favorite): Draft {
  return { ...emptyDraft("local"), share: normalisePath(favorite.path) };
}

/** What a drop onto the FAVORITES section would add, or why it cannot. */
export type FavoriteDrop =
  | { ok: true; paths: string[] }
  | { ok: false; reason: string };

/**
 * A drag released over the FAVORITES section (E27-S1).
 *
 * A favourite is an absolute path on this machine, so only a local mount has
 * anything to give: a folder on a share is reachable through its mount and
 * nowhere else, and a favourite pointing at a share-relative path would open
 * whatever happened to be mounted at the time. Refused with the reason rather
 * than silently ignored — the same argument `dnd.ts` makes about a drop that
 * lands and does nothing.
 *
 * **Whether the dragged items are folders is not decided here.** The
 * pasteboard carries paths and a mount, never a kind (see `asEntries` in
 * `dnd.ts`), so the window resolves it against the filesystem after the drop
 * with the same `existingDirectories` call the section already makes. Files in
 * the selection are dropped from the list there, not refused here — a
 * mixed selection should still add the folders in it.
 */
export function favoriteDrop(
  selection: DragSelection,
  from: DropMount | null,
): FavoriteDrop {
  if (!from) return { ok: false, reason: "That share is no longer open." };
  if (from.protocol !== "local") {
    return { ok: false, reason: "Only folders on this computer can be a favourite." };
  }
  return { ok: true, paths: selection.paths.map((path) => osPath(from, path)) };
}

/**
 * The drop ring's line over the FAVORITES section.
 *
 * "Add to Favorites" whatever the count, because the count is not the thing in
 * question — the section takes all of them or refuses all of them, and a
 * number here would suggest a choice the drop does not offer.
 */
export function describeFavoriteDrop(drop: FavoriteDrop): string {
  return drop.ok ? "Add to Favorites" : drop.reason;
}
