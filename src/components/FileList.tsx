import { For, Match, Show, Switch, createEffect, createMemo, createSignal, onCleanup, onMount, type Accessor } from "solid-js";
import styles from "./shell.module.css";
import type { Category, Entry, RowLayout, SortColumn } from "../state/types";
import type { Sort } from "../state/sort";
import {
  ICON_CELL_WIDTH,
  ROW_HEIGHTS,
  template,
  visibleColumns,
  type Divider,
  type VisibleColumns,
  type Widths,
} from "../state/columns";
import {
  cellsInBand,
  gridRange,
  itemsPerRow,
  rowsInBand,
  visibleRange,
} from "../state/virtual";
import { extensionOf, formatModified, formatModifiedGrid, formatModifiedShort, formatSizeTerse } from "../state/format";
import { formatSize } from "../state/format";
export function FileList(props: {
  entries: Entry[];
  selected: Accessor<Set<string>>;
  onSelect: (path: string, modifiers: { shift?: boolean; toggle?: boolean }) => void;
  /** Descend into a folder. */
  onOpen: (entry: Entry) => void;
  /**
   * Right-click, with the row under the pointer — `null` for the list
   * background, which is a menu about the folder rather than about a file
   * (E14-S2). The coordinates are the viewport's, which is where the menu is
   * positioned.
   */
  onContextMenu?: (entry: Entry | null, at: { x: number; y: number }) => void;
  /**
   * Go to the enclosing folder (E8-S10).
   *
   * Absent at the share root, and that absence is what withholds the `..`
   * row: there is no state here saying whether ascending is possible, so the
   * row cannot come to disagree with the toolbar's `↑` about it.
   */
  onAscend?: () => void;
  /** A listing has been in flight past the indicator delay. */
  slow?: boolean;
  sort: Sort;
  onSort: (column: SortColumn) => void;
  /** Select the rows a rubber band covered, by index range. */
  onBand: (paths: string[], additive: boolean) => void;
  /** Scroll this row into view. Set by type-to-select; cleared by the parent. */
  reveal?: number | null;
  /**
   * How many whole rows fit on screen, for Page Up/Down (E26-S2).
   *
   * The list reports it rather than the window measuring: the row height is a
   * property of the layout this component chose, and the viewport is the
   * element it owns. A parent asking the DOM for both would be reading state
   * that lives here.
   */
  onViewportRows?: (rows: number) => void;
  widths: Widths;
  /**
   * `visible` travels with the drag because which pair a divider sits between
   * depends on what is drawn: with Added dropped, the divider left of Modified
   * has Size on its other side.
   */
  onResize: (divider: Divider, delta: number, visible: VisibleColumns) => void;
  /** Called when a drag finishes, so the result is persisted once. */
  onResizeEnd: () => void;
  /** The row being renamed inline, by path. */
  renaming?: string | null;
  /** Commit an inline rename. An unchanged name is the caller's to ignore. */
  onRename?: (from: string, name: string) => void;
  /** A drag is over the list, and which folder row it is on. */
  dropTarget?: {
    folder: string | null;
    /** What a drop would do, or why it cannot (E24-S2). Empty for an OS drag. */
    message?: string;
    refused?: boolean;
  } | null;
  /** Drag a selection out of the list. The gesture E8-S8 reserved (E24-S2). */
  onDragOut?: (entry: Entry) => void;
  /** Which of the handoff's three row treatments to draw (E8-S3/S4). */
  layout: RowLayout;
  /**
   * Rows that are still arriving (XTP-E3-S3).
   *
   * A file mid-transfer is drawn in the destination before it exists there,
   * because the alternative is a folder that stays empty while something is
   * obviously happening. It becomes an ordinary row the moment the listing
   * knows about it, which needs no relist.
   */
  inFlight?: (path: string) => boolean;
}) {
  // Every row in a layout is the same height, which is what lets the virtual
  // window be arithmetic rather than measurement — but the height differs
  // between layouts, so it is read per render rather than baked in.
  const rowHeight = () => ROW_HEIGHTS[props.layout];
  /**
   * Rows drawn above the listing: the `..` row, or nothing at the root.
   *
   * The virtual window's arithmetic is all in rows, so the parent row is one
   * row of offset rather than a special case in five places — the spacer is a
   * row taller, the drawn window starts a row lower, and an index into
   * `entries` is one row further down the content than it used to be.
   */
  const leadRows = () => (props.onAscend ? 1 : 0);
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

  const icons = () => props.layout === "icons";
  /**
   * Cells across, in the icon grid (E26-S4). One everywhere else, which makes
   * the row layouts a grid one cell wide and lets the arithmetic below be
   * written once.
   */
  const perRow = createMemo(() =>
    icons() ? itemsPerRow(listWidth(), ICON_CELL_WIDTH) : 1,
  );
  /**
   * The `..` row keeps its own height in the grid.
   *
   * It is not a cell: it is one full-width row above them, and giving it a
   * cell's 96px would draw an enormous empty band over the listing. So the
   * lead is measured in pixels rather than in rows from here down.
   */
  const leadHeight = () => leadRows() * (icons() ? ROW_HEIGHTS.columns : rowHeight());
  const gridRows = () => Math.ceil(props.entries.length / perRow());
  /** The scrollable content, which the spacer has to be exactly as tall as. */
  const contentHeight = () =>
    leadHeight() + (icons() ? gridRows() * rowHeight() : props.entries.length * rowHeight());

  const range = createMemo(() =>
    icons()
      ? gridRange(
          Math.max(0, scrollTop() - leadHeight()),
          viewport(),
          rowHeight(),
          perRow(),
          props.entries.length,
        )
      : visibleRange(scrollTop(), viewport(), rowHeight(), props.entries.length),
  );
  const window_ = createMemo(() => props.entries.slice(range().start, range().end));
  /** Where the drawn window sits inside the spacer. */
  const windowOffset = () =>
    leadHeight() +
    (icons()
      ? Math.floor(range().start / perRow()) * rowHeight()
      : range().start * rowHeight());

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

  // A page is what is on screen, so it follows both the window's height and the
  // row treatment — `1h`'s shorter rows put more of them in the same viewport.
  // At least one, so a list measured before layout still moves when the key is
  // pressed rather than sitting still.
  createEffect(() => {
    props.onViewportRows?.(Math.max(1, Math.floor(viewport() / rowHeight())));
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
    const top = (index + leadRows()) * rowHeight();
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
  /**
    * The rubber band, in content coordinates.
    *
    * Two dimensions because the grid needs both: a band over rows is a
    * vertical extent, but a band over cells is a rectangle. The row layouts
    * ignore `x` and stretch edge to edge, which is what `.band` does by
    * default.
    */
   const [bandBox, setBandBox] = createSignal<{
     from: { x: number; y: number };
     to: { x: number; y: number };
   } | null>(null);

  /**
   * Whether a pointer landed on the part of a row that starts a drag.
   *
   * All three layouts, not just `1f`'s: the name is the handle in every one of
   * them, and a rubber band that started on the name in two layouts and a drag
   * in the third would be the same gesture meaning two things.
   */
  const onName = (target: HTMLElement): boolean =>
    Boolean(
      target.closest(
        `.${styles.nameCell}, .${styles.nameLedCell}, .${styles.monoNameCell}`,
      ),
    );

  const startBand = (event: PointerEvent) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement;
    if (onName(target)) return;

    const box = rows.getBoundingClientRect();
    // Measured from the first *entry*, not from the top of the content: the
    // parent row sits above them and is not one of them. Both band functions
    // clamp, so a band started on the `..` row begins at the first entry.
    const lead = leadHeight();
    const at = (event: PointerEvent) => ({
      x: event.clientX - box.left + rows.scrollLeft,
      y: event.clientY - box.top + rows.scrollTop - lead,
    });
    const origin = at(event);
    const additive = event.metaKey || event.ctrlKey;
    let moved = false;

    const move = (event: PointerEvent) => {
      const current = at(event);
      // A few pixels of slop, so a click with a shaky hand stays a click. The
      // grid moves in two directions, so the slop has to as well.
      const travelled = icons()
        ? Math.hypot(current.x - origin.x, current.y - origin.y)
        : Math.abs(current.y - origin.y);
      if (!moved && travelled < 4) return;
      moved = true;
      setBandBox({ from: origin, to: current });
      const covered = icons()
        ? cellsInBand(
            origin,
            current,
            listWidth() / perRow(),
            rowHeight(),
            perRow(),
            props.entries.length,
          )
        : (() => {
            const { start, end } = rowsInBand(
              origin.y,
              current.y,
              rowHeight(),
              props.entries.length,
            );
            const range: number[] = [];
            for (let index = start; index < end; index += 1) range.push(index);
            return range;
          })();
      // Paths rather than indices: the window already holds the entries, and
      // handing the caller a range meant it had to reproduce this list's own
      // ordering to turn one back into files.
      props.onBand(
        covered.map((index) => props.entries[index]!.path),
        additive,
      );
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
   * The drag-out gesture: pointer down on the name, then 4px of movement.
   *
   * The same 4px of slop the rubber band uses, so a click with a shaky hand is
   * still a click and not a drag of somebody's files. From the moment
   * `onDragOut` is called AppKit owns the pointer, which is why nothing here
   * tracks it afterwards — and why the listeners are removed at that point
   * rather than on the pointer-up that will never arrive.
   */
  const startDragOut = (event: PointerEvent, entry: Entry) => {
    if (event.button !== 0 || !props.onDragOut) return;
    const target = event.target as HTMLElement;
    // Not while renaming: the field inside the name cell owns its own pointer.
    if (!onName(target) || target.closest("input")) return;
    const from = { x: event.clientX, y: event.clientY };
    const move = (at: PointerEvent) => {
      if (Math.abs(at.clientX - from.x) < 4 && Math.abs(at.clientY - from.y) < 4) return;
      stop();
      props.onDragOut?.(entry);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    // `pointercancel` too: the OS can take a pointer away — a system gesture,
    // a window losing focus mid-press — and there is then no `pointerup` ever.
    // The listener that survived it would turn the *next* movement anywhere in
    // the window into a drag of a row nobody is touching.
    window.addEventListener("pointercancel", stop);
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
            onDrag={(delta) => props.onResize(which(), delta, columns())}
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
        : props.layout === "icons"
          ? styles.iconCell
          : "";

  /** How many columns this layout actually has, for the a11y tree. */
  const columnCount = () => {
    if (props.layout === "mono") return 3;
    // One cell says one thing, so one column — the grid's own columns are a
    // wrapping detail, not a table's.
    if (props.layout === "name-led" || props.layout === "icons") return 1;
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
      aria-rowcount={props.entries.length + leadRows()}
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
          {header(
            "size",
            "Size",
            columns().added ? "added" : columns().modified ? "modified" : undefined,
            true,
          )}
          <Show when={columns().added}>
            {header("added", "Added", columns().modified ? "modified" : undefined)}
          </Show>
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
        } ${props.dropTarget?.refused ? styles.rowsRefusing : ""}`}
        role="rowgroup"
        ref={rows}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        onPointerDown={startBand}
        onContextMenu={(event) => {
          if (!props.onContextMenu) return;
          event.preventDefault();
          props.onContextMenu(null, { x: event.clientX, y: event.clientY });
        }}
      >
        {/* What the drop would do, or why it cannot. Sticky rather than fixed,
            so it stays visible while a spring-loaded descent scrolls the list
            under it. */}
        <Show when={props.dropTarget?.message}>
          {(message) => (
            <div
              class={`${styles.dropCaption} ${
                props.dropTarget?.refused ? styles.dropCaptionRefused : ""
              }`}
              role="status"
            >
              {message()}
            </div>
          )}
        </Show>
        {/* One spacer of the full height carries the scrollbar, and the drawn
            rows are offset into it. */}
        <div
          class={styles.virtualSpacer}
          style={{ height: `${contentHeight()}px` }}
        >
          {/* `..`, the way every file browser spells the enclosing folder.
              Drawn here rather than pushed into `entries`, because it is not
              one: nothing selects it, sorts it, counts it, filters it,
              downloads it or deletes it, and a synthetic row in that array
              would have to be excluded from each of those in turn — one
              forgotten exclusion is a download of a folder nobody picked.
              It opens on a double-click like every other row; a single click
              does nothing, exactly as clicking past the last row does
              (E8-S10). */}
          <Show when={props.onAscend !== undefined}>
            <div
              class={`${styles.rowParent} ${
                props.layout === "mono" ? styles.rowParentMono : ""
              }`}
              style={{ height: `${leadHeight()}px` }}
              role="row"
              aria-rowindex={1}
              title="Enclosing folder"
              onDblClick={() => props.onAscend?.()}
            >
              <div
                class={styles.rowParentCell}
                role="gridcell"
                aria-colspan={columnCount()}
                aria-label="Enclosing folder"
              >
                <div class={styles.folderGlyph} />
                <span class={styles.rowParentName}>..</span>
              </div>
            </div>
          </Show>
          <Show when={bandBox()}>
            {(box) => (
              <div
                class={styles.band}
                style={{
                  top: `${Math.min(box().from.y, box().to.y) + leadHeight()}px`,
                  height: `${Math.abs(box().to.y - box().from.y)}px`,
                  // The grid's band is a rectangle; the row layouts leave these
                  // unset and `.band` stretches edge to edge.
                  ...(icons()
                    ? {
                        left: `${Math.min(box().from.x, box().to.x)}px`,
                        width: `${Math.abs(box().to.x - box().from.x)}px`,
                        right: "auto",
                      }
                    : {}),
                }}
              />
            )}
          </Show>
          <div
            class={icons() ? styles.iconGrid : undefined}
            style={{
              transform: `translateY(${windowOffset()}px)`,
              // The cells share out whatever the width is, so a wide window
              // has no ragged margin. `minmax(0, 1fr)` rather than `1fr`
              // because a long name would otherwise widen its own column.
              ...(icons()
                ? { "grid-template-columns": `repeat(${perRow()}, minmax(0, 1fr))` }
                : {}),
            }}
          >
            <For each={window_()}>
              {(entry, index) => {
                const selected = () => props.selected().has(entry.path);
                return (
                  <div
                    class={`${styles.row} ${layoutClass()} ${
                      selected() ? styles.rowSelected : ""
                    } ${props.inFlight?.(entry.path) ? styles.rowInFlight : ""} ${
                      entry.hidden ? styles.rowHidden : ""
                    } ${
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
                    aria-rowindex={range().start + index() + 1 + leadRows()}
                    aria-selected={selected()}
                    onPointerDown={(event) => startDragOut(event, entry)}
                    onClick={(event) =>
                      props.onSelect(entry.path, {
                        shift: event.shiftKey,
                        toggle: event.metaKey || event.ctrlKey,
                      })
                    }
                    onDblClick={() => props.onOpen(entry)}
                    onContextMenu={(event) => {
                      if (!props.onContextMenu) return;
                      event.preventDefault();
                      // Stopped, or the rowgroup below would immediately
                      // replace this menu with the background's one.
                      event.stopPropagation();
                      props.onContextMenu(entry, { x: event.clientX, y: event.clientY });
                    }}
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
                        <Show when={columns().added}>
                          <div class={styles.metaCell} role="gridcell">
                            {formatModified(entry.addedMs)}
                          </div>
                        </Show>
                        <Show when={columns().modified}>
                          <div class={styles.metaCell} role="gridcell">
                            {/* There is no mtime yet — the file has not been
                                written — so the cell says what is happening
                                rather than a dash that means "unknown". */}
                            {props.inFlight?.(entry.path)
                              ? "Transferring…"
                              : formatModified(entry.modifiedMs)}
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
                      {/* The icon grid (E26-S4). A big glyph over a name that
                          wraps to two lines and then ellipsises — the name is
                          the whole of what a cell says, so it gets the room
                          the columns would have taken. */}
                      <Match when={props.layout === "icons"}>
                        <div class={styles.iconCellInner}>
                          {/* The same folder/file glyphs the rows use, scaled
                              up: one vocabulary, so a folder does not look
                              like two different things in two layouts. */}
                          <div
                            class={`${styles.iconGlyph} ${glyphClass(entry.kindCategory) ?? ""}`}
                          />
                          <span class={styles.iconName}>{entry.name}</span>
                        </div>
                      </Match>
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
