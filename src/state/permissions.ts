// Permission gating: one derived value, one place.
//
// The design's rule is that `mount.writable` — which comes from the server and
// travels with each listing — is the *only* input to every write affordance.
// The failure mode it exists to prevent is a UI that offers a write the mount
// will refuse, and that failure arrives by accretion: someone adds a verb and
// writes `disabled={...}` from whatever seemed relevant at the time.
//
// So the affordances are enumerated here rather than being implied by which
// components happen to pass the flag. A new verb has to be added to this list
// and sorted into read, write or recovery, and the tests fail until it is.

/** Every affordance whose availability depends on the mount. */
export type Affordance =
  | "quickLook"
  | "download"
  | "edit"
  | "upload"
  | "rename"
  | "delete"
  | "newFolder"
  | "dropTarget"
  | "remountRw";

/** Affordances that only read. A read-only mount does not touch these. */
export const READ_AFFORDANCES: readonly Affordance[] = ["quickLook", "download"];

/**
 * Affordances that write. Every one is disabled when `writable` is false.
 *
 * `dropTarget` is here because accepting a drop *is* offering a write: a list
 * that highlights on drag-over and then refuses the drop is worse than one
 * that never highlighted.
 */
export const WRITE_AFFORDANCES: readonly Affordance[] = [
  "edit",
  "upload",
  "rename",
  "delete",
  "newFolder",
  "dropTarget",
];

/**
 * Affordances that exist *because* the mount is read-only.
 *
 * Gated on the same value, inverted: offering "Remount as RW" on a mount that
 * already is read-write is an invitation to reconnect for no reason.
 */
export const RECOVERY_AFFORDANCES: readonly Affordance[] = ["remountRw"];

export const AFFORDANCES: readonly Affordance[] = [
  ...READ_AFFORDANCES,
  ...WRITE_AFFORDANCES,
  ...RECOVERY_AFFORDANCES,
];

/** Affordances that need something selected to act on. */
const NEEDS_SELECTION: readonly Affordance[] = ["quickLook", "download", "edit", "rename", "delete"];

/** What the window knows when it draws a verb. */
export interface Context {
  /** From the latest `list_dir`/`mount_info`. Never from what the user asked for. */
  writable: boolean;
  /** The server's explanation, shown verbatim when it refuses. */
  reason: string;
  hasSelection: boolean;
}

/** Whether an affordance is available, and why not. */
export interface Gate {
  disabled: boolean;
  /**
   * Tooltip text, or null when enabled.
   *
   * A read-only mount's reason wins over "nothing selected": it is the one the
   * user cannot fix by clicking a row, so it is the more useful thing to say.
   */
  reason: string | null;
}

export function gate(affordance: Affordance, context: Context): Gate {
  if (RECOVERY_AFFORDANCES.includes(affordance)) {
    return context.writable
      ? { disabled: true, reason: "Already mounted read-write" }
      : { disabled: false, reason: null };
  }

  if (WRITE_AFFORDANCES.includes(affordance) && !context.writable) {
    return { disabled: true, reason: context.reason };
  }

  if (NEEDS_SELECTION.includes(affordance) && !context.hasSelection) {
    return { disabled: true, reason: "Nothing selected" };
  }

  return { disabled: false, reason: null };
}

/** Whether the list should accept a drag hovering over it. */
export function acceptsDrop(context: Context): boolean {
  return !gate("dropTarget", context).disabled;
}

/**
 * The Quick Look footer's copy.
 *
 * The design gives the read-only wording, which explains what to do rather
 * than only what is refused.
 */
export function quickLookFooter(context: Context): string {
    return context.writable
      ? "Edit opens this in your default editor"
      : "Read-only — download a copy to edit";
}
