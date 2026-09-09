/* The Add Download dialog (XDL-E3): the reference's screen 2 in the existing
   dialog chrome. Named queues do not exist yet (XDL-E6-S1), so the reference's
   "Save to / Queue" pair becomes "Save to" with a remembered folder. */

import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import dialog from "./dialog.module.css";
import styles from "./addDownload.module.css";
import { redactUrlHost, validateDownloadUrl } from "../state/downloads";
import { formatSize } from "../state/format";
import type { HttpDownloadLimits, HttpDownloadOptions, HttpProbe } from "../state/types";

/**
 * What every field in this dialog needs from the webview (E3-S5): nothing here
 * is prose — a URL and a path — so the platform's help can only corrupt it.
 * The same set the connection dialog spreads, kept here so a URL box that
 * autocapitalised `Https` would be a bug someone would have to remember.
 */
const PLAIN_FIELD = {
  autocapitalize: "none",
  autocorrect: "off",
  spellcheck: false,
  autocomplete: "off",
} as const;

type ProbeState = {
  status: "resolving" | "resolved" | "failed";
  probe?: HttpProbe;
  error?: string;
};

/** Lines the user actually typed. */
function lines(text: string): string[] {
  return text.split(/[\r\n]+/).map((line) => line.trim()).filter((line) => line !== "");
}

export function AddDownloadDialog(props: {
  /** The default destination, resolved by the caller (preference or OS). */
  folder: string;
  /** URLs supplied by a drop; opening the dialog remains the only action. */
  initialUrls?: string[];
  /** Opens the OS folder picker, like the connection dialog's Local tab. */
  pickFolder?: () => Promise<string | null>;
  /** Starting is in flight, so the buttons say so. */
  busy?: boolean;
  backend?: {
    downloadLimits?: () => Promise<HttpDownloadLimits>;
    probeHttpDownload?: (url: string) => Promise<HttpProbe>;
  };
  onCancel: () => void;
  onAdd: (urls: string[], folder: string, options?: HttpDownloadOptions[]) => void;
}) {
  const [text, setText] = createSignal(props.initialUrls?.join("\n") ?? "");
  const [dest, setDest] = createSignal(props.folder);
  const [mode, setMode] = createSignal<"separate" | "mirrors">("separate");
  const [limits, setLimits] = createSignal<HttpDownloadLimits | null>(null);
  const [connectionChoice, setConnectionChoice] = createSignal<number | null>(null);
  const [preferredMirror, setPreferredMirror] = createSignal(0);
  const [probes, setProbes] = createSignal<Record<string, ProbeState>>({});

  let urlBox: HTMLTextAreaElement | undefined;

  // The dialog deliberately does *not* read the clipboard.
  //
  // It used to, on mount and on every window focus, to offer a URL it found
  // there without filling the field. macOS Sequoia made a programmatic
  // pasteboard read raise a system "Paste" confirmation the user has to click,
  // so the offer cost a prompt every single time the dialog opened — two
  // clicks to reach a downloader that should take one (reported 2026-09-09).
  //
  // A real ⌘V is not a programmatic read and raises nothing, and the field
  // below is focused the moment this opens, so pasting is already one
  // keystroke. Recorded in `docs/design-gaps.md`.
  onMount(() => {
    urlBox?.focus();
    // Capture phase, stopped here, the way Move to… does: the window's own
    // Escape clears the filter and then the selection in the list underneath,
    // and one Escape must close one thing.
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      event.preventDefault();
      props.onCancel();
    };
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => {
      window.removeEventListener("keydown", onKey, true);
    });
  });

  const urls = createMemo(() => lines(text()));
  const validUrls = createMemo(() => urls().filter((url) => validateDownloadUrl(url) === null));
  /** The first reason one of the lines will be refused, once it exists. */
  const firstProblem = createMemo(() => {
    for (const url of urls()) {
      const problem = validateDownloadUrl(url);
      if (problem !== null) return problem;
    }
    return null;
  });
  const canStart = createMemo(
    () => urls().length > 0 && firstProblem() === null && dest().trim() !== "" && !props.busy,
  );

  onMount(() => {
    void props.backend?.downloadLimits?.().then(setLimits).catch(() => setLimits(null));
  });

  // Debounced, sequence protected probes. A late answer from an old URL list
  // must never repaint a newer row.
  let probeTimer: ReturnType<typeof setTimeout> | undefined;
  let probeGeneration = 0;
  createEffect(() => {
    const current = validUrls();
    const generation = ++probeGeneration;
    if (probeTimer) clearTimeout(probeTimer);
    setProbes((old) => {
      const next: Record<string, ProbeState> = {};
      for (const url of current) next[url] = old[url] ?? { status: "resolving" };
      return next;
    });
    if (!props.backend?.probeHttpDownload || current.length === 0) return;
    probeTimer = setTimeout(() => {
      void (async () => {
        for (const url of current) {
          if (generation !== probeGeneration) return;
          setProbes((old) => ({ ...old, [url]: { status: "resolving" } }));
          try {
            const probe = await props.backend!.probeHttpDownload!(url);
            if (generation !== probeGeneration || !validUrls().includes(url)) return;
            setProbes((old) => ({
              ...old,
              [url]: probe.warning
                ? { status: "failed", probe, error: probe.warning }
                : { status: "resolved", probe },
            }));
          } catch (error) {
            if (generation !== probeGeneration || !validUrls().includes(url)) return;
            setProbes((old) => ({
              ...old,
              [url]: {
                status: "failed",
                error: error instanceof Error ? error.message : "Probe failed",
              },
            }));
          }
        }
      })();
    }, 250);
  });
  onCleanup(() => {
    probeGeneration += 1;
    if (probeTimer) clearTimeout(probeTimer);
  });

  // Do not turn an unavailable capability query into a claimed numeric cap.
  // The engine's automatic choice remains available until limits arrive.
  const effectiveCap = createMemo(() =>
    limits() === null
      ? 0
      : Math.max(1, Math.min(limits()!.maxConnections, limits()!.maxPerHost)),
  );
  const noRangeCount = createMemo(
    () => validUrls().filter((url) => probes()[url]?.probe?.ranges === false).length,
  );
  const selectedMirrorUrl = createMemo(() => validUrls()[preferredMirror()]);
  createEffect(() => {
    if (preferredMirror() >= validUrls().length) setPreferredMirror(0);
  });
  const connectionsLocked = createMemo(() => {
    if (mode() === "mirrors") {
      const selected = selectedMirrorUrl();
      return selected !== undefined && probes()[selected]?.probe?.ranges === false;
    }
    return validUrls().length > 0 && noRangeCount() === validUrls().length;
  });
  const connectionValue = createMemo(() => (connectionsLocked() ? 1 : connectionChoice()));

  const choose = async () => {
    try {
      const picked = await props.pickFolder?.();
      if (picked) setDest(picked);
    } catch {
      // A picker that failed is not a reason to close the dialog; the field's
      // typed value stands.
    }
  };

  const start = () => {
    if (!canStart()) return;
    const found = validUrls();
    if (mode() === "mirrors") {
      const option: HttpDownloadOptions = {
        mirrors: found.slice(1),
        preferredMirror: preferredMirror(),
      };
      const chosen = connectionValue();
      if (chosen !== null) option.connections = chosen;
      props.onAdd(found, dest().trim(), [option]);
      return;
    }
    const options = found.map((url) => {
      const option: HttpDownloadOptions = {};
      if (connectionChoice() !== null) option.connections = connectionChoice();
      if (probes()[url]?.probe?.ranges === false) option.connections = 1;
      return option;
    });
    props.onAdd(found, dest().trim(), options);
  };

  return (
    <div class={dialog.scrim} onClick={props.onCancel}>
      <div
        class={dialog.dialog}
        role="dialog"
        aria-modal="true"
        aria-label="Add Download"
        onClick={(event) => event.stopPropagation()}
      >
        <div class={dialog.titlebar}>Add Download</div>
        <div class={dialog.body}>
          <div class={dialog.form}>
            <label class={dialog.label} for="download-urls">
              URLs
            </label>
            <div>
              <textarea
                ref={urlBox}
                id="download-urls"
                {...PLAIN_FIELD}
                class={`${styles.urlField} ${dialog.inputMono}`}
                rows={4}
                placeholder="https://example.test/isos/arrival.2016.mkv"
                value={text()}
                onInput={(event) => setText(event.currentTarget.value)}
              />
            <Show when={urls().length > 0}>
                <p class={dialog.fieldHint}>
                  {urls().length} {urls().length === 1 ? "URL" : "URLs"}
                  {firstProblem() !== null ? ` · ${firstProblem()}` : ""}
                </p>
              </Show>
            </div>


            <Show when={validUrls().length > 0 && props.backend?.probeHttpDownload}>
              <span />
              <div class={styles.probes} aria-live="polite">
                <For each={validUrls()}>{(url) => {
                  const state = () => probes()[url];
                  const probe = () => state()?.probe;
                  const details = () => {
                    const p = probe();
                    return `${p?.length == null ? "size unknown" : formatSize(p.length)} · ${p?.ranges === false ? "no ranges" : p?.ranges === true ? "ranges supported" : "ranges unknown"}`;
                  };
                  return (
                    <div class={styles.probeRow}>
                      <span class={styles.probeUrl}>{redactUrlHost(url) ?? url}</span>
                      <span title={state()?.error}>
                        {state()?.status === "resolving"
                          ? "Resolving…"
                          : state()?.status === "failed"
                            ? `Probe failed · ${details()}`
                            : `Resolved · ${details()}`}
                      </span>
                    </div>
                  );
                }}</For>
              </div>
            </Show>

            <label class={dialog.label} for="download-mode-separate">Downloads</label>
            <div class={styles.modeRow}>
              <label>
                <input
                  id="download-mode-separate"
                  type="radio"
                  name="download-mode"
                  checked={mode() === "separate"}
                  onChange={() => setMode("separate")}
                />
                <span>Separate downloads</span>
              </label>
              <label>
                <input
                  type="radio"
                  name="download-mode"
                  checked={mode() === "mirrors"}
                  onChange={() => setMode("mirrors")}
                />
                <span>Mirrors of one file</span>
              </label>
            </div>
            <Show when={mode() === "mirrors"}>
              <span />
              <div>
                <p class={dialog.fieldHint}>
                  Sources are tried in order before transfer; they are not mixed during a download.
                </p>
                <Show when={validUrls().length > 1}>
                  <fieldset class={styles.mirrorOrder}>
                    <legend>Start with</legend>
                    <For each={validUrls()}>
                      {(url, index) => (
                        <label>
                          <input
                            type="radio"
                            name="preferred-mirror"
                            checked={preferredMirror() === index()}
                            onChange={() => setPreferredMirror(index())}
                          />
                          <span>{redactUrlHost(url) ?? url}</span>
                        </label>
                      )}
                    </For>
                  </fieldset>
                </Show>
              </div>
            </Show>

            <label class={dialog.label} for="download-connections">Connection override</label>
            <div class={styles.connections}>
              <label class={styles.autoConnection}>
                <input
                  id="download-connections"
                  type="radio"
                  name="download-connections"
                  checked={connectionChoice() === null && !connectionsLocked()}
                  disabled={connectionsLocked()}
                  onChange={() => setConnectionChoice(null)}
                />
                <span>Let the engine choose</span>
              </label>
              <div class={styles.connectionScale} aria-label="Connection override">
                <For
                  each={Array.from(
                    { length: Math.max(effectiveCap(), connectionsLocked() ? 1 : 0) },
                    (_, index) => index + 1,
                  )}
                >
                  {(count) => (
                    <label class={styles.connectionChoice}>
                      <input
                        type="radio"
                        name="download-connections"
                        checked={connectionValue() === count}
                        disabled={connectionsLocked()}
                        onChange={() => setConnectionChoice(count)}
                      />
                      <span class={styles.connectionCount}>{count}</span>
                      <span class={styles.connectionBars} aria-hidden="true">
                        <For each={Array.from({ length: count })}>
                          {(_, index) => <i style={{ height: `${5 + index() * 2}px` }} />}
                        </For>
                      </span>
                    </label>
                  )}
                </For>
              </div>
              <span class={styles.connectionHint}>
                {connectionsLocked()
                  ? "This server does not support ranges; using 1 connection."
                  : noRangeCount() > 0 && mode() === "separate"
                    ? `${noRangeCount()} ${noRangeCount() === 1 ? "server uses" : "servers use"} 1 connection; the override applies to the rest.`
                    : limits() === null
                      ? "The engine will choose connections; limits are unavailable."
                      : `Up to ${effectiveCap()} connections per origin (engine max ${limits()!.maxConnections}).`}
              </span>
            </div>

            <label class={dialog.label} for="download-dest">
              Save to
            </label>
            {/* The picker sits beside the field rather than replacing it: a
                typed path has to keep working, because pasting one is faster
                than clicking down to it (the connection dialog's rule). */}
            <div class={dialog.folderRow}>
              <input
                id="download-dest"
                {...PLAIN_FIELD}
                class={`${dialog.input} ${dialog.inputMono}`}
                value={dest()}
                onInput={(event) => setDest(event.currentTarget.value)}
              />
              <Show when={props.pickFolder}>
                <button class={dialog.action} onClick={() => void choose()}>
                  Choose…
                </button>
              </Show>
            </div>
          </div>

          <div class={dialog.actions}>
            <button class={dialog.action} onClick={props.onCancel}>
              Cancel
            </button>
            <button
              class={`${dialog.action} ${dialog.spacer} ${dialog.actionPrimary}`}
              disabled={!canStart()}
              title={
                urls().length === 0
                  ? "Paste one or more URLs, one per line"
                  : (firstProblem() ?? undefined)
              }
              onClick={start}
            >
              {props.busy ? "Starting…" : "Start download"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
