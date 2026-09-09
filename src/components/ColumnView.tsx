import { For, Show, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import styles from "./shell.module.css";
import { ROW_HEIGHTS } from "../state/columns";
import { visibleRange } from "../state/virtual";
import * as nav from "../state/navigation";
import { engineFailure } from "../state/errors";
import type { Entry, Listing } from "../state/types";

/**
 * Column view (E26-S4): one pane per level of the path.
 *
 * The one layout that is not the current directory drawn differently. The
 * others render `entries` — this renders the *ancestors* too, each its own
 * listing, which is why it is a component beside `FileList` rather than a
 * fourth arm inside it. A layout switch in `App` picks between them.
 *
 * Each column loads itself, the way `GoToFolder` does, rather than the window
 * fetching a list of listings and holding them: the columns are derived from
 * the path, so the path changing is the only event that matters, and
 * `createResource` already keys a load to exactly that.
 *
 * `prepare` is the window's own filter/hidden/sort rule, handed down. A column
 * that sorted differently from the one beside it, or showed dotfiles the list
 * was hiding, would be describing a different folder.
 */
export function ColumnView(props: {
  /** The directory the window is in; the columns are its ancestors and it. */
  path: string;
  listDir: (path: string) => Promise<Listing>;
  prepare: (entries: Entry[]) => Entry[];
  selected: () => Set<string>;
  onSelect: (path: string, modifiers: { shift?: boolean; toggle?: boolean }) => void;
  /** A folder was chosen: the window moves there and a column appears. */
  onNavigate: (path: string) => void;
  onOpen: (entry: Entry) => void;
}) {
  /**
   * One column per level, ending with the directory the window is in.
   *
   * `["", "Films", "Films/Arrival"]` for `Films/Arrival`. The first is the
   * share root, which is a column like any other.
   */
  const columns = createMemo(() => {
    const paths = [""];
    let accumulated = "";
    for (const segment of nav.segments(props.path)) {
      accumulated = nav.join(accumulated, segment);
      paths.push(accumulated);
    }
    return paths;
  });

  let strip!: HTMLDivElement;
  // Descending should show where you have arrived. Deliberately not on every
  // render: scrolling the strip while the user is looking at an earlier column
  // would take the view away from them.
  const depth = () => columns().length;
  let lastDepth = 0;
  const revealLast = () => {
    if (depth() === lastDepth) return;
    lastDepth = depth();
    queueMicrotask(() => {
      strip.scrollLeft = strip.scrollWidth;
    });
  };

  return (
    <div class={styles.columnStrip} ref={strip} role="group" aria-label="Columns">
      <For each={columns()}>
        {(columnPath, index) => (
          <Column
            path={columnPath}
            listDir={props.listDir}
            prepare={props.prepare}
            // The child on the path is the one that opened the next column, so
            // it stays marked while you look further along.
            leadsTo={columns()[index() + 1] ?? null}
            selected={props.selected}
            onSelect={props.onSelect}
            onNavigate={(path) => {
              props.onNavigate(path);
              revealLast();
            }}
            onOpen={props.onOpen}
          />
        )}
      </For>
    </div>
  );
}

/** One directory, in its own scroller. */
function Column(props: {
  path: string;
  listDir: (path: string) => Promise<Listing>;
  prepare: (entries: Entry[]) => Entry[];
  leadsTo: string | null;
  selected: () => Set<string>;
  onSelect: (path: string, modifiers: { shift?: boolean; toggle?: boolean }) => void;
  onNavigate: (path: string) => void;
  onOpen: (entry: Entry) => void;
}) {
  /**
   * The listing, or why there isn't one.
   *
   * Caught here rather than left to the resource's own error state: a rejected
   * fetcher escapes as an unhandled rejection, and a column that failed has to
   * say so in place — the window's banner is about the *mount*, and one column
   * of a path being unreadable is not that.
   */
  const [listing] = createResource(
    () => props.path,
    async (path): Promise<{ entries: Entry[] } | { problem: string }> => {
      try {
        return await props.listDir(path);
      } catch (error) {
        return { problem: engineFailure(error).message };
      }
    },
  );
  const problem = () => {
    const data = listing();
    return data !== undefined && "problem" in data ? data.problem : null;
  };
  const entries = createMemo(() => {
    const data = listing();
    return data !== undefined && "entries" in data ? props.prepare(data.entries) : [];
  });

  // The same virtual window the row layouts use. A column is narrow, not
  // short: a folder with ten thousand files has ten thousand of them here too.
  const rowHeight = ROW_HEIGHTS.columns;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewport, setViewport] = createSignal(0);
  let rows!: HTMLDivElement;
  const range = createMemo(() =>
    visibleRange(scrollTop(), viewport(), rowHeight, entries().length),
  );

  onMount(() => {
    const measure = () => setViewport(rows.clientHeight);
    measure();
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(rows);
    onCleanup(() => observer?.disconnect());
  });

  return (
    <div
      class={styles.column}
      ref={rows}
      role="listbox"
      aria-label={props.path === "" ? "Share root" : props.path}
      onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
    >
      <Show when={problem()}>
        {/* The engine's own message. A column that simply stayed blank would
            read as an empty folder, which is a different thing. */}
        {(message) => <div class={styles.columnProblem}>{message()}</div>}
      </Show>
      <div style={{ height: `${entries().length * rowHeight}px`, position: "relative" }}>
        <div style={{ transform: `translateY(${range().start * rowHeight}px)` }}>
          <For each={entries().slice(range().start, range().end)}>
            {(entry) => {
              const onPath = () => entry.path === props.leadsTo;
              const chosen = () => props.selected().has(entry.path);
              return (
                <div
                  class={`${styles.columnRow} ${onPath() ? styles.columnRowOnPath : ""} ${
                    chosen() ? styles.rowSelected : ""
                  } ${entry.hidden ? styles.rowHidden : ""}`}
                  data-path={entry.path}
                  role="option"
                  aria-selected={chosen() || onPath()}
                  style={{ height: `${rowHeight}px` }}
                  onClick={(event) => {
                    // A folder opens its column on a single click — that is
                    // the whole gesture of this view — while a file is only
                    // selected, because there is no column past it.
                    if (entry.kind === "dir") props.onNavigate(entry.path);
                    else
                      props.onSelect(entry.path, {
                        shift: event.shiftKey,
                        toggle: event.metaKey || event.ctrlKey,
                      });
                  }}
                  onDblClick={() => props.onOpen(entry)}
                >
                  <div
                    class={
                      entry.kindCategory === "folder" || entry.kindCategory === "alias"
                        ? styles.folderGlyph
                        : styles.fileGlyph
                    }
                  />
                  <span class={styles.columnName}>{entry.name}</span>
                  {/* The disclosure a folder gets, which is what says a click
                      here goes somewhere rather than selecting something. */}
                  <Show when={entry.kind === "dir"}>
                    <span class={styles.columnChevron}>›</span>
                  </Show>
                </div>
              );
            }}
          </For>
        </div>
      </div>
    </div>
  );
}
