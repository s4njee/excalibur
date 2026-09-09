import { Show } from "solid-js";
import styles from "./shell.module.css";
/**
 * `<share> — <host>`, centred, as the design specifies.
 *
 * Both are null when nothing is mounted, and then the bar names nothing
 * (E1-S6). The design draws no first-run title, and the alternative — keeping
 * the last share's name, or the demo's — would title the window after a share
 * it is not showing.
 *
 * `title` names a view that is not a share (XDL-E2-S1: the download queue),
 * which has no host to pair with.
 */
export function TitleBar(props: {
  share: string | null;
  host: string | null;
  title?: string;
}) {
  return (
    <div class={styles.titlebar} data-tauri-drag-region>
      <Show
        when={
          props.title ??
          (props.share !== null && props.host !== null
            ? `${props.share} — ${props.host}`
            : null)
        }
      >
        {(text) => <span class={styles.title}>{text()}</span>}
      </Show>
    </div>
  );
}
