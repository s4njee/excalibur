import { Show, onMount } from "solid-js";

import dialog from "./dialog.module.css";
import styles from "./properties.module.css";
import { formatModified, formatSize } from "../state/format";
import {
  formatCountLine,
  formatExactBytes,
  locationLabel,
  newestModified,
  sharePath,
  summarise,
} from "../state/properties";
import type { Entry } from "../state/types";

/**
 * Properties (E14-S4) — the sheet the context menu's last item opens.
 *
 * It states what the window already knows and asks the engine nothing. Every
 * field here rode in with the listing, which is what makes it safe to open on
 * a selection of any size: there is no per-entry `stat`, no walk, and nothing
 * that gets slower as the selection grows.
 *
 * It is read-only in the strongest sense available — there is no control in it
 * but the one that closes it — so `writable` is not a prop and there is
 * nothing here for a read-only mount to disable. The handoff draws no such
 * sheet; this borrows the New Connection dialog's chrome and is recorded in
 * `docs/design-gaps.md`.
 */
export function Properties(props: {
  /** One or many. Never empty. */
  entries: Entry[];
  /** The folder they are in, share-relative; "" is the share root. */
  location: string;
  /** The share's name, for naming the location of a root-level entry. */
  shareName: string;
  onClose: () => void;
}) {
  // `undefined` for a selection, so the two shapes are one `Show` rather than
  // a length check repeated at every row.
  const single = () => (props.entries.length === 1 ? props.entries[0] : undefined);
  const summary = () => summarise(props.entries);
  const newest = () => newestModified(props.entries);

  let sheet: HTMLDivElement | undefined;
  // Focus the sheet itself rather than the Done button: a modal that opens
  // with its dismiss button focused reads to a screen reader as "Done", which
  // is the one thing in it that is not the information the user asked for.
  onMount(() => sheet?.focus());

  return (
    <div class={dialog.scrim} onClick={props.onClose}>
      <div
        ref={sheet}
        class={dialog.dialog}
        role="dialog"
        aria-modal="true"
        aria-label="Properties"
        tabindex="-1"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          // Stopped here rather than left to bubble: the window's own Escape
          // handler clears the filter and then the selection, and closing this
          // sheet must not also empty the selection it was describing.
          event.stopPropagation();
          props.onClose();
        }}
      >
        <div class={dialog.titlebar}>Properties</div>
        <div class={dialog.body}>
          <div class={styles.header}>
            <div class={styles.name}>
              {single()?.name ?? formatCountLine(summary())}
            </div>
          </div>

          <dl class={styles.rows}>
            <Show
              when={single()}
              fallback={
                <>
                  <dt class={styles.term}>Items</dt>
                  <dd class={styles.value}>{formatCountLine(summary())}</dd>

                  <dt class={styles.term}>Size</dt>
                  <dd class={styles.value}>
                    {formatSize(summary().bytes)}{" "}
                    <span class={styles.exact}>({formatExactBytes(summary().bytes)})</span>
                  </dd>
                  <Show when={summary().folders > 0}>
                    {/* The AC forbids walking a folder to size it, so the
                        total is of the files only. Saying that is the whole
                        point: a total that silently omits the folders is a
                        wrong number, and one that says so is a right one. */}
                    <dd class={styles.note}>
                      Folders are not included — their contents are not measured.
                    </dd>
                  </Show>

                  <Show when={newest() !== null}>
                    <dt class={styles.term}>Last modified</dt>
                    <dd class={styles.value}>{formatModified(newest())}</dd>
                  </Show>

                  <dt class={styles.term}>Where</dt>
                  <dd class={`${styles.value} ${styles.mono}`}>
                    {locationLabel(props.location, props.shareName)}
                  </dd>
                </>
              }
            >
              {(entry) => (
                <>
                  <dt class={styles.term}>Kind</dt>
                  {/* The engine's kind table, never re-derived from the
                      extension: the label is what the rest of the window shows
                      in the Kind column, and two answers to "what is this" is
                      one too many. */}
                  <dd class={styles.value}>{entry().kindLabel}</dd>

                  <dt class={styles.term}>Size</dt>
                  <Show
                    when={entry().kind !== "dir"}
                    fallback={
                      <dd class={`${styles.value} ${styles.unknown}`}>—</dd>
                    }
                  >
                    <dd class={styles.value}>
                      {formatSize(entry().size)}{" "}
                      <span class={styles.exact}>({formatExactBytes(entry().size)})</span>
                    </dd>
                  </Show>
                  <Show when={entry().kind === "dir"}>
                    {/* The same dash the list shows in the Size column, and
                        for the same reason. `0 B` here would be a claim that
                        the folder is empty. */}
                    <dd class={styles.note}>
                      A folder's contents are not measured — that would mean reading the whole
                      tree.
                    </dd>
                  </Show>

                  <dt class={styles.term}>Modified</dt>
                  <dd class={styles.value}>{formatModified(entry().modifiedMs)}</dd>

                  <dt class={styles.term}>Path</dt>
                  <dd class={`${styles.value} ${styles.mono}`}>{sharePath(entry().path)}</dd>

                  <Show when={entry().linkTarget}>
                    {(target) => (
                      <>
                        <dt class={styles.term}>Links to</dt>
                        {/* Verbatim, as the server gave it. A symlink target
                            can be relative, absolute, or dangling, and
                            resolving it here would be this window guessing at
                            something only the server can answer. */}
                        <dd class={`${styles.value} ${styles.mono}`}>{target()}</dd>
                      </>
                    )}
                  </Show>
                </>
              )}
            </Show>
          </dl>

          <div class={dialog.actions}>
            <button class={`${dialog.action} ${dialog.spacer}`} onClick={props.onClose}>
              Done
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
