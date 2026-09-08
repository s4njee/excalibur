import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import styles from "./downloadsTable.module.css";
import { ContextMenu } from "./ContextMenu";
import { visibleRange } from "../state/virtual";
import { formatDuration, formatRate, formatSize } from "../state/format";
import { fractionOf, originHost } from "../state/downloads";
import type { NamedQueue, Transfer } from "../state/types";
import { downloadMenu, type DownloadMenuAction } from "../state/menu";
import { DownloadDetail } from "./DownloadDetail";

/**
 * Every table row is one height, which is what keeps the virtual window
 * arithmetic rather than measurement — the same rule as the file list
 * (E8-S2). Two lines: the filename over the origin host.
 */
export const DOWNLOAD_ROW_HEIGHT = 52;

/** The reference's five states, in this app's badge tokens (XDL-E2-S2). */
function badge(transfer: Transfer): { label: string; tone: string | undefined } {
  switch (transfer.status) {
    case "running":
      return { label: "Downloading", tone: styles.badgeDownloading };
    case "queued":
      return { label: "Queued", tone: styles.badgeQueued };
    case "paused":
      return { label: "Paused", tone: styles.badgePaused };
    case "done":
      return { label: "Completed", tone: styles.badgeCompleted };
    case "failed":
      return { label: "Failed", tone: styles.badgeFailed };
    case "cancelled":
      return { label: "Cancelled", tone: styles.badgePaused };
  }
}

/**
 * The slice of the queue in the DOM for a scroll position — the file list's
 * `visibleRange` over this table's row height, not a second virtualisation
 * (XDL-E2-S2). Exported so a test can state the windowing without scrolling.
 */
export function downloadWindow(
  transfers: Transfer[],
  scrollTop: number,
  viewportHeight: number,
): Transfer[] {
  const range = visibleRange(scrollTop, viewportHeight, DOWNLOAD_ROW_HEIGHT, transfers.length);
  return transfers.slice(range.start, range.end);
}

/** Whether a failed row may offer reconnecting read-write (E9-S3's rule). */
export function transferRemountable(transfer: Transfer): boolean {
  return (
    transfer.status === "failed" &&
    transfer.direction === "up" &&
    /read-only|permission/i.test(transfer.error ?? "")
  );
}

/**
 * The reference's download table: file, size, progress, speed, ETA, status.
 * Language is sans, machine values are mono and right-aligned where numeric;
 * the only actions are the row's own, in a narrow trailing column.
 */
export function DownloadsTable(props: {
  transfers: Transfer[];
  onPause: (id: string) => void;
  onCancel: (id: string) => void;
  onRetry: (id: string) => void;
  onRemove?: (id: string) => void;
  onOpenFolder?: (path: string) => void;
  onCopyUrl?: (url: string) => void;
  /** Offered on a failed upload that a writable mount would fix. */
  onRemount?: () => void;
  queues?: NamedQueue[];
  selectedQueue?: string;
  onMoveToQueue?: (id: string, queueId: string) => void;
  onReorder?: (queueId: string, ids: string[]) => void;
}) {
  // Scroll position and viewport height are the only two things the virtual
  // window needs — same as the file list, which owns the same two signals.
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewport, setViewport] = createSignal(0);
  const [menu, setMenu] = createSignal<{ transfer: Transfer; x: number; y: number } | null>(null);
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  let scroller: HTMLDivElement | undefined;

  onMount(() => {
    const measure = () => setViewport(scroller?.clientHeight ?? 0);
    measure();
    const onScroll = () => setScrollTop(scroller?.scrollTop ?? 0);
    scroller?.addEventListener("scroll", onScroll);
    // The file list assumes a ResizeObserver unconditionally and its tests
    // shim one; the table degrades to the overscan window instead, which is
    // still bounded and still the same arithmetic.
    const observer =
      typeof ResizeObserver === "undefined" || !scroller
        ? null
        : new ResizeObserver(measure);
    if (scroller && observer) observer.observe(scroller);
    onCleanup(() => {
      scroller?.removeEventListener("scroll", onScroll);
      observer?.disconnect();
    });
  });

  // A filtered queue shorter than the last one can leave the container
  // scrolled past its own end — the file list carries the same guard.
  createEffect(() => {
    props.transfers.length;
    if (scroller && scroller.scrollTop > scroller.scrollHeight - scroller.clientHeight) {
      scroller.scrollTop = 0;
      setScrollTop(0);
    }
  });

  const range = () =>
    visibleRange(scrollTop(), viewport(), DOWNLOAD_ROW_HEIGHT, props.transfers.length);
  const rows = () => props.transfers.slice(range().start, range().end);
  const padTop = () => range().start * DOWNLOAD_ROW_HEIGHT;
  const padBottom = () => (props.transfers.length - range().end) * DOWNLOAD_ROW_HEIGHT;

  const chooseMenu = (action: DownloadMenuAction) => {
    const current = menu();
    setMenu(null);
    if (!current) return;
    const transfer = current.transfer;
    switch (action) {
      case "pause": props.onPause(transfer.id); break;
      case "resume": props.onRetry(transfer.id); break;
      case "moveToQueue":
        if (props.selectedQueue && props.selectedQueue !== "all") {
          props.onMoveToQueue?.(transfer.id, props.selectedQueue);
        }
        break;
      case "remove": {
        const wording =
          transfer.status === "done" || transfer.status === "cancelled"
            ? "Remove this completed download from the queue?"
            : "Remove this download and delete its partial file?";
        if (window.confirm(wording)) props.onRemove?.(transfer.id);
        break;
      }
      case "openFolder": props.onOpenFolder?.(transfer.destPath); break;
      case "copyUrl":
        if (transfer.from?.kind === "http") {
          try {
            const safe = new URL(transfer.from.url);
            safe.username = "";
            safe.password = "";
            props.onCopyUrl?.(safe.toString());
          } catch {
            // Invalid legacy rows still get a useful best-effort copy, with
            // the authority credentials removed before the clipboard sees it.
            props.onCopyUrl?.(transfer.from.url.replace(/\/\/[^/@]+@/, "//"));
          }
        }
        break;
    }
  };

  return (
    <div class={styles.table} ref={scroller} role="grid" aria-label="Download list">
      <div class={`${styles.grid} ${styles.head}`} role="row">
        <span role="columnheader">File</span>
        <span role="columnheader" class={styles.headCellNumeric}>
          Size
        </span>
        <span role="columnheader">Progress</span>
        <span role="columnheader" class={styles.headCellNumeric}>
          Speed
        </span>
        <span role="columnheader" class={styles.headCellNumeric}>
          ETA
        </span>
        <span role="columnheader">Status</span>
        <span />
      </div>
      <Show
        when={props.transfers.length > 0}
        fallback={<div class={styles.empty}>Nothing transferring</div>}
      >
        <div style={{ height: `${padTop()}px` }} />
        <For each={rows()}>
          {(transfer) => {
            const state = badge(transfer);
            // The engine's rule, not a second one: null while the size is
            // unknown, a clamped fraction otherwise (XDL-E2-S3).
            const fraction = fractionOf(transfer);
            const origin = originHost(transfer);
            return (
              <div
                class={`${styles.grid} ${styles.row}`}
              role="row"
              data-transfer={transfer.id}
              tabIndex={props.selectedQueue !== undefined && props.selectedQueue !== "all" && props.onReorder ? 0 : undefined}
              aria-label={`${transfer.name}, ${state.label}`}
                onClick={(event) => {
                  if (!(event.target as Element).closest("button, select")) {
                    setSelectedId((current) => (current === transfer.id ? null : transfer.id));
                  }
                }}
                aria-expanded={selectedId() === transfer.id}
                onContextMenu={(event) => {
                  event.preventDefault();
                setMenu({ transfer, x: event.clientX, y: event.clientY });
              }}
              draggable={props.selectedQueue !== undefined && props.selectedQueue !== "all" && props.onReorder ? "true" : undefined}
              onDragStart={(event) => {
                if (props.selectedQueue && props.selectedQueue !== "all") {
                  event.dataTransfer?.setData("text/plain", transfer.id);
                  if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
                }
              }}
              onDragOver={(event) => {
                if (props.selectedQueue && props.selectedQueue !== "all" && props.onReorder) {
                  event.preventDefault();
                  if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
                }
              }}
              onDrop={(event) => {
                event.preventDefault();
                const queueId = props.selectedQueue;
                const dragged = event.dataTransfer?.getData("text/plain");
                if (!queueId || queueId === "all" || !dragged || dragged === transfer.id || !props.onReorder) return;
                const ids = props.transfers.map((candidate) => candidate.id);
                const from = ids.indexOf(dragged);
                const to = ids.indexOf(transfer.id);
                if (from < 0 || to < 0) return;
                ids.splice(from, 1);
                ids.splice(to, 0, dragged);
                props.onReorder(queueId, ids);
              }}
              onKeyDown={(event) => {
                const queueId = props.selectedQueue;
                if ((event.key !== "ArrowUp" && event.key !== "ArrowDown") || !queueId || queueId === "all" || !props.onReorder) return;
                event.preventDefault();
                const ids = props.transfers.map((candidate) => candidate.id);
                const index = ids.indexOf(transfer.id);
                const next = index + (event.key === "ArrowUp" ? -1 : 1);
                if (index < 0 || next < 0 || next >= ids.length) return;
                [ids[index], ids[next]] = [ids[next]!, ids[index]!];
                props.onReorder(queueId, ids);
              }}
            >
                <div class={styles.cellFile}>
                  <div class={styles.name} title={transfer.name}>
                    {transfer.name}
                  </div>
                  <div class={styles.origin} title={origin ?? undefined}>
                    {origin ?? "—"}
                  </div>
                </div>
                <div class={`${styles.numeric} ${fraction !== null ? "" : styles.dim}`}>
                  {fraction !== null ? formatSize(transfer.bytesTotal) : "—"}
                </div>
                <div class={styles.progress}>
                  {fraction === null ? (
                    <div
                      class={styles.track}
                      role="progressbar"
                      aria-label={`${transfer.name} progress, size unknown`}
                    >
                      <div class={styles.fillIndeterminate} />
                    </div>
                  ) : (
                    <div
                      class={styles.track}
                      role="progressbar"
                      aria-label={`${transfer.name} progress`}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={Math.round(fraction * 100)}
                    >
                      <div class={styles.fill} style={{ width: `${fraction * 100}%` }} />
                    </div>
                  )}
                  {/* Unknown reads the growing byte count with no denominator
                      and no percentage — never a bar at zero claiming 0%
                      (XDL-E2-S3). */}
                  <span class={styles.percent}>
                    {fraction === null
                      ? formatSize(transfer.bytesDone)
                      : `${Math.round(fraction * 100)}%`}
                  </span>
                </div>
                <div
                  class={`${styles.numeric} ${
                    transfer.status === "running" && transfer.rate !== null
                      ? styles.speedActive
                      : styles.dim
                  }`}
                >
                  {transfer.status === "running" && transfer.rate !== null
                    ? formatRate(transfer.rate)
                    : "—"}
                </div>
                <div class={`${styles.numeric} ${transfer.etaSeconds !== null ? "" : styles.dim}`}>
                  {transfer.etaSeconds !== null ? formatDuration(transfer.etaSeconds) : "—"}
                </div>
                <div>
                  <span
                    class={`${styles.badge} ${state.tone}`}
                    title={transfer.status === "failed" ? (transfer.error ?? undefined) : undefined}
                  >
                    {state.label}
                  </span>
                  <Show when={transfer.status === "failed" && transfer.error}>
                    {(error) => <div class={styles.error}>{error()}</div>}
                  </Show>
                </div>
                <div class={styles.actions}>
                  <button
                    class={styles.action}
                    aria-label={`Show details for ${transfer.name}`}
                    onClick={() =>
                      setSelectedId((current) => (current === transfer.id ? null : transfer.id))
                    }
                  >
                    Details
                  </button>
                  <Show when={props.queues && props.onMoveToQueue}>
                    <select
                      class={styles.queueSelect}
                      aria-label={`Queue for ${transfer.name}`}
                      value={transfer.queueId ?? "default"}
                      onChange={(event) => props.onMoveToQueue?.(transfer.id, event.currentTarget.value)}
                    >
                      <For each={props.queues}>{(queue) => <option value={queue.id}>{queue.name}</option>}</For>
                    </select>
                  </Show>
                  <Show when={transfer.status === "running" || transfer.status === "queued"}>
                    <button class={styles.action} onClick={() => props.onPause(transfer.id)}>
                      Pause
                    </button>
                    <button class={styles.action} onClick={() => props.onCancel(transfer.id)}>
                      Cancel
                    </button>
                  </Show>
                  <Show when={transfer.status === "paused"}>
                    <button class={styles.action} onClick={() => props.onRetry(transfer.id)}>
                      Resume
                    </button>
                  </Show>
                  <Show when={transfer.status === "failed"}>
                    <button class={styles.action} onClick={() => props.onRetry(transfer.id)}>
                      Retry
                    </button>
                    <Show when={transferRemountable(transfer) && props.onRemount}>
                      {(remount) => (
                        <button class={styles.action} onClick={() => remount()()}>
                          Remount as RW
                        </button>
                      )}
                    </Show>
                  </Show>
                  <button
                    class={styles.action}
                    onClick={() => {
                      const wording =
                        transfer.status === "done" || transfer.status === "cancelled"
                          ? "Remove this completed download from the queue?"
                          : "Remove this download and delete its partial file?";
                      if (window.confirm(wording)) props.onRemove?.(transfer.id);
                    }}
                  >
                    Remove
                  </button>
                </div>
              </div>
            );
          }}
        </For>
        <div style={{ height: `${padBottom()}px` }} />
      </Show>
      <Show when={selectedId()}>
        {(id) => {
          const selected = () => props.transfers.find((transfer) => transfer.id === id());
          return <Show when={selected()}>{(transfer) => <DownloadDetail transfer={transfer()} />}</Show>;
        }}
      </Show>
      <Show when={menu()}>
        {(current) => (
          <ContextMenu
            items={downloadMenu(current().transfer, {
              moveEnabled: props.selectedQueue !== undefined && props.selectedQueue !== "all" && !!props.onMoveToQueue,
            })}
            at={{ x: current().x, y: current().y }}
            label="Download actions"
            onChoose={chooseMenu}
            onClose={() => setMenu(null)}
          />
        )}
      </Show>
    </div>
  );
}
