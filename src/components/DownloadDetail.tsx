import { For, Show } from "solid-js";
import { formatRate, formatSize } from "../state/format";
import type { Transfer, TransferFileProgress, TransferSegmentProgress } from "../state/types";
import styles from "./downloadDetail.module.css";

/**
 * The selected download's connection lanes (XDL-E4-S1). Geometry comes from
 * xsync's ProgressWatch snapshot: a lane with no declared total is described
 * in words and never assigned a made-up position.
 */
export function DownloadDetail(props: { transfer: Transfer }) {
  const progress = () => props.transfer.progress;
  const file = () => progress()?.files[0];
  const fastest = () =>
    file()?.segments.reduce<TransferSegmentProgress | null>(
      (best, segment) => (best === null || segment.rate > best.rate ? segment : best),
      null,
    ) ?? null;
  const stalled = () => file()?.segments.filter((segment) => segment.stalled).length ?? 0;

  return (
    <section class={styles.detail} aria-label={`${props.transfer.name} download detail`}>
      <div class={styles.heading}>
        <span class={styles.title}>Connection lanes</span>
        <span class={styles.path}>{file()?.path ?? props.transfer.name}</span>
      </div>
      <Show
        when={file()}
        fallback={<div class={styles.empty}>Waiting for per-connection progress</div>}
      >
        {(current) => (
          <>
            <div class={styles.summary}>
              <span>Fastest {fastest() ? formatRate(fastest()!.rate) : "—"}</span>
              <span>{stalled()} stalled</span>
              <span>{progress()?.retries ?? 0} retries</span>
            </div>
            <div class={styles.lanes}>
              <For each={current().segments}>
                {(segment) => <Lane segment={segment} total={current().total} />}
              </For>
            </div>
          </>
        )}
      </Show>
    </section>
  );
}

function Lane(props: { segment: TransferSegmentProgress; total: number | null }) {
  const hasGeometry = () => props.total !== null && props.total > 0 && props.segment.length !== null;
  const range = () => props.segment.length ?? 0;
  const fill = () => Math.min(1, Math.max(0, props.segment.written / Math.max(1, range())));
  const left = () =>
    hasGeometry() ? `${Math.min(100, (props.segment.offset / props.total!) * 100)}%` : "0%";
  const width = () =>
    hasGeometry() ? `${Math.min(100, (range() / props.total!) * 100)}%` : "100%";
  const fillWidth = () => `${fill() * 100}%`;

  return (
    <div class={styles.lane} data-stream={props.segment.stream}>
      <span class={styles.laneLabel}>Stream {props.segment.stream + 1}</span>
      <div
        class={`${styles.track} ${props.segment.stalled ? styles.trackStalled : ""}`}
        aria-label={
          props.segment.stalled
            ? `Stream ${props.segment.stream + 1} stalled`
            : `Stream ${props.segment.stream + 1}`
        }
      >
        <Show
          when={hasGeometry()}
          fallback={<span class={styles.unknown} style={{ width: fillWidth() }} />}
        >
          <span class={styles.range} data-range="true" style={{ left: left(), width: width() }}>
            <span class={styles.fill} style={{ width: fillWidth() }} />
            <span class={styles.playhead} style={{ left: fillWidth() }} />
          </span>
        </Show>
      </div>
      <span class={`${styles.laneRate} ${props.segment.stalled ? styles.rateStalled : ""}`}>
        {props.segment.stalled ? "Stalled" : formatRate(props.segment.rate)}
      </span>
      <span class={styles.laneBytes}>
        {formatSize(props.segment.written)}
        {props.segment.length !== null ? ` / ${formatSize(props.segment.length)}` : ""}
      </span>
    </div>
  );
}

export function downloadFileProgress(transfer: Transfer): TransferFileProgress | null {
  return transfer.progress?.files[0] ?? null;
}
