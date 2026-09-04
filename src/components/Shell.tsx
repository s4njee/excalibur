import {
  For,
  Match,
  Show,
  Switch,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  type Accessor,
} from "solid-js";
import styles from "./shell.module.css";

import type { Bookmark, Category, Entry, MountInfo, RowLayout, SortColumn } from "../state/types";
import type { Sort } from "../state/sort";
import {
  ROW_HEIGHTS,
  template,
  visibleColumns,
  type Divider,
  type Widths,
} from "../state/columns";
import { rowsInBand, visibleRange } from "../state/virtual";
import { gate, type Affordance, type Context } from "../state/permissions";
import * as shortcuts from "../state/shortcuts";
import {
  extensionOf,
  formatCapacity,
  formatCounts,
  formatModified,
  formatModifiedGrid,
  formatModifiedShort,
  formatSizeTerse,
} from "../state/format";
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
  /** Named from the same table that binds the key (E16-S1). */
  shortcut?: shortcuts.ShortcutId;
}) {
  const state = () => gate(props.affordance, props.context);
  // A disabled button's tooltip owes the user the reason it is disabled;
  // teaching a shortcut for something they cannot do would be worse than
  // saying nothing. So the refusal wins, and the hint fills the rest.
  const title = () =>
    state().reason ?? (props.shortcut ? shortcuts.tooltip(props.shortcut) : undefined);
  return (
    <button
      class={`${styles.button} ${props.primary ? styles.primary : ""}`}
      disabled={state().disabled}
      title={title()}
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
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  onPreferences: () => void;
  /** So ⌘F can put the caret here. */
  filterRef?: (element: HTMLInputElement) => void;
}) {
  const context = (): Context => ({
    writable: props.mount.writable,
    reason: props.mount.reason,
    hasSelection: props.hasSelection,
  });

  return (
    <div class={styles.toolbar}>
      {/* The design says the sidebar is collapsible and that the flag persists
          (E1-S3), but not where the control lives. Here because the sidebar's
          own footer is three equal buttons at a specified size, and a fourth
          would break that. Recorded in docs/design-gaps.md. */}
      <button
        class={styles.iconButton}
        aria-label={props.sidebarCollapsed ? "Show sidebar" : "Hide sidebar"}
        aria-pressed={!props.sidebarCollapsed}
        onClick={props.onToggleSidebar}
      >
        ▚
      </button>
      <button
        class={styles.iconButton}
        aria-label="Preferences"
        title={shortcuts.tooltip("preferences")}
        onClick={props.onPreferences}
      >
        ⚙
      </button>
      <div class={styles.segmented}>
        <button
          class={styles.segment}
          disabled={!props.canBack}
          aria-label="Back"
          title={shortcuts.tooltip("back")}
          onClick={props.onBack}
        >
          ‹
        </button>
        <button
          class={styles.segment}
          disabled={!props.canForward}
          aria-label="Forward"
          title={shortcuts.tooltip("forward")}
          onClick={props.onForward}
        >
          ›
        </button>
      </div>
      <button
        class={styles.button}
        disabled={!props.canAscend}
        aria-label="Enclosing folder"
        title={shortcuts.tooltip("ascend")}
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
        <Verb affordance="quickLook" context={context()} label="Quick Look" shortcut="quickLook" />
        <Verb
          affordance="edit"
          context={context()}
          label="Edit"
          shortcut="edit"
          onClick={props.onEdit}
        />
        <Verb
          affordance="download"
          context={context()}
          label="Download"
          shortcut="download"
          onClick={props.onDownload}
        />
        <Verb
          affordance="upload"
          context={context()}
          label="Upload"
          shortcut="upload"
          primary
          onClick={props.onUpload}
        />
      </div>
      <input
        ref={props.filterRef}
        class={styles.filter}
        placeholder="Filter"
        title={shortcuts.tooltip("focusFilter")}
        value={props.filter}
        onInput={(event) => props.onFilter(event.currentTarget.value)}
      />
    </div>
  );
}

export function Sidebar(props: {
  /** Double-click: mount the bookmark and open its root. */
  onOpen: (id: string) => void;
  /** Hidden entirely rather than narrowed: there is no rail in the design. */
  collapsed?: boolean;
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
    // Server-to-server is marked future in the handoff, so a drop here is a
    // deliberate no-op with an explanation rather than something that looks
    // broken. `onDragOver` without a `preventDefault` is what refuses it.
    <div
      class={`${styles.sidebar} ${props.collapsed ? styles.sidebarCollapsed : ""}`}
      title="Dropping onto a share is coming later"
      onDragOver={(event) => event.stopPropagation()}
    >
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
  /** A drag from the OS is over the list, and which folder row it is on. */
  dropTarget?: { folder: string | null } | null;
  /** Which of the handoff's three row treatments to draw (E8-S3/S4). */
  layout: RowLayout;
}) {
  // Every row in a layout is the same height, which is what lets the virtual
  // window be arithmetic rather than measurement — but the height differs
  // between layouts, so it is read per render rather than baked in.
  const rowHeight = () => ROW_HEIGHTS[props.layout];
  // Scroll position and viewport height are the only two things the virtual
  // window needs; every row is the same height, so which rows to draw is
  // arithmetic rather than measurement.
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewport, setViewport] = createSignal(0);
  // The list's own width, not the window's: collapsing the sidebar gives the
  // list ~200px back, and the columns should take it (E1-S4).
  const [listWidth, setListWidth] = createSignal(Number.POSITIVE_INFINITY);
  const columns = createMemo(() => visibleColumns(listWidth()));
  let rows!: HTMLDivElement;

  const range = createMemo(() =>
    visibleRange(scrollTop(), viewport(), rowHeight(), props.entries.length),
  );
  const window_ = createMemo(() => props.entries.slice(range().start, range().end));

  onMount(() => {
    const measure = () => {
      setViewport(rows.clientHeight);
      setListWidth(rows.clientWidth);
    };
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
    const top = index * rowHeight();
    const bottom = top + rowHeight();
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
      const { start, end } = rowsInBand(origin, current, rowHeight(), props.entries.length);
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
    <div class={styles.headerSlot} role="columnheader" aria-sort={
      props.sort.column === column ? (props.sort.ascending ? "ascending" : "descending") : "none"
    }>
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

  const layoutClass = () =>
    props.layout === "name-led"
      ? styles.rowNameLed
      : props.layout === "mono"
        ? styles.rowMono
        : "";

  /** How many columns this layout actually has, for the a11y tree. */
  const columnCount = () => {
    if (props.layout === "mono") return 3;
    if (props.layout === "name-led") return 1;
    // Name and Size always; the other two only when they are drawn.
    return 2 + (columns().modified ? 1 : 0) + (columns().kind ? 1 : 0);
  };

  /** One `1h` header cell. Sortable, but with nothing to drag. */
  const monoHeader = (column: SortColumn, label: string) => (
    <div
      class={styles.monoHeaderCell}
      role="columnheader"
      aria-sort={
        props.sort.column === column ? (props.sort.ascending ? "ascending" : "descending") : "none"
      }
    >
      <button class={styles.monoHeaderButton} onClick={() => props.onSort(column)}>
        {label}
        <Show when={props.sort.column === column}>
          <span class={styles.sortMark}>{props.sort.ascending ? "▲" : "▼"}</span>
        </Show>
      </button>
    </div>
  );

  return (
    // The grid wraps both the fixed header and the scrolling body. It uses
    // `display: contents` so the two keep the layout they had as siblings —
    // an ARIA grid whose header row lives outside it cannot associate a column
    // name with a cell, and the header has to stay put while the body scrolls.
    <div
      class={styles.grid}
      role="grid"
      aria-rowcount={props.entries.length}
      aria-colcount={columnCount()}
    >
      <Show when={props.layout === "columns"}>
        <div
          class={styles.columnHeader}
          role="row"
          style={{ "grid-template-columns": template(props.widths, columns()) }}
        >
          {/* Each divider sits on the right edge of the cell before it, so the
              name of the divider is the column it resizes into — and a divider
              for a dropped column is withheld, because dragging it would
              resize something nobody can see. */}
          {header("name", "Name", "size")}
          {header("size", "Size", columns().modified ? "modified" : undefined, true)}
          <Show when={columns().modified}>
            {header("modified", "Modified", columns().kind ? "kind" : undefined)}
          </Show>
          <Show when={columns().kind}>{header("kind", "Kind")}</Show>
        </div>
      </Show>
      {/* `1h`'s own header: mono, uppercase, and no dividers, because the
          layout aligns by character rather than by a width anyone can drag. */}
      <Show when={props.layout === "mono"}>
        <div class={styles.monoHeader} role="row">
          {monoHeader("name", "NAME")}
          {monoHeader("size", "SIZE")}
          {monoHeader("modified", "MTIME")}
        </div>
      </Show>
      {/* Under the header, per the handoff, so it reads as the list loading
          rather than as the window doing something. */}
      <div class={styles.loadingTrack} aria-hidden={!props.slow}>
        <Show when={props.slow}>
          <div class={styles.loadingBar} role="progressbar" aria-label="Loading" />
        </Show>
      </div>
      <div
        class={`${styles.rows} ${props.slow ? styles.rowsStale : ""} ${
          props.dropTarget ? styles.rowsDropping : ""
        }`}
        role="rowgroup"
        ref={rows}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        onPointerDown={startBand}
      >
        {/* One spacer of the full height carries the scrollbar, and the drawn
            rows are offset into it. */}
        <div class={styles.virtualSpacer} style={{ height: `${props.entries.length * rowHeight()}px` }}>
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
          <div style={{ transform: `translateY(${range().start * rowHeight()}px)` }}>
            <For each={window_()}>
              {(entry, index) => {
                const selected = () => props.selected().has(entry.path);
                return (
                  <div
                    class={`${styles.row} ${layoutClass()} ${
                      selected() ? styles.rowSelected : ""
                    } ${entry.hidden ? styles.rowHidden : ""} ${
                      props.dropTarget?.folder === entry.path ? styles.rowDropTarget : ""
                    }`}
                    data-path={entry.path}
                    style={
                      props.layout === "columns"
                        ? {
                            "grid-template-columns": template(props.widths, columns()),
                            height: `${rowHeight()}px`,
                          }
                        : { height: `${rowHeight()}px` }
                    }
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
                    <Switch>
                      {/* `1f` — the dense four-column grid. */}
                      <Match when={props.layout === "columns"}>
                        <div class={styles.nameCell} role="gridcell">
                          {/* The glyph follows the engine's category, not the
                              raw entry kind, so one table decides the label,
                              the icon and Quick Look's viewer. E0-S3 swaps this
                              element for a Lucide icon at the same seam. */}
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
                        <div class={styles.sizeCell} role="gridcell">
                          {entry.kind === "dir" ? (
                            <span class={styles.sizeDash}>—</span>
                          ) : (
                            formatSize(entry.size)
                          )}
                        </div>
                        <Show when={columns().modified}>
                          <div class={styles.metaCell} role="gridcell">
                            {formatModified(entry.modifiedMs)}
                          </div>
                        </Show>
                        <Show when={columns().kind}>
                          <div class={styles.metaCell} role="gridcell">
                            {entry.kindLabel}
                          </div>
                        </Show>
                      </Match>

                      {/* `1g` — name-led and two-line. One cell, because the
                          metadata line is a description of the name rather
                          than a column anyone can sort or align to. */}
                      <Match when={props.layout === "name-led"}>
                        <div class={styles.nameLedCell} role="gridcell">
                          <div
                            class={`${glyphClass(entry.kindCategory)} ${styles.glyphLarge}`}
                            data-category={entry.kindCategory}
                          />
                          <div class={styles.nameLedText}>
                            <Show
                              when={props.renaming === entry.path}
                              fallback={<div class={styles.nameLedName}>{entry.name}</div>}
                            >
                              <RenameField
                                name={entry.name}
                                onCommit={(name) => props.onRename?.(entry.path, name)}
                              />
                            </Show>
                            <div class={styles.nameLedMeta}>
                              {entry.kind === "dir"
                                ? `Folder · ${formatModifiedShort(entry.modifiedMs)}`
                                : `${formatSize(entry.size)} · ${extensionOf(entry.name)} · ${formatModifiedShort(entry.modifiedMs)}`}
                            </div>
                          </div>
                        </div>
                      </Match>

                      {/* `1h` — the mono grid. Kind collapses to a leading
                          type character, the way `ls -l` opens each line. */}
                      <Match when={props.layout === "mono"}>
                        <div class={styles.monoNameCell} role="gridcell">
                          <span
                            class={
                              entry.kind === "dir" ? styles.monoTypeDir : styles.monoTypeFile
                            }
                          >
                            {entry.kind === "dir" ? "d" : "-"}
                          </span>
                          <Show
                            when={props.renaming === entry.path}
                            fallback={<span class={styles.monoName}>{entry.name}</span>}
                          >
                            <RenameField
                              name={entry.name}
                              onCommit={(name) => props.onRename?.(entry.path, name)}
                            />
                          </Show>
                        </div>
                        <div class={styles.monoSizeCell} role="gridcell">
                          {entry.kind === "dir" ? "-" : formatSizeTerse(entry.size)}
                        </div>
                        <div class={styles.monoDateCell} role="gridcell">
                          {formatModifiedGrid(entry.modifiedMs)}
                        </div>
                      </Match>
                    </Switch>
                  </div>
                );
              }}
            </For>
          </div>
        </div>
      </div>
    </div>
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
    // A live region (E16-S2): whether the mount is writable, and why it is
    // not, changes without the user doing anything — a reconnect can come back
    // read-only. `polite` because it should not interrupt, and it is the only
    // place that news appears.
    <div class={styles.statusbar} role="status" aria-live="polite">
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
