import { For, Show } from "solid-js";
import styles from "./transfers.module.css";
import { formatRate, formatSize, formatTransferStatus } from "../state/format";
import type { Transfer } from "../state/types";

/**
 * The transfer queue (E13-S1, E13-S2).
 *
 * A panel rather than a second OS window. The design offers "separate window
 * (default) or slide-down sheet (preference)"; this is the sheet, and the
 * preference that would offer the other has nowhere to live until E17.
 * Recorded in `docs/design-gaps.md`.
 */
export function Transfers(props: {
  transfers: Transfer[];
  onClose: () => void;
  onPauseAll: () => void;
  onClearFinished: () => void;
  onPause: (id: string) => void;
  onCancel: (id: string) => void;
  onRetry: (id: string) => void;
  /** Offered on a failed upload that a writable mount would fix. */
  onRemount?: () => void;
}) {
  const counts = () => {
    const active = props.transfers.filter((t) => t.status === "running").length;
    const queued = props.transfers.filter((t) => t.status === "queued").length;
    const failed = props.transfers.filter((t) => t.status === "failed").length;
    return { active, queued, failed };
  };

  // The aggregate is of what is *moving*. Including paused rows at zero would
  // drag it down and misdescribe the link.
  const aggregate = () =>
    props.transfers
      .filter((transfer) => transfer.status === "running")
      .reduce((total, transfer) => total + (transfer.rate ?? 0), 0);

  return (
    <div class={styles.panel} role="dialog" aria-label="Transfers">
      <div class={styles.header}>
        <span class={styles.title}>Transfers</span>
        <button class={styles.headerAction} onClick={props.onPauseAll}>
          Pause all
        </button>
        <button class={styles.headerAction} onClick={props.onClearFinished}>
          Clear finished
        </button>
        <button class={styles.headerAction} aria-label="Close" onClick={props.onClose}>
          ✕
        </button>
      </div>

      <div class={styles.rows}>
        <Show
          when={props.transfers.length > 0}
          fallback={<div class={styles.empty}>Nothing transferring</div>}
        >
          <For each={props.transfers}>
            {(transfer) => (
              <Row
                transfer={transfer}
                onPause={() => props.onPause(transfer.id)}
                onCancel={() => props.onCancel(transfer.id)}
                onRetry={() => props.onRetry(transfer.id)}
                onRemount={props.onRemount}
              />
            )}
          </For>
        </Show>
      </div>

      <div class={styles.footer}>
        <span>
          {counts().active} active · {counts().queued} queued · {counts().failed} failed
        </span>
        <Show when={aggregate() > 0}>
          <span class={styles.aggregate}>{formatRate(aggregate())} aggregate</span>
        </Show>
      </div>
    </div>
  );
}

function Row(props: {
  transfer: Transfer;
  onPause: () => void;
  onCancel: () => void;
  onRetry: () => void;
  onRemount?: () => void;
}) {
  const finished = () =>
    props.transfer.status === "done" || props.transfer.status === "cancelled";
  const failed = () => props.transfer.status === "failed";
  // The engine decides this: it knows whether the failure was a permission
  // problem on an upload, which is the only case reconnecting read-write
  // would fix. Offering it on a timeout would be a button that changes
  // nothing.
  const remountable = () =>
    failed() &&
    props.transfer.direction === "up" &&
    /read-only|permission/i.test(props.transfer.error ?? "");

  const fraction = () =>
    props.transfer.bytesTotal > 0
      ? Math.min(1, props.transfer.bytesDone / props.transfer.bytesTotal)
      : 0;

  return (
    <div class={`${styles.row} ${finished() ? styles.rowFinished : ""}`}>
      <div class={styles.rowTop}>
        <span
          class={`${styles.chip} ${
            failed() ? styles.chipFailed : props.transfer.direction === "up" ? styles.chipUp : styles.chipDown
          }`}
        >
          {props.transfer.direction === "up" ? "↑ UP" : "↓ DOWN"}
        </span>
        <span
          class={`${styles.name} ${props.transfer.status === "running" ? styles.nameActive : ""}`}
        >
          {props.transfer.name}
        </span>
        <span
          class={`${styles.status} ${props.transfer.status === "done" ? styles.statusDone : ""}`}
        >
          {formatTransferStatus(props.transfer)}
        </span>
      </div>

      <Show
        when={!finished() && !failed()}
        fallback={
          <Show when={failed()}>
            <div class={styles.rowFailure}>
              <span class={styles.error}>{props.transfer.error}</span>
              <button class={styles.rowAction} onClick={props.onRetry}>
                Retry
              </button>
              <Show when={remountable() && props.onRemount}>
                {(remount) => (
                  <button class={styles.rowAction} onClick={() => remount()()}>
                    Remount as RW
                  </button>
                )}
              </Show>
            </div>
          </Show>
        }
      >
        {/* Queued rows show an empty track, per the design: the row is there
            and its bar has not started, which is exactly the state. */}
        <div class={styles.track}>
          <div class={styles.fill} style={{ width: `${fraction() * 100}%` }} />
        </div>
      </Show>

      <div class={styles.rowBottom}>
        <span class={styles.dest}>{props.transfer.destPath}</span>
        <Show when={props.transfer.status === "running" || props.transfer.status === "queued"}>
          <button class={styles.rowAction} onClick={props.onPause}>
            Pause
          </button>
          <button class={styles.rowAction} onClick={props.onCancel}>
            Cancel
          </button>
        </Show>
        <Show when={props.transfer.status === "paused"}>
          <button class={styles.rowAction} onClick={props.onRetry}>
            Resume
          </button>
        </Show>
      </div>
    </div>
  );
}

/** Bytes so far across everything unfinished, for the toolbar's hint. */
export function pendingBytes(transfers: Transfer[]): number {
  return transfers
    .filter((transfer) => transfer.status === "running" || transfer.status === "queued")
    .reduce((total, transfer) => total + Math.max(0, transfer.bytesTotal - transfer.bytesDone), 0);
}

export { formatSize };
