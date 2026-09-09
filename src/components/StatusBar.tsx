import { Show } from "solid-js";
import styles from "./shell.module.css";
import type { MountInfo } from "../state/types";
import { formatCapacity, formatCounts, formatMountLabels } from "../state/format";
/**
 * The permission surface. Reading left to right, a user learns whether they can
 * write before anything else, which is the whole point of its ordering.
 */
export function StatusBar(props: {
  /** Absent when nothing is mounted (E1-S6). */
  mount?: MountInfo;
  path: string;
  items: number;
  selected: number;
  selectedBytes: number;
  /** The unfiltered count, when a filter is hiding rows. */
  total?: number;
  /** Reconnect asking for read-write. Absent when there is no mount to retry. */
  onRemount?: () => void;
  /** A remount is in flight. */
  remounting?: boolean;
}) {
  return (
    // A live region (E16-S2): whether the mount is writable, and why it is
    // not, changes without the user doing anything — a reconnect can come back
    // read-only. `polite` because it should not interrupt, and it is the only
    // place that news appears.
    <div class={styles.statusbar} role="status" aria-live="polite">
      {/* Nothing is claimed when there is no mount (E1-S6): no badge, no
          capacity, no protocol. `RO` here would describe a share the window
          does not have, and the badge is the one thing in this bar a user is
          meant to be able to trust. */}
      <Show when={props.mount} fallback={<span>Not connected</span>}>
        {(mount) => (
          <>
            <span
              class={`${styles.badge} ${mount().writable ? styles.badgeRw : styles.badgeRo}`}
            >
              {mount().writable ? "RW" : "RO"}
            </span>
            <span class={styles.truncate}>
              <Show
                when={mount().writable}
                fallback={
                  <>
                    <span>{mount().reason}</span>
                    {/* The design's only Remount button lives on a failed
                        transfer row (E13-S2), which does not exist yet —
                        leaving a read-only mount with no recovery path at all.
                        Recorded in docs/design-gaps.md for sign-off. */}
                    <Show when={props.onRemount}>
                      {(remount) => (
                        <button
                          class={styles.statusAction}
                          disabled={props.remounting}
                          onClick={() => remount()()}
                        >
                          {props.remounting ? "Remounting…" : "Remount as RW"}
                        </button>
                      )}
                    </Show>
                  </>
                }
              >
                Read &amp; write on <span class={styles.statusPath}>{props.path}</span>
              </Show>
            </span>
            <span class={styles.divider} />
            <span>
              {formatCounts(props.items, props.selected, props.selectedBytes, props.total)}
            </span>
            <Show when={mount().capacity}>
              {(capacity) => (
                <>
                  <span class={styles.divider} />
                  <span>{formatCapacity(capacity().free, capacity().total)}</span>
                </>
              )}
            </Show>
            <span class={styles.statusRight}>
              <span class={styles.protocol}>
                {formatMountLabels(mount().protocolLabel, mount().securityLabel)}
              </span>
              <span
                class={`${styles.healthDot} ${
                  mount().status === "ready" ? "" : styles.healthDotFailed
                }`}
              />
            </span>
          </>
        )}
      </Show>
    </div>
  );
}

export { styles as shellStyles };
