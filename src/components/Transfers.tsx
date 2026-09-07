import { createMemo, createSignal, For, Show, onCleanup, onMount } from "solid-js";
import styles from "./transfers.module.css";
import { ContextMenu } from "./ContextMenu";
import { DownloadsTable, transferRemountable } from "./DownloadsTable";
import { Sparkline } from "./Sparkline";
import { fractionOf } from "../state/downloads";
import { activeCount, aggregateRate, pushSample, remainingBytes } from "../state/throughput";
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
  // The aggregate is of what is *moving*. Including paused rows at zero would
  // drag it down and misdescribe the link.
  const aggregate = () => aggregateRate(props.transfers);

  /**
   * The sparkline's ring (XDL-E2-S4): the last 60 one-second samples of the
   * aggregate rate. Sampled on a timer rather than on every transfer event —
   * a rate can change without a new event, and the reference's "last 60s" is
   * a time window, not a count of messages. Drawn from the queue the window
   * already holds; no second data source.
   */
  const [samples, setSamples] = createSignal<number[]>([]);
  onMount(() => {
    const timer = setInterval(() => {
      setSamples((current) => pushSample(current, aggregate()));
    }, 1000);
    onCleanup(() => clearInterval(timer));
  });

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
      class={`${styles.panel} ${props.embedded ? styles.panelEmbedded : ""}`}
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

      {/* The tab is the reference's table; the sheet keeps its compact rows.
          Two densities for two jobs, sharing the header, the bulk menu and
          the footer above and below (XDL-E2-S2). */}
      <Show
        when={props.embedded}
        fallback={
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
        }
      >
        <DownloadsTable
          transfers={props.transfers}
          onPause={props.onPause}
          onCancel={props.onCancel}
          onRetry={props.onRetry}
          onRemount={props.onRemount}
        />
      </Show>

      <div class={styles.footer}>
        <span class={styles.footerStats}>
          {/* The reference's "3 active · 48 connections". The count we have is
              active rows; the connection count is the engine's and unplumbed
              to the row (XD-E2-S4), so the footer says nothing rather than
              invent one (XDL-E2-S4). */}
          <span class={styles.footerMono}>
            {activeCount(props.transfers)} active
          </span>
          <span class={styles.footerMono}>
            {formatSize(remainingBytes(props.transfers))} remaining
          </span>
        </span>
        <Show when={aggregate() > 0}>
          <span class={styles.aggregate}>{formatRate(aggregate())} aggregate</span>
        </Show>
        <Sparkline samples={samples()} />
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
  // nothing. Shared with the downloads table, which offers the same action.
  const remountable = () => transferRemountable(props.transfer);
  // The engine's rule (XDL-E2-S3): null while the size is unknown, so an
  // unfinished row that cannot name a total draws indeterminate rather than
  // a bar parked at zero.
  const fraction = () => fractionOf(props.transfer);

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
            and its bar has not started, which is exactly the state. A row
            that cannot name a total draws indeterminate instead — a bar at
            zero for a whole transfer looks broken (XDL-E2-S3). */}
        {fraction() === null ? (
          <div
            class={styles.track}
            role="progressbar"
            aria-label={`${props.transfer.name} progress, size unknown`}
          >
            <div class={styles.fillIndeterminate} />
          </div>
        ) : (
          <div
            class={styles.track}
            role="progressbar"
            aria-label={`${props.transfer.name} progress`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round((fraction() ?? 0) * 100)}
          >
            <div class={styles.fill} style={{ width: `${(fraction() ?? 0) * 100}%` }} />
          </div>
        )}
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
