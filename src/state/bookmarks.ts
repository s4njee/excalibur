// Saved connections.
//
// A bookmark is what the app remembers about a share: enough to offer it in the
// sidebar and to prefill the dialog, and *never* a secret. Passwords and
// passphrases live in the OS keychain, keyed by bookmark id (E3-S3). This file
// is plain JSON in the app's data directory, so anything written here should be
// assumed readable.

import type { Bookmark } from "./types";
import type { Draft } from "./connection";
import { connectionName, emptyDraft, usesHost } from "./connection";

/** Where bookmarks live, and how they get there. */
export interface BookmarkStore {
  load(): Promise<Bookmark[]>;
  save(bookmarks: Bookmark[]): Promise<void>;
}

const KEY = "bookmarks";

/** The Tauri store plugin: JSON under the app's data directory. */
function tauriStore(): BookmarkStore {
  const open = async () => {
    const { load } = await import("@tauri-apps/plugin-store");
    return load("bookmarks.json", { autoSave: true });
  };
  return {
    async load() {
      const store = await open();
      return (await store.get<Bookmark[]>(KEY)) ?? [];
    },
    async save(bookmarks) {
      const store = await open();
      await store.set(KEY, bookmarks);
      await store.save();
    },
  };
}

/**
 * The rows the design's sidebar shows, including the unreachable one.
 *
 * Seeded only into the browser preview, and only when nothing is saved. The
 * preview has no engine behind it, so without these the sidebar is empty and
 * the states worth looking at — an active mount, a dimmed unreachable row —
 * cannot be seen at all. The desktop app starts empty, as it should.
 */
const DEMO_BOOKMARKS: Bookmark[] = [
  {
    id: "smb:nas.local:media",
    name: "media",
    protocol: "smb",
    host: "nas.local",
    share: "media",
    mountPreference: "rw",
    mounted: true,
    reachable: true,
  },
  {
    id: "smb:nas.local:backups",
    name: "backups",
    protocol: "smb",
    host: "nas.local",
    share: "backups",
    mountPreference: "rw",
    mounted: false,
    reachable: true,
  },
  {
    id: "nfs:10.0.0.4:/export/scratch",
    name: "scratch",
    protocol: "nfs",
    host: "10.0.0.4",
    share: "/export/scratch",
    mountPreference: "rw",
    mounted: false,
    reachable: true,
  },
  {
    id: "sftp:vault.example:/srv/archive",
    name: "archive",
    protocol: "sftp",
    host: "vault.example",
    share: "/srv/archive",
    mountPreference: "ro",
    mounted: false,
    reachable: false,
  },
];

/** `localStorage`, so the browser preview keeps its bookmarks too. */
function browserStore(): BookmarkStore {
  return {
    async load() {
      try {
        const raw = window.localStorage.getItem(KEY);
        return raw ? (JSON.parse(raw) as Bookmark[]) : DEMO_BOOKMARKS;
      } catch {
        // A corrupt or unreadable store is not worth failing the app over:
        // the bookmarks are a convenience, and an empty sidebar is recoverable
        // where a window that will not open is not.
        return [];
      }
    },
    async save(bookmarks) {
      try {
        window.localStorage.setItem(KEY, JSON.stringify(bookmarks));
      } catch {
        // Full or blocked storage: the mount still works for this session.
      }
    },
  };
}

export function defaultBookmarkStore(): BookmarkStore {
  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  return inTauri ? tauriStore() : browserStore();
}

/**
 * A stable id for a connection, so reconnecting the same share reuses its row.
 *
 * Local folds into the same shape rather than getting a scheme of its own: its
 * `host` is empty and its `share` is the absolute path, so the id reads
 * `local::/users/dorian/documents`. The protocol prefix is what keeps that
 * apart from everything else — no network bookmark can produce a `local:`
 * prefix — and an empty host cannot collide with a real one because the host
 * of a network bookmark is required (`validate`).
 *
 * The lower-casing is wrong for a case-sensitive path, and is kept anyway: an
 * *id* only has to be stable and to make "the same share" one row. The cost is
 * that `/data/Foo` and `/data/foo` on a case-sensitive filesystem would share
 * a row; the bookmark still carries the path exactly as typed, so the folder
 * that opens is never the wrong one. Casing the id per protocol would mean a
 * bookmark's identity depending on a filesystem property the window cannot
 * see, which is the worse trade.
 */
export function bookmarkId(draft: Draft): string {
  return `${draft.protocol}:${draft.host}:${draft.share}`.toLowerCase();
}

/** The bookmark a completed dialog produces. */
export function fromDraft(draft: Draft): Bookmark {
  return {
    id: bookmarkId(draft),
    name: connectionName(draft),
    protocol: draft.protocol,
    // Empty for a local folder, whose `share` is the whole address.
    host: usesHost(draft.protocol) ? draft.host.trim() : "",
    share: draft.share.trim(),
    username: draft.username || undefined,
    mountPreference: draft.mountPreference,
    mounted: false,
    // Assumed reachable until a probe says otherwise, so a fresh bookmark does
    // not flash as dimmed before the first probe completes.
    reachable: true,
  };
}

/** The draft that reopens the dialog on an existing bookmark. */
export function toDraft(bookmark: Bookmark): Draft {
  return {
    ...emptyDraft(bookmark.protocol),
    host: bookmark.host,
    share: bookmark.share,
    username: bookmark.username ?? "",
    mountPreference: bookmark.mountPreference,
  };
}

/**
 * Add or replace a bookmark.
 *
 * Replacing rather than appending: connecting to the same share twice should
 * update its row, not grow a second identical one.
 */
export function upsert(bookmarks: Bookmark[], bookmark: Bookmark): Bookmark[] {
  const index = bookmarks.findIndex((candidate) => candidate.id === bookmark.id);
  if (index === -1) return [...bookmarks, bookmark];
  const next = bookmarks.slice();
  next[index] = { ...next[index], ...bookmark };
  return next;
}

export function remove(bookmarks: Bookmark[], id: string): Bookmark[] {
  return bookmarks.filter((bookmark) => bookmark.id !== id);
}

/** Put a removed bookmark back where it was, for undo. */
export function restore(bookmarks: Bookmark[], bookmark: Bookmark, index: number): Bookmark[] {
  const next = bookmarks.slice();
  next.splice(Math.min(Math.max(index, 0), next.length), 0, bookmark);
  return next;
}

/** Mark which bookmarks are currently mounted. */
export function withMounted(bookmarks: Bookmark[], mountedIds: Set<string>): Bookmark[] {
  return bookmarks.map((bookmark) => ({ ...bookmark, mounted: mountedIds.has(bookmark.id) }));
}

/**
 * The port a reachability probe should try for a bookmark, or `null` when the
 * bookmark is not a thing that can be probed.
 *
 * A folder on this machine answers no TCP connect, and "unreachable" is not a
 * state it has: the disk is either there or the mount fails with a real error
 * that says so. Probing it would dim the row for a question it was never
 * asked, so `null` means *do not probe*, not "probe port zero".
 */
export function probePort(bookmark: Bookmark): number | null {
  switch (bookmark.protocol) {
    case "smb":
      return 445;
    case "nfs":
      return 2049;
    // xsync reaches its server by running `xs --server` over SSH, so the port
    // that matters is SSH's.
    case "sftp":
    case "xsync":
      return 22;
    case "local":
      return null;
  }
}

/**
 * The second line of a sidebar row: what this bookmark is and where.
 *
 * `LOCAL · ` — the protocol and an empty host — is what the network shape
 * produces for a folder, and it reads like a row that failed to load. A local
 * bookmark names the folder instead, which is the only part of it a user needs
 * to tell two of them apart.
 *
 * Reachability is deliberately not in here: only the BOOKMARKS section
 * replaces the subtitle with `unreachable`, and a row under MOUNTED is
 * connected whatever a stale probe last said.
 */
export function bookmarkSubtitle(bookmark: Bookmark): string {
  return bookmark.protocol === "local"
    ? `LOCAL · ${bookmark.share}`
    : `${bookmark.protocol.toUpperCase()} · ${bookmark.host}`;
}
