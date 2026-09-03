import { Show, createEffect, createSignal, onCleanup } from "solid-js";
import styles from "./quicklook.module.css";
import { decodeText, looksBinary, previewFor, streamUrl, TEXT_WINDOW } from "../state/preview";
import { formatModified, formatSize } from "../state/format";
import type { Caps, Entry, MountInfo } from "../state/types";

/**
 * Quick Look (E11).
 *
 * Always dark, whatever the app's theme: the handoff is explicit that previews
 * read better on dark, and a panel that followed the theme would show a photo
 * against white half the time.
 *
 * Nothing here reads a file through a command. Every viewer points at
 * `excalibur-stream://`, so the bytes go from the backend to the element that
 * needs them without crossing IPC — which is what makes previewing a 40 GB film
 * possible at all.
 */
export function QuickLook(props: {
  entry: Entry;
  mount: MountInfo;
  caps: Caps;
  onClose: () => void;
  /** Open in the OS editor. Absent until E12 wires it. */
  onEdit?: () => void;
}) {
  const preview = () => previewFor(props.entry, props.caps);
  const url = () => streamUrl(props.entry.path);

  return (
    <div class={styles.panel} role="dialog" aria-label={`Preview of ${props.entry.name}`}>
      <div class={styles.header}>
        <span class={styles.filename}>{props.entry.name}</span>
        <span class={styles.meta}>
          {formatSize(props.entry.size)} · {props.entry.kindLabel}
        </span>
        <Show when={props.onEdit}>
          {(edit) => (
            <button class={styles.headerAction} onClick={() => edit()()}>
              Open in editor
            </button>
          )}
        </Show>
        <button class={styles.headerAction} aria-label="Close" onClick={props.onClose}>
          ✕
        </button>
      </div>

      <div class={styles.body}>
        <Show when={preview().kind === "text"}>
          <TextPreview path={props.entry.path} />
        </Show>
        <Show when={preview().kind === "image"}>
          <img class={styles.image} src={url()} alt={props.entry.name} />
        </Show>
        <Show when={preview().kind === "video"}>
          {/* The element does its own range requests against the stream
              protocol, so seeking a large film reads only what it needs. */}
          <video class={styles.media} src={url()} controls preload="metadata" />
        </Show>
        <Show when={preview().kind === "audio"}>
          <audio class={styles.media} src={url()} controls preload="metadata" />
        </Show>
        <Show when={preview().kind === "pdf"}>
          <Unpreviewable entry={props.entry} reason="PDF preview is not built yet" />
        </Show>
        <Show when={preview().kind === "none"}>
          <Unpreviewable
            entry={props.entry}
            reason={preview().kind === "none" ? (preview() as { reason: string }).reason : ""}
          />
        </Show>
      </div>

      <div class={styles.footer}>
        <span class={`${styles.chip} ${props.mount.writable ? styles.chipRw : styles.chipRo}`}>
          {props.mount.writable ? "RW" : "RO"}
        </span>
        <span class={styles.footerText}>
          {props.mount.writable
            ? "Edit opens this in your default editor"
            : "Read-only — download a copy to edit"}
        </span>
        <span class={styles.footerHint}>Space to close</span>
      </div>
    </div>
  );
}

/**
 * A text preview, read through the stream protocol.
 *
 * `fetch` against the custom scheme rather than a command, for the same reason
 * every other viewer uses it: the bytes must not cross IPC. It also means the
 * `Range` header does the windowing, so this reads the first megabyte and no
 * more however large the file is.
 */
function TextPreview(props: { path: string }) {
  const [text, setText] = createSignal<string | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);

  createEffect(() => {
    const path = props.path;
    let cancelled = false;
    onCleanup(() => {
      cancelled = true;
    });
    void (async () => {
      try {
        const response = await fetch(streamUrl(path), {
          headers: { Range: `bytes=0-${TEXT_WINDOW - 1}` },
        });
        const bytes = new Uint8Array(await response.arrayBuffer());
        if (cancelled) return;
        // Checked before decoding, not after: decoding a binary as Latin-1
        // always "succeeds" and produces a screen of noise.
        if (looksBinary(bytes)) {
          setProblem("This looks like a binary file");
          return;
        }
        setText(decodeText(bytes));
      } catch (error) {
        if (!cancelled) setProblem(String(error));
      }
    })();
  });

  return (
    <Show
      when={text()}
      fallback={<div class={styles.notice}>{problem() ?? "Reading…"}</div>}
    >
      {(content) => (
        <div class={styles.text}>
          {content()
            .split("\n")
            .map((line, index) => (
              <div class={styles.line}>
                {/* Not selectable, so copying the preview copies the file's
                    text and not a column of numbers with it. */}
                <span class={styles.lineNumber}>{index + 1}</span>
                <span class={styles.lineText}>{line}</span>
              </div>
            ))}
        </div>
      )}
    </Show>
  );
}

/** The card shown when nothing can render the file (E11-S4). */
function Unpreviewable(props: { entry: Entry; reason: string }) {
  return (
    <div class={styles.card}>
      <div class={styles.cardKind}>{props.entry.kindLabel}</div>
      <div class={styles.cardFacts}>
        {formatSize(props.entry.size)} · {formatModified(props.entry.modifiedMs)}
      </div>
      <div class={styles.cardReason}>{props.reason}</div>
      {/* Download belongs to E13's queue. Shown as unavailable rather than
          hidden, so the card says what will be possible rather than looking
          like a dead end. */}
      <button class={styles.cardAction} disabled title="Transfers arrive with E13">
        Download
      </button>
    </div>
  );
}
