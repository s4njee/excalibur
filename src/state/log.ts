// What the message log shows, and what it hides (`twopane.md` XTP-E5-S1).
//
// Pure, so the rules that decide which lines a user sees — and the exact text
// the Copy button puts on the clipboard — are testable without a pane on
// screen.
//
// **The resolution these lines describe.** The design's log is per protocol
// frame. `xsync-client` exposes no tap (XTP-E5-S0, `integration.md` §8) and
// SMB, NFS and SFTP have no frames to expose at all, so what the engine
// records is one line per verb at the `ShareBackend` seam. The pane says so
// rather than implying frames it does not have.

import type { LogCategory, LogLine } from "./types";

/**
 * How many lines a window keeps, matching the engine's own ring
 * (`crates/engine/src/log.rs`).
 *
 * Held in both places rather than asked for: a window left open for a week
 * would otherwise accumulate a week of lines while the engine holds five
 * hundred, and the two would disagree about what the log is.
 */
export const LOG_CAPACITY = 500;

/**
 * The categories a quiet log leaves out.
 *
 * Verbose is off by default and these are what it adds: the answer to every
 * command, and the timing of every listing. They are the chatty half — a
 * browse writes one of each per directory — and none of them is the reason
 * anybody opens a log. What stays is what was asked for, what the mount is
 * doing, what went wrong and what can be done about it.
 */
export const VERBOSE_ONLY: readonly LogCategory[] = ["response", "listing"];

/** The lines a pane draws, given the Verbose toggle. */
export function visibleLines(lines: LogLine[], verbose: boolean): LogLine[] {
  return verbose ? lines : lines.filter((line) => !VERBOSE_ONLY.includes(line.category));
}

/** The label before the message: `Command:`, `Error:`. */
export function categoryLabel(category: LogCategory): string {
  return `${category.charAt(0).toUpperCase()}${category.slice(1)}:`;
}

/**
 * The clock the pane draws: `09:22:41`, in the reader's own timezone.
 *
 * Seconds and no date. A log is read about what just happened, and a line
 * carrying a date would push the message it is about off the end.
 */
export function formatLogTime(at: number): string {
  const when = new Date(at);
  if (!Number.isFinite(at)) return "--:--:--";
  const pad = (value: number) => `${value}`.padStart(2, "0");
  return `${pad(when.getHours())}:${pad(when.getMinutes())}:${pad(when.getSeconds())}`;
}

/**
 * The log as text, for the clipboard.
 *
 * What is on screen, not the whole ring: a user who turned Verbose off and
 * pressed Copy is asking for the log they are looking at. Tab-separated so it
 * lands in a bug report as columns rather than as prose.
 */
export function logText(lines: LogLine[]): string {
  return lines
    .map((line) => `${formatLogTime(line.at)}\t${categoryLabel(line.category)}\t${line.text}`)
    .join("\n");
}

/**
 * What the pane says it is showing, above the lines.
 *
 * The design's log is per protocol frame. Only xsync produces those; SMB, NFS
 * and SFTP have none to give and are recorded a verb at a time, and a window
 * with both kinds of mount open holds both kinds of line. Saying which is not
 * a caveat to be tucked away — a reader who believes they are seeing the wire
 * when they are seeing `list_dir` will draw the wrong conclusion from a log
 * that is missing exactly the frame they are looking for.
 */
export function resolutionNote(lines: LogLine[]): string {
  const frames = lines.some((line) => line.resolution === "frame");
  const verbs = lines.some((line) => line.resolution !== "frame");
  if (frames && verbs) return "xsync protocol frames · engine operations elsewhere";
  if (frames) return "xsync protocol frames";
  return "Engine operations — not protocol frames";
}
