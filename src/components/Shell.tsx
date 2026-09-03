import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount, type Accessor } from "solid-js";
import styles from "./shell.module.css";

/** Must match `--height-row`; the virtual window is arithmetic on it. */
const ROW_HEIGHT = 24;
import type { Bookmark, Category, Entry, MountInfo, SortColumn } from "../state/types";
import type { Sort } from "../state/sort";
import { template, type Divider, type Widths } from "../state/columns";
import { rowsInBand, visibleRange } from "../state/virtual";
import { gate, type Affordance, type Context } from "../state/permissions";
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
 * One toolbar verb, drawn from the permission gate.
 *
 * The tooltip is the server's own reason, verbatim. Paraphrasing it loses the
 * detail that tells a user whether it is their export or their credentials —
 * "Read-only — export mapped `ro,root_squash`" is actionable; "not allowed" is
 * not.
 */
function Verb(props: {
  affordance: Affordance;
  context: Context;
  label: string;
  primary?: boolean;
  onClick?: () => void;
}) {
  const state = () => gate(props.affordance, props.context);
  return (
    <button
      class={`${styles.button} ${props.primary ? styles.primary : ""}`}
      disabled={state().disabled}
      title={state().reason ?? undefined}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}

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
  onEdit?: () => void;
  onDownload?: () => void;
  onUpload?: () => void;
}) {
  const context = (): Context => ({
    writable: props.mount.writable,
    reason: props.mount.reason,
    hasSelection: props.hasSelection,
  });

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
        {/* Every verb goes through `gate`. None of them reads `writable`
            directly: an inline condition here is exactly how a write affordance
            ends up gated on something that is not the mount. */}
        <Verb affordance="quickLook" context={context()} label="Quick Look" />
        <Verb affordance="edit" context={context()} label="Edit" onClick={props.onEdit} />
        <Verb
          affordance="download"
          context={context()}
          label="Download"
          onClick={props.onDownload}
        />
        <Verb
          affordance="upload"
          context={context()}
          label="Upload"
          primary
          onClick={props.onUpload}
        />
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
  onSelect: (path: string, modifiers: { shift?: boolean; toggle?: boolean }) => void;
  /** Descend into a folder. */
  onOpen: (entry: Entry) => void;
  /** A listing has been in flight past the indicator delay. */
  slow?: boolean;
  sort: Sort;
  onSort: (column: SortColumn) => void;
  /** Select the rows a rubber band covered, by index range. */
  onBand: (start: number, end: number, additive: boolean) => void;
  /** Scroll this row into view. Set by type-to-select; cleared by the parent. */
  reveal?: number | null;
  widths: Widths;
  onResize: (divider: Divider, delta: number) => void;
  /** Called when a drag finishes, so the result is persisted once. */
  onResizeEnd: () => void;
  /** The row being renamed inline, by path. */
  renaming?: string | null;
  /** Commit an inline rename. An unchanged name is the caller's to ignore. */
  onRename?: (from: string, name: string) => void;
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

  // Type-to-select can land on a row that is not rendered, so revealing it is
  // a scroll rather than a `scrollIntoView` on an element that may not exist.
  createEffect(() => {
    const index = props.reveal;
    if (index === null || index === undefined) return;
    const top = index * ROW_HEIGHT;
    const bottom = top + ROW_HEIGHT;
    if (top < rows.scrollTop) rows.scrollTop = top;
    else if (bottom > rows.scrollTop + rows.clientHeight) {
      rows.scrollTop = bottom - rows.clientHeight;
    }
    setScrollTop(rows.scrollTop);
  });

  /**
   * Rubber-band selection.
   *
   * Deliberately does not start on the name cell. That is the part of a row a
   * user grabs to drag a file out (E13-S5), and taking it now would mean giving
   * it back later. Everything else — the rest of every row, and the empty space
   * below the last one — bands.
   *
   * The covered rows are computed from content coordinates rather than by
   * hit-testing elements, because hit-testing would only ever find the
   * virtualised window: a band dragged past the viewport must still select the
   * rows it scrolled over.
   */
  const [bandBox, setBandBox] = createSignal<{ from: number; to: number } | null>(null);

  const startBand = (event: PointerEvent) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement;
    if (target.closest(`.${styles.nameCell}`)) return;

    const box = rows.getBoundingClientRect();
    const origin = event.clientY - box.top + rows.scrollTop;
    const additive = event.metaKey || event.ctrlKey;
    let moved = false;

    const move = (at: PointerEvent) => {
      const current = at.clientY - box.top + rows.scrollTop;
      // A few pixels of slop, so a click with a shaky hand stays a click.
      if (!moved && Math.abs(current - origin) < 4) return;
      moved = true;
      setBandBox({ from: origin, to: current });
      const { start, end } = rowsInBand(origin, current, ROW_HEIGHT, props.entries.length);
      props.onBand(start, end, additive);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      setBandBox(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

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
        onPointerDown={startBand}
      >
        {/* One spacer of the full height carries the scrollbar, and the drawn
            rows are offset into it. */}
        <div class={styles.virtualSpacer} style={{ height: `${props.entries.length * ROW_HEIGHT}px` }}>
          <Show when={bandBox()}>
            {(box) => (
              <div
                class={styles.band}
                style={{
                  top: `${Math.min(box().from, box().to)}px`,
                  height: `${Math.abs(box().to - box().from)}px`,
                }}
              />
            )}
          </Show>
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
                    onClick={(event) =>
                      props.onSelect(entry.path, {
                        shift: event.shiftKey,
                        toggle: event.metaKey || event.ctrlKey,
                      })
                    }
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
                      <Show
                        when={props.renaming === entry.path}
                        fallback={<span class={styles.name}>{entry.name}</span>}
                      >
                        <RenameField
                          name={entry.name}
                          onCommit={(name) => props.onRename?.(entry.path, name)}
                        />
                      </Show>
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
    <div
      class={styles.columnDivider}
      onPointerDown={start}
      role="separator"
      aria-orientation="vertical"
    />
  );
}

/**
 * The inline rename editor.
 *
 * Committing on blur as well as on Enter: a user who clicks away has finished
 * typing, and throwing the name away would be the surprising reading of that.
 * Escape is handled by the window, which cancels without committing.
 *
 * The extension is left out of the initial selection, because renaming
 * `Arrival.2016.mkv` almost never means renaming the `.mkv`.
 */
function RenameField(props: { name: string; onCommit: (name: string) => void }) {
  let input!: HTMLInputElement;
  onMount(() => {
    input.focus();
    const dot = props.name.lastIndexOf(".");
    input.setSelectionRange(0, dot > 0 ? dot : props.name.length);
  });
  return (
    <input
      ref={input}
      class={styles.renameField}
      value={props.name}
      onClick={(event) => event.stopPropagation()}
      onDblClick={(event) => event.stopPropagation()}
      onBlur={(event) => props.onCommit(event.currentTarget.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          props.onCommit(event.currentTarget.value);
        }
      }}
    />
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
  /** Reconnect asking for read-write. Absent when there is no mount to retry. */
  onRemount?: () => void;
  /** A remount is in flight. */
  remounting?: boolean;
}) {
  return (
    <div class={styles.statusbar}>
      <span class={`${styles.badge} ${props.mount.writable ? styles.badgeRw : styles.badgeRo}`}>
        {props.mount.writable ? "RW" : "RO"}
      </span>
      <span class={styles.truncate}>
        <Show
          when={props.mount.writable}
          fallback={
            <>
              <span>{props.mount.reason}</span>
              {/* The design's only Remount button lives on a failed transfer
                  row (E13-S2), which does not exist yet — leaving a read-only
                  mount with no recovery path at all. Recorded in
                  docs/design-gaps.md for sign-off. */}
              <Show when={props.onRemount}>
                {(remount) => (
                  <button
                    class={styles.statusAction}
                    disabled={props.remounting}
                    onClick={() => remount()()}
                  >
                    {props.remounting ? "Remounting…" : "Remount as RW"}
                  </button>
                )}
              </Show>
            </>
          }
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
