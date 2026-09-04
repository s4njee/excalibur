import { Show } from "solid-js";

import styles from "./dialog.module.css";
import { MAX_CONCURRENCY, type ConflictDefault, type Settings, type ThemeChoice } from "../state/settings";
import * as shortcuts from "../state/shortcuts";
import type { RowLayout } from "../state/types";

/**
 * The preferences sheet (E17-S1).
 *
 * Deliberately plain. The design handoff has no preferences screen, so rather
 * than invent one this borrows the New Connection dialog's chrome and lays the
 * fields out in the same two-column grid. Recorded in `docs/design-gaps.md`;
 * when a real design arrives this is the thing it replaces.
 *
 * Only settings that *do* something today get a control. `sidebarCollapsed` is
 * stored but its control is the toolbar toggle, where the user can see what it
 * does. `rowLayout` gained its picker with E8-S3/S4, once there were three real
 * layouts to choose between.
 */
export function Preferences(props: {
  settings: Settings;
  onChange: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
  onClose: () => void;
  /** Opens a folder picker. Absent in the browser preview, which has none. */
  onPickFolder?: () => void;
}) {
  return (
    <div class={styles.scrim} onClick={props.onClose}>
      <div
        class={styles.dialog}
        role="dialog"
        aria-label="Preferences"
        onClick={(event) => event.stopPropagation()}
      >
        <div class={styles.titlebar}>Preferences</div>
        <div class={styles.body}>
          <div class={styles.form}>
            <label class={styles.label} for="theme">
              Appearance
            </label>
            <select
              id="theme"
              class={styles.input}
              value={props.settings.theme}
              onChange={(event) =>
                props.onChange("theme", event.currentTarget.value as ThemeChoice)
              }
            >
              <option value="system">Match system</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select>

            <label class={styles.label} for="layout">
              Rows
            </label>
            <select
              id="layout"
              class={styles.input}
              value={props.settings.rowLayout}
              onChange={(event) =>
                props.onChange("rowLayout", event.currentTarget.value as RowLayout)
              }
            >
              <option value="columns">Columns ({shortcuts.hint("layoutColumns")})</option>
              <option value="name-led">Name-led ({shortcuts.hint("layoutNameLed")})</option>
              <option value="mono">Mono grid ({shortcuts.hint("layoutMono")})</option>
            </select>

            <span class={styles.label}>Files</span>
            <div>
              <label class={styles.checkRow}>
                <input
                  type="checkbox"
                  checked={props.settings.showHidden}
                  onChange={(event) => props.onChange("showHidden", event.currentTarget.checked)}
                />
                Show hidden files
              </label>
              <label class={styles.checkRow}>
                <input
                  type="checkbox"
                  checked={props.settings.foldersFirst}
                  onChange={(event) => props.onChange("foldersFirst", event.currentTarget.checked)}
                />
                Keep folders first when sorting
              </label>
            </div>

            <label class={styles.label} for="downloads">
              Downloads
            </label>
            <div class={styles.folderRow}>
              <input
                id="downloads"
                class={styles.input}
                placeholder="System download folder"
                value={props.settings.downloadFolder ?? ""}
                onInput={(event) =>
                  props.onChange("downloadFolder", event.currentTarget.value.trim() || null)
                }
              />
              <Show when={props.onPickFolder}>
                <button class={styles.action} onClick={props.onPickFolder}>
                  Choose…
                </button>
              </Show>
            </div>

            <label class={styles.label} for="concurrency">
              Transfers at once
            </label>
            <input
              id="concurrency"
              class={styles.input}
              type="number"
              min={1}
              max={MAX_CONCURRENCY}
              value={props.settings.transferConcurrency}
              onChange={(event) =>
                props.onChange("transferConcurrency", Number(event.currentTarget.value))
              }
            />

            <label class={styles.label} for="conflict">
              If a file exists
            </label>
            <select
              id="conflict"
              class={styles.input}
              value={props.settings.conflictDefault}
              onChange={(event) =>
                props.onChange("conflictDefault", event.currentTarget.value as ConflictDefault)
              }
            >
              <option value="ask">Ask every time</option>
              <option value="replace">Replace it</option>
              <option value="keepBoth">Keep both</option>
              <option value="skip">Skip it</option>
            </select>
            <p class={styles.fieldHint}>
              Anything but “Ask” applies to a whole batch without prompting.
            </p>
          </div>

          <div class={styles.actions}>
            <button class={styles.action} onClick={props.onClose}>
              Done
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
