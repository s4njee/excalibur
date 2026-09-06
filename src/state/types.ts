// Mirrors `excalibur-engine`'s serde shapes. Kept by hand rather than
// generated: the set is small, and the Tauri DTO layer is where a mismatch
// would surface loudly anyway.

export type Protocol = "smb" | "nfs" | "sftp" | "xsync" | "local";
export type Access = "rw" | "ro";
export type MountStatus = "mounting" | "ready" | "error" | "reconnecting";
export type EntryKind = "file" | "dir" | "symlink" | "other";

/**
 * What sort of thing a file is, from the engine's kind table.
 *
 * Beside `kindLabel` rather than derived from it: the label is prose meant for
 * a person, and matching on prose to pick an icon or a viewer is how
 * "Matroska Video" and "MPEG-4 Video" end up handled differently by accident.
 */
export type Category =
  | "folder"
  | "alias"
  | "video"
  | "audio"
  | "image"
  | "pdf"
  | "text"
  | "code"
  | "document"
  | "archive"
  | "disk"
  | "application"
  | "font"
  | "data"
  | "other";

export interface Capacity {
  free: number;
  total: number;
}

export interface Caps {
  randomAccess: boolean;
  resumeWrite: boolean;
  atomicRename: boolean;
  symlinks: boolean;
  watch: boolean;
}

/**
 * Everything the status bar shows and every write affordance reads.
 *
 * `writable` is the single derived value the design gates on. It comes from
 * the server, travels with each listing, and is never inferred from what the
 * user asked for in the connection dialog.
 */
export interface MountInfo {
  status: MountStatus;
  writable: boolean;
  reason: string;
  protocolLabel: string;
  securityLabel: string;
  options: string;
  capacity: Capacity | null;
  caps: Caps;
}

export interface Entry {
  name: string;
  path: string;
  kind: EntryKind;
  size: number;
  modifiedMs: number | null;
  mode: number | null;
  linkTarget: string | null;
  kindLabel: string;
  kindCategory: Category;
  hidden: boolean;
}

/**
 * A mount changing state, pushed from the engine.
 *
 * `mount` is present on every `ready`, because that is the moment a window must
 * re-read `writable` rather than trusting what it had — a share can come back
 * from a reconnect read-only.
 */
export interface MountEvent {
  id: string;
  status: MountStatus;
  mount: MountInfo | null;
  reason: string | null;
}

export type TransferDirection = "up" | "down";
export type TransferStatus = "queued" | "running" | "paused" | "done" | "failed" | "cancelled";

/** One file moving in one direction. A folder expands to one of these per file. */
export interface Transfer {
  id: string;
  direction: TransferDirection;
  name: string;
  sourcePath: string;
  destPath: string;
  bytesDone: number;
  /** `0` when the size is not known, which the progress bar must handle. */
  bytesTotal: number;
  rate: number | null;
  etaSeconds: number | null;
  status: TransferStatus;
  error: string | null;
  queuedAtMs: number;
  /** The share this row moves bytes through, when it names one. */
  mount?: string | null;
  /**
   * Whether every byte that landed was checked against the source's digest.
   *
   * Set only by the route through xsync's engine (E22-S1). A finished row says
   * `verified` instead of `done` when it is true, and the two words are claims
   * of different strength rather than decoration.
   */
  verified?: boolean;
  /** A short phrase a finished row shows instead of a size, e.g. `unchanged`. */
  note?: string | null;
}

/** A destination that already exists, awaiting a decision. */
export interface Conflict {
  transferId: string;
  name: string;
  destPath: string;
  /** What "Keep both" would call it — the engine decides the name. */
  keepBothAs: string;
}

/** A file checked out for editing. */
export interface EditSession {
  id: string;
  remotePath: string;
  localPath: string;
  name: string;
  saves: number;
}

/** What a write-back did. */
export type SaveOutcome =
  | { outcome: "saved"; saves: number }
  /** The remote moved. Nothing was written; the user decides. */
  | { outcome: "conflict"; size: number; modifiedMs: number | null }
  | { outcome: "unchanged" };

export interface Listing {
  entries: Entry[];
  mount: MountInfo;
}

/** A saved connection. Never holds a secret; those live in the OS keychain. */
export interface Bookmark {
  id: string;
  name: string;
  protocol: Protocol;
  host: string;
  share: string;
  username?: string;
  /** Windows domain or workgroup. SMB only. */
  domain?: string;
  mountPreference: Access;
  /** Mounted and usable right now. */
  mounted: boolean;
  /** Probed and not answering; the row dims and reads "unreachable". */
  reachable: boolean;
}

export type SortColumn = "name" | "size" | "modified" | "kind";
export type RowLayout = "columns" | "name-led" | "mono";
export type Theme = "light" | "dark";
