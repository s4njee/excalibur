import { createMemo, createSignal, For, Show } from "solid-js";
import styles from "./transfers.module.css";
import { ContextMenu } from "./ContextMenu";
import { formatRate, formatSize, formatTransferStatus } from "../state/format";
import { transferMenu, type TransferMenuAction } from "../state/transferMenu";
import type { Transfer } from "../state/types";

/**
 * Rows drawn before the panel stops and offers the rest.
 *
 * Generous on purpose: an ordinary queue is a handful of rows and never reaches
 * this, and a folder is one row now (E22-S3). It exists for the batch of
 * individual files that used to be the normal case, where the panel opening on
 * launch cost seconds of a blank window.
 */
const ROW_CAP = 200;

/**
 * The transfer queue (E13-S1, E13-S2).
 *
 * A panel rather than a second OS window. The design offers "separate window
 * (default) or slide-down sheet (preference)"; this is the sheet, and the
 * preference that would offer the other has nowhere to live until E17.
 * Recorded in `docs/design-gaps.md`.
 *
 * `embedded` draws the same queue as the window's downloads view instead
 * (XDL-E2-S1): a region titled Downloads with no Close button, because there
 * is no panel to dismiss — leaving is the sidebar's job. The rows, the bulk
 * menu and the footer are untouched, so the sheet and the tab cannot disagree.
 */
export function Transfers(props: {
  transfers: Transfer[];
  /** Absent in the embedded view, where leaving is the sidebar's job. */
  onClose?: () => void;
  /** Draw as the downloads view rather than the sheet. */
  embedded?: boolean;
  onPauseAll: () => void;
  onClearFinished: () => void;
  onRetryFailed: () => void;
  onClearFailed: () => void;
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

  /** How many rows are drawn before the panel offers the rest. */
  const [cap, setCap] = createSignal<number>(ROW_CAP);
  const shown = createMemo(() =>
    props.transfers.length > cap() ? props.transfers.slice(0, cap()) : props.transfers,
  );

  /** Where the overflow menu was opened, while it is open. */
  const [menuAt, setMenuAt] = createSignal<{ x: number; y: number } | null>(null);
  let hamburger!: HTMLButtonElement;

  const toggleMenu = () => {
    if (menuAt() !== null) {
      setMenuAt(null);
      return;
    }
    const box = hamburger.getBoundingClientRect();
    // Under the button rather than at the pointer: this menu is opened from a
    // fixed control, not by right-clicking a row, so it should appear in the
    // same place whether it was reached by mouse or by keyboard.
    setMenuAt({ x: box.left, y: box.bottom + 2 });
  };

  const runMenuAction = (action: TransferMenuAction) => {
    setMenuAt(null);
    switch (action) {
      case "retryFailed":
        props.onRetryFailed();
        return;
      case "clearFailed":
        props.onClearFailed();
        return;
      case "clearFinished":
        props.onClearFinished();
        return;
      case "pauseAll":
        props.onPauseAll();
    }
  };

  return (
    <div
      class={styles.panel}
      role={props.embedded ? "region" : "dialog"}
      aria-label={props.embedded ? "Downloads" : "Transfers"}
    >
      <div class={styles.header}>
        <span class={styles.title}>{props.embedded ? "Downloads" : "Transfers"}</span>
        {/* The bulk actions live behind one control (asked for directly,
            2026-09-07). The handoff draws three buttons in this header and
            there are now four verbs, two of them about failures — recorded in
            docs/design-gaps.md. Close stays a button of its own: it acts on the
            panel, and everything in the menu acts on the queue. */}
        <button
          class={styles.headerAction}
          ref={hamburger}
          aria-label="Queue actions"
          aria-haspopup="menu"
          aria-expanded={menuAt() !== null}
          onClick={toggleMenu}
        >
          ☰
        </button>
        <Show when={!props.embedded && props.onClose}>
          {(close) => (
            <button class={styles.headerAction} aria-label="Close" onClick={() => close()()}>
              ✕
            </button>
          )}
        </Show>
      </div>

      <Show when={menuAt()}>
        {(at) => (
          <ContextMenu
            items={transferMenu(props.transfers)}
            at={at()}
            label="Queue actions"
            anchor={hamburger}
            onChoose={runMenuAction}
            onClose={() => setMenuAt(null)}
          />
        )}
      </Show>

      <div class={styles.rows}>
        <Show
          when={props.transfers.length > 0}
          fallback={<div class={styles.empty}>Nothing transferring</div>}
        >
          <For each={shown()}>
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
          {/* A queue this long is a batch of individual files, and every row
              carries a bar and a caption — rendering all of them is seconds of
              a frozen window, which is exactly what the panel opening on launch
              used to cost. Bounded rather than virtualised because these rows
              are not one height: a finished row drops its bar. The rest are one
              click away, and the click is the user accepting the wait. */}
          <Show when={props.transfers.length > shown().length}>
            <button class={styles.showAll} onClick={() => setCap(Infinity)}>
              Show {props.transfers.length - shown().length} more
            </button>
          </Show>
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
    <div
      class={`${styles.row} ${finished() ? styles.rowFinished : ""}`}
      // Which transfer this row is, the way a file row carries `data-path`.
      // Class names are mangled by CSS modules, so there is otherwise nothing
      // stable to count rows by.
      data-transfer={props.transfer.id}
    >
      <div class={styles.rowTop}>
        <span
          class={`${styles.chip} ${
            failed() ? styles.chipFailed : props.transfer.direction === "up" ? styles.chipUp : styles.chipDown
          }`}
        >
          {props.transfer.from?.kind === "http"
            ? "↓ HTTP"
            : props.transfer.direction === "up" ? "↑ UP" : "↓ DOWN"}
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
