import { For, Show, createMemo, createResource, createSignal } from "solid-js";
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
import { bookmarkId } from "../state/bookmarks";
import type { MountInfo, Protocol } from "../state/types";
import { engineFailure, type HostKeyProblem } from "../state/errors";

const PROTOCOLS: { id: Protocol; label: string }[] = [
  { id: "smb", label: "SMB" },
  { id: "nfs", label: "NFS" },
  { id: "sftp", label: "SFTP" },
  { id: "xsync", label: "xsync" },
];

type TestResult = { ok: true; mount: MountInfo } | { ok: false; message: string };

/** The host-key state the dialog can be in. */
type HostKeyState =
  /** The host is unknown. A question, with an answer that unblocks it. */
  | { kind: "ask"; problem: HostKeyProblem }
  /** The key changed. A report, with no answer offered here. */
  | { kind: "changed"; problem: HostKeyProblem };

/**
 * The New Connection dialog.
 *
 * The field set changes with the protocol rather than showing every field and
 * disabling the irrelevant ones: an inert password box on the NFS tab would
 * suggest NFS has a password, which it does not.
 */
export function ConnectionDialog(props: {
  backend: Backend;
  /** Prefill, when reopening the dialog on an existing bookmark. */
  initial?: Draft;
  onCancel: () => void;
  onConnected: (draft: Draft) => void;
}) {
  const [draft, setDraft] = createSignal<Draft>(props.initial ?? emptyDraft("xsync"));
  const [result, setResult] = createSignal<TestResult | null>(null);
  const [busy, setBusy] = createSignal<"testing" | "connecting" | "trusting" | null>(null);
  const [hostKey, setHostKey] = createSignal<HostKeyState | null>(null);
  // Asked once: a machine either has a credential service or it does not, and
  // offering to save into nothing is worse than saying so.
  const [keychain] = createResource(() => props.backend.keychainAvailable());

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => {
    setDraft((current) => ({ ...current, [key]: value }));
    // A field change invalidates a test that described the old draft.
    setResult(null);
    setHostKey(null);
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

  /**
   * Routes a failure to the surface the engine chose for it.
   *
   * The two host-key cases are deliberately different shapes on screen. An
   * unknown host is a question: it gets a prompt with a fingerprint to check
   * and a button that answers it. A changed key is not a question — it is
   * indistinguishable from someone impersonating the host, and no fact
   * available in this dialog would let a user tell that apart from a rebuilt
   * server. So it gets a report and no way through.
   */
  const report = (error: unknown) => {
    const failure = engineFailure(error);
    if (failure.hostKey) {
      setHostKey({
        kind: failure.hostKey.expected ? "changed" : "ask",
        problem: failure.hostKey,
      });
      return;
    }
    setResult({ ok: false, message: failure.message });
  };

  const test = async () => {
    setBusy("testing");
    setResult(null);
    setHostKey(null);
    try {
      const mount = await props.backend.testConnection(toSpec(draft(), "test"));
      setResult({ ok: true, mount });
    } catch (error) {
      report(error);
    } finally {
      setBusy(null);
    }
  };

  /** Accept the offered key, then carry on with the connection it blocked. */
  const trust = async () => {
    const state = hostKey();
    if (state?.kind !== "ask") return;
    setBusy("trusting");
    try {
      await props.backend.trustHostKey(state.problem.host, state.problem.port);
      setHostKey(null);
      setBusy(null);
      await connect();
    } catch (error) {
      report(error);
      setBusy(null);
    }
  };

  const connect = async () => {
    setBusy("connecting");
    try {
      const id = bookmarkId(draft());
      await props.backend.mount(toSpec(draft(), id));
      // The credential goes to the OS keychain after the connection worked, and
      // only if asked for. Unticking the box is an instruction to forget, not
      // merely to skip saving.
      if (draft().password && draft().saveCredentials && keychain()) {
        await props.backend.saveSecret(id, draft().password);
      } else if (!draft().saveCredentials) {
        await props.backend.forgetSecret(id).catch(() => undefined);
      }
      const connected = draft();
      // Drop the password from component state as soon as it has been handed
      // over. JavaScript cannot zeroize, but leaving it in a live signal for
      // the rest of the session is a choice, and this is the other one.
      setDraft((current) => ({ ...current, password: "" }));
      props.onConnected(connected);
    } catch (error) {
      report(error);
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
              <label
                class={styles.checkRow}
                title={
                  keychain()
                    ? undefined
                    : "No keychain available — credentials will be asked for each time"
                }
              >
                <input
                  type="checkbox"
                  disabled={!keychain()}
                  checked={draft().saveCredentials && (keychain() ?? false)}
                  onChange={(event) => set("saveCredentials", event.currentTarget.checked)}
                />
                Save in keychain
                <Show when={keychain() === false}>
                  <span class={styles.hint}>— none on this machine</span>
                </Show>
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
              engine answers it with “unsupported”. xsync and SFTP work today.
            </div>
          </Show>

          <Show when={hostKey()?.kind === "ask" ? hostKey()?.problem : null}>
            {(problem) => (
              <div class={styles.trust} role="alertdialog" aria-label="Unknown host key">
                <div class={styles.trustTitle}>
                  {problem().host} is not a host you have connected to before
                </div>
                <div class={styles.trustBody}>
                  Its {problem().algorithm} key fingerprint is
                  <code class={styles.fingerprint}>{problem().fingerprint}</code>
                  Check that against the server itself before accepting it. If it does not
                  match, something is answering in its place.
                </div>
                <div class={styles.trustActions}>
                  <button class={styles.action} onClick={() => setHostKey(null)}>
                    Cancel
                  </button>
                  <button
                    class={`${styles.action} ${styles.actionPrimary}`}
                    disabled={busy() !== null}
                    onClick={trust}
                  >
                    {busy() === "trusting" ? "Trusting…" : "Trust and connect"}
                  </button>
                </div>
              </div>
            )}
          </Show>

          <Show when={hostKey()?.kind === "changed" ? hostKey()?.problem : null}>
            {(problem) => (
              // No accept button, by design. The AC calls this a deliberate
              // second step: clearing it means going to the bookmark editor and
              // forgetting the old key on purpose, which is a different act
              // from dismissing a warning that is in your way.
              <div class={styles.keyChanged} role="alert">
                <div class={styles.trustTitle}>
                  The host key for {problem().host} has changed
                </div>
                <div class={styles.trustBody}>
                  Expected<code class={styles.fingerprint}>{problem().expected}</code>
                  but it offered<code class={styles.fingerprint}>{problem().fingerprint}</code>
                  This is what an impersonated host looks like. It is also what a rebuilt
                  server looks like — find out which before going further. To connect anyway
                  you have to forget the old key in the bookmark’s editor.
                </div>
              </div>
            )}
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
