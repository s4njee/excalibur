import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount, type Accessor } from "solid-js";
import styles from "./shell.module.css";

/** Must match `--height-row`; the virtual window is arithmetic on it. */
const ROW_HEIGHT = 24;
import type { Bookmark, Category, Entry, MountInfo, SortColumn } from "../state/types";
import type { Sort } from "../state/sort";
import { template, type Divider, type Widths } from "../state/columns";
import { visibleRange } from "../state/virtual";
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
function PathField(props: {
  scheme: string;
  host: string;
  segments: string[];
  onCrumb: (depth: number) => void;
}) {
  return (
    <div class={styles.pathField}>
      <span class={styles.crumbScheme}>{props.scheme}://</span>
      {/* The host is the share root: depth 0. */}
      <button class={styles.crumb} onClick={() => props.onCrumb(0)}>
        {props.host}
      </button>
      <For each={props.segments}>
        {(segment, index) => {
          const last = () => index() === props.segments.length - 1;
          return (
            <>
              <span class={styles.crumbSeparator}>/</span>
              {/* The last crumb is where we already are. Rendering it as a
                  button would offer a click that does nothing. */}
              <Show
                when={!last()}
                fallback={<span class={styles.crumbCurrent}>{segment}</span>}
              >
                <button class={styles.crumb} onClick={() => props.onCrumb(index() + 1)}>
                  {segment}
                </button>
              </Show>
            </>
          );
        }}
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
  canBack: boolean;
  canForward: boolean;
  canAscend: boolean;
  onBack: () => void;
  onForward: () => void;
  onAscend: () => void;
  onCrumb: (depth: number) => void;
}) {
  return (
    <div class={styles.toolbar}>
      <div class={styles.segmented}>
        <button
          class={styles.segment}
          disabled={!props.canBack}
          aria-label="Back"
          onClick={props.onBack}
        >
          ‹
        </button>
        <button
          class={styles.segment}
          disabled={!props.canForward}
          aria-label="Forward"
          onClick={props.onForward}
        >
          ›
        </button>
      </div>
      <button
        class={styles.button}
        disabled={!props.canAscend}
        aria-label="Enclosing folder"
        onClick={props.onAscend}
      >
        ↑
      </button>
      <PathField
        scheme={props.scheme}
        host={props.host}
        segments={props.segments}
        onCrumb={props.onCrumb}
      />
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
  /** Double-click: mount the bookmark and open its root. */
  onOpen: (id: string) => void;
  bookmarks: Bookmark[];
  activeId: string;
  selectedId: string | null;
  onNewConnection: () => void;
  onSelect: (id: string) => void;
  onRemove: (id: string) => void;
  onEdit: (id: string) => void;
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
              onClick={() => props.onSelect(bookmark.id)}
              onDblClick={() => props.onOpen(bookmark.id)}
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
            } ${bookmark.id === props.selectedId ? styles.sidebarRowActive : ""}`}
            role="button"
            tabindex="0"
            onClick={() => props.onSelect(bookmark.id)}
            onDblClick={() => props.onOpen(bookmark.id)}
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
        <button
          class={styles.sidebarFooterButton}
          aria-label="Remove bookmark"
          disabled={props.selectedId === null}
          onClick={() => props.selectedId && props.onRemove(props.selectedId)}
        >
          −
        </button>
        <button
          class={styles.sidebarFooterButton}
          disabled={props.selectedId === null}
          onClick={() => props.selectedId && props.onEdit(props.selectedId)}
        >
          Edit
        </button>
      </div>
    </div>
  );
}

export function FileList(props: {
  entries: Entry[];
  selected: Accessor<Set<string>>;
  onSelect: (path: string, additive: boolean) => void;
  /** Descend into a folder. */
  onOpen: (entry: Entry) => void;
  /** A listing has been in flight past the indicator delay. */
  slow?: boolean;
  sort: Sort;
  onSort: (column: SortColumn) => void;
  widths: Widths;
  onResize: (divider: Divider, delta: number) => void;
  /** Called when a drag finishes, so the result is persisted once. */
  onResizeEnd: () => void;
}) {
  // Scroll position and viewport height are the only two things the virtual
  // window needs; every row is the same height, so which rows to draw is
  // arithmetic rather than measurement.
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewport, setViewport] = createSignal(0);
  let rows!: HTMLDivElement;

  const range = createMemo(() =>
    visibleRange(scrollTop(), viewport(), ROW_HEIGHT, props.entries.length),
  );
  const window_ = createMemo(() => props.entries.slice(range().start, range().end));

  onMount(() => {
    const measure = () => setViewport(rows.clientHeight);
    measure();
    // The list is a flex child of a resizable window, so its height changes
    // without anything scrolling. A resize that is not observed leaves the
    // window rendering a viewport's worth of rows for the old height.
    const observer = new ResizeObserver(measure);
    observer.observe(rows);
    onCleanup(() => observer.disconnect());
  });

  // A listing shorter than the last one can leave the container scrolled past
  // its own end, which renders as an empty list until the user scrolls up.
  createEffect(() => {
    props.entries.length;
    if (rows.scrollTop !== 0 && rows.scrollTop > rows.scrollHeight - rows.clientHeight) {
      rows.scrollTop = 0;
      setScrollTop(0);
    }
  });

  /**
   * One header cell, with the divider that follows it.
   *
   * The divider is a child of the cell's slot rather than a positioned sibling
   * over the grid. An earlier version placed them by summing column widths from
   * the right edge, which quietly ignored the 16px gaps and the header's own
   * padding and put every divider ~50px into the next column, on top of its
   * text. Anchoring to the slot's own edge cannot drift: it is the boundary,
   * whatever the widths are.
   */
  const header = (column: SortColumn, label: string, divider?: Divider, right = false) => (
    <div class={styles.headerSlot}>
      <button
        class={`${styles.headerCell} ${right ? styles.headerRight : ""}`}
        aria-sort={
          props.sort.column === column ? (props.sort.ascending ? "ascending" : "descending") : "none"
        }
        onClick={() => props.onSort(column)}
      >
        {label}
        <Show when={props.sort.column === column}>
          <span class={styles.sortMark}>{props.sort.ascending ? "▲" : "▼"}</span>
        </Show>
      </button>
      <Show when={divider}>
        {(which) => (
          <ColumnDivider
            onDrag={(delta) => props.onResize(which(), delta)}
            onDone={props.onResizeEnd}
          />
        )}
      </Show>
    </div>
  );

  return (
    <>
      <div class={styles.columnHeader} style={{ "grid-template-columns": template(props.widths) }}>
        {/* Each divider sits on the right edge of the cell before it, so the
            name of the divider is the column it resizes into. */}
        {header("name", "Name", "size")}
        {header("size", "Size", "modified", true)}
        {header("modified", "Modified", "kind")}
        {header("kind", "Kind")}
      </div>
      {/* Under the header, per the handoff, so it reads as the list loading
          rather than as the window doing something. */}
      <div class={styles.loadingTrack} aria-hidden={!props.slow}>
        <Show when={props.slow}>
          <div class={styles.loadingBar} role="progressbar" aria-label="Loading" />
        </Show>
      </div>
      <div
        class={`${styles.rows} ${props.slow ? styles.rowsStale : ""}`}
        role="grid"
        aria-rowcount={props.entries.length}
        ref={rows}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        {/* One spacer of the full height carries the scrollbar, and the drawn
            rows are offset into it. */}
        <div class={styles.virtualSpacer} style={{ height: `${props.entries.length * ROW_HEIGHT}px` }}>
          <div style={{ transform: `translateY(${range().start * ROW_HEIGHT}px)` }}>
            <For each={window_()}>
              {(entry, index) => {
                const selected = () => props.selected().has(entry.path);
                return (
                  <div
                    class={`${styles.row} ${selected() ? styles.rowSelected : ""} ${
                      entry.hidden ? styles.rowHidden : ""
                    }`}
                    style={{ "grid-template-columns": template(props.widths) }}
                    role="row"
                    aria-rowindex={range().start + index() + 1}
                    aria-selected={selected()}
                    onClick={(event) => props.onSelect(entry.path, event.metaKey || event.ctrlKey)}
                    onDblClick={() => props.onOpen(entry)}
                  >
                    <div class={styles.nameCell}>
                      {/* The glyph follows the engine's category, not the raw
                          entry kind, so one table decides the label, the icon
                          and Quick Look's viewer. E0-S3 swaps this element for
                          a Lucide icon at the same seam. */}
                      <div
                        class={glyphClass(entry.kindCategory)}
                        data-category={entry.kindCategory}
                      />
                      <span class={styles.name}>{entry.name}</span>
                    </div>
                    <div class={styles.sizeCell}>
                      {entry.kind === "dir" ? (
                        <span class={styles.sizeDash}>—</span>
                      ) : (
                        formatSize(entry.size)
                      )}
                    </div>
                    <div class={styles.metaCell}>{formatModified(entry.modifiedMs)}</div>
                    <div class={styles.metaCell}>{entry.kindLabel}</div>
                  </div>
                );
              }}
            </For>
          </div>
        </div>
      </div>
    </>
  );
}

/**
 * One draggable column boundary.
 *
 * Listens on the window rather than on itself for the duration of a drag: a
 * pointer moving faster than the element can follow would otherwise leave the
 * divider behind and drop the drag halfway across the header.
 */
function ColumnDivider(props: { onDrag: (delta: number) => void; onDone: () => void }) {
  const start = (event: PointerEvent) => {
    event.preventDefault();
    let last = event.clientX;
    const move = (moved: PointerEvent) => {
      props.onDrag(moved.clientX - last);
      last = moved.clientX;
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      props.onDone();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div class={styles.divider} onPointerDown={start} role="separator" aria-orientation="vertical" />
  );
}

/** Which glyph shape a category gets. Colour is per-category in CSS. */
function glyphClass(category: Category): string | undefined {
  return category === "folder" || category === "alias" ? styles.folderGlyph : styles.fileGlyph;
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
  /** The unfiltered count, when a filter is hiding rows. */
  total?: number;
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
      <span>{formatCounts(props.items, props.selected, props.selectedBytes, props.total)}</span>
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
