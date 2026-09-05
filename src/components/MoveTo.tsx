import { For, Show, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";

import dialog from "./dialog.module.css";
import styles from "./moveto.module.css";
import { engineFailure } from "../state/errors";
import { illegalDestination } from "../state/move";
import { ancestor, canAscend, parent, segments } from "../state/navigation";
import { DEFAULT_SORT, sortEntries } from "../state/sort";
import type { Entry, Listing } from "../state/types";

/**
 * The Move to… destination picker (E14-S3).
 *
 * It picks a destination and nothing else: it does not move anything, does not
 * know whether the mount is writable, and does not read the window's listing.
 * The caller has already gated the verb and owns the `rename` calls, the
 * partial-failure report and the re-list — all of which need the backend, and
 * none of which need a folder tree on screen.
 *
 * The model is "walk to the folder, then Move here", not "select a row and
 * confirm". A row is a place to go, so a single click goes there; the
 * destination is always the folder whose name is at the end of the breadcrumb,
 * which is the same folder the button names. Selecting a row *and* being
 * somewhere would be two destinations on screen at once.
 *
 * The handoff draws no picker, so the chrome is the New Connection dialog's
 * (`dialog.module.css`) rather than a second dialog language. Recorded in
 * `docs/design-gaps.md`.
 */
export function MoveTo(props: {
  /** The entries being moved. Never empty. */
  entries: Entry[];
  /** The folder they are in now, share-relative; "" is the share root. */
  from: string;
  /** The share's name, for the root crumb. */
  shareName: string;
  /** List one directory of the share — the window's own backend call. */
  listDir: (path: string) => Promise<Listing>;
  /** The chosen destination, share-relative; "" is the share root. */
  onMove: (destination: string) => void;
  onCancel: () => void;
}) {
  /** The folder on screen, which is also the destination on offer. */
  const [at, setAt] = createSignal(props.from);

  // The source is wrapped in an object because `""` is a real path here — the
  // share root — and a bare string source invites the reading where a falsy
  // source means "not ready yet". `createResource` also drops a response that
  // a later navigation has already superseded, which a hand-rolled effect
  // would have to remember to do.
  const [listing] = createResource(
    () => ({ path: at() }),
    ({ path }) => props.listDir(path),
  );

  /**
   * Only directories, in the list's own order.
   *
   * Folders being moved are listed like any other. Hiding them would leave a
   * gap where the user can see a folder ought to be, and the refusal that
   * names it is more use than its absence.
   */
  const folders = createMemo(() => {
    // `listing.error` first, and the resource itself only after: reading a
    // failed resource re-throws, which tears the sheet down instead of showing
    // the failure in it.
    if (listing.error) return [];
    const entries = listing()?.entries ?? [];
    return sortEntries(
      entries.filter((entry) => entry.kind === "dir"),
      DEFAULT_SORT,
      false,
    );
  });

  const reason = () => illegalDestination(props.entries, props.from, at());

  const failure = () => {
    const error = listing.error as unknown;
    return error === undefined || error === null ? null : engineFailure(error).message;
  };

  const summary = () => {
    const only = props.entries.length === 1 ? props.entries[0] : undefined;
    return only ? `Moving “${only.name}”` : `Moving ${props.entries.length} items`;
  };

  let sheet: HTMLDivElement | undefined;

  onMount(() => {
    // Focus lands on the sheet so the first Tab is inside it, and so the keys
    // the list behind this one listens for do not reach a focused row.
    sheet?.focus();

    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // Capture phase, and stopped here: the window's own Escape handler
      // clears the filter and then the selection in the list underneath, and
      // one Escape must close one thing. Stopping during capture at `window`
      // is what keeps the event from bubbling back to that listener.
      event.stopPropagation();
      event.preventDefault();
      props.onCancel();
    };
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });

  return (
    <div class={dialog.scrim} onClick={props.onCancel}>
      <div
        ref={sheet}
        class={dialog.dialog}
        role="dialog"
        aria-modal="true"
        aria-label="Move to"
        tabindex="-1"
        onClick={(event) => event.stopPropagation()}
      >
        <div class={dialog.titlebar}>Move to…</div>
        <div class={dialog.body}>
          <nav class={styles.crumbs} aria-label="Destination">
            <button class={styles.crumb} onClick={() => setAt("")}>
              {props.shareName}
            </button>
            <For each={segments(at())}>
              {(segment, index) => (
                <>
                  <span class={styles.crumbSeparator}>/</span>
                  {/* The last crumb is where we already are; a button that
                      cannot go anywhere is a button that looks broken. */}
                  <Show
                    when={index() < segments(at()).length - 1}
                    fallback={<span class={styles.crumbCurrent}>{segment}</span>}
                  >
                    <button
                      class={styles.crumb}
                      onClick={() => setAt(ancestor(at(), index() + 1))}
                    >
                      {segment}
                    </button>
                  </Show>
                </>
              )}
            </For>
          </nav>

          {/* Fixed height, not `max-height`: a picker that resizes as you walk
              the tree moves Move here out from under the pointer. */}
          <div class={styles.list} role="group" aria-label="Folders">
            <Show when={canAscend(at())}>
              <button
                class={`${styles.row} ${styles.up}`}
                data-destination={parent(at())}
                onClick={() => setAt(parent(at()))}
              >
                <span class={styles.glyph} />
                <span class={styles.name}>..</span>
              </button>
            </Show>
            <For each={folders()}>
              {(folder) => (
                <button
                  class={styles.row}
                  data-destination={folder.path}
                  onClick={() => setAt(folder.path)}
                >
                  <span class={styles.glyph} />
                  <span class={styles.name}>{folder.name}</span>
                </button>
              )}
            </For>
            <Show when={failure()}>
              {(message) => <p class={`${styles.note} ${styles.noteError}`}>{message()}</p>}
            </Show>
            <Show when={!failure() && !listing.loading && folders().length === 0}>
              {/* Still a destination: an empty folder is a perfectly good
                  place to move things into. */}
              <p class={styles.note}>No folders here.</p>
            </Show>
          </div>

          <p class={styles.summary}>{summary()}</p>

          <div class={dialog.actions}>
            <Show when={reason()}>
              {(text) => (
                <span id="move-to-reason" class={styles.reason} role="status">
                  {text()}
                </span>
              )}
            </Show>
            <span class={dialog.spacer} />
            <button class={dialog.action} onClick={props.onCancel}>
              Cancel
            </button>
            <button
              class={`${dialog.action} ${dialog.actionPrimary}`}
              disabled={reason() !== null}
              aria-describedby={reason() ? "move-to-reason" : undefined}
              onClick={() => props.onMove(at())}
            >
              Move here
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
