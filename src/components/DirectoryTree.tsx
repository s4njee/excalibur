import { For, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import styles from "./shell.module.css";
import { MAX_TREE_HEIGHT, resizedBand, type TreeRow } from "../state/tree";
import { visibleRange } from "../state/virtual";

/** The handoff's 20px tree row. Fixed, which is what makes the band virtual. */
const ROW_HEIGHT = 20;

/**
 * A pane's folder tree (`twopane.md` XTP-E2-S4).
 *
 * The first tree in this app: `MoveTo.tsx:18` declined one and column view
 * draws ancestors rather than a hierarchy, so nothing here is a generalisation
 * of something existing.
 *
 * It draws rows and reports gestures; what a row *is* comes from
 * `state/tree.ts` and what has been listed from the pane's `app/tree.ts`. That
 * split is what lets the interesting rules — a folder that is open but still
 * loading, one whose children are all hidden — be stated and tested without
 * a band on screen.
 *
 * The selected row is the folder the pane is *in*, not a selection of its own.
 * A tree with its own idea of where you are is two answers to one question,
 * and the pane's path is the one the rest of the window already reads.
 */
export function DirectoryTree(props: {
  rows: TreeRow[];
  /** The pane's folder, share-relative. The row for it is the selected one. */
  current: string;
  /** Pixels. Never `0` — a collapsed band is not drawn at all. */
  height: number;
  /** Names the tree for a screen reader, e.g. "Left pane folders". */
  label: string;
  /** Make `current` reachable: its ancestors open and the root listed. */
  onReveal: (path: string) => void;
  onNavigate: (path: string) => void;
  onToggle: (path: string) => void;
  /** Mid-drag heights, then the one to remember. `0` means put away. */
  onResize: (height: number) => void;
  onResizeEnd: () => void;
}) {
  // The tree follows the pane, not the other way round: navigating by any
  // means — a row in the list, the breadcrumb, Back — opens the ancestors of
  // wherever the pane landed. Only while the band is drawn, which is why this
  // lives here rather than in the window.
  createEffect(() => props.onReveal(props.current));

  // The same virtual window the file list uses. A band is short, not small: a
  // share root with ten thousand folders has ten thousand rows here, and the
  // rows are a fixed height, so which of them to draw is arithmetic.
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewport, setViewport] = createSignal(0);
  let scroller!: HTMLDivElement;
  const range = createMemo(() =>
    visibleRange(scrollTop(), viewport(), ROW_HEIGHT, props.rows.length),
  );
  onMount(() => {
    const measure = () => setViewport(scroller.clientHeight);
    measure();
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(scroller);
    onCleanup(() => observer?.disconnect());
  });

  const startDrag = (event: PointerEvent) => {
    if (event.button !== 0) return;
    event.preventDefault();
    const startY = event.clientY;
    const startHeight = props.height;
    const move = (at: PointerEvent) =>
      props.onResize(resizedBand(startHeight, at.clientY - startY));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      props.onResizeEnd();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div class={styles.treeBand} style={{ height: `${props.height}px` }}>
      <div
        class={styles.treeRows}
        role="tree"
        aria-label={props.label}
        ref={scroller}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        <div style={{ height: `${props.rows.length * ROW_HEIGHT}px`, position: "relative" }}>
        <div style={{ transform: `translateY(${range().start * ROW_HEIGHT}px)` }}>
        <For each={props.rows.slice(range().start, range().end)}>
          {(row) => {
            const here = () => row.path === props.current;
            return (
              <div
                class={`${styles.treeRow} ${here() ? styles.treeRowCurrent : ""} ${
                  row.hidden ? styles.rowHidden : ""
                }`}
                role="treeitem"
                aria-level={row.depth + 1}
                // Omitted, not `false`, for a folder known to have nothing in
                // it: `aria-expanded="false"` on a leaf announces a disclosure
                // that is not there.
                aria-expanded={row.hasChildren === false ? undefined : row.expanded}
                aria-selected={here()}
                tabindex="0"
                data-tree-path={row.path}
                style={{ "padding-left": `${10 + row.depth * 16}px` }}
                onClick={() => props.onNavigate(row.path)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    props.onNavigate(row.path);
                  } else if (event.key === "ArrowRight" && !row.expanded) {
                    props.onToggle(row.path);
                  } else if (event.key === "ArrowLeft" && row.expanded) {
                    props.onToggle(row.path);
                  }
                }}
              >
                <span
                  class={styles.treeDisclosure}
                  // The disclosure opens the folder; the row goes there. Two
                  // gestures in one row, so the inner one stops the outer.
                  onClick={(event) => {
                    event.stopPropagation();
                    props.onToggle(row.path);
                  }}
                  aria-hidden="true"
                >
                  {row.hasChildren === false ? "" : row.expanded ? "▼" : "▶"}
                </span>
                <span
                  class={`${styles.folderGlyph} ${here() ? styles.treeGlyphCurrent : ""}`}
                />
                <span class={styles.treeName}>{row.name}</span>
              </div>
            );
          }}
        </For>
        </div>
        </div>
      </div>
      {/* The band's bottom edge. Dragging it up far enough puts the tree away,
          which is what makes "collapsible to zero" a gesture and not only a
          menu row. */}
      <div
        class={styles.treeHandle}
        role="separator"
        aria-orientation="horizontal"
        aria-label={`Resize ${props.label}`}
        aria-valuenow={props.height}
        aria-valuemin={0}
        aria-valuemax={MAX_TREE_HEIGHT}
        onPointerDown={startDrag}
      />
    </div>
  );
}
