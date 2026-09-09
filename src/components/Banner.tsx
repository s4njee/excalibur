import { Show } from "solid-js";
import styles from "./shell.module.css";
/**
 * The connection banner (E15-S1).
 *
 * Built to the handoff's prose — §Errors describes it and says "build to this
 * description or ask for the mock". Its screenshot is in `docs/design-gaps.md`
 * awaiting sign-off.
 *
 * Connection-level only. A per-item failure belongs in the transfer queue, and
 * a validation failure belongs on its field; the engine already classifies
 * which is which, so this renders `Surface::Banner` and nothing else.
 */
export function Banner(props: {
  message: string;
  /** Absent while a reconnect is already in flight, or when there is no mount. */
  onReconnect?: () => void;
  reconnecting?: boolean;
  onDismiss: () => void;
}) {
  return (
    <div class={styles.banner} role="alert">
      <span class={styles.bannerMessage}>{props.message}</span>
      <Show when={props.onReconnect}>
        {(reconnect) => (
          <button
            class={styles.bannerAction}
            disabled={props.reconnecting}
            onClick={() => reconnect()()}
          >
            {props.reconnecting ? "Reconnecting…" : "Reconnect"}
          </button>
        )}
      </Show>
      <button class={styles.bannerDismiss} aria-label="Dismiss" onClick={props.onDismiss}>
        ✕
      </button>
    </div>
  );
}
