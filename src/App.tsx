import { createEffect, createMemo, createSignal, onCleanup, onMount, Show } from "solid-js";
import { ConnectionDialog } from "./components/ConnectionDialog";
import {
  defaultBookmarkStore,
  fromDraft,
  probePort,
  remove as removeBookmark,
  restore,
  toDraft,
  upsert,
  type BookmarkStore,
} from "./state/bookmarks";
import { toSpec, type Draft } from "./state/connection";
import { FileList, shellStyles as styles, Sidebar, StatusBar, TitleBar, Toolbar } from "./components/Shell";
import { defaultBackend, fixtureBackend, type Backend } from "./state/backend";
import type { Bookmark, Entry, Theme } from "./state/types";
import { createListing } from "./state/listing";
import * as nav from "./state/navigation";
import { DEFAULT_SORT, sortEntries, toggle, type Sort } from "./state/sort";
import { filterEntries } from "./state/filter";
import { EMPTY_SELECTION, band, click as clickSelect, typeAhead } from "./state/selection";
import { DEFAULT_WIDTHS, resize, sanitise, type Divider, type Widths } from "./state/columns";
import { defaultPrefs, type Prefs } from "./state/prefs";

export default function App(props: {
  backend?: Backend;
  theme?: Theme;
  readOnly?: boolean;
  bookmarks?: BookmarkStore;
  prefs?: Prefs;
}) {
  // `readOnly` exists so the design's second screen can be rendered from the
  // same component tree rather than a separate mock.
  const backend =
    props.backend ?? (props.readOnly ? fixtureBackend({ readOnly: true }) : defaultBackend());
  // Where the window is looking. The listing follows this rather than the
  // other way round, so every way of navigating — a double-click, the
  // breadcrumb, Back, the keyboard — goes through one place.
  const [history, setHistory] = createSignal<nav.History>(nav.initial());
  const path = () => nav.current(history());
  const listingState = createListing((target) => backend.listDir(target));
  const listing = listingState.listing;

  createEffect(() => listingState.load(path()));

  const goTo = (target: string) => setHistory((current) => nav.go(current, target));
  const ascend = () => goTo(nav.parent(path()));

  const [filter, setFilter] = createSignal("");
  const prefs = props.prefs ?? defaultPrefs();
  const [sort, setSort] = createSignal<Sort>(DEFAULT_SORT);
  const [widths, setWidths] = createSignal<Widths>(DEFAULT_WIDTHS);

  onMount(async () => {
    // Per layout, because `1g` and `1h` have no columns to size. Only `1f`
    // exists today; the key is shaped for the other two rather than needing a
    // migration when they land.
    setWidths(sanitise(await prefs.get("columnWidths.columns")));
    const saved = await prefs.get("sort");
    if (saved && typeof saved === "object") {
      const { column, ascending } = saved as Partial<Sort>;
      if (typeof column === "string" && typeof ascending === "boolean") {
        setSort({ column, ascending } as Sort);
      }
    }
  });

  const onSort = (column: import("./state/types").SortColumn) => {
    const next = toggle(sort(), column);
    setSort(next);
    void prefs.set("sort", next);
  };

  // The drag itself only moves the signal. Writing on every pointermove would
  // be a file write per frame, so it is persisted once when the drag ends.
  const onResize = (divider: Divider, delta: number) =>
    setWidths((current) => resize(current, divider, delta));
  const onResizeEnd = () => void prefs.set("columnWidths.columns", widths());

  const [showHidden] = createSignal(true);
  // A preference, default on, per E8-S6. No UI yet; E17 gives it one.
  const [foldersFirst] = createSignal(true);
  const [selection, setSelection] = createSignal(clickSelect(EMPTY_SELECTION, [], "Arrival.2016.mkv"));
  // The row type-to-select jumped to, so the list can scroll it into view.
  const [reveal, setReveal] = createSignal<number | null>(null);
  const selected = () => selection().paths as Set<string>;
  const [connecting, setConnecting] = createSignal<Draft | true | null>(null);
  const store = props.bookmarks ?? defaultBookmarkStore();
  const [bookmarks, setBookmarks] = createSignal<Bookmark[]>([]);
  const [selectedBookmark, setSelectedBookmark] = createSignal<string | null>(null);
  const [undo, setUndo] = createSignal<{ bookmark: Bookmark; index: number } | null>(null);

  const persist = (next: Bookmark[]) => {
    setBookmarks(next);
    void store.save(next);
  };

  onMount(async () => {
    const saved = await store.load();
    setBookmarks(saved);
    // Probe once on start. Every 60s while idle is E3-S2's other half and wants
    // a visible-window check first, so it does not wake a sleeping laptop.
    // A bookmark the design shows as unreachable stays that way in the preview,
    // where there is no engine to ask.
    for (const bookmark of saved.filter((candidate) => candidate.reachable)) {
      void backend
        .reachable(bookmark.host, probePort(bookmark))
        .then((reachable) =>
          setBookmarks((current) =>
            current.map((candidate) =>
              candidate.id === bookmark.id ? { ...candidate, reachable } : candidate,
            ),
          ),
        )
        .catch(() => undefined);
    }
  });

  // A selection is a set of paths in the directory that was on screen. Carrying
  // it across a navigation would leave rows selected that are not there, and
  // would let a verb act on a file the user can no longer see.
  createEffect((previous: string | undefined) => {
    const here = path();
    if (previous !== undefined && previous !== here) setSelection(EMPTY_SELECTION);
    return here;
  });

  /**
   * Descend into the one selected folder, for Cmd/Ctrl+Down.
   *
   * Only acts on a single selection: descending "into" three folders has no
   * meaning, and picking one of them arbitrarily would be a guess.
   */
  const descend = () => {
    const chosen = entries().filter((entry) => selected().has(entry.path));
    if (chosen.length === 1 && chosen[0]?.kind === "dir") goTo(chosen[0].path);
  };

  // Type-to-select accumulates keystrokes for 700ms. The buffer is a plain
  // variable rather than a signal: nothing renders from it, and making it
  // reactive would re-run the list for every keypress.
  let typed = "";
  let typedAt = 0;
  // Where the selection was when this burst began. Extending the buffer
  // searches from here so `b` then `l` refines to the first `bl` rather than
  // stepping past the row `b` just landed on.
  let typedOrigin: string | null = null;
  const TYPING_WINDOW_MS = 700;

  const jumpTo = (character: string) => {
    const now = Date.now();
    if (now - typedAt > TYPING_WINDOW_MS) {
      typed = character;
      typedOrigin = selection().anchor;
    } else {
      typed += character;
    }
    typedAt = now;
    const names = entries().map((entry) => entry.name);
    const found = typeAhead(order(), names, typed, {
      current: selection().anchor,
      origin: typedOrigin,
    });
    if (found === null) return;
    setSelection((current) => clickSelect(current, order(), found));
    setReveal(order().indexOf(found));
  };

  onMount(() => {
    const onKey = (event: KeyboardEvent) => {
      // Anything typed into the filter box or a dialog field belongs to that
      // field, not to the list.
      const target = event.target as HTMLElement | null;
      if (target && (target.tagName === "INPUT" || target.isContentEditable)) {
        // Escape is the exception: it is how the filter gets cleared, and the
        // field is exactly where the user will be when they want that.
        if (event.key !== "Escape") return;
      }

      if (event.key === "Escape") {
        // The design's order: the filter first, then the selection. A filter
        // is the more disruptive of the two states to be stuck in, and
        // clearing both at once would make one Escape do two things.
        if (filter() !== "") setFilter("");
        else setSelection(EMPTY_SELECTION);
        return;
      }

      if (event.metaKey || event.ctrlKey) {
        if (event.key === "ArrowDown") {
          event.preventDefault();
          descend();
        } else if (event.key === "ArrowUp") {
          event.preventDefault();
          if (nav.canAscend(path())) ascend();
        }
        return;
      }

      if (event.altKey) return;
      // A single printable character is type-to-select. `key.length === 1`
      // is what separates those from `ArrowDown`, `Enter` and the rest.
      if (event.key.length === 1 && event.key !== " ") {
        jumpTo(event.key);
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  /**
   * Mount a saved bookmark and open its root.
   *
   * The sidebar row is the only place a share gets connected without going
   * through the dialog, so a failure has nowhere inline to land: it goes to the
   * bookmark's editor, pre-filled, which is where it can actually be fixed.
   */
  const mountBookmark = async (id: string) => {
    const bookmark = bookmarks().find((candidate) => candidate.id === id);
    if (!bookmark) return;
    try {
      await backend.mount(toSpec(toDraft(bookmark), bookmark.id));
      persist(
        bookmarks().map((candidate) =>
          candidate.id === id
            ? { ...candidate, mounted: true }
            : { ...candidate, mounted: false },
        ),
      );
      setHistory(nav.initial());
      listingState.load("");
    } catch {
      setConnecting(toDraft(bookmark));
    }
  };

  const removeSelected = (id: string) => {
    const current = bookmarks();
    const index = current.findIndex((bookmark) => bookmark.id === id);
    const bookmark = current[index];
    if (!bookmark) return;
    persist(removeBookmark(current, id));
    setSelectedBookmark(null);
    // A forgotten bookmark must not leave its password behind in the keychain.
    // Done before the undo window closes rather than after, because a
    // credential outliving the thing that named it is the worse failure; undo
    // restores the bookmark and asks for the password again.
    void backend.forgetSecret(id).catch(() => undefined);
    // A one-step undo rather than a confirmation: the design does not specify
    // one, and asking before every removal is more intrusive than letting the
    // rare mistake be taken back.
    setUndo({ bookmark, index });
    window.setTimeout(() => {
      setUndo((pending) => (pending?.bookmark.id === bookmark.id ? null : pending));
    }, 8000);
  };

  const entries = createMemo(() => {
    const data = listing();
    if (!data) return [];
    const matching = filterEntries(data.entries, filter());
    const visible = showHidden() ? matching : matching.filter((entry) => !entry.hidden);
    return sortEntries(visible, sort(), foldersFirst());
  });

  /**
   * How many rows there would be with no filter.
   *
   * Counted with the same `showHidden` rule as the visible set, so the two
   * numbers describe one population — "3 of 13" must not compare a filtered
   * count against a total that includes dotfiles the list is not showing.
   */
  const totalEntries = createMemo(() => {
    const data = listing();
    if (!data) return 0;
    return showHidden() ? data.entries.length : data.entries.filter((e) => !e.hidden).length;
  });

  /**
   * The selection as it is on screen.
   *
   * A row that a filter has hidden is not part of what the status bar
   * describes or what a toolbar verb acts on: "1 selected" beside a list that
   * visibly contains no selected row is a claim the user cannot check, and
   * Download would then act on a file they cannot see. The underlying set is
   * left alone, so clearing the filter brings the selection back.
   */
  const selectedVisible = createMemo(() =>
    entries().filter((entry) => selected().has(entry.path)),
  );

  const selectedBytes = createMemo(() =>
    selectedVisible().reduce((total, entry) => total + entry.size, 0),
  );

  // The two M0 screens describe different mounts, so the chrome that names the
  // mount has to follow them rather than being fixed. The share root's own name
  // is still fixed here — it comes from the active mount, which is E10's job —
  // but everything below it is now the real path.
  const host = () => (props.readOnly ? "10.0.0.4" : "nas.local");
  const share = () => (props.readOnly ? "scratch" : "media");
  const scheme = () => (props.readOnly ? "nfs" : "smb");
  const rootSegments = () => (props.readOnly ? ["export", "scratch"] : ["media"]);
  const segments = () => [...rootSegments(), ...nav.segments(path())];
  const mountPath = () => ["", ...segments()].join("/");

  /**
   * A breadcrumb click, by depth across the whole path field.
   *
   * The first `rootSegments().length` crumbs name the mount itself, which is
   * not somewhere this window can navigate above — clicking them goes to the
   * share root rather than doing nothing, which is what they visually promise.
   */
  const onCrumb = (depth: number) => {
    const within = depth - rootSegments().length;
    goTo(within <= 0 ? "" : nav.ancestor(path(), within));
  };

  /** Descend into a folder; anything else is not a navigation. */
  const open = (entry: Entry) => {
    if (entry.kind === "dir") goTo(entry.path);
  };

  /** The visible paths, in display order — what a range or a band spans. */
  const order = createMemo(() => entries().map((entry) => entry.path));

  const select = (path: string, modifiers: { shift?: boolean; toggle?: boolean }) => {
    setSelection((current) => clickSelect(current, order(), path, modifiers));
    setReveal(null);
  };

  const selectBand = (start: number, end: number, additive: boolean) => {
    setSelection((current) => band(current, order().slice(start, end), additive));
  };

  return (
    <Show when={listing()} fallback={<div class={styles.window} />}>
      {(data) => (
        <div class={styles.window}>
          <TitleBar share={share()} host={host()} />
          <Toolbar
            mount={data().mount}
            scheme={scheme()}
            host={host()}
            segments={segments()}
            hasSelection={selectedVisible().length > 0}
            filter={filter()}
            onFilter={setFilter}
            canBack={nav.canBack(history())}
            canForward={nav.canForward(history())}
            canAscend={nav.canAscend(path())}
            onBack={() => setHistory(nav.back)}
            onForward={() => setHistory(nav.forward)}
            onAscend={ascend}
            onCrumb={onCrumb}
          />
          <div class={styles.body}>
            <Sidebar
              bookmarks={bookmarks()}
              activeId={bookmarks().find((bookmark) => bookmark.mounted)?.id ?? ""}
              selectedId={selectedBookmark()}
              onNewConnection={() => setConnecting(true)}
              onSelect={setSelectedBookmark}
              onOpen={(id) => void mountBookmark(id)}
              onRemove={removeSelected}
              onEdit={(id) => {
                const bookmark = bookmarks().find((candidate) => candidate.id === id);
                if (bookmark) setConnecting(toDraft(bookmark));
              }}
            />
            <div class={styles.listColumn}>
              <FileList
                entries={entries()}
                selected={selected}
                onSelect={select}
                onBand={selectBand}
                reveal={reveal()}
                onOpen={open}
                slow={listingState.slow()}
                sort={sort()}
                onSort={onSort}
                widths={widths()}
                onResize={onResize}
                onResizeEnd={onResizeEnd}
              />
              <StatusBar
                mount={data().mount}
                path={mountPath()}
                items={entries().length}
                total={totalEntries()}
                selected={selectedVisible().length}
                selectedBytes={selectedBytes()}
              />
            </div>
          </div>
          <Show when={undo()}>
            {(pending) => (
              <div class={styles.toast}>
                <span>Removed “{pending().bookmark.name}”</span>
                <button
                  class={styles.toastAction}
                  onClick={() => {
                    persist(restore(bookmarks(), pending().bookmark, pending().index));
                    setUndo(null);
                  }}
                >
                  Undo
                </button>
              </div>
            )}
          </Show>
          <Show when={connecting()}>
            {(pending) => (
              <ConnectionDialog
                backend={backend}
                initial={pending() === true ? undefined : (pending() as Draft)}
                onCancel={() => setConnecting(null)}
                onConnected={(draft) => {
                  setConnecting(null);
                  // The bookmark is saved on a successful connect, not on
                  // opening the dialog: a connection that never worked is not
                  // worth remembering.
                  persist(upsert(bookmarks(), { ...fromDraft(draft), mounted: true }));
                  // A new mount is a new root, so the history starts over
                  // rather than keeping paths that belonged to the old share.
                  setHistory(nav.initial());
                  listingState.load("");
                }}
              />
            )}
          </Show>
        </div>
      )}
    </Show>
  );
}
