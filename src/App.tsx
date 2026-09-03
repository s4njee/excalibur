import { createMemo, createResource, createSignal, onMount, Show } from "solid-js";
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
import type { Draft } from "./state/connection";
import { FileList, shellStyles as styles, Sidebar, StatusBar, TitleBar, Toolbar } from "./components/Shell";
import { defaultBackend, fixtureBackend, type Backend } from "./state/backend";
import type { Bookmark, Entry, Theme } from "./state/types";

/** Sorted the way the list is: folders first, then by name. */
function order(entries: Entry[], showHidden: boolean): Entry[] {
  return entries
    .filter((entry) => showHidden || !entry.hidden)
    .slice()
    .sort((left, right) => {
      const leftIsDir = left.kind === "dir";
      const rightIsDir = right.kind === "dir";
      if (leftIsDir !== rightIsDir) return leftIsDir ? -1 : 1;
      return left.name.localeCompare(right.name, undefined, { numeric: true });
    });
}

export default function App(props: {
  backend?: Backend;
  theme?: Theme;
  readOnly?: boolean;
  bookmarks?: BookmarkStore;
}) {
  // `readOnly` exists so the design's second screen can be rendered from the
  // same component tree rather than a separate mock.
  const backend =
    props.backend ?? (props.readOnly ? fixtureBackend({ readOnly: true }) : defaultBackend());
  const [listing, { refetch }] = createResource(() => backend.listDir(""));
  const [filter, setFilter] = createSignal("");
  const [showHidden] = createSignal(true);
  const [selected, setSelected] = createSignal<Set<string>>(new Set(["Arrival.2016.mkv"]));
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

  const removeSelected = (id: string) => {
    const current = bookmarks();
    const index = current.findIndex((bookmark) => bookmark.id === id);
    const bookmark = current[index];
    if (!bookmark) return;
    persist(removeBookmark(current, id));
    setSelectedBookmark(null);
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
    const needle = filter().toLowerCase();
    const matching = needle
      ? data.entries.filter((entry) => entry.name.toLowerCase().includes(needle))
      : data.entries;
    return order(matching, showHidden());
  });

  const selectedBytes = createMemo(() =>
    entries()
      .filter((entry) => selected().has(entry.path))
      .reduce((total, entry) => total + entry.size, 0),
  );

  // The two M0 screens describe different mounts, so the chrome that names the
  // mount has to follow them rather than being fixed.
  const host = () => (props.readOnly ? "10.0.0.4" : "nas.local");
  const share = () => (props.readOnly ? "scratch" : "media");
  const scheme = () => (props.readOnly ? "nfs" : "smb");
  const segments = () => (props.readOnly ? ["export", "scratch"] : ["media"]);
  const mountPath = () => (props.readOnly ? "/export/scratch" : "/media");

  const select = (path: string, additive: boolean) => {
    setSelected((current) => {
      const next = additive ? new Set(current) : new Set<string>();
      if (additive && current.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
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
            hasSelection={selected().size > 0}
            filter={filter()}
            onFilter={setFilter}
          />
          <div class={styles.body}>
            <Sidebar
              bookmarks={bookmarks()}
              activeId={bookmarks().find((bookmark) => bookmark.mounted)?.id ?? ""}
              selectedId={selectedBookmark()}
              onNewConnection={() => setConnecting(true)}
              onSelect={setSelectedBookmark}
              onRemove={removeSelected}
              onEdit={(id) => {
                const bookmark = bookmarks().find((candidate) => candidate.id === id);
                if (bookmark) setConnecting(toDraft(bookmark));
              }}
            />
            <div class={styles.listColumn}>
              <FileList entries={entries()} selected={selected} onSelect={select} />
              <StatusBar
                mount={data().mount}
                path={mountPath()}
                items={entries().length}
                selected={selected().size}
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
                  // Re-listing the new mount is E8-S7's navigation; for now the
                  // dialog closes and the mount is held by the session manager.
                  refetch();
                }}
              />
            )}
          </Show>
        </div>
      )}
    </Show>
  );
}
