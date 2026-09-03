// Mirrors `excalibur-engine`'s serde shapes. Kept by hand rather than
// generated: the set is small, and the Tauri DTO layer is where a mismatch
// would surface loudly anyway.

export type Protocol = "smb" | "nfs" | "sftp" | "xsync";
export type Access = "rw" | "ro";
export type MountStatus = "mounting" | "ready" | "error" | "reconnecting";
export type EntryKind = "file" | "dir" | "symlink" | "other";

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
  hidden: boolean;
}

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
  mountPreference: Access;
  /** Mounted and usable right now. */
  mounted: boolean;
  /** Probed and not answering; the row dims and reads "unreachable". */
  reachable: boolean;
}

export type SortColumn = "name" | "size" | "modified" | "kind";
export type RowLayout = "columns" | "name-led" | "mono";
export type Theme = "light" | "dark";
