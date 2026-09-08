import { Show } from "solid-js";

import styles from "./dialog.module.css";
import {
  MAX_CONCURRENCY,
  MAX_DOWNLOAD_ATTEMPTS,
  MAX_DOWNLOAD_CONNECTIONS,
  MAX_DOWNLOAD_WAIT_SECONDS,
  type ConflictDefault,
  type DownloadRetryPolicy,
  type Settings,
  type ThemeChoice,
} from "../state/settings";
import * as shortcuts from "../state/shortcuts";
import type { HttpDownloadLimits, RowLayout } from "../state/types";

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
  /** Native HTTP limits, when the shell has answered its capability query. */
  downloadLimits?: HttpDownloadLimits;
}) {
  const connectionLimit = () =>
    Math.max(
      1,
      Math.min(
        MAX_DOWNLOAD_CONNECTIONS,
        props.downloadLimits
          ? Math.min(props.downloadLimits.maxConnections, props.downloadLimits.maxPerHost)
          : MAX_DOWNLOAD_CONNECTIONS,
      ),
    );
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
              Global concurrency
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

            <p class={styles.fieldHint}>
              Worker limit for all transfers (`transferConcurrency`).
            </p>

            <label class={styles.label} for="download-connections">
              Download connections
            </label>
            <select
              id="download-connections"
              class={styles.input}
              value={
                props.settings.downloadConnections === null
                  ? "auto"
                  : String(props.settings.downloadConnections)
              }
              onChange={(event) => {
                const value = event.currentTarget.value;
                props.onChange(
                  "downloadConnections",
                  value === "auto" ? null : Number(value),
                );
              }}
            >
              <option value="auto">Automatic (engine chooses)</option>
              {Array.from({ length: connectionLimit() }, (_, index) => index + 1).map((count) => (
                <option value={count}>{count}</option>
              ))}
            </select>
            <p class={styles.fieldHint}>
              HTTP `connections`; the engine caps this at its per-origin
              `max-per-host` limit.
            </p>

            <span class={styles.label}>Retry policy</span>
            <div>
              <label class={styles.checkRow} for="download-retry-attempts">
                <span>Attempts</span>
                <input
                  id="download-retry-attempts"
                  class={styles.input}
                  type="number"
                  min={1}
                  max={MAX_DOWNLOAD_ATTEMPTS}
                  value={props.settings.downloadRetry.maxAttempts}
                  onChange={(event) =>
                    props.onChange("downloadRetry", {
                      ...props.settings.downloadRetry,
                      maxAttempts: Number(event.currentTarget.value),
                    } satisfies DownloadRetryPolicy)
                  }
                />
              </label>
              <label class={styles.checkRow} for="download-retry-initial-wait">
                <span>Initial wait (seconds)</span>
                <input
                  id="download-retry-initial-wait"
                  class={styles.input}
                  type="number"
                  min={0}
                  max={MAX_DOWNLOAD_WAIT_SECONDS}
                  value={props.settings.downloadRetry.initialWaitSeconds}
                  onChange={(event) =>
                    props.onChange("downloadRetry", {
                      ...props.settings.downloadRetry,
                      initialWaitSeconds: Number(event.currentTarget.value),
                    } satisfies DownloadRetryPolicy)
                  }
                />
              </label>
              <label class={styles.checkRow} for="download-retry-max-wait">
                <span>Maximum wait (seconds)</span>
                <input
                  id="download-retry-max-wait"
                  class={styles.input}
                  type="number"
                  min={0}
                  max={MAX_DOWNLOAD_WAIT_SECONDS}
                  value={props.settings.downloadRetry.maxWaitSeconds}
                  onChange={(event) =>
                    props.onChange("downloadRetry", {
                      ...props.settings.downloadRetry,
                      maxWaitSeconds: Number(event.currentTarget.value),
                    } satisfies DownloadRetryPolicy)
                  }
                />
              </label>
              <p class={styles.fieldHint}>
                Maps to the HTTP provider&apos;s `max_tries`, `initial_wait`, and `max_wait`.
              </p>
            </div>

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
