import { createEffect, createMemo, createSignal, For, Show, onCleanup, onMount } from "solid-js";
import styles from "./transfers.module.css";
import { ContextMenu } from "./ContextMenu";
import { DownloadsTable } from "./DownloadsTable";
import { Sparkline } from "./Sparkline";
import { filterByCategory, type DownloadCategory } from "../state/downloads";
import { resized, type PaneSize } from "../state/paneSize";
import { activeCount, aggregateRate, pushSample, remainingBytes } from "../state/throughput";
import { formatRate, formatSize } from "../state/format";
import { transferMenu, type TransferMenuAction } from "../state/transferMenu";
import type { Transfer } from "../state/types";
import type { NamedQueue } from "../state/types";
import { QueueControls } from "./QueueControls";

/**
 * The transfer queue (E13-S1, E13-S2).
 *
 * A panel rather than a second OS window. The design offers "separate window
 * (default) or slide-down sheet (preference)"; this is the sheet, and the
 * preference that would offer the other has nowhere to live until E17.
 * Recorded in `docs/design-gaps.md`.
 *
 * The only transfers surface there is: the window always browses, and the
 * categories that used to be sidebar rows are this panel's tab strip. It
 * draws the reference's table rather than a second, compact row shape — one
 * queue seen one way, so the two densities can no longer disagree.
 */
export function Transfers(props: {
  transfers: Transfer[];
  /** Required: a sheet with no way out is a trap. */
  onClose: () => void;
  /**
   * The queue grouped the way the reference groups it (XDL-E2-S1), with counts
   * off the *whole* queue so a tab's number does not change as tabs are
   * switched. Owned by the window, which persists the chosen one.
   */
  categories: { id: DownloadCategory; label: string; count: number }[];
  category: DownloadCategory;
  onCategory: (id: DownloadCategory) => void;
  /** Remembered across launches; `height: null` is as tall as the queue needs. */
  size: PaneSize;
  /** During a drag, every frame. */
  onResize: (size: PaneSize) => void;
  /** On release, which is when it is worth writing to the store. */
  onResizeEnd: () => void;
  onPauseAll: () => void;
  onClearFinished: () => void;
  onRetryFailed: () => void;
  onClearFailed: () => void;
  onPause: (id: string) => void;
  onCancel: (id: string) => void;
  onRemove?: (id: string) => void;
  onRetry: (id: string) => void;
  onOpenFolder?: (path: string) => void;
  onCopyUrl?: (url: string) => void;
  /** Offered on a failed upload that a writable mount would fix. */
  onRemount?: () => void;
  queues?: NamedQueue[];
  onCreateQueue?: (name: string, color: string, concurrency: number) => void;
  onUpdateQueue?: (id: string, name: string, color: string, concurrency: number) => void;
  onDeleteQueue?: (id: string) => void;
  onMoveToQueue?: (id: string, queueId: string) => void;
  onReorderQueue?: (queueId: string, ids: string[]) => void;
  onReorderQueues?: (ids: string[]) => void;
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

  const [selectedQueue, setSelectedQueue] = createSignal("all");
  const queues = () => props.queues ?? [];
  // Two independent narrowings, applied in the order the user thinks of them:
  // the tab says which *kind* of row, the queue control says which *queue*.
  const queueTransfers = createMemo(() => {
    const byCategory = filterByCategory(props.transfers, props.category);
    return selectedQueue() === "all"
      ? byCategory
      : byCategory.filter((transfer) => (transfer.queueId ?? "default") === selectedQueue());
  });
  createEffect(() => {
    const id = selectedQueue();
    if (id !== "all" && !queues().some((queue) => queue.id === id)) setSelectedQueue("all");
  });
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

  let panel!: HTMLDivElement;

  /**
   * Drag one edge, or the corner.
   *
   * The measurements are taken here rather than in `paneSize` because they are
   * the DOM's: what the panel is drawing right now (which is how a `null`
   * height becomes a number the user can pull on), and how much room it has
   * left in the window. `resized` is the arithmetic over those.
   */
  const startResize = (edges: { left: boolean; bottom: boolean }) => (event: PointerEvent) => {
    event.preventDefault();
    event.stopPropagation();
    const box = panel.getBoundingClientRect();
    const start = { width: box.width, height: box.height };
    const origin = { x: event.clientX, y: event.clientY };
    // The pane is anchored under the chrome, so the room below it is the
    // window minus where it starts. A margin keeps a corner reachable.
    const available = {
      width: Math.max(0, window.innerWidth - 16),
      height: Math.max(0, window.innerHeight - box.top - 16),
    };
    const move = (moved: PointerEvent) => {
      props.onResize(
        resized(start, { x: moved.clientX - origin.x, y: moved.clientY - origin.y }, edges, available),
      );
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      props.onResizeEnd();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
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
      ref={panel}
      class={styles.panel}
      role="dialog"
      aria-label="Transfers"
      style={{
        width: `${props.size.width}px`,
        ...(props.size.height === null ? {} : { height: `${props.size.height}px` }),
      }}
    >
      {/* Three grips for two dimensions. `separator` with an orientation is
          what the column dividers use, and the corner is given the vertical
          one because width is the dimension people reach for it to change. */}
      <div
        class={styles.resizeLeft}
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize pane width"
        onPointerDown={startResize({ left: true, bottom: false })}
      />
      <div
        class={styles.resizeBottom}
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize pane height"
        onPointerDown={startResize({ left: false, bottom: true })}
      />
      <div
        class={styles.resizeCorner}
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize pane"
        onPointerDown={startResize({ left: true, bottom: true })}
      />
      <div class={styles.header}>
        <span class={styles.title}>Transfers</span>
        <Show when={props.queues && props.onCreateQueue && props.onUpdateQueue && props.onDeleteQueue && props.onMoveToQueue && props.onReorderQueue && props.onReorderQueues}>
          <QueueControls
            queues={queues()}
            selected={selectedQueue()}
            onSelect={setSelectedQueue}
            onCreate={props.onCreateQueue!}
            onUpdate={props.onUpdateQueue!}
            onDelete={props.onDeleteQueue!}
            onReorder={props.onReorderQueues!}
          />
        </Show>
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
        <button class={styles.headerAction} aria-label="Close" onClick={props.onClose}>
          ✕
        </button>
      </div>

      {/* CATEGORIES (XDL-E2-S1), which used to be sidebar rows. A tab strip
          rather than a list: these narrow one queue rather than switching the
          window between locations, and a location list is what the sidebar is
          for. A count is live and a zero is an answer, so every tab is drawn
          whether or not it has rows. Recorded in `docs/design-gaps.md`. */}
      <div class={styles.tabs} role="tablist" aria-label="Categories">
        <For each={props.categories}>
          {(category) => {
            const active = () => category.id === props.category;
            return (
              <button
                class={`${styles.tab} ${active() ? styles.tabActive : ""}`}
                role="tab"
                data-category={category.id}
                aria-selected={active()}
                onClick={() => props.onCategory(category.id)}
              >
                {category.label}
                <span class={styles.tabCount}>{category.count}</span>
              </button>
            );
          }}
        </For>
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

      <DownloadsTable
        transfers={queueTransfers()}
        onPause={props.onPause}
        onCancel={props.onCancel}
        onRemove={props.onRemove}
        onRetry={props.onRetry}
        onOpenFolder={props.onOpenFolder}
        onCopyUrl={props.onCopyUrl}
        onRemount={props.onRemount}
        queues={queues()}
        selectedQueue={selectedQueue()}
        onMoveToQueue={props.onMoveToQueue}
        onReorder={props.onReorderQueue}
      />

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
