import { For, Show, createMemo, createSignal } from "solid-js";
import styles from "./dialog.module.css";
import {
  defaultPort,
  emptyDraft,
  isImplemented,
  previewUri,
  shareLabel,
  toSpec,
  usesDomain,
  usesPassword,
  usesUsername,
  validate,
  type Draft,
} from "../state/connection";
import type { Backend } from "../state/backend";
import type { MountInfo, Protocol } from "../state/types";

const PROTOCOLS: { id: Protocol; label: string }[] = [
  { id: "smb", label: "SMB" },
  { id: "nfs", label: "NFS" },
  { id: "sftp", label: "SFTP" },
  { id: "xsync", label: "xsync" },
];

type TestResult = { ok: true; mount: MountInfo } | { ok: false; message: string };

/**
 * The New Connection dialog.
 *
 * The field set changes with the protocol rather than showing every field and
 * disabling the irrelevant ones: an inert password box on the NFS tab would
 * suggest NFS has a password, which it does not.
 */
export function ConnectionDialog(props: {
  backend: Backend;
  onCancel: () => void;
  onConnected: (name: string) => void;
}) {
  const [draft, setDraft] = createSignal<Draft>(emptyDraft("xsync"));
  const [result, setResult] = createSignal<TestResult | null>(null);
  const [busy, setBusy] = createSignal<"testing" | "connecting" | null>(null);

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => {
    setDraft((current) => ({ ...current, [key]: value }));
    // A field change invalidates a test that described the old draft.
    setResult(null);
  };

  // Split rather than narrowed inside the markup: a discriminated union does
  // not narrow through Solid's `Show`, and two accessors read better than a
  // cast would.
  const succeeded = () => {
    const outcome = result();
    return outcome?.ok ? outcome : null;
  };
  const failed = () => {
    const outcome = result();
    return outcome && !outcome.ok ? outcome : null;
  };

  const problem = createMemo(() => validate(draft()));
  const supported = createMemo(() => isImplemented(draft().protocol));
  const canSubmit = createMemo(() => problem() === null && supported() && busy() === null);

  const test = async () => {
    setBusy("testing");
    setResult(null);
    try {
      const mount = await props.backend.testConnection(toSpec(draft(), "test"));
      setResult({ ok: true, mount });
    } catch (error) {
      setResult({ ok: false, message: String(error) });
    } finally {
      setBusy(null);
    }
  };

  const connect = async () => {
    setBusy("connecting");
    try {
      const id = `${draft().protocol}-${draft().host}-${draft().share}`;
      await props.backend.mount(toSpec(draft(), id));
      props.onConnected(id);
    } catch (error) {
      setResult({ ok: false, message: String(error) });
    } finally {
      setBusy(null);
    }
  };

  return (
    <div class={styles.scrim} onClick={props.onCancel}>
      <div class={styles.dialog} role="dialog" aria-label="New Connection" onClick={(e) => e.stopPropagation()}>
        <div class={styles.titlebar}>New Connection</div>
        <div class={styles.body}>
          <div class={styles.segmentTrack} role="tablist">
            <For each={PROTOCOLS}>
              {(protocol) => (
                <button
                  role="tab"
                  aria-selected={draft().protocol === protocol.id}
                  class={`${styles.segmentTab} ${
                    draft().protocol === protocol.id ? styles.segmentTabActive : ""
                  }`}
                  onClick={() => {
                    setDraft((current) => ({
                      ...emptyDraft(protocol.id),
                      host: current.host,
                      username: current.username,
                      mountPreference: current.mountPreference,
                    }));
                    setResult(null);
                  }}
                >
                  {protocol.label}
                </button>
              )}
            </For>
          </div>

          <div class={styles.form}>
            <label class={styles.label} for="host">
              Server
            </label>
            <input
              id="host"
              class={`${styles.input} ${styles.inputMono}`}
              value={draft().host}
              placeholder="nas.local"
              onInput={(event) => set("host", event.currentTarget.value)}
            />

            <label class={styles.label} for="share">
              {shareLabel(draft().protocol)}
            </label>
            <input
              id="share"
              class={`${styles.input} ${styles.inputMono}`}
              value={draft().share}
              placeholder={draft().protocol === "smb" ? "media" : "/srv/media"}
              onInput={(event) => set("share", event.currentTarget.value)}
            />

            <Show when={defaultPort(draft().protocol) !== null}>
              <label class={styles.label} for="port">
                Port
              </label>
              <input
                id="port"
                class={`${styles.input} ${styles.inputMono}`}
                value={draft().port}
                placeholder={String(defaultPort(draft().protocol))}
                onInput={(event) => set("port", event.currentTarget.value)}
              />
            </Show>

            <Show when={usesUsername(draft().protocol)}>
              <label class={styles.label} for="username">
                Username
              </label>
              <input
                id="username"
                class={styles.input}
                value={draft().username}
                onInput={(event) => set("username", event.currentTarget.value)}
              />
            </Show>

            <Show when={usesPassword(draft().protocol)}>
              <label class={styles.label} for="password">
                Password
              </label>
              <input
                id="password"
                type="password"
                class={styles.input}
                value={draft().password}
                onInput={(event) => set("password", event.currentTarget.value)}
              />
            </Show>

            <Show when={usesDomain(draft().protocol)}>
              <label class={styles.label} for="domain">
                Domain
              </label>
              <input
                id="domain"
                class={styles.input}
                value={draft().domain}
                onInput={(event) => set("domain", event.currentTarget.value)}
              />
            </Show>

            <Show when={usesPassword(draft().protocol)}>
              <span />
              <label class={styles.checkRow}>
                <input
                  type="checkbox"
                  checked={draft().saveCredentials}
                  onChange={(event) => set("saveCredentials", event.currentTarget.checked)}
                />
                Save in keychain
              </label>
            </Show>

            <span class={styles.label}>Mount as</span>
            <div class={styles.radioRow}>
              <label class={styles.radioOption}>
                <input
                  type="radio"
                  name="mount-as"
                  checked={draft().mountPreference === "rw"}
                  onChange={() => set("mountPreference", "rw")}
                />
                Read &amp; write
              </label>
              <label class={styles.radioOption}>
                <input
                  type="radio"
                  name="mount-as"
                  checked={draft().mountPreference === "ro"}
                  onChange={() => set("mountPreference", "ro")}
                />
                Read-only
              </label>
            </div>
          </div>

          {/* Live, and built from a partial draft, so it moves as fields change. */}
          <div class={styles.preview}>{previewUri(draft())}</div>

          <Show when={!supported()}>
            <div class={styles.notice}>
              {PROTOCOLS.find((p) => p.id === draft().protocol)?.label} is not connected yet — the
              engine answers it with “unsupported”. xsync works today.
            </div>
          </Show>

          <div class={styles.actions}>
            <button class={styles.action} disabled={!canSubmit()} onClick={test}>
              {busy() === "testing" ? "Testing…" : "Test"}
            </button>
            <Show when={succeeded()}>
              {(outcome) => (
                <span class={`${styles.result} ${styles.resultOk}`}>
                  {/* What a user is actually asking Test: can I reach it, what
                      am I talking to, and will I be able to write. */}
                  Reachable · {outcome().mount.protocolLabel} ·{" "}
                  {outcome().mount.writable ? "read & write" : outcome().mount.reason}
                </span>
              )}
            </Show>
            <Show when={failed()}>
              {(outcome) => (
                <span class={`${styles.result} ${styles.resultError}`}>{outcome().message}</span>
              )}
            </Show>
            <button class={`${styles.action} ${styles.spacer}`} onClick={props.onCancel}>
              Cancel
            </button>
            <button
              class={`${styles.action} ${styles.actionPrimary}`}
              disabled={!canSubmit()}
              title={problem() ?? undefined}
              onClick={connect}
            >
              {busy() === "connecting" ? "Connecting…" : "Connect"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
