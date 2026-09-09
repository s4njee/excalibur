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
  /** When the file came into being here; `null` where nothing can say. */
  addedMs: number | null;
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

/**
 * The message log's six categories, which are also its six colours
 * (`twopane.md` XTP-E5-S1).
 */
export type LogCategory =
  | "command"
  | "response"
  | "status"
  | "listing"
  | "hint"
  | "error";

/** A recovery a log line offers, named by verb rather than by label. */
export type LogAction = "remountRw";

export interface LogLine {
  /** Milliseconds since the Unix epoch; the pane draws `HH:MM:SS`. */
  at: number;
  category: LogCategory;
  /** Verbatim. A message the server sent is never paraphrased. */
  text: string;
  action?: LogAction;
  /** Which mount the line is about, when it is about one. */
  mount?: string;
}

export type TransferDirection = "up" | "down";
export type TransferEndpoint =
  | { kind: "local"; path: string }
  | { kind: "share"; mount: string | null; path: string }
  | { kind: "http"; url: string };
export type TransferStatus = "queued" | "running" | "paused" | "done" | "failed" | "cancelled";

/** A coalesced snapshot of the provider's real per-file ranges. */
export interface TransferProgress {
  files: TransferFileProgress[];
  retries: number;
  redirects: number;
}

export interface TransferFileProgress {
  path: string;
  completed: number;
  /** `null` means the provider has no declared total. */
  total: number | null;
  segments: TransferSegmentProgress[];
}

export interface TransferSegmentProgress {
  stream: number;
  offset: number;
  length: number | null;
  written: number;
  rate: number;
  stalled: boolean;
}

/** A persisted app-level download queue and its scheduling policy. */
export interface NamedQueue {
  id: string;
  name: string;
  color: string;
  concurrency: number;
  order: number;
}

/** Per-file overrides; mirror indices include the primary URL at index zero. */
export interface HttpDownloadOptions {
  connections?: number | null;
  /** Alternate URLs for this same object, not additional download jobs. */
  mirrors?: string[];
  preferredMirror?: number | null;
  retry?: HttpRetryPolicy | null;
}

export interface HttpRetryPolicy {
  maxAttempts: number;
  initialWaitSeconds: number;
  maxWaitSeconds: number;
}

/** Limits enforced by the native download route, shared across windows/jobs. */
export interface HttpDownloadLimits {
  maxConnections: number;
  maxPerHost: number;
}

/** A probe is advisory. Unknown size/range support never prevents Start. */
export interface HttpProbe {
  length: number | null;
  ranges: boolean | null;
  filename: string | null;
  finalUrl: string | null;
  warning: string | null;
}

/** One item moving in one direction — a file, or a whole folder (E22-S3). */
export interface Transfer {
  id: string;
  /** Legacy share direction; HTTP rows have an explicit from/to pair. */
  direction: TransferDirection | null;
  from?: TransferEndpoint | null;
  to?: TransferEndpoint | null;
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
  /**
   * Files the planner has finished with, of the ones it found.
   *
   * Both `0` until the planner's scan ends, and for a single-file row they
   * stay that way. A folder row is the reason they exist: it is one row for a
   * tree, and a byte count alone cannot say where in that tree the copy is.
   */
  filesDone?: number;
  filesTotal?: number;
  /** Live per-file connection lanes; omitted from durable queue state. */
  progress?: TransferProgress | null;
  /** Absent on legacy queues and ordinary share transfers. */
  httpOptions?: HttpDownloadOptions | null;
  /** Named queue membership and persisted priority (lower runs first). */
  queueId?: string;
  queuePosition?: number | null;
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

export type SortColumn = "name" | "size" | "added" | "modified" | "kind";
export type RowLayout = "columns" | "name-led" | "mono" | "icons" | "column-view";

/**
 * One pane or two (`twopane.md` XTP-E7-S1).
 *
 * A separate axis from `RowLayout`, because a pane still draws rows in one of
 * the layouts. Both are chosen from the View menu.
 */
export type PaneMode = "single" | "dual";
export type Theme = "light" | "dark";
