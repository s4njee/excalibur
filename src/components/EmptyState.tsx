import styles from "./shell.module.css";
/**
 * The list area with no share behind it (E1-S6).
 *
 * The handoff has no mock for a window that is not connected to anything —
 * every screen it draws has a share in it — so this is deliberately the
 * smallest thing that answers "what now": the state, and the one action that
 * changes it. Recorded in `docs/design-gaps.md`.
 */
export function EmptyState(props: { onNewConnection: () => void }) {
  return (
    <div class={styles.emptyState}>
      <div class={styles.emptyTitle}>No share connected</div>
      <p class={styles.emptyBody}>
        Connect to an SMB, NFS, SFTP or xsync share, open a folder on this machine, or
        open one of your bookmarks.
      </p>
      <button
        class={`${styles.button} ${styles.primary}`}
        onClick={props.onNewConnection}
      >
        New connection…
      </button>
    </div>
  );
}
