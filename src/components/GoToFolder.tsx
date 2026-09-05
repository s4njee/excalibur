import { For, Show, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";

import dialog from "./dialog.module.css";
import styles from "./goto.module.css";
import { engineFailure } from "../state/errors";
import { candidates, complete, parentOf, resolve } from "../state/goto";
import * as shortcuts from "../state/shortcuts";
import type { Listing } from "../state/types";

/**
 * Go to Folder (E26-S2's ⌘⇧G).
 *
 * One field and one verb. The handoff draws no such sheet, so the chrome is the
 * New Connection dialog's (`dialog.module.css`), the way Preferences and Move
 * to… borrow it; recorded in `docs/design-gaps.md`.
 *
 * The rules it obeys are in `state/goto.ts` and tested there. What is left here
 * is the wiring, which has its own ways of being wrong: completing from the
 * *window's* listing rather than from the folder being typed into (so `docs/re`
 * would offer the root's names), losing the caret to the end of a completion
 * the user then has to retype, or reporting "not found" for a path it never
 * asked the engine about.
 *
 * It does not decide whether the path exists — it asks. Listing the resolved
 * path is the same question the window would ask on arriving there, so a file,
 * a missing folder and a permission refusal all produce the engine's own words
 * rather than a guess this sheet made up.
 */
export function GoToFolder(props: {
  /** The folder the window is showing, share-relative; `""` is the root. */
  initial: string;
  /** The share's name, shown as the fixed part of the path. */
  shareName: string;
  /** List one directory of the share — the window's own backend call. */
  listDir: (path: string) => Promise<Listing>;
  /** Go there. Share-relative; `""` is the root. */
  onGo: (path: string) => void;
  onCancel: () => void;
}) {
  // Seeded with where the window already is, and with the trailing slash that
  // makes the first Tab complete a *child* rather than re-complete the folder
  // the user is standing in.
  const [text, setText] = createSignal(props.initial === "" ? "/" : `/${props.initial}/`);
  /** What the engine said about the path the user tried to go to. */
  const [failure, setFailure] = createSignal<string | null>(null);
  const [checking, setChecking] = createSignal(false);

  let field: HTMLInputElement | undefined;

  /**
   * The folder whose names complete the component being typed.
   *
   * A memo rather than the expression, so a keystroke inside one component does
   * not re-list the folder above it: the resource refetches when this *value*
   * changes, which is exactly when a `/` is typed or removed.
   */
  const parent = createMemo(() => parentOf(text()));
  const [siblings] = createResource(parent, (path) => props.listDir(path));

  /**
   * Folder names only.
   *
   * A file cannot be gone to, and offering one to Tab would complete a path
   * that Enter then has to refuse.
   *
   * `siblings.error` first, and the resource only after: reading a failed
   * resource re-throws, which would tear the sheet down instead of leaving a
   * half-typed path with no completions — which is all an unlistable parent
   * means. The message for it belongs to Enter, which is when the user has
   * actually claimed the path is real.
   */
  const names = createMemo(() => {
    if (siblings.error) return [];
    return (siblings()?.entries ?? [])
      .filter((entry) => entry.kind === "dir")
      .map((entry) => entry.name);
  });

  /** What Tab would choose between, shown so the choice is not a secret. */
  const matches = createMemo(() => candidates(text(), names()));

  const type = (value: string) => {
    setText(value);
    // The old message described the old path. Keeping it on screen while the
    // user edits their way out of the problem is how a stale error gets
    // believed.
    setFailure(null);
  };

  const completeHere = () => {
    const next = complete(text(), names());
    if (next === null) return;
    type(next);
    // Solid writes the value on the next tick; moving the caret before that
    // would put it in the old text. The end is where typing continues from.
    queueMicrotask(() => field?.setSelectionRange(next.length, next.length));
  };

  const go = async () => {
    const target = resolve(text());
    setChecking(true);
    setFailure(null);
    try {
      // Asked, not guessed. This is the same listing the window would do on
      // arriving, so a file or a refusal answers in the engine's own words.
      await props.listDir(target);
      props.onGo(target);
    } catch (error) {
      setFailure(engineFailure(error).message);
    } finally {
      setChecking(false);
    }
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Tab" && !event.shiftKey) {
      // Tab completes rather than leaving the field: there is one field, and
      // the next control is a button the user reaches with Enter anyway.
      event.preventDefault();
      completeHere();
      return;
    }
    if (event.key === "ArrowRight") {
      // → completes only from the end of the line. Anywhere else it is how a
      // user moves through what they typed, and taking that would be worse
      // than not offering the second key at all.
      const caret = field?.selectionStart ?? 0;
      const end = field?.selectionEnd ?? 0;
      if (caret === end && caret === text().length) {
        event.preventDefault();
        completeHere();
      }
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      void go();
    }
  };

  onMount(() => {
    field?.focus();
    field?.select();

    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // Capture phase, stopped here, for the reason Move to… gives: the
      // window's Escape clears the filter and then the selection in the list
      // underneath, and one Escape must close one thing.
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
        class={dialog.dialog}
        role="dialog"
        aria-modal="true"
        aria-label="Go to Folder"
        onClick={(event) => event.stopPropagation()}
      >
        <div class={dialog.titlebar}>Go to Folder</div>
        <div class={dialog.body}>
          <div class={styles.field}>
            <span class={styles.share} aria-hidden="true">
              {props.shareName}
            </span>
            <input
              ref={field}
              class={`${dialog.input} ${dialog.inputMono} ${styles.path}`}
              type="text"
              spellcheck={false}
              autocapitalize="off"
              autocomplete="off"
              aria-label="Path"
              value={text()}
              onInput={(event) => type(event.currentTarget.value)}
              onKeyDown={onKeyDown}
            />
          </div>

          <p class={styles.hint}>
            Tab or → completes the next folder. {shortcuts.hint("goToFolder")} reopens this.
          </p>

          <Show when={failure()}>
            {(message) => (
              <p class={styles.error} role="alert">
                {message()}
              </p>
            )}
          </Show>

          {/* What Tab is choosing between. A completion that stopped short is
              otherwise indistinguishable from one that did nothing. */}
          <Show when={!failure() && matches().length > 1}>
            <ul class={styles.matches} aria-label="Matching folders">
              <For each={matches().slice(0, 8)}>
                {(name) => <li class={styles.match}>{name}</li>}
              </For>
              <Show when={matches().length > 8}>
                <li class={styles.more}>+{matches().length - 8} more</li>
              </Show>
            </ul>
          </Show>

          <div class={dialog.actions}>
            <span class={dialog.spacer} />
            <button class={dialog.action} onClick={props.onCancel}>
              Cancel
            </button>
            <button
              class={`${dialog.action} ${dialog.actionPrimary}`}
              disabled={checking()}
              onClick={() => void go()}
            >
              Go
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
