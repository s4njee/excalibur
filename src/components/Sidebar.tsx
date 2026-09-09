import { For, createMemo } from "solid-js";
import styles from "./shell.module.css";
import type { Bookmark } from "../state/types";
import { bookmarkSubtitle } from "../state/bookmarks";
import { favoriteGlyph, type Favorite } from "../state/favorites";
import type { UserFolders } from "../state/backend";
import { Glyph } from "./Glyph";
export function Sidebar(props: {
  /** Double-click: mount the bookmark and open its root. */
  onOpen: (id: string) => void;
  /** Hidden entirely rather than narrowed: there is no rail in the design. */
  collapsed?: boolean;
  bookmarks: Bookmark[];
  activeId: string;
  selectedId: string | null;
  onNewConnection: () => void;
  /** The bookmark a drag would drop onto — that share's root (E24-S2). */
  dropBookmark?: string | null;
  onSelect: (id: string) => void;
  onRemove: (id: string) => void;
  onEdit: (id: string) => void;
  /** Folders on this machine, above MOUNTED (E27-S1). */
  favorites: Favorite[];
  /** Compared against each favourite's path to choose its glyph. */
  userFolders: UserFolders;
  /** Single click, unlike a bookmark: see the section's comment below. */
  onOpenFavorite: (path: string) => void;
  onFavoriteMenu: (path: string, at: { x: number; y: number }) => void;
  /** Right-click a share: the route to forgetting one (E3-S1). */
  onBookmarkMenu: (id: string, at: { x: number; y: number }) => void;
  /** Whether a drag is over the section, so the whole of it highlights. */
  dropFavorites?: boolean;
}) {
  const mounted = createMemo(() => props.bookmarks.filter((bookmark) => bookmark.mounted));
  const saved = createMemo(() => props.bookmarks.filter((bookmark) => !bookmark.mounted));

  return (
    // A drop onto a share row copies to that share's root (E24-S2) — the
    // "coming later" E13-S5 left here. `onDragOver` still refuses HTML5 drags
    // without a `preventDefault`: an Excalibur drag is a native one and never
    // reaches the DOM, so letting the webview's own drag through would be a
    // second, differently-behaved path to the same gesture.
    <div
      class={`${styles.sidebar} ${props.collapsed ? styles.sidebarCollapsed : ""}`}
      onDragOver={(event) => {
        // URL pasteboards belong to the window-level download drop target
        // (XDL-E8-S1), even while the pointer is over the sidebar. Native file
        // drags still stay on the existing Tauri path and never become a
        // second HTML upload route.
        const types = [...(event.dataTransfer?.types ?? [])];
        if (types.includes("text/uri-list") || types.includes("text/plain")) return;
        event.stopPropagation();
      }}
    >
      {/* Only the sections scroll. The footer is pinned, so a long list of
          favourites and shares cannot push +/−/Edit off the bottom — and
          without a scroll region here the overflow used to fall out of the
          sidebar and scroll the whole webview instead. */}
      <div class={styles.sidebarScroll}>
        <div class={styles.sectionHeader}>Mounted</div>
        <For each={mounted()}>
          {(bookmark) => {
            const active = () => bookmark.id === props.activeId;
            return (
              <div
                class={`${styles.sidebarRow} ${active() ? styles.sidebarRowActive : ""} ${
                  props.dropBookmark === bookmark.id ? styles.sidebarRowDrop : ""
                }`}
                // Hit-tested by the native drop target, which has a point and no
                // element (E24-S2). Only mounted rows carry it: an unmounted
                // share has no session to copy into.
                data-bookmark={bookmark.id}
                role="button"
                tabindex="0"
                onClick={() => props.onSelect(bookmark.id)}
                onDblClick={() => props.onOpen(bookmark.id)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  props.onBookmarkMenu(bookmark.id, { x: event.clientX, y: event.clientY });
                }}
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
                    // The line truncates, and a folder path is the case where
                    // what gets cut off is the part that identifies it.
                    title={bookmarkSubtitle(bookmark)}
                  >
                    {bookmarkSubtitle(bookmark)}
                  </div>
                </div>
              </div>
            );
          }}
        </For>

        {/* FAVORITES (E27-S1). Folders on this machine, under the mounts
            (asked for directly 2026-09-09: "move mounted above the
            favorites"), and the section a user reaches for without thinking.

            The header is drawn even with no rows: it is the drop target, and a
            section that disappears when the last favourite is removed leaves
            nowhere to put one back.

            A single click opens, where a bookmark needs a double. The two rows
            are doing different things: a bookmark is *selected* first — the
            footer's − and Edit act on the selection — and mounting a share over
            the network is not something to do by brushing past a row. A
            favourite has no selected state and opens a folder on this disk. */}
        <div
          class={`${styles.favorites} ${props.dropFavorites ? styles.sectionDrop : ""}`}
          // Hit-tested by the native drop target, which has a point and no
          // element (E24-S2) — the section, not a row: a drop adds folders to
          // the list rather than landing *in* any one of them.
          data-favorites="true"
        >
          <div class={`${styles.sectionHeader} ${styles.sectionHeaderSpaced}`}>Favorites</div>
          <For each={props.favorites}>
            {(favorite) => (
              <div
                class={styles.sidebarRow}
                role="button"
                tabindex="0"
                // The path is the only thing that tells two `Documents` apart,
                // and it is too long for a 220px row — so it is the tooltip
                // rather than a second line.
                title={favorite.path}
                onClick={() => props.onOpenFavorite(favorite.path)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  props.onFavoriteMenu(favorite.path, { x: event.clientX, y: event.clientY });
                }}
              >
                <Glyph
                  class={styles.sidebarGlyph}
                  kind={favoriteGlyph(favorite.path, props.userFolders)}
                />
                <div class={styles.sidebarText}>
                  <div class={styles.sidebarName}>{favorite.name}</div>
                </div>
              </div>
            )}
          </For>
        </div>

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
              onContextMenu={(event) => {
                event.preventDefault();
                props.onBookmarkMenu(bookmark.id, { x: event.clientX, y: event.clientY });
              }}
            >
              <div class={styles.fileGlyph} />
              <div class={styles.sidebarText}>
                <div class={styles.sidebarName}>{bookmark.name}</div>
                <div class={styles.sidebarSubtitle} title={bookmarkSubtitle(bookmark)}>
                  {bookmark.reachable ? bookmarkSubtitle(bookmark) : "unreachable"}
                </div>
              </div>
            </div>
          )}
        </For>
      </div>

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
