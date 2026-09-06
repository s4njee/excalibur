// The FAVORITES section's rules (E27-S1).
//
// Three of these are the kind that is wrong invisibly, and they are why this
// file exists rather than only a component test:
//
//   - **Seeding happens once.** The failure is not a crash, it is a default
//     coming back after the user removed it — which nobody notices until the
//     third launch, and which makes the section impossible to curate. The
//     property held here is that only a `null` from the store seeds anything.
//   - **A missing folder is hidden, not forgotten.** The two halves are
//     asserted separately, because a version that deletes the row instead
//     looks identical on screen and destroys the list when a disk is
//     unplugged.
//   - **A drop can only come from a local mount.** A favourite is an absolute
//     path; a share-relative one would open whatever happened to be mounted.

import { describe, expect, it } from "vitest";
import {
  addFavorite,
  defaultFavorites,
  describeFavoriteDrop,
  favoriteDraft,
  favoriteDrop,
  favoriteName,
  isFavorite,
  normalisePath,
  removeFavorite,
  restoreFavorite,
  toFavorite,
  visibleFavorites,
  type Favorite,
  type FavoriteStore,
} from "./favorites";
import type { UserFolders } from "./backend";
import type { DragSelection, DropMount } from "./dnd";

const MAC: UserFolders = {
  home: "/Users/dorian",
  applications: "/Applications",
  desktop: "/Users/dorian/Desktop",
  documents: "/Users/dorian/Documents",
  pictures: "/Users/dorian/Pictures",
  videos: "/Users/dorian/Movies",
};

const LINUX: UserFolders = {
  home: "/home/dorian",
  // No Applications folder anybody browses, which the platform says by
  // answering with nothing.
  applications: "",
  desktop: "/home/dorian/Desktop",
  documents: "/home/dorian/Documents",
  pictures: "/home/dorian/Pictures",
  videos: "/home/dorian/Videos",
};

const names = (favorites: Favorite[]) => favorites.map((favorite) => favorite.name);
const paths = (favorites: Favorite[]) => favorites.map((favorite) => favorite.path);

/** A store standing in for the app's, so "seeded once" can be replayed. */
function memoryStore(initial: Favorite[] | null = null): FavoriteStore & {
  saves: number;
} {
  let held = initial;
  const store = {
    saves: 0,
    async load() {
      return held;
    },
    async save(favorites: Favorite[]) {
      held = favorites;
      store.saves += 1;
    },
  };
  return store;
}

/** What the window does on start: seed only when nothing was ever saved. */
async function startUp(store: FavoriteStore, folders: UserFolders): Promise<Favorite[]> {
  const saved = await store.load();
  if (saved !== null) return saved;
  const seeded = defaultFavorites(folders);
  await store.save(seeded);
  return seeded;
}

describe("the default favourites", () => {
  it("are Finder's own set, in Finder's order", () => {
    expect(names(defaultFavorites(MAC))).toEqual([
      "Home",
      "Applications",
      "Desktop",
      "Documents",
      "Pictures",
      "Movies",
    ]);
  });

  it("names the video folder after the folder that is actually there", () => {
    // The whole point of asking the platform rather than assuming: a Mac has
    // `~/Movies` and no `~/Videos`, so a row labelled "Videos" would open
    // nothing. The label follows the path, not the other way round.
    expect(paths(defaultFavorites(MAC))).toContain("/Users/dorian/Movies");
    expect(names(defaultFavorites(MAC))).toContain("Movies");
    expect(paths(defaultFavorites(LINUX))).toContain("/home/dorian/Videos");
    expect(names(defaultFavorites(LINUX))).toContain("Videos");
  });

  it("drops a folder the platform says it does not have", () => {
    expect(names(defaultFavorites(LINUX))).not.toContain("Applications");
    expect(paths(defaultFavorites(LINUX))).not.toContain("");
  });

  it("calls the home directory Home rather than the account name", () => {
    expect(favoriteName("/Users/dorian", "/Users/dorian")).toBe("Home");
    // Every other folder is its own last segment, which is the name a person
    // gave it.
    expect(favoriteName("/Users/dorian/Documents", "/Users/dorian")).toBe("Documents");
    // Trailing slashes are not a different folder.
    expect(favoriteName("/Users/dorian/", "/Users/dorian")).toBe("Home");
    expect(favoriteName("/", "/Users/dorian")).toBe("/");
  });
});

describe("seeding", () => {
  it("happens on a first run, and never again", async () => {
    const store = memoryStore();
    const first = await startUp(store, MAC);
    expect(names(first)).toContain("Pictures");
    expect(store.saves).toBe(1);

    // The user removes one, and the app is restarted.
    await store.save(removeFavorite(first, "/Users/dorian/Pictures"));
    const second = await startUp(store, MAC);

    // This is the assertion the whole feature turns on: a removed default is
    // removed. Re-seeding "because the list looks short" would make the
    // section impossible to curate.
    expect(names(second)).not.toContain("Pictures");
    expect(names(second)).toContain("Home");
  });

  it("does not seed over a list the user emptied", async () => {
    const store = memoryStore([]);
    expect(await startUp(store, MAC)).toEqual([]);
    // Nothing was written: an empty saved list is an answer, not an absence.
    expect(store.saves).toBe(0);
  });
});

describe("a favourite whose folder is gone", () => {
  const list = defaultFavorites(MAC);

  it("is not shown, because clicking it could only fail", () => {
    const present = new Set(["/Users/dorian", "/Users/dorian/Documents"]);
    expect(names(visibleFavorites(list, present))).toEqual(["Home", "Documents"]);
  });

  it("is still saved, so an unplugged disk coming back brings the row back", () => {
    // Hidden and forgotten look the same on screen for as long as the folder
    // is missing, and completely different the moment it returns.
    const gone = visibleFavorites(list, new Set(["/Users/dorian"]));
    expect(names(gone)).toEqual(["Home"]);
    expect(names(visibleFavorites(list, new Set(paths(list))))).toEqual(names(list));
  });

  it("keeps the saved order rather than the filesystem's answer", () => {
    // `existingDirectories` answers in whatever order it likes; the section is
    // the user's list.
    const shuffled = new Set(["/Users/dorian/Pictures", "/Applications", "/Users/dorian"]);
    expect(names(visibleFavorites(list, shuffled))).toEqual([
      "Home",
      "Applications",
      "Pictures",
    ]);
  });
});

describe("adding and removing", () => {
  const home = "/Users/dorian";

  it("appends a folder, and refuses to add it twice", () => {
    const one = addFavorite([], toFavorite("/Users/dorian/Code", home));
    expect(names(one)).toEqual(["Code"]);
    // The gestures that reach this — a menu item, a drop — are ones a user can
    // repeat without noticing, and two identical rows is the worse outcome.
    expect(addFavorite(one, toFavorite("/Users/dorian/Code/", home))).toBe(one);
    expect(isFavorite(one, "/Users/dorian/Code")).toBe(true);
  });

  it("treats two paths that differ only in case as two folders", () => {
    // Unlike `bookmarkId`, which lower-cases and says why. A favourite is only
    // ever a path, and on a case-sensitive filesystem these are two folders —
    // folding them would refuse to favourite the second.
    const list = addFavorite([], toFavorite("/data/Foo", home));
    expect(isFavorite(list, "/data/foo")).toBe(false);
  });

  it("removes by path, trailing slash or not", () => {
    const list = defaultFavorites(MAC);
    expect(names(removeFavorite(list, "/Users/dorian/Desktop/"))).not.toContain("Desktop");
    expect(removeFavorite(list, "/nothing/here")).toHaveLength(list.length);
  });

  it("puts a removed favourite back where it was", () => {
    const list = defaultFavorites(MAC);
    const [gone] = list.slice(2, 3);
    if (!gone) throw new Error("the default set is shorter than the test assumes");
    expect(names(restoreFavorite(removeFavorite(list, gone.path), gone, 2))).toEqual(
      names(list),
    );
  });

  it("normalises the path it stores, so the id is one shape", () => {
    expect(normalisePath("/Users/dorian/Code//")).toBe("/Users/dorian/Code");
    expect(normalisePath("/")).toBe("/");
    expect(paths(addFavorite([], toFavorite("/Users/dorian/Code/", home)))).toEqual([
      "/Users/dorian/Code",
    ]);
  });
});

describe("opening a favourite", () => {
  it("is the connection the dialog's Local tab makes, and nothing else", () => {
    const draft = favoriteDraft({ name: "Documents", path: "/Users/dorian/Documents" });
    expect(draft.protocol).toBe("local");
    expect(draft.share).toBe("/Users/dorian/Documents");
    // No host, no account, no port: a folder on this machine has none of them,
    // and a favourite that invented one would reach the engine as a different
    // shape than the same folder typed into the dialog.
    expect(draft.host).toBe("");
    expect(draft.username).toBe("");
    expect(draft.port).toBe("");
  });
});

describe("a drop onto the section", () => {
  const local: DropMount = { id: "local::/users/dorian", protocol: "local", root: "/Users/dorian" };
  const share: DropMount = { id: "smb:nas.local:media", protocol: "smb", root: "" };
  const dragging = (mount: string, ...paths: string[]): DragSelection => ({
    mount,
    paths,
    window: "browser-1",
  });

  it("turns share-relative paths into absolute ones", () => {
    const drop = favoriteDrop(dragging(local.id, "Code", "Code/excalibur"), local);
    expect(drop).toEqual({
      ok: true,
      paths: ["/Users/dorian/Code", "/Users/dorian/Code/excalibur"],
    });
    expect(describeFavoriteDrop(drop)).toBe("Add to Favorites");
  });

  it("refuses anything that is not on this machine, and says why", () => {
    const drop = favoriteDrop(dragging(share.id, "Films"), share);
    expect(drop.ok).toBe(false);
    // A favourite is an absolute path. `Films` on a share means nothing
    // without the mount behind it, and saving it would open whatever happened
    // to be mounted next time.
    expect(describeFavoriteDrop(drop)).toMatch(/this computer/i);
  });

  it("refuses a drag whose mount this app no longer holds", () => {
    const drop = favoriteDrop(dragging("smb:nas.local:media", "Films"), null);
    expect(drop.ok).toBe(false);
    expect(describeFavoriteDrop(drop)).toMatch(/no longer open/i);
  });

  it("says nothing about whether the paths are folders", () => {
    // Deliberate: the pasteboard carries no kind (`dnd.ts`'s `asEntries` says
    // so), and guessing from the name would call `Season 2` a folder and
    // `Makefile` a file. The window resolves it against the filesystem after
    // the drop instead.
    expect(favoriteDrop(dragging(local.id, "notes.txt"), local)).toEqual({
      ok: true,
      paths: ["/Users/dorian/notes.txt"],
    });
  });
});
