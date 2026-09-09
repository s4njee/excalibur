import { For, Show, createEffect, onMount } from "solid-js";
import styles from "./transfers.module.css";
import { categoryLabel, formatLogTime, resolutionNote } from "../state/log";
import type { LogAction, LogLine } from "../state/types";

/**
 * The message log (`twopane.md` XTP-E5-S1).
 *
 * The handoff is specific that this is not a convenience: "The log is where
 * protocol truth lives: mount command and its export flags, listing timings,
 * and failures verbatim". So nothing here paraphrases: a line's text is the
 * engine's or the server's own words, and the pane's whole job is to make them
 * legible and copyable.
 *
 * **It says what resolution it is showing.** The design's stream is per
 * protocol frame; `xsync-client` exposes no tap (XTP-E5-S0, in another repo)
 * and SMB, NFS and SFTP have no frames to expose at all, so these lines are
 * one per engine verb. A pane that let a reader believe they were seeing the
 * wire would be worse than one that says which they are seeing.
 */
export function LogPane(props: {
  lines: LogLine[];
  /** Absent when a line's recovery cannot be run — no mount to run it on. */
  onAction?: (action: LogAction, mount: string | undefined) => void;
}) {
  let scroller!: HTMLDivElement;
  /**
   * Follow the newest line, unless the reader has scrolled away from it.
   *
   * A log that does not follow has to be scrolled after every line; one that
   * follows regardless yanks the view away from somebody reading what just
   * went wrong. Sticking only when already at the bottom is the rule that
   * serves both.
   */
  const atBottom = () =>
    scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24;
  let stick = true;
  onMount(() => {
    scroller.scrollTop = scroller.scrollHeight;
  });
  createEffect(() => {
    props.lines.length;
    if (stick) queueMicrotask(() => (scroller.scrollTop = scroller.scrollHeight));
  });

  return (
    <div
      class={styles.log}
      role="log"
      aria-label="Message log"
      ref={scroller}
      onScroll={() => (stick = atBottom())}
    >
      {/* What these lines are. xsync's client has a frame tap (XTP-E5-S0) and
          the other protocols have no frames to expose, so a window with both
          kinds of mount open holds both kinds of line. A pane that let a
          reader believe they were seeing the wire would be worse than one that
          says which they are seeing. */}
      <div
        class={styles.logResolution}
        title="xsync sessions are logged frame by frame; every other protocol a verb at a time"
      >
        {resolutionNote(props.lines)}
      </div>
      <Show
        when={props.lines.length > 0}
        fallback={<div class={styles.empty}>Nothing logged yet</div>}
      >
        <For each={props.lines}>
          {(line) => (
            <div class={styles.logLine} data-category={line.category}>
              <span class={styles.logTime}>{formatLogTime(line.at)}</span>
              <span class={`${styles.logCategory} ${categoryStyle(line.category)}`}>
                {categoryLabel(line.category)}
              </span>
              <span class={styles.logText}>{line.text}</span>
              {/* `2b` draws `Remount as RW` on the hint under a read-only
                  error. The line names the verb rather than the label, so this
                  is not matching on prose to decide what a button does. */}
              <Show when={line.action && props.onAction}>
                <button
                  class={styles.logAction}
                  onClick={() => props.onAction?.(line.action!, line.mount)}
                >
                  Remount as RW
                </button>
              </Show>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}

/** The handoff's six colours, one per category. */
function categoryStyle(category: LogLine["category"]): string | undefined {
  switch (category) {
    case "command":
      return styles.logCommand;
    case "status":
      return styles.logStatus;
    case "error":
      return styles.logError;
    default:
      // `Response:`, `Listing:` and `Hint:` share the secondary colour: they
      // are the log's ordinary voice, and the three that differ are the ones
      // worth finding by eye.
      return styles.logMuted;
  }
}
