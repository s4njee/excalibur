import { createEffect, createMemo, createSignal, onCleanup, onMount, Show } from "solid-js";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { QuickLook } from "./components/QuickLook";
import { Transfers } from "./components/Transfers";
import {
  defaultBookmarkStore,
  fromDraft,
  probePort,
  remove as removeBookmark,
  restore,
  toDraft,
  upsert,
  withMounted,
  type BookmarkStore,
} from "./state/bookmarks";
import { toSpec, uriScheme, type Draft } from "./state/connection";
import { thisMachine } from "./state/platform";
import {
  Banner,
  EmptyState,
  FileList,
  shellStyles as styles,
  Sidebar,
  StatusBar,
  TitleBar,
  Toolbar,
} from "./components/Shell";
import { defaultBackend, fixtureBackend, writeGuarded, type Backend } from "./state/backend";
import type {
  Bookmark,
  Conflict,
  EditSession,
  Entry,
  MountEvent,
  MountInfo,
  Theme,
  Transfer,
} from "./state/types";
import { createListing } from "./state/listing";
import * as nav from "./state/navigation";
import { sortEntries, toggle } from "./state/sort";
import { filterEntries } from "./state/filter";
import {
  NO_SHARE_REASON,
  acceptsDrop,
  gate,
  type Affordance,
  type Context as PermissionContext,
} from "./state/permissions";
import { engineFailure } from "./state/errors";
import { EMPTY_SELECTION, band, click as clickSelect, typeAhead } from "./state/selection";
import { ContextMenu } from "./components/ContextMenu";
import { Properties } from "./components/Properties";
import { MoveTo } from "./components/MoveTo";
import { destinationOf } from "./state/move";
import { menuFor, type MenuAction, type MenuItem } from "./state/menu";
import { editable } from "./state/preview";
import { DEFAULT_WIDTHS, resize, type Divider, type Widths } from "./state/columns";
import { defaultPrefs, inTauri, type Prefs } from "./state/prefs";
import * as shortcuts from "./state/shortcuts";
import { coalesce } from "./state/refresh";
import { Preferences } from "./components/Preferences";
import {
  DEFAULTS as DEFAULT_SETTINGS,
  loadSettings,
  resolveTheme,
  saveSetting,
  type Settings,
} from "./state/settings";

export default function App(props: {
  backend?: Backend;
  theme?: Theme;
  readOnly?: boolean;
  bookmarks?: BookmarkStore;
  prefs?: Prefs;
}) {
  // `readOnly` exists so the design's second screen can be rendered from the
  // same component tree rather than a separate mock.
  const raw =
    props.backend ?? (props.readOnly ? fixtureBackend({ readOnly: true }) : defaultBackend());
  // Wrapped once, here, so every write below crosses the guard. E9-S1 asks
  // that no write command *reaches* the engine on a read-only mount, and
  // disabling the buttons is not that — a shortcut or a later caller can still
  // reach the method.
  const backend = writeGuarded(raw, () => listing()?.mount.writable ?? false);
  // Where the window is looking. The listing follows this rather than the
  // other way round, so every way of navigating — a double-click, the
  // breadcrumb, Back, the keyboard — goes through one place.
  const [history, setHistory] = createSignal<nav.History>(nav.initial());
  const path = () => nav.current(history());
  const listingState = createListing((target) => backend.listDir(target));
  const listing = listingState.listing;

  createEffect(() => listingState.load(path()));

  /**
   * Relist the current directory, at most once per 250 ms (E8-S9).
   *
   * Several things ask for this at once — a mutation finished, a transfer
   * landed, a reconnect republished the mount. Deleting twenty files gives
   * twenty reasons to relist and one result worth showing.
   *
   * Navigation deliberately does not go through here: that is a *different*
   * directory, and delaying it would leave the previous one's rows on screen
   * under the new path.
   */
  const refresh = coalesce(() => listingState.load(path()));
  onCleanup(() => refresh.dispose());

  /**
   * Nothing is mounted: the engine refused the listing rather than returning
   * a directory (E1-S6).
   *
   * Deliberately not "there is no listing yet", which is also true for the
   * frame before the first request settles — an empty state that flashed
   * before every window would be worse than none. `error` is set only once a
   * request has come back, and a *failed navigation* on a live mount leaves
   * the previous listing in place, so it does not land here either.
   */
  const disconnected = () => listing() === null && listingState.error() !== null;

  const goTo = (target: string) => setHistory((current) => nav.go(current, target));
  const ascend = () => goTo(nav.parent(path()));

  const [filter, setFilter] = createSignal("");
  const prefs = props.prefs ?? defaultPrefs();

  // Everything persisted lives here (E17-S1), except the column widths, which
  // move on every pointer frame during a drag and would make this signal — and
  // so the whole window — react at that rate.
  const [settings, setSettings] = createSignal<Settings>(DEFAULT_SETTINGS);
  const [widths, setWidths] = createSignal<Widths>(DEFAULT_WIDTHS);

  /** Change one preference and remember it. */
  const update = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSettings((current) => ({ ...current, [key]: value }));
    void saveSetting(prefs, key, value);
  };

  const [showPreferences, setShowPreferences] = createSignal(false);
  // Held so ⌘F can put the caret in the filter box, which is a DOM action
  // rather than a state change.
  let filterInput: HTMLInputElement | undefined;

  /**
   * Change a preference, and carry it wherever it has to go.
   *
   * Most settings are read straight off the signal by whoever needs them. The
   * transfer limit is the exception: the worker lives in the engine and holds
   * its own semaphore, so it has to be told.
   */
  const changeSetting = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    update(key, value);
    if (key === "transferConcurrency") {
      void backend.setTransferConcurrency(value as number);
    }
  };

  const pickDownloadFolder = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === "string") changeSetting("downloadFolder", picked);
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  /**
   * The Local tab's `Choose…`, which returns a path rather than applying one.
   *
   * The dialog owns the draft, so the picker hands the folder back instead of
   * writing it; `null` is a cancelled picker, which must leave the field as
   * the user left it.
   */
  const pickShareFolder = async (): Promise<string | null> => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false });
    return typeof picked === "string" ? picked : null;
  };

  const sort = () => settings().sort;
  const showHidden = () => settings().showHidden;
  const foldersFirst = () => settings().foldersFirst;

  onMount(async () => {
    const loaded = await loadSettings(prefs);
    setSettings(loaded);
    setWidths(loaded.columnWidths);
    // The engine holds the transfer worker, so its concurrency has to be told
    // rather than read.
    void backend.setTransferConcurrency(loaded.transferConcurrency);
  });

  // The OS half of `theme: "system"`. Followed live rather than read once, so
  // a Mac flipping to dark at sunset takes the window with it.
  const [prefersDark, setPrefersDark] = createSignal(
    typeof window !== "undefined" && window.matchMedia
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
      : false,
  );
  onMount(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (event: MediaQueryListEvent) => setPrefersDark(event.matches);
    query.addEventListener("change", onChange);
    onCleanup(() => query.removeEventListener("change", onChange));
  });

  createEffect(() => {
    // `props.theme` is the design harness pinning a screen; a real window
    // follows the preference.
    const painted = props.theme ?? resolveTheme(settings().theme, prefersDark());
    document.documentElement.dataset.theme = painted;
  });

  const onSort = (column: import("./state/types").SortColumn) => {
    update("sort", toggle(sort(), column));
  };

  // The drag itself only moves the signal. Writing on every pointermove would
  // be a file write per frame, so it is persisted once when the drag ends.
  const onResize = (divider: Divider, delta: number) =>
    setWidths((current) => resize(current, divider, delta));
  const onResizeEnd = () => void saveSetting(prefs, "columnWidths", widths());
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
    // Nothing is mounted when the app starts: sessions are not persisted, so
    // a `mounted` flag read back from the store is from a previous run and
    // describes a share the shell is not holding. Found 2026-09-05 by
    // relaunching with a saved xsync bookmark: the sidebar showed it under
    // MOUNTED and the window titled itself after it, over no listing at all.
    // Double-clicking the row under BOOKMARKS is what mounts it.
    const saved = withMounted(await store.load(), new Set<string>());
    setBookmarks(saved);
    // Probe once on start. Every 60s while idle is E3-S2's other half and wants
    // a visible-window check first, so it does not wake a sleeping laptop.
    // A bookmark the design shows as unreachable stays that way in the preview,
    // where there is no engine to ask. In the app every bookmark is probed,
    // whatever the store says: `reachable` is persisted with the rest of the
    // row, so filtering on it meant one failed probe dimmed a bookmark for
    // good — nothing would ever ask again.
    const toProbe = inTauri() ? saved : saved.filter((candidate) => candidate.reachable);
    for (const bookmark of toProbe) {
      const port = probePort(bookmark);
      // A folder on this machine is never probed and never dimmed:
      // reachability is not a question a directory answers, and a TCP connect
      // to an empty host would fail and dim a row that is perfectly fine.
      if (port === null) continue;
      void backend
        .reachable(bookmark.host, port)
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
    if (previous !== undefined && previous !== here) {
      setSelection(EMPTY_SELECTION);
      setRenaming(null);
      flushDelete();
    }
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
      // A menu owns the keyboard while it is open. It stops its own keys from
      // reaching here, but a key pressed while focus is elsewhere would
      // otherwise reach the list *through* the menu — Delete deleting the
      // selection behind an open menu that also offers Delete (E14-S2).
      if (contextMenu()) return;
      // Anything typed into the filter box or a dialog field belongs to that
      // field, not to the list.
      const target = event.target as HTMLElement | null;
      const typing = Boolean(target && (target.tagName === "INPUT" || target.isContentEditable));
      if (typing) {
        // A rename field is an input, and Escape there cancels the rename
        // rather than the filter.
        if (event.key === "Escape" && renaming() !== null) {
          setRenaming(null);
          return;
        }
        // Escape is the exception: it is how the filter gets cleared, and the
        // field is exactly where the user will be when they want that.
        if (event.key !== "Escape") return;
      }

      // One table decides what a keystroke means (E16-S1); everything below
      // decides whether it is allowed to happen now.
      const verb = shortcuts.match(event);
      switch (verb) {
        case "escape":
          // A layered key, in the design's order: the panel is the most
          // recently opened thing, then the filter, then the selection. A
          // filter is the more disruptive state to be stuck in, and clearing
          // both at once would make one Escape do two things.
          if (quickLook()) setQuickLook(false);
          else if (filter() !== "") setFilter("");
          else setSelection(EMPTY_SELECTION);
          return;

        case "contextMenu": {
          // The keyboard's way to the same menu. It opens on the selection —
          // there is no pointer to have been over a row — anchored to the
          // first selected row so the menu appears beside what it acts on
          // rather than in a corner.
          event.preventDefault();
          const chosen = selectedVisible();
          const first = chosen[0];
          const row = first
            ? document.querySelector(`[data-path="${CSS.escape(first.path)}"]`)
            : null;
          const at = row
            ? (() => {
                const rect = row.getBoundingClientRect();
                return { x: rect.left + 24, y: rect.bottom };
              })()
            : { x: 120, y: 120 };
          openContextMenu(first ?? null, at);
          return;
        }

        case "quickLook":
          if (renaming() !== null) return;
          event.preventDefault();
          toggleQuickLook();
          return;

        case "rename": {
          if (renaming() !== null) return;
          // Enter renames the one selected row. On several it has no meaning,
          // and picking one of them would be a guess.
          const chosen = selectedVisible();
          if (chosen.length === 1 && chosen[0]) {
            event.preventDefault();
            if (!refusedBy("rename")) setRenaming(chosen[0].path);
          }
          return;
        }

        case "delete":
          if (typing) return;
          event.preventDefault();
          deleteSelected();
          return;

        case "descend":
          event.preventDefault();
          descend();
          return;

        case "ascend":
          event.preventDefault();
          if (nav.canAscend(path())) ascend();
          return;

        case "back":
          event.preventDefault();
          if (nav.canBack(history())) setHistory(nav.back);
          return;

        case "forward":
          event.preventDefault();
          if (nav.canForward(history())) setHistory(nav.forward);
          return;

        case "refresh":
          event.preventDefault();
          // Straight through, not coalesced: the user asked for it now, and a
          // quarter-second pause would read as the key not having worked.
          listingState.load(path());
          return;

        case "focusFilter":
          event.preventDefault();
          // Selects what is there, so a second ⌘F retypes rather than appends.
          filterInput?.select();
          filterInput?.focus();
          return;

        case "download":
          event.preventDefault();
          void downloadSelected();
          return;

        case "upload":
          event.preventDefault();
          void uploadHere();
          return;

        case "edit":
          event.preventDefault();
          void editSelected();
          return;

        case "newFolder":
          event.preventDefault();
          void newFolder();
          return;

        case "preferences":
          event.preventDefault();
          setShowPreferences(true);
          return;

        case "transfers":
          event.preventDefault();
          setShowTransfers(true);
          return;

        case "layoutColumns":
        case "layoutNameLed":
        case "layoutMono":
          event.preventDefault();
          update(
            "rowLayout",
            verb === "layoutColumns" ? "columns" : verb === "layoutNameLed" ? "name-led" : "mono",
          );
          return;

        default:
          break;
      }

      if (typing || event.altKey || event.metaKey || event.ctrlKey) return;
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

  const [remounting, setRemounting] = createSignal(false);
  /**
   * A connection-level failure, shown in the banner under the toolbar.
   *
   * Only `Surface::Banner` reaches here. The engine classifies its own errors,
   * so a per-item failure goes to the transfer queue and a validation failure
   * to its field, without this deciding which is which.
   */
  const [problem, setProblem] = createSignal<string | null>(null);
  /** Something worth saying that is not a failure. Shown as a toast. */
  const [notice, setNotice] = createSignal<string | null>(null);
  // The last thing the engine said about the active mount. Held separately
  // from the listing because a mount that has *dropped* has no listing to
  // carry its state — that is exactly when the health dot has to turn red.
  const [mountEvent, setMountEvent] = createSignal<MountEvent | null>(null);
  const [transfers, setTransfers] = createSignal<Transfer[]>([]);
  const [showTransfers, setShowTransfers] = createSignal(false);
  // The preview route seeds a queue; if there is one, show it.
  onMount(() => void backend.transfers().then((all) => all.length > 0 && setShowTransfers(true)));
  /** Destinations already taken, awaiting Replace / Keep both / Skip. */
  const [conflicts, setConflicts] = createSignal<{
    items: Conflict[];
    upload: boolean;
    sources: string[];
    destination: string;
  } | null>(null);
  /** A drag from the OS is over the list, and where it would land. */
  const [dropTarget, setDropTarget] = createSignal<{ folder: string | null } | null>(null);
  /** A write-back that the remote refused, awaiting Replace / Cancel. */
  const [saveConflict, setSaveConflict] = createSignal<EditSession | null>(null);
  /** Whether Quick Look is open. It follows the selection while it is. */
  const [quickLook, setQuickLook] = createSignal(false);
  /**
   * Open or close the panel, on exactly one selected row.
   *
   * One function rather than one per entry point (E3-S5): the toolbar button
   * and the Space key are the same verb, and the toolbar's was inert because
   * the rule lived in the keyboard handler where a button could not reach it.
   */
  const toggleQuickLook = () =>
    setQuickLook((open) => !open && selectedVisible().length === 1);
  /**
   * The panel outlives `quickLook` by the length of its exit animation
   * (E1-S5).
   *
   * Solid unmounts on the frame the signal flips, so without this only the
   * opening would ever be seen — the close would be a disappearance.
   */
  const [quickLookClosing, setQuickLookClosing] = createSignal(false);
  const [quickLookMounted, setQuickLookMounted] = createSignal(false);
  createEffect(() => {
    if (quickLook()) {
      setQuickLookClosing(false);
      setQuickLookMounted(true);
      return;
    }
    if (!quickLookMounted()) return;
    setQuickLookClosing(true);
    // Matches `--transition-panel`. A timer rather than `animationend`
    // because a panel that never animates — reduced motion — would never
    // fire it, and would then never unmount.
    const timer = setTimeout(() => {
      setQuickLookMounted(false);
      setQuickLookClosing(false);
    }, 140);
    onCleanup(() => clearTimeout(timer));
  });
  /** The row being renamed inline, by path. */
  const [renaming, setRenaming] = createSignal<string | null>(null);
  /** A delete waiting out its undo window, and the timer that will run it. */
  const [pendingDelete, setPendingDelete] = createSignal<{
    paths: string[];
    timer: number;
  } | null>(null);
  /** More than the undo toast is willing to take back without asking. */
  const [confirmDelete, setConfirmDelete] = createSignal<string[] | null>(null);

  /** The mount the window is looking at, when there is a real one. */
  const activeMountId = () => bookmarks().find((bookmark) => bookmark.mounted)?.id ?? null;
  const activeBookmark = () => bookmarks().find((bookmark) => bookmark.mounted) ?? null;

  onMount(() => {
    const stop = backend.onMountChanged((event) => {
      if (event.id !== activeMountId()) return;
      setMountEvent(event);
      // A mount that came back is the one moment `writable` and capacity must
      // be re-read rather than trusted — a reconnected share can be read-only
      // now. Re-listing rather than taking them from the event keeps the badge
      // and the rows sourced from one place.
      if (event.status === "ready") {
        // Back, so the banner goes and the facts are re-read together.
        setProblem(null);
        refresh.request();
      } else if (event.status === "reconnecting" || event.status === "error") {
        setProblem(
          event.reason ?? "The connection to this share dropped. Reconnecting…",
        );
        // A rejected credential is not something Reconnect can fix, and the
        // engine has already stopped retrying rather than lock the account
        // (E15-S2). The only useful next step is the editor, pre-filled, so
        // the password can be corrected — and unticking "Save in keychain"
        // there is how a wrong stored one is forgotten.
        if (event.status === "error" && /authentication/i.test(event.reason ?? "")) {
          const bookmark = bookmarks().find((candidate) => candidate.id === event.id);
          if (bookmark) setConnecting(toDraft(bookmark));
        }
      }
    });
    onCleanup(stop);
  });

  /**
   * The mount as the status bar should describe it.
   *
   * The listing is the authority on `writable` and capacity, because those
   * travel with it. The *status* comes from the event stream, which is the only
   * thing that knows about a mount that has stopped answering.
   */
  const mountFacts = (): MountInfo | undefined => {
    const current = listing()?.mount;
    if (!current) return undefined;
    const event = mountEvent();
    // A banner and a green health dot at the same time would be the window
    // disagreeing with itself, so the dot follows whichever is worse.
    const status = event?.status ?? (problem() === null ? current.status : "error");
    return { ...current, status };
  };

  /**
   * The read-only recovery path.
   *
   * A refused upgrade is not an error: the engine resolves with the mount facts
   * that came back, and the status bar re-reads the reason from them. Treating
   * it as a failure is how this button becomes a loop.
   */
  const remountReadWrite = async (id: string) => {
    setRemounting(true);
    try {
      await backend.remountReadWrite(id);
      // Re-list rather than patching the mount facts by hand: `writable`
      // travels with the listing, and taking it from anywhere else is how the
      // badge and the rows come to disagree.
      refresh.request();
    } catch (error) {
      // A connection that failed outright, as opposed to an upgrade the server
      // declined. E15-S1's banner is where this belongs; until then it is at
      // least visible rather than swallowed.
      setProblem(engineFailure(error).message);
    } finally {
      setRemounting(false);
    }
  };

  /**
   * The Remount handler, or nothing when there is no mount to retry.
   *
   * Absent rather than disabled: the preview screens and the demo share are not
   * connected to a session, and a button that cannot possibly work should not
   * be drawn at all.
   */
  const remountAction = () => {
    const id = activeMountId();
    return id === null ? undefined : () => void remountReadWrite(id);
  };

  /**
   * Run a pending delete now, rather than waiting out its undo window.
   *
   * Called when the window navigates away or the app is closing. A deferred
   * delete that vanished because the user changed directory would be a file
   * they believe they deleted and still have.
   */
  const flushDelete = () => {
    const pending = pendingDelete();
    if (!pending) return;
    window.clearTimeout(pending.timer);
    setPendingDelete(null);
    void runDelete(pending.paths);
  };

  const runDelete = async (paths: string[]) => {
    for (const path of paths) {
      const entry = listing()?.entries.find((candidate) => candidate.path === path);
      try {
        await backend.remove(path, entry?.kind === "dir");
      } catch (error) {
        setProblem(engineFailure(error).message);
      }
    }
    refresh.request();
  };

  /**
   * Delete the selection.
   *
   * Small deletions are *deferred*, not undone: the rows disappear at once and
   * the files are removed when the toast expires. An "Undo" offered after the
   * files were already gone would be a button that cannot do what it says.
   * Above five items the design has no mock, so this asks first — flagged in
   * `docs/design-gaps.md`.
   */
  /** The mount as the permission gate reads it. */
  const permissions = (): PermissionContext => ({
    writable: listing()?.mount.writable ?? false,
    // With nothing mounted, every verb is refused for a reason the user can
    // act on: there is a share to connect (E1-S6).
    reason: listing()?.mount.reason ?? NO_SHARE_REASON,
    hasSelection: selectedVisible().length > 0,
    // Only where there is exactly one thing selected is there a type to have
    // an opinion about. With several, the reason Edit is unavailable is the
    // count, and `undefined` leaves that to whoever is asking (E14-S2).
    editable: (() => {
      const chosen = selectedVisible();
      return chosen.length === 1 && chosen[0] ? editable(chosen[0]) : undefined;
    })(),
  });

  /**
   * The context menu, if one is open (E14-S2).
   *
   * The items are computed when it opens rather than while it is open: a menu
   * whose entries changed under the pointer — a listing arriving, a mount
   * going read-only — would move the item beneath the cursor between press and
   * release. Anything that invalidates it closes it instead.
   */
  const [contextMenu, setContextMenu] =
    createSignal<{ at: { x: number; y: number }; items: MenuItem[] } | null>(null);
  /** The selection a Properties sheet is describing, while it is open. */
  const [inspecting, setInspecting] = createSignal<Entry[] | null>(null);
  /** The selection a Move to… picker is choosing a destination for. */
  const [moving, setMoving] = createSignal<Entry[] | null>(null);

  /**
   * Move a selection into `destination` (E14-S3).
   *
   * One `rename` per item, with `replace: false`, so a name already taken over
   * there is the **server's** refusal at the moment of the move rather than a
   * check here that a second writer could invalidate between the looking and
   * the moving — the same reasoning E14-S1's inline rename gives.
   *
   * It stops at the first failure instead of trying the rest. The failures
   * that matter here are properties of the destination — no permission, no
   * space, gone — so the remaining items would fail identically, and a dozen
   * identical errors are worse than one plus a count of what did move.
   */
  const runMove = async (entries: Entry[], destination: string) => {
    setMoving(null);
    if (refusedBy("move")) return;
    let moved = 0;
    let failure: { name: string; message: string } | null = null;
    for (const entry of entries) {
      try {
        await backend.rename(entry.path, destinationOf(entry, destination), false);
        moved += 1;
      } catch (error) {
        failure = { name: entry.name, message: engineFailure(error).message };
        break;
      }
    }
    // Either way: some of them may have moved, and the rows on screen are
    // stale the moment the first one does.
    refresh.request();
    setSelection(EMPTY_SELECTION);
    const where = destination === "" ? (share() ?? "the share root") : destination;
    if (!failure) {
      setNotice(
        entries.length === 1 && entries[0]
          ? `Moved ${entries[0].name} to ${where}`
          : `Moved ${entries.length} items to ${where}`,
      );
      return;
    }
    // The count first, because "did any of them move?" is the question a
    // half-finished move leaves the user with.
    const progress = moved > 0 ? `Moved ${moved} of ${entries.length}. ` : "";
    setProblem(`${progress}${failure.name} could not be moved: ${failure.message}`);
  };

  /**
   * Open the menu, having first settled what it acts on.
   *
   * Right-clicking a row **outside** the selection selects it, the way every
   * file browser does — the alternative is a menu that acts on rows the user
   * cannot see are chosen. Right-clicking one that is already in the selection
   * leaves it alone, which is the whole point of the gesture on a multiple
   * selection.
   */
  const openContextMenu = (entry: Entry | null, at: { x: number; y: number }) => {
    if (entry && !selected().has(entry.path)) {
      setSelection(clickSelect(EMPTY_SELECTION, order(), entry.path));
    }
    // Read after the selection has settled: `menuFor` is told what the menu
    // acts on, and that must be what is highlighted by the time it is drawn.
    const subject = entry ? selectedVisible() : [];
    const context = entry
      ? permissions()
      : { ...permissions(), hasSelection: false, editable: undefined };
    setContextMenu({ at, items: menuFor(subject, context) });
  };

  const runMenuAction = (action: MenuAction) => {
    setContextMenu(null);
    const chosen = selectedVisible();
    switch (action) {
      case "quickLook":
        if (chosen.length === 1) setQuickLook(true);
        return;
      case "edit":
        void editSelected();
        return;
      case "download":
        void downloadSelected();
        return;
      case "rename":
        // The same guard the Enter shortcut applies, for the same reason: a
        // refusal has to be shown before the field opens, not after the user
        // has typed a new name into it.
        if (chosen.length === 1 && chosen[0] && !refusedBy("rename")) {
          setRenaming(chosen[0].path);
        }
        return;
      case "move":
        if (chosen.length > 0 && !refusedBy("move")) setMoving(chosen);
        return;
      case "delete":
        deleteSelected();
        return;
      case "properties":
        if (chosen.length > 0) setInspecting(chosen);
        return;
      case "newFolder":
        void newFolder();
        return;
      case "upload":
        void uploadHere();
        return;
    }
  };

  /**
   * Refuse a write affordance the mount does not allow, and say why.
   *
   * The seam guard behind this stops the command being *sent*, but it cannot
   * stop a deferred delete from hiding rows and promising a deletion first —
   * the refusal would arrive eight seconds later, after the user watched the
   * rows disappear. So the UI checks before it acts, and the guard remains the
   * thing that makes the check impossible to skip.
   */
  const refusedBy = (affordance: Affordance): boolean => {
    const state = gate(affordance, permissions());
    if (state.disabled && !listing()?.mount.writable) {
      setProblem(state.reason ?? "This mount is read-only");
      return true;
    }
    return state.disabled;
  };

  const deleteSelected = () => {
    if (refusedBy("delete")) return;
    const paths = selectedVisible().map((entry) => entry.path);
    if (paths.length === 0) return;
    if (paths.length > 5) {
      setConfirmDelete(paths);
      return;
    }
    flushDelete();
    const timer = window.setTimeout(() => {
      setPendingDelete(null);
      void runDelete(paths);
    }, 8000);
    setPendingDelete({ paths, timer });
    setSelection(EMPTY_SELECTION);
  };

  const undoDelete = () => {
    const pending = pendingDelete();
    if (!pending) return;
    window.clearTimeout(pending.timer);
    setPendingDelete(null);
  };

  /** Create `untitled folder` and open it for renaming, as the design asks. */
  const newFolder = async () => {
    if (refusedBy("newFolder")) return;
    const target = nav.join(path(), "untitled folder");
    try {
      await backend.mkdir(target);
      refresh.request();
      // Selected and in rename, so the next thing typed names it. Creating a
      // folder called "untitled folder" and leaving it that way is not what
      // anybody meant by the shortcut.
      setSelection(clickSelect(EMPTY_SELECTION, order(), target));
      setRenaming(target);
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  /** Commit an inline rename. An unchanged or empty name just closes it. */
  const commitRename = async (from: string, name: string) => {
    setRenaming(null);
    const trimmed = name.trim();
    const current = from.slice(from.lastIndexOf("/") + 1);
    if (trimmed === "" || trimmed === current || trimmed.includes("/")) return;
    const to = nav.join(nav.parent(from), trimmed);
    try {
      // `replace: false`: a rename that silently overwrote another file would
      // be a destructive act the user did not ask for, and the server refuses
      // it atomically rather than this checking first.
      await backend.rename(from, to, false);
      refresh.request();
      setSelection(clickSelect(EMPTY_SELECTION, order(), to));
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  onMount(() => {
    void backend.transfers().then(setTransfers);
    const stop = backend.onTransferChanged((changed) => {
      setTransfers((current) => {
        const index = current.findIndex((transfer) => transfer.id === changed.id);
        if (index === -1) return [...current, changed];
        const next = current.slice();
        next[index] = changed;
        return next;
      });
    });
    onCleanup(stop);
  });

  /** Re-read the whole queue, for the cases an event stream cannot cover. */
  const refreshTransfers = () => void backend.transfers().then(setTransfers);

  /**
   * Start a transfer, asking about anything already at the destination.
   *
   * The check happens once for the whole batch, so a user dropping twenty
   * files answers at most twenty questions rather than being interrupted
   * twenty times mid-copy. `apply to all` collapses it to one.
   */
  const startTransfer = async (sources: string[], destination: string, upload: boolean) => {
    if (sources.length === 0) return;
    try {
      const clashes = await backend.checkConflicts(sources, destination, upload);
      const standing = settings().conflictDefault;
      if (clashes.length > 0 && standing !== "ask") {
        // A standing answer, applied to the whole batch. The prompt is still
        // the default because replacing a file is not something to do on the
        // user's behalf unless they said so once, deliberately.
        setConflicts({ items: clashes, upload, sources, destination });
        await resolveConflicts(standing, true);
        return;
      }
      if (clashes.length > 0) {
        setConflicts({ items: clashes, upload, sources, destination });
        return;
      }
      await enqueue(sources, destination, upload);
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  const enqueue = async (sources: string[], destination: string, upload: boolean) => {
    if (sources.length === 0) return;
    await (upload
      ? backend.enqueueUploads(sources, destination)
      : backend.enqueueDownloads(sources, destination));
    setShowTransfers(true);
    refreshTransfers();
  };

  /** Apply one answer to the pending conflicts and start what survives. */
  const resolveConflicts = async (
    choice: "replace" | "keepBoth" | "skip",
    applyToAll: boolean,
  ) => {
    const pending = conflicts();
    if (!pending) return;
    setConflicts(null);
    const clashing = new Set(pending.items.map((item) => item.name));
    const answered = applyToAll ? pending.items : pending.items.slice(0, 1);
    const answeredNames = new Set(answered.map((item) => item.name));

    // Skip drops them; Replace and Keep both send them. "Keep both" renames on
    // the *server's* suggestion, because the name has to be free where the
    // file is going and only that side knows.
    const keep = pending.sources.filter((source) => {
      const name = source.split(/[\\/]/).pop() ?? source;
      if (!clashing.has(name)) return true;
      if (!answeredNames.has(name)) return true;
      return choice !== "skip";
    });
    try {
      await enqueue(keep, pending.destination, pending.upload);
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  const downloadSelected = async () => {
    const chosen = selectedVisible();
    if (chosen.length === 0) return;
    // The preference wins; `null` means "wherever the OS puts downloads",
    // which only the engine can answer.
    const folder = settings().downloadFolder ?? (await backend.downloadFolder());
    await startTransfer(chosen.map((entry) => entry.path), folder, false);
  };

  const uploadHere = async () => {
    if (refusedBy("upload")) return;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({ multiple: true, directory: false });
      const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
      await startTransfer(paths, path(), true);
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  /**
   * Write a working copy back, and say what happened.
   *
   * A conflict is not an error and is not swallowed: it raises the prompt, and
   * nothing has been written at that point. Anything else is a toast, because
   * the handoff asks for "a toast or a queue entry" per write-back and a
   * silent save gives the user no reason to believe it worked.
   */
  const writeBack = async (session: EditSession, force: boolean) => {
    try {
      const outcome = await backend.saveEdit(session.id, force);
      if (outcome.outcome === "conflict") {
        setSaveConflict(session);
        return;
      }
      if (outcome.outcome === "saved") {
        setNotice(`Saved ${session.name} to the share`);
      }
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  onMount(() => {
    const stop = backend.onEditSaved((session) => void writeBack(session, false));
    onCleanup(stop);
  });

  /**
   * Files dragged in from the OS (E13-S5).
   *
   * Gated on `acceptsDrop`, which is the same permission gate the toolbar
   * uses: highlighting on drag-over and then refusing the drop is worse than
   * never highlighting, so a read-only mount shows nothing at all.
   */
  onMount(() => {
    const stop = backend.onFileDrop((event) => {
      if (!acceptsDrop(permissions())) return;
      if (event.phase === "leave") {
        setDropTarget(null);
        return;
      }
      const folder = folderUnder(event.position);
      if (event.phase === "over") {
        setDropTarget({ folder });
        return;
      }
      setDropTarget(null);
      // Into the hovered folder if there is one, otherwise here. Dropping on
      // a folder row and having the files land beside it instead would be the
      // surprising reading of that gesture.
      void startTransfer(event.paths, folder ?? path(), true);
    });
    onCleanup(stop);
  });

  /** The folder row under a point, if the pointer is over one. */
  const folderUnder = (position: { x: number; y: number } | null): string | null => {
    if (!position) return null;
    const element = document.elementFromPoint(position.x, position.y);
    const row = element?.closest("[data-path]");
    const path = row?.getAttribute("data-path");
    if (path === null || path === undefined) return null;
    return entries().some((entry) => entry.path === path && entry.kind === "dir")
      ? path
      : null;
  };

  /** Open the selected file in the OS editor and start watching it. */
  const editSelected = async () => {
    if (refusedBy("edit")) return;
    const chosen = selectedVisible();
    if (chosen.length !== 1 || !chosen[0]) return;
    try {
      const session = await backend.editFile(chosen[0].path);
      setNotice(`Opened ${session.name} in your editor`);
    } catch (error) {
      setProblem(engineFailure(error).message);
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
    // A row waiting out its undo window is already gone as far as the user is
    // concerned. It is hidden rather than deleted, which is what makes Undo
    // able to do anything at all.
    const doomed = new Set(pendingDelete()?.paths ?? []);
    const remaining = doomed.size === 0 ? visible : visible.filter((e) => !doomed.has(e.path));
    return sortEntries(remaining, sort(), foldersFirst());
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

  /**
   * What the title bar, the breadcrumb and the status path name.
   *
   * **The listing is the gate, not the bookmark.** A window with no listing has
   * no share behind it, and a bookmark still flagged `mounted` from a previous
   * session would otherwise title the window after a share the shell is not
   * holding — which is the first-run bug in a second costume (E1-S6).
   *
   * Below that gate the bookmark wins, and the design's two demo screens fill
   * in for the preview: the fixture describes a share no session is holding,
   * and the mock's own names are a better answer there than a connection
   * invented to justify them.
   */
  const named = () => {
    if (listing() === null) return null;
    const bookmark = activeBookmark();
    if (bookmark) {
      const root = bookmark.share.split("/").filter(Boolean);
      // Two different hosts, because the two places that show one are asking
      // different questions. The title bar asks *which machine is this* and,
      // for a local folder, the honest answer is the machine you are sitting
      // at. The path field asks for the URI's authority, and `file:` has none
      // — so it is empty there and the crumbs read `file:///Users/you/Docs`,
      // which is the path a user would type anywhere else.
      const local = bookmark.protocol === "local";
      return {
        host: local ? thisMachine() : bookmark.host,
        uriHost: local ? "" : bookmark.host,
        share: root[root.length - 1] ?? bookmark.share,
        scheme: uriScheme(bookmark.protocol),
        root,
      };
    }
    return props.readOnly
      ? {
          host: "10.0.0.4",
          uriHost: "10.0.0.4",
          share: "scratch",
          scheme: "nfs",
          root: ["export", "scratch"],
        }
      : {
          host: "nas.local",
          uriHost: "nas.local",
          share: "media",
          scheme: "smb",
          root: ["media"],
        };
  };

  const host = () => named()?.host ?? null;
  /** The URI's authority, which is empty for a `file:` mount. */
  const uriHost = () => named()?.uriHost ?? null;
  const share = () => named()?.share ?? null;
  const scheme = () => named()?.scheme ?? null;
  const rootSegments = () => named()?.root ?? [];
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
    <div class={styles.window}>
      <TitleBar share={share()} host={host()} />
      <Toolbar
        mount={listing()?.mount}
        scheme={scheme()}
        host={uriHost()}
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
        filterRef={(element) => (filterInput = element)}
        onQuickLook={toggleQuickLook}
        onEdit={() => void editSelected()}
        onDownload={() => void downloadSelected()}
        onUpload={() => void uploadHere()}
        sidebarCollapsed={settings().sidebarCollapsed}
        onToggleSidebar={() => update("sidebarCollapsed", !settings().sidebarCollapsed)}
        onPreferences={() => setShowPreferences(true)}
      />
      {/* The preview follows the selection: it shows whatever single row
          is selected, and closes on its own when that stops being one. */}
      <Show when={notice()}>
        {(message) => (
          <div class={styles.toast}>
            <span>{message()}</span>
            <button class={styles.toastAction} onClick={() => setNotice(null)}>
              Dismiss
            </button>
          </div>
        )}
      </Show>
      <Show when={saveConflict()}>
        {(session) => (
          <div class={styles.scrim}>
            <div class={styles.sheet} role="alertdialog" aria-label="Remote changed">
              <div class={styles.sheetTitle}>
                “{session().name}” changed on the share
              </div>
              <div class={styles.sheetBody}>
                Somebody else edited it since you opened it, so nothing has been
                written. Your copy is still where your editor has it open.
              </div>
              <div class={styles.sheetActions}>
                <button class={styles.action} onClick={() => setSaveConflict(null)}>
                  Cancel
                </button>
                <button
                  class={`${styles.action} ${styles.actionDanger}`}
                  onClick={() => {
                    const pending = session();
                    setSaveConflict(null);
                    void writeBack(pending, true);
                  }}
                >
                  Replace
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
      <Show when={showTransfers()}>
        <Transfers
          transfers={transfers()}
          onClose={() => setShowTransfers(false)}
          onPauseAll={() => void backend.pauseAllTransfers().then(refreshTransfers)}
          onClearFinished={() => void backend.clearFinishedTransfers().then(refreshTransfers)}
          onPause={(id) => void backend.pauseTransfer(id).then(refreshTransfers)}
          onCancel={(id) => void backend.cancelTransfer(id).then(refreshTransfers)}
          onRetry={(id) => void backend.retryTransfer(id).then(refreshTransfers)}
          onRemount={remountAction()}
        />
      </Show>
      <Show when={conflicts()}>
        {(pending) => (
          <div class={styles.scrim}>
            <div class={styles.sheet} role="alertdialog" aria-label="Destination exists">
              <div class={styles.sheetTitle}>
                “{pending().items[0]?.name}” is already there
              </div>
              <div class={styles.sheetBody}>
                Keep both would call it “{pending().items[0]?.keepBothAs}”.
                <Show when={pending().items.length > 1}>
                  {" "}
                  {pending().items.length - 1} more like this. Your answer applies to all
                  of them.
                </Show>
              </div>
              <div class={styles.sheetActions}>
                <button class={styles.action} onClick={() => void resolveConflicts("skip", true)}>
                  Skip
                </button>
                <button
                  class={styles.action}
                  onClick={() => void resolveConflicts("keepBoth", true)}
                >
                  Keep both
                </button>
                <button
                  class={`${styles.action} ${styles.actionDanger}`}
                  onClick={() => void resolveConflicts("replace", true)}
                >
                  Replace
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
      <Show when={listing()}>
        {(data) => (
          <Show
            when={quickLookMounted() && selectedVisible().length === 1 ? selectedVisible()[0] : null}
          >
            {(entry) => (
              <QuickLook
                entry={entry()}
                mount={data().mount}
                caps={data().mount.caps}
                closing={quickLookClosing()}
                onClose={() => setQuickLook(false)}
                // The panel's own entry point into E12, offered only where it
                // would work — the footer already says so on a read-only mount.
                onEdit={data().mount.writable ? () => void editSelected() : undefined}
              />
            )}
          </Show>
        )}
      </Show>
      {/* Above the list and below the sheets: a menu is transient, and the
          dialogs it opens are the thing it hands over to. */}
      <Show when={contextMenu()}>
        {(menu) => (
          <ContextMenu
            items={menu().items}
            at={menu().at}
            onChoose={runMenuAction}
            onClose={() => setContextMenu(null)}
          />
        )}
      </Show>
      <Show when={moving()}>
        {(entries) => (
          <MoveTo
            entries={entries()}
            from={path()}
            shareName={share() ?? ""}
            listDir={(at) => backend.listDir(at)}
            onMove={(destination) => void runMove(entries(), destination)}
            onCancel={() => setMoving(null)}
          />
        )}
      </Show>
      <Show when={inspecting()}>
        {(entries) => (
          <Properties
            entries={entries()}
            location={path()}
            // Never actually null here: the menu that opens this needs rows,
            // and rows need a listing, and a listing names its share.
            shareName={share() ?? ""}
            onClose={() => setInspecting(null)}
          />
        )}
      </Show>
      <Show when={problem()}>
        {(message) => (
          <Banner
            message={message()}
            reconnecting={remounting()}
            onReconnect={remountAction()}
            onDismiss={() => setProblem(null)}
          />
        )}
      </Show>
      <div class={styles.body}>
        <Sidebar
          collapsed={settings().sidebarCollapsed}
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
        <div class={`${styles.listColumn} ${problem() ? styles.listDimmed : ""}`}>
          {/* Nothing mounted is not an empty directory: there is no list to
              draw, and the one thing to offer is the way out of it (E1-S6). */}
          <Show
            when={!disconnected()}
            fallback={<EmptyState onNewConnection={() => setConnecting(true)} />}
          >
            <FileList
              layout={settings().rowLayout}
              entries={entries()}
              selected={selected}
              onSelect={select}
              onBand={selectBand}
              reveal={reveal()}
              onOpen={open}
              onContextMenu={openContextMenu}
              // Undefined at the share root, which is what withholds the `..`
              // row — read from the same `canAscend` as the toolbar's arrow.
              onAscend={nav.canAscend(path()) ? ascend : undefined}
              slow={listingState.slow()}
              sort={sort()}
              onSort={onSort}
              widths={widths()}
              onResize={onResize}
              onResizeEnd={onResizeEnd}
              renaming={renaming()}
              dropTarget={dropTarget()}
              onRename={(from, name) => void commitRename(from, name)}
            />
          </Show>
          <StatusBar
            mount={mountFacts()}
            path={mountPath()}
            items={entries().length}
            total={totalEntries()}
            selected={selectedVisible().length}
            remounting={remounting()}
            onRemount={remountAction()}
            selectedBytes={selectedBytes()}
          />
        </div>
      </div>
      <Show when={pendingDelete()}>
        {(pending) => (
          <div class={styles.toast}>
            <span>
              Deleting {pending().paths.length}{" "}
              {pending().paths.length === 1 ? "item" : "items"}
            </span>
            <button class={styles.toastAction} onClick={undoDelete}>
              Undo
            </button>
          </div>
        )}
      </Show>
      <Show when={confirmDelete()}>
        {(paths) => (
          <div class={styles.scrim}>
            <div class={styles.sheet} role="alertdialog" aria-label="Confirm delete">
              <div class={styles.sheetTitle}>Delete {paths().length} items?</div>
              <div class={styles.sheetBody}>
                This cannot be undone. Deleting more than five items at once is not
                in the design yet, so this asks rather than guessing — see
                docs/design-gaps.md.
              </div>
              <div class={styles.sheetActions}>
                <button class={styles.action} onClick={() => setConfirmDelete(null)}>
                  Cancel
                </button>
                <button
                  class={`${styles.action} ${styles.actionDanger}`}
                  onClick={() => {
                    const doomed = paths();
                    setConfirmDelete(null);
                    setSelection(EMPTY_SELECTION);
                    void runDelete(doomed);
                  }}
                >
                  Delete
                </button>
              </div>
            </div>
          </div>
        )}
      </Show>
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
      <Show when={showPreferences()}>
        <Preferences
          settings={settings()}
          onChange={changeSetting}
          onClose={() => setShowPreferences(false)}
          // The browser preview has no folder picker, so the field stays
          // typeable and the button simply is not offered.
          onPickFolder={inTauri() ? () => void pickDownloadFolder() : undefined}
        />
      </Show>
      <Show when={connecting()}>
        {(pending) => (
          <ConnectionDialog
            backend={backend}
            initial={pending() === true ? undefined : (pending() as Draft)}
            // No picker in the browser preview, so the Folder field stays
            // typeable there and the button is not offered at all.
            pickFolder={inTauri() ? pickShareFolder : undefined}
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
  );
}
