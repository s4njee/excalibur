import { For, Show, createMemo, type Accessor } from "solid-js";
import styles from "./shell.module.css";
import type { Bookmark, Entry, MountInfo } from "../state/types";
import { formatCapacity, formatCounts, formatModified } from "../state/format";
import { formatSize } from "../state/format";

/** `<share> — <host>`, centred, as the design specifies. */
export function TitleBar(props: { share: string; host: string }) {
  return (
    <div class={styles.titlebar}>
      <span class={styles.title}>
        {props.share} — {props.host}
      </span>
    </div>
  );
}

/**
 * The path field, rendered as clickable breadcrumb segments.
 *
 * Segments rather than a text field by default: the design wants an ancestor
 * to be one click away, and only reveals an editable field when the user
 * clicks the empty space (not yet wired — E1-S2 finishes it).
 */
function PathField(props: { scheme: string; host: string; segments: string[] }) {
  return (
    <div class={styles.pathField}>
      <span class={styles.crumbScheme}>{props.scheme}://</span>
      <span class={styles.crumb}>{props.host}</span>
      <For each={props.segments}>
        {(segment, index) => (
          <>
            <span class={styles.crumbSeparator}>/</span>
            <span
              class={index() === props.segments.length - 1 ? styles.crumbCurrent : styles.crumb}
            >
              {segment}
            </span>
          </>
        )}
      </For>
    </div>
  );
}

/**
 * The verbs act on the selection; the write ones are disabled from
 * `mount.writable` alone.
 */
export function Toolbar(props: {
  mount: MountInfo;
  scheme: string;
  host: string;
  segments: string[];
  hasSelection: boolean;
  filter: string;
  onFilter: (value: string) => void;
}) {
  return (
    <div class={styles.toolbar}>
      <div class={styles.segmented}>
        <button class={styles.segment} disabled aria-label="Back">
          ‹
        </button>
        <button class={styles.segment} disabled aria-label="Forward">
          ›
        </button>
      </div>
      <button class={styles.button} disabled aria-label="Enclosing folder">
        ↑
      </button>
      <PathField scheme={props.scheme} host={props.host} segments={props.segments} />
      <div class={styles.verbs}>
        <button class={styles.button} disabled={!props.hasSelection}>
          Quick Look
        </button>
        {/* Edit and Upload write, so they read `writable` and nothing else. */}
        <button class={styles.button} disabled={!props.hasSelection || !props.mount.writable}>
          Edit
        </button>
        <button class={styles.button} disabled={!props.hasSelection}>
          Download
        </button>
        <button
          class={`${styles.button} ${styles.primary}`}
          disabled={!props.mount.writable}
          title={props.mount.writable ? undefined : props.mount.reason}
        >
          Upload
        </button>
      </div>
      <input
        class={styles.filter}
        placeholder="Filter"
        value={props.filter}
        onInput={(event) => props.onFilter(event.currentTarget.value)}
      />
    </div>
  );
}

export function Sidebar(props: {
  bookmarks: Bookmark[];
  activeId: string;
  writable: boolean;
  onNewConnection: () => void;
}) {
  const mounted = createMemo(() => props.bookmarks.filter((bookmark) => bookmark.mounted));
  const saved = createMemo(() => props.bookmarks.filter((bookmark) => !bookmark.mounted));

  return (
    <div class={styles.sidebar}>
      <div class={styles.sectionHeader}>Mounted</div>
      <For each={mounted()}>
        {(bookmark) => {
          const active = () => bookmark.id === props.activeId;
          return (
            <div
              class={`${styles.sidebarRow} ${active() ? styles.sidebarRowActive : ""}`}
              role="button"
              tabindex="0"
            >
              <div class={styles.sidebarText}>
                <div
                  class={`${styles.sidebarName} ${active() ? styles.sidebarNameActive : ""}`}
                >
                  {bookmark.name}
                </div>
                <div
                  class={`${styles.sidebarSubtitle} ${
                    active() ? styles.sidebarSubtitleActive : ""
                  }`}
                >
                  {bookmark.protocol.toUpperCase()} · {bookmark.host}
                </div>
              </div>
            </div>
          );
        }}
      </For>

      <div class={`${styles.sectionHeader} ${styles.sectionHeaderSpaced}`}>Bookmarks</div>
      <For each={saved()}>
        {(bookmark) => (
          <div
            class={`${styles.sidebarRow} ${
              bookmark.reachable ? "" : styles.sidebarRowUnreachable
            }`}
            role="button"
            tabindex="0"
          >
            <div class={styles.fileGlyph} />
            <div class={styles.sidebarText}>
              <div class={styles.sidebarName}>{bookmark.name}</div>
              <div class={styles.sidebarSubtitle}>
                {bookmark.reachable
                  ? `${bookmark.protocol.toUpperCase()} · ${bookmark.host}`
                  : "unreachable"}
              </div>
            </div>
          </div>
        )}
      </For>

      <div class={styles.sidebarFooter}>
        <button
          class={styles.sidebarFooterButton}
          aria-label="New connection"
          onClick={props.onNewConnection}
        >
          +
        </button>
        {/* Removing a bookmark is a write to local state, not the share, so it
            is not gated on the mount. Editing likewise. */}
        <button class={styles.sidebarFooterButton} aria-label="Remove bookmark">
          −
        </button>
        <button class={styles.sidebarFooterButton}>Edit</button>
      </div>
    </div>
  );
}

export function FileList(props: {
  entries: Entry[];
  selected: Accessor<Set<string>>;
  onSelect: (path: string, additive: boolean) => void;
}) {
  return (
    <>
      <div class={styles.columnHeader}>
        <div>
          Name<span class={styles.sortMark}>▲</span>
        </div>
        <div class={styles.headerRight}>Size</div>
        <div>Modified</div>
        <div>Kind</div>
      </div>
      <div class={styles.rows} role="grid">
        <For each={props.entries}>
          {(entry) => {
            const selected = () => props.selected().has(entry.path);
            return (
              <div
                class={`${styles.row} ${selected() ? styles.rowSelected : ""} ${
                  entry.hidden ? styles.rowHidden : ""
                }`}
                role="row"
                onClick={(event) => props.onSelect(entry.path, event.metaKey || event.ctrlKey)}
              >
                <div class={styles.nameCell}>
                  <div class={entry.kind === "dir" ? styles.folderGlyph : styles.fileGlyph} />
                  <span class={styles.name}>{entry.name}</span>
                </div>
                <div class={styles.sizeCell}>
                  {entry.kind === "dir" ? <span class={styles.sizeDash}>—</span> : formatSize(entry.size)}
                </div>
                <div class={styles.metaCell}>{formatModified(entry.modifiedMs)}</div>
                <div class={styles.metaCell}>{entry.kindLabel}</div>
              </div>
            );
          }}
        </For>
      </div>
    </>
  );
}

/**
 * The permission surface. Reading left to right, a user learns whether they can
 * write before anything else, which is the whole point of its ordering.
 */
export function StatusBar(props: {
  mount: MountInfo;
  path: string;
  items: number;
  selected: number;
  selectedBytes: number;
}) {
  return (
    <div class={styles.statusbar}>
      <span class={`${styles.badge} ${props.mount.writable ? styles.badgeRw : styles.badgeRo}`}>
        {props.mount.writable ? "RW" : "RO"}
      </span>
      <span class={styles.truncate}>
        <Show
          when={props.mount.writable}
          fallback={<span>{props.mount.reason}</span>}
        >
          Read &amp; write on <span class={styles.statusPath}>{props.path}</span>
        </Show>
      </span>
      <span class={styles.divider} />
      <span>{formatCounts(props.items, props.selected, props.selectedBytes)}</span>
      <Show when={props.mount.capacity}>
        {(capacity) => (
          <>
            <span class={styles.divider} />
            <span>{formatCapacity(capacity().free, capacity().total)}</span>
          </>
        )}
      </Show>
      <span class={styles.statusRight}>
        <span class={styles.protocol}>
          {props.mount.protocolLabel} · {props.mount.securityLabel}
        </span>
        <span
          class={`${styles.healthDot} ${
            props.mount.status === "ready" ? "" : styles.healthDotFailed
          }`}
        />
      </span>
    </div>
  );
}

export { styles as shellStyles };
