import { createEffect, createMemo, createSignal, onCleanup, onMount, untrack, Show } from "solid-js";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { QuickLook } from "./components/QuickLook";
import { Transfers } from "./components/Transfers";
import {
  defaultBookmarkStore,
  fromDraft,
  matchBookmark,
  probePort,
  remove as removeBookmark,
  restore,
  toDraft,
  upsert,
  withMounted,
  type BookmarkStore,
} from "./state/bookmarks";
import {
  addFavorite,
  defaultFavoriteStore,
  defaultFavorites,
  describeFavoriteDrop,
  favoriteDraft,
  favoriteDrop,
  isFavorite,
  NO_USER_FOLDERS,
  normalisePath,
  removeFavorite,
  toFavorite,
  visibleFavorites,
  type Favorite,
  type FavoriteStore,
} from "./state/favorites";
import { defaultPort, toSpec, uriScheme, type Draft } from "./state/connection";
import { thisMachine } from "./state/platform";
import { Banner } from "./components/Banner";
import { EmptyState } from "./components/EmptyState";
import { FileList } from "./components/FileList";
import { ColumnView } from "./components/ColumnView";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { DirectoryTree } from "./components/DirectoryTree";
import { TitleBar } from "./components/TitleBar";
import { Toolbar } from "./components/Toolbar";
// The window's own chrome — the body, the list column, the scrims and sheets
// App draws itself — shares the shell stylesheet with the components above.
import styles from "./components/shell.module.css";
import {
  defaultBackend,
  fixtureBackend,
  writeGuarded,
  type Backend,
  type UserFolders,
} from "./state/backend";
import type {
  Bookmark,
  Conflict,
  EditSession,
  Entry,
  LogAction,
  LogLine,
  MountEvent,
  MountInfo,
  RowLayout,
  Theme,
  Transfer,
} from "./state/types";
import { LOG_CAPACITY } from "./state/log";
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
import { createPane, type Pane, type PaneSide } from "./app/pane";
import { createQueues } from "./app/queues";
import { createAddDownload } from "./app/addDownload";
import {
  EMPTY_SELECTION,
  band,
  click as clickSelect,
  extendBy,
  moveTo,
  step,
  typeAhead,
  type Move,
} from "./state/selection";
import { GoToFolder } from "./components/GoToFolder";
import { ContextMenu } from "./components/ContextMenu";
import { Properties } from "./components/Properties";
import { MoveTo } from "./components/MoveTo";
import { destinationOf } from "./state/move";
import {
  SPRING_DELAY_MS,
  describeDrop,
  moveTargets,
  osPath,
  resolveDrop,
  type DropMount,
  type DropPlan,
} from "./state/dnd";
import { viewMenu, type ViewMenuAction } from "./state/viewMenu";
import { bookmarkMenu, favoriteMenu, menuFor, type MenuAction, type MenuItem } from "./state/menu";
import { editable } from "./state/preview";
import {
  DEFAULT_WIDTHS,
  resize,
  type Divider,
  type VisibleColumns,
  type Widths,
} from "./state/columns";
import { defaultPrefs, inTauri, type Prefs } from "./state/prefs";
import * as shortcuts from "./state/shortcuts";
import { windowParams, type WindowParams } from "./state/window";
import { parseLocation } from "./state/location";
// The same resolver ⌘⇧G uses, so a path typed in either place means one thing.
import { resolve as resolveGoto } from "./state/goto";
import { DEFAULT_PANE_SIZE, clampToWindow, type PaneSize } from "./state/paneSize";
import {
  DOWNLOAD_CATEGORIES,
  countByCategory,
  outstanding,
  validDownloadDrop,
  type DownloadCategory,
} from "./state/downloads";
import { Preferences } from "./components/Preferences";
import { AddDownloadDialog } from "./components/AddDownloadDialog";
import {
  DEFAULTS,
  DEFAULTS as DEFAULT_SETTINGS,
  loadSettings,
  resolveTheme,
  saveSetting,
  type Settings,
  type TreeBand,
} from "./state/settings";
import { DEFAULT_TREE_HEIGHT } from "./state/tree";

/**
 * Which layout each verb names.
 *
 * One table because two paths reach it — ⌘1–⌘4 and the View menu — and they
 * used to say it twice, in a nested ternary that a fourth layout would have
 * made unreadable.
 */
const LAYOUT_KEYS: Record<
  "layoutColumns" | "layoutNameLed" | "layoutMono" | "layoutIcons" | "layoutColumnView",
  RowLayout
> = {
  layoutColumns: "columns",
  layoutNameLed: "name-led",
  layoutMono: "mono",
  layoutIcons: "icons",
  layoutColumnView: "column-view",
};

export default function App(props: {
  backend?: Backend;
  theme?: Theme;
  readOnly?: boolean;
  bookmarks?: BookmarkStore;
  /** Where FAVORITES is kept. Injected the same way bookmarks are (E27-S1). */
  favorites?: FavoriteStore;
  prefs?: Prefs;
  /**
   * Which window this is and what it opened on (E23-S1).
   *
   * Read from this window's own URL when absent, which is how the real app
   * supplies it — a test says it out loud instead of writing to `location`.
   */
  window?: WindowParams;
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
  // Which window this is, and what it was opened on (E23-S1). Everything below
  // is per-window already — a window is its own webview running its own `App`
  // — so this only says which window the engine should answer for, and where
  // ⌘N asked this one to start.
  const self = props.window ?? windowParams();
  // Where the window is looking. The listing follows this rather than the
  // other way round, so every way of navigating — a double-click, the
  // breadcrumb, Back, the keyboard — goes through one place.
  //
  // The mount travels with the request: a pane holding its own share must not
  // have its listing served by the window's (XTP-E8-S1). Dropping the argument
  // here is how both panes come to show the focused pane's share.
  const listDir = (target: string, mount?: string) => backend.listDir(target, mount);
  /**
   * Where this window is, and what is selected there (`app/pane.ts`).
   *
   * One of these today; two when `paneMode` lands (`twopane.md` XTP-E1-S2).
   * Destructured so every reader below says `path()` and `listing()` as it
   * always did — the point of this step is that nothing had to change to fit.
   */
  const panes: Record<PaneSide, Pane> = {
    // The browser preview opens with a row selected, because the design's
    // screens draw one and there is no way to click in a screenshot.
    left: createPane({
      path: self.path,
      listDir,
      onProblem: (message) => setProblem(message),
      selection: inTauri()
        ? undefined
        : clickSelect(EMPTY_SELECTION, [], "Arrival.2016.mkv"),
    }),
    // Loaded only once two panes are drawn, so a single-pane window still makes
    // exactly one listing request. It takes a mount of its own when a bookmark
    // is opened into it (XTP-E8-S1), and until then reads the window's.
    right: createPane({ listDir, onProblem: (message) => setProblem(message) }),
  };
  const [focus, setFocus] = createSignal<PaneSide>("left");
  const focused = () => panes[focus()];
  /**
   * Move focus between the panes.
   *
   * One answer to "which pane" for the whole window: the toolbar's verbs, the
   * keyboard, and the pane a bookmark opens into (`twopane.md` XTP-E8-S1) all
   * read this rather than each deciding for itself. A no-op while one pane is
   * drawn, which is what makes this step invisible.
   */
  const focusPane = (side: PaneSide) => setFocus(side);
  /**
   * The panes actually on screen.
   *
   * One today. XTP-E7's `paneMode` is what makes this two, and everything that
   * has to behave differently with two panes asks this rather than assuming —
   * Tab, below, would otherwise move focus to a pane nobody can see, and the
   * window would go blank because that pane has no mount.
   */
  const drawnPanes = (): PaneSide[] =>
    paneMode() === "dual" ? ["left", "right"] : ["left"];

  /**
   * The focused pane's state, under the names the window already used.
   *
   * Delegates rather than a destructure: `path()` still means what it did, and
   * now means *the focused pane's*. Every reader below is unchanged, which is
   * how a change this wide can be trusted — and with one pane drawn, focus
   * never moves, so it resolves to the same pane every time.
   */
  const history = () => focused().history();
  const setHistory: Pane["setHistory"] = (next) => focused().setHistory(next as never);
  const path = () => focused().path();
  const listing = () => focused().listing();
  const goTo = (target: string) => focused().goTo(target);
  const ascend = () => focused().ascend();
  const filter = () => focused().filter();
  const setFilter: Pane["setFilter"] = (next) => focused().setFilter(next as never);
  const selection = () => focused().selection();
  const setSelection: Pane["setSelection"] = (next) =>
    focused().setSelection(next as never);
  const selected = () => focused().selected();
  const refresh = { request: () => focused().refresh.request() };
  const listingState = {
    load: (target: string) => focused().listingState.load(target),
    slow: () => focused().listingState.slow(),
  };

  /**
   * Which category the transfers pane is filtered to (XDL-E2-S1).
   *
   * Remembered per window label so a restart keeps the tab, the way the view
   * used to be. A *new* key rather than the old `window-view:` record: that
   * one carried a `view` alongside the category, and reading it here is how a
   * removed view comes back from the dead. Stale records are simply never
   * read again.
   */
  const [category, setCategory] = createSignal<DownloadCategory>("all");
  const categoryKey = () => `transfers-category:${self.label}`;
  onMount(async () => {
    const saved = await prefs.get(categoryKey());
    // Only ever `setCategory`. Anything else here runs *after* the queue seed
    // below has decided whether to open the pane, and would silently overrule
    // it on every launch.
    if (DOWNLOAD_CATEGORIES.some((candidate) => candidate.id === saved)) {
      setCategory(saved as DownloadCategory);
    }
  });

  /**
   * A new failure selects the Failed tab — but only with the queue on screen
   * (`twopane.md` XTP-E4-S1).
   *
   * A transfer failing is not a reason to take the window over. The drawer
   * never opens itself; what it does, when it is already open, is stop showing
   * a tab where nothing has changed while a row has just gone red one tab
   * along.
   *
   * Keyed on ids rather than a count: a failure that arrives as another is
   * cleared leaves the count where it was, and that is exactly the moment the
   * tab is most wrong.
   */
  let knownFailures = new Set<string>();
  createEffect(() => {
    const failed = new Set(
      transfers().filter((transfer) => transfer.status === "failed").map((transfer) => transfer.id),
    );
    const fresh = [...failed].some((id) => !knownFailures.has(id));
    knownFailures = failed;
    if (fresh && untrack(showTransfers)) openCategory("failed");
  });

  /** The pane's tab strip, with live counts off the whole queue. */
  const categories = () => {
    const counts = countByCategory(transfers());
    return DOWNLOAD_CATEGORIES.map(({ id, label }) => ({ id, label, count: counts[id] }));
  };

  /** Choose a tab in the transfers pane, and remember it for next launch. */
  const openCategory = (id: DownloadCategory) => {
    setCategory(id);
    void prefs.set(categoryKey(), id);
  };

  /**
   * Each drawn pane loads its own path.
   *
   * One effect per pane rather than one that follows focus: a focus-following
   * effect reloads the pane you just moved to, which already has its listing,
   * and it would load the second pane while it is inert. The guard is what
   * keeps a single-pane window to one request (XTP-E1-S2).
   */
  createEffect(() => panes.left.listingState.load(panes.left.path()));
  createEffect(() => {
    // Only this one is guarded: the left pane is always drawn, so making it
    // depend on the mode as well would refetch it every time the second pane
    // is opened or put away.
    if (paneMode() !== "dual") return;
    panes.right.listingState.load(panes.right.path());
  });

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
  const prefs = props.prefs ?? defaultPrefs();

  // Everything persisted lives here (E17-S1), except the column widths, which
  // move on every pointer frame during a drag and would make this signal — and
  // so the whole window — react at that rate.
  const [settings, setSettings] = createSignal<Settings>(DEFAULT_SETTINGS);
  /**
   * A memo, not a plain read of `settings()`.
   *
   * `settings` is one signal holding one object, so reading `.paneMode` off it
   * subscribes to *every* setting. The per-pane load effects below read this,
   * and without the memo changing the theme — or the sort, or hidden files —
   * would re-run them and refetch both listings.
   */
  const paneMode = createMemo(() => settings().paneMode);
  const [downloadLimits, setDownloadLimits] = createSignal<
    import("./state/types").HttpDownloadLimits
  >();
  const [keychainAvailable, setKeychainAvailable] = createSignal<boolean | undefined>();
  const [widths, setWidths] = createSignal<Widths>(DEFAULT_WIDTHS);
  const [paneSize, setPaneSize] = createSignal<PaneSize>(DEFAULT_PANE_SIZE);

  const nativeHttpProxy = (proxy: Settings["downloadProxy"]) =>
    proxy === "environment"
      ? ({ kind: "environment" } as const)
      : proxy === "disabled"
        ? ({ kind: "disabled" } as const)
        : ({ kind: "explicit", url: proxy.url, noProxy: proxy.noProxy } as const);

  /** Change one preference and remember it. */
  const update = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSettings((current) => ({ ...current, [key]: value }));
    void saveSetting(prefs, key, value);
  };

  const [showPreferences, setShowPreferences] = createSignal(false);
  // The Add Download dialog and its submit path (`app/addDownload.ts`).
  const addDownload = createAddDownload({
    backend,
    prefs,
    label: self.label,
    settings,
    // Thunks: this runs before those are declared, and a direct reference
    // would be a use-before-declaration rather than a late call.
    refreshTransfers: () => refreshTransfers(),
    onProblem: (message) => setProblem(message),
    onNotice: (message) => setNotice(message),
  });
  const openAddDownload = addDownload.open;
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
    if (key === "downloadProxy" || key === "downloadHeaders" || key === "downloadNetrcPath" || key === "downloadUsername") {
      const next = { ...settings(), [key]: value };
      void backend.configureHttp?.({
        proxy: nativeHttpProxy(next.downloadProxy),
        headers: next.downloadHeaders,
        netrcPath: next.downloadNetrcPath,
        username: next.downloadUsername,
      });
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
    setSplitRatio(loaded.splitRatio);
    setTreeBand(loaded.treeBand);
    setDrawerHeight(loaded.drawerHeight);
    if (loaded.drawerHeight > 0) restoreDrawer = loaded.drawerHeight;
    // Clamped on the way in: a pane sized on a bigger display would otherwise
    // come back wider than this window, with its left edge — and the grip on
    // it — off screen, so it could never be made smaller again.
    setPaneSize(
      clampToWindow(loaded.transfersPane, {
        width: Math.max(0, window.innerWidth - 16),
        height: Math.max(0, window.innerHeight - 16),
      }),
    );
    // The engine holds the transfer worker, so its concurrency has to be told
    // rather than read.
    void backend.setTransferConcurrency(loaded.transferConcurrency);
    // The HTTP client lives in Rust. Only nonsecret settings cross this call;
    // the native side reads the password from its keychain account.
    void backend.configureHttp?.({
      proxy: nativeHttpProxy(loaded.downloadProxy),
      headers: loaded.downloadHeaders,
      netrcPath: loaded.downloadNetrcPath,
      username: loaded.downloadUsername,
    });
    void backend.keychainAvailable().then(setKeychainAvailable).catch(() => setKeychainAvailable(false));
    void backend.downloadLimits().then(setDownloadLimits).catch(() => setDownloadLimits(undefined));
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
  const onResize = (divider: Divider, delta: number, visible: VisibleColumns) =>
    setWidths((current) => resize(current, divider, delta, visible));
  const onResizeEnd = () => void saveSetting(prefs, "columnWidths", widths());
  const onPaneResizeEnd = () => void saveSetting(prefs, "transfersPane", paneSize());

  /**
   * Where the two panes divide, as the left pane's fraction (XTP-E2-S1).
   *
   * A signal during the drag and a setting on release, the same split the
   * column dividers use: writing on every pointermove would be a file write
   * per frame.
   */
  const [splitRatio, setSplitRatio] = createSignal(DEFAULTS.splitRatio);
  /**
   * How tall each pane's directory tree is, `0` meaning put away (XTP-E2-S4).
   *
   * A signal during the drag and a setting on release, like the split beside
   * it: a file write per pointermove is a write per frame.
   */
  const [treeBand, setTreeBand] = createSignal<TreeBand>(DEFAULTS.treeBand);
  /**
   * The docked drawer's height, `0` being its tab strip alone (XTP-E4-S1).
   *
   * A signal during the drag and a setting on release, like the split and the
   * tree band. `restoreDrawer` is where a collapse goes back to: collapsing to
   * `0` and expanding to the default would lose a height the user chose, and
   * the store cannot hold both at once.
   */
  const [drawerHeight, setDrawerHeight] = createSignal(DEFAULTS.drawerHeight);
  let restoreDrawer = DEFAULTS.drawerHeight;
  const saveDrawer = () => void saveSetting(prefs, "drawerHeight", drawerHeight());
  const resizeDrawer = (height: number) => {
    if (height > 0) restoreDrawer = height;
    setDrawerHeight(height);
  };
  const toggleDrawer = () => {
    resizeDrawer(drawerHeight() > 0 ? 0 : restoreDrawer);
    saveDrawer();
  };
  /**
   * The queue is docked with two panes and floats with one (XTP-E4-S1).
   *
   * The handoff draws the drawer in its two-pane screens alone, and the reason
   * generalises: a sheet anchored under the toolbar covers one of the two
   * columns a transfer is between, which is the pair the user is watching.
   */
  const dockedTransfers = () => drawnPanes().length > 1;
  /**
   * The band's height for one side, or `0` where there is no band.
   *
   * Two panes only. The handoff draws the tree in its two-pane screens alone,
   * and a single-pane window already has the sidebar for this — a second
   * navigator beside it would be two places to click the same folder.
   */
  const treeHeightOf = (side: PaneSide) =>
    drawnPanes().length > 1 ? treeBand()[side] : 0;
  const resizeTree = (side: PaneSide, height: number) =>
    setTreeBand((current) => ({ ...current, [side]: height }));
  const saveTreeBand = () => void saveSetting(prefs, "treeBand", treeBand());
  const toggleTree = () => {
    const side = focus();
    resizeTree(side, treeBand()[side] > 0 ? 0 : DEFAULT_TREE_HEIGHT);
    saveTreeBand();
  };
  let paneRow: HTMLDivElement | undefined;
  const startSplitDrag = (event: PointerEvent) => {
    if (event.button !== 0 || !paneRow) return;
    event.preventDefault();
    const box = paneRow.getBoundingClientRect();
    const move = (at: PointerEvent) => {
      if (box.width === 0) return;
      // Clamped where the setting clamps: a pane of no width takes its
      // splitter to the window's edge with it, and there is no way back.
      const ratio = Math.min(0.8, Math.max(0.2, (at.clientX - box.left) / box.width));
      setSplitRatio(ratio);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      void saveSetting(prefs, "splitRatio", splitRatio());
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  // The row type-to-select jumped to, so the list can scroll it into view.
  const [reveal, setReveal] = createSignal<number | null>(null);
  /**
   * How many rows Page Up/Down moves (E26-S2).
   *
   * Reported by the list, because a page is its viewport measured in its row
   * height — both of which belong to the component that drew them. Reaching
   * into the DOM from here would be reading its state behind its back.
   */
  const [pageRows, setPageRows] = createSignal(1);
  /** The Go to Folder sheet's path, or `null` when it is closed. */
  const [goingTo, setGoingTo] = createSignal<string | null>(null);
  const [connecting, setConnecting] = createSignal<Draft | true | null>(null);
  const store = props.bookmarks ?? defaultBookmarkStore();
  const [bookmarks, setBookmarks] = createSignal<Bookmark[]>([]);
  const [selectedBookmark, setSelectedBookmark] = createSignal<string | null>(null);
  const [undo, setUndo] = createSignal<{ bookmark: Bookmark; index: number } | null>(null);

  /**
   * Bookmarks that exist only because a favourite was opened.
   *
   * The window's model is one mount at a time and a mount is a bookmark, so
   * opening a folder from FAVORITES has to produce one. It should not *outlive*
   * the mount: the favourite is already the permanent row for that folder, and
   * a second one under BOOKMARKS saying the same thing is the redundancy this
   * removes (asked for directly 2026-09-09).
   *
   * Membership, not a path test. Asking "is this path a favourite?" would also
   * hide a bookmark the user deliberately saved for a folder that happens to
   * be one — and the OS folders are seeded as favourites, so that is most of
   * them. Only a bookmark *this window created by opening a favourite* is
   * transient, and only when there was not already one saved for it.
   */
  const [favoriteMounts, setFavoriteMounts] = createSignal<ReadonlySet<string>>(new Set());
  const isFavoriteMount = (bookmark: Bookmark) => favoriteMounts().has(bookmark.id);

  const persist = (next: Bookmark[]) => {
    setBookmarks(next);
    // Held in memory so MOUNTED can draw it and the window knows what it is
    // on, and left out of the store so it is not there next launch.
    void store.save(next.filter((bookmark) => !isFavoriteMount(bookmark)));
  };

  /**
   * What the sidebar draws: everything, less the favourite-derived mounts that
   * are not currently mounted. Filtering here rather than in `Sidebar` because
   * the section split is `mounted` — a row removed from this list simply never
   * reaches BOOKMARKS, while a mounted one still reaches MOUNTED.
   */
  const sidebarBookmarks = () =>
    bookmarks().filter((bookmark) => bookmark.mounted || !isFavoriteMount(bookmark));

  // FAVORITES (E27-S1). The saved list, and separately the subset whose
  // folders are actually there — kept apart so that unplugging a disk hides a
  // row without forgetting it, and plugging it back in brings it back.
  const favoriteStore = props.favorites ?? defaultFavoriteStore();
  const [favorites, setFavorites] = createSignal<Favorite[]>([]);
  const [presentFolders, setPresentFolders] = createSignal<Set<string>>(new Set());
  /** The home directory, which is the one favourite not named after its leaf. */
  // The whole set, not just home: the sidebar picks each favourite's glyph by
  // comparing its path against these, so a folder the *user* happens to have
  // named "Documents" keeps the plain folder icon and only the real one gets
  // the document. Empty until the first answer comes back, which is the state
  // `favoriteGlyph` treats as "nothing is special yet".
  const [folders, setFolders] = createSignal<UserFolders>(NO_USER_FOLDERS);
  const home = () => folders().home;
  const shownFavorites = createMemo(() => visibleFavorites(favorites(), presentFolders()));

  const persistFavorites = (next: Favorite[]) => {
    setFavorites(next);
    void favoriteStore.save(next);
  };

  /**
   * Which of these favourites are folders that exist, as normalised paths.
   *
   * A failed check answers *all of them* rather than none. The section is a
   * convenience; emptying a user's sidebar because one IPC call failed is a
   * worse outcome than offering a row that turns out to be gone, which fails
   * with the engine's own message and can be tried again.
   */
  const foldersPresent = async (list: Favorite[]): Promise<Set<string>> => {
    const paths = list.map((favorite) => normalisePath(favorite.path));
    try {
      return new Set((await backend.existingDirectories(paths)).map(normalisePath));
    } catch {
      return new Set(paths);
    }
  };

  /**
   * Hold the favourites' folders before anything asks for one.
   *
   * *Asked for directly, 2026-09-07: "all of the local favorites should be
   * preconnected on startup".*
   *
   * **Here rather than in Rust**, and the reason is what a favourite *is* on
   * each side. The list lives in `favorites.json` through the store plugin, and
   * the mount's id is `bookmarkId`'s — a rule about protocol, host and share
   * that the dialog, the sidebar and every enqueued row already share. The
   * engine reading that file would be the engine learning a frontend store's
   * format *and* re-deriving an id it must match exactly, in a second place,
   * for ever; getting it wrong there is a preconnect that mounts something no
   * queued row is waiting for, which is silent.
   *
   * That leaves the timing objection, which is that a window exists only after
   * the engine has drained its restored queue — and it is answered rather than
   * dodged: a queued row whose share is not connected now *waits* instead of
   * failing, and a mount reaching `ready` drains the queue again. So this being
   * late is fine. Before that pair of fixes it would not have been.
   *
   * Only folders that are actually there, and every failure dropped: an ejected
   * disk must produce no banner and no failed row, and `preconnect` puts no
   * window on the mount, so nothing is told about it either way.
   */
  const preconnectFavorites = (list: Favorite[], present: Set<string>) => {
    for (const favorite of list) {
      if (!present.has(normalisePath(favorite.path))) continue;
      const bookmark = fromDraft(favoriteDraft(favorite));
      void backend.preconnect(toSpec(toDraft(bookmark), bookmark.id)).catch(() => undefined);
    }
  };

  onMount(async () => {
    let saved: Favorite[] | null;
    try {
      saved = await favoriteStore.load();
    } catch {
      // Unreadable store. The section stays empty for this run, and — the part
      // that matters — nothing is written: seeding the defaults over a list
      // that is still on disk would destroy it to fix a transient failure.
      return;
    }
    if (saved !== null) {
      setFavorites(saved);
      const present = await foldersPresent(saved);
      setPresentFolders(present);
      preconnectFavorites(saved, present);
      // Only for the naming rule, so a folder added later can still be
      // recognised as home. Nothing is seeded from it.
      void backend
        .userFolders()
        .then(setFolders)
        .catch(() => undefined);
      return;
    }
    // Nothing has ever been saved: this is a first run, and the only run that
    // seeds. Once the list exists, removing a default is final.
    try {
      const folders = await backend.userFolders();
      setFolders(folders);
      const candidates = defaultFavorites(folders);
      const present = await foldersPresent(candidates);
      // Seeded with what is actually there, so the saved list is exactly what
      // the user sees. Seeding a folder this machine does not have would put a
      // permanently invisible row in everyone's store.
      const seeded = candidates.filter((favorite) =>
        present.has(normalisePath(favorite.path)),
      );
      setPresentFolders(present);
      persistFavorites(seeded);
      preconnectFavorites(seeded, present);
    } catch {
      // No answer about this machine's folders, so nothing to seed from. The
      // next launch asks again.
    }
  });

  /** Add folders to the section, skipping any already there. */
  const addFavorites = (paths: string[]) => {
    const before = favorites();
    let next = before;
    for (const path of paths) next = addFavorite(next, toFavorite(path, home()));
    if (next === before) return before;
    // These paths came from a listing or from a check against the filesystem,
    // so they are known to be there — recorded now rather than waiting for the
    // next load, which would otherwise draw nothing for the row just added.
    setPresentFolders(new Set([...presentFolders(), ...paths.map(normalisePath)]));
    persistFavorites(next);
    return next;
  };

  onMount(async () => {
    // Nothing is mounted when the app starts: sessions are not persisted, so
    // a `mounted` flag read back from the store is from a previous run and
    // describes a share the shell is not holding. Found 2026-09-05 by
    // relaunching with a saved xsync bookmark: the sidebar showed it under
    // MOUNTED and the window titled itself after it, over no listing at all.
    // Double-clicking the row under BOOKMARKS is what mounts it.
    //
    // The one exception is a window opened by ⌘N (E23-S1): the shell put that
    // window on a mount *before* the webview loaded, and told it which in the
    // URL. That is a fact about this run, from the process that holds the
    // session — which is exactly what the flag on disk is not.
    const saved = withMounted(
      await store.load(),
      new Set<string>(self.mount === null ? [] : [self.mount]),
    );
    // Set rather than persisted: which share a window is looking at is the
    // window's, and writing it to the shared store would have two windows
    // overwriting each other's answer to a question the store cannot hold.
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
      // A sheet owns the keyboard the same way (E26-S2). Go to Folder has its
      // own Tab, Enter and Escape, and the arrows must not walk the selection
      // in the list behind it while the user is typing a path.
      if (goingTo() !== null) return;
      // Anything typed into the filter box or a dialog field belongs to that
      // field, not to the list. That includes the Add Download dialog's URL
      // box, which is a textarea rather than an input (XDL-E3-S1): without
      // it a Backspace there deletes the file selection behind the dialog.
      const target = event.target as HTMLElement | null;
      const typing = Boolean(
        target &&
          (target.tagName === "INPUT" ||
            target.tagName === "TEXTAREA" ||
            target.isContentEditable),
      );
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
          // The button is absent on downloads, so the key is too (XDL-E2-S1).
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

        // Moving through the list without a pointer (E26-S2). Eight keys in
        // one block, because they differ only in how far they go and whether
        // the anchor comes along; the rules are `selection.ts`'s and are
        // tested there.
        case "moveDown":
        case "moveUp":
        case "extendDown":
        case "extendUp":
        case "pageDown":
        case "pageUp":
        case "first":
        case "last": {
          if (renaming() !== null) return;
          // The arrows and Page keys scroll a container by default. A key that
          // both scrolls and moves the selection moves the eye twice as far as
          // it looks, so the browser's scroll is refused and `reveal` does it.
          event.preventDefault();
          const rows = order();
          const moved: Move = (() => {
            switch (verb) {
              case "first":
                return moveTo(selection(), rows, 0);
              case "last":
                return moveTo(selection(), rows, rows.length - 1);
              // Shift extends and leaves the anchor where it is (E8-S8), so
              // ⇧↓⇧↑ shrinks the range it just grew rather than walking down.
              case "extendDown":
                return extendBy(selection(), rows, 1);
              case "extendUp":
                return extendBy(selection(), rows, -1);
              case "moveDown":
                return step(selection(), rows, 1);
              case "moveUp":
                return step(selection(), rows, -1);
              case "pageDown":
                return step(selection(), rows, pageRows());
              default:
                return step(selection(), rows, -pageRows());
            }
          })();
          if (moved.reveal === null) return;
          setSelection(moved.selection);
          setReveal(moved.reveal);
          return;
        }

        case "open": {
          if (renaming() !== null) return;
          event.preventDefault();
          const chosen = selectedVisible();
          if (chosen.length !== 1 || !chosen[0]) return;
          if (chosen[0].kind === "dir") {
            goTo(chosen[0].path);
            return;
          }
          // A file has no "open" in this window yet: handing it to the OS is
          // E25's, and inventing a launcher now would be a promise this build
          // cannot keep. Quick Look is the honest thing it can do with a file
          // today, and it is what ⌘O does until then.
          setQuickLook(true);
          return;
        }

        case "goToFolder":
          event.preventDefault();
          // Seeded with where the window is, so the sheet is a place to edit a
          // path rather than an empty box to retype one into.
          setGoingTo(path());
          return;

        case "toggleHidden":
          event.preventDefault();
          changeSetting("showHidden", !showHidden());
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
          // Gone with the button, not merely hidden behind it: leaving the key
          // bound would keep a verb the toolbar says does not exist, and ⌘D
          // would still send a file to the downloads folder while the two panes
          // say direction is chosen by pointing (XTP-E7-S2).
          if (drawnPanes().length > 1) return;
          event.preventDefault();
          void downloadSelected();
          return;

        case "upload":
          if (drawnPanes().length > 1) return;
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

        case "newWindow":
          // Prevented before anything else: on a Mac ⌘N is the webview's own
          // "new window" too, and letting it through opens a second, chromeless
          // one beside ours.
          event.preventDefault();
          void backend
            .newWindow(path())
            .catch((error) => setProblem(engineFailure(error).message));
          return;

        case "preferences":
          event.preventDefault();
          setShowPreferences(true);
          return;

        case "transfers":
          // Toggles: it is the same key the View menu's item carries, and a
          // control that only opens leaves ⇧⌘T unable to undo itself. On
          // macOS the menu answers this first and the webview never sees it;
          // this is the path everywhere else.
          event.preventDefault();
          setShowTransfers((open) => !open);
          return;

        case "switchPane": {
          // Left to the browser while one pane is drawn: Tab is how the
          // toolbar's controls are reached, and taking it to do nothing would
          // cost that for a gesture with nowhere to go.
          const drawn = drawnPanes();
          if (drawn.length < 2) return;
          event.preventDefault();
          const next = drawn[(drawn.indexOf(focus()) + 1) % drawn.length]!;
          focusPane(next);
          return;
        }

        case "layoutColumns":
        case "layoutNameLed":
        case "layoutMono":
        case "layoutIcons":
        case "layoutColumnView":
          event.preventDefault();
          update("rowLayout", LAYOUT_KEYS[verb]);
          return;

        case "togglePanes":
          event.preventDefault();
          togglePanes();
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
    if (bookmark) await openMount(bookmark);
  };

  /**
   * Connect a share, make it this window's, and remember it.
   *
   * Split out of `mountBookmark` so that opening a favourite is the *same*
   * connection rather than a second one that happens to look like it (E27-S1).
   * A favourite is a local folder with the path already chosen, so it arrives
   * here as the bookmark the dialog's Local tab would have produced —
   * `bookmarkId` gives the two the same id, which is what stops opening
   * Documents twice from growing two rows under MOUNTED.
   *
   * `upsert` rather than a map over the existing list: the bookmark may be one
   * this window has never held.
   */
  /**
   * Mount a bookmark into the focused pane (XTP-E8-S1).
   *
   * The focused pane, not "the window", because with two panes there are two
   * answers and the focused one is the single value every other verb already
   * reads. With one pane it is the only pane, so this is what it always did.
   */
  const openMount = async (bookmark: Bookmark) => {
    try {
      await backend.mount(toSpec(toDraft(bookmark), bookmark.id));
      const target = focused();
      target.setMountId(bookmark.id);
      target.setHistory(nav.initial());
      // `mounted` is now "some pane is on it", which is why it is computed from
      // the panes rather than set to the one bookmark just opened: with two
      // panes on two shares, two bookmarks are mounted at once.
      const held = mountedIds();
      persist(
        upsert(bookmarks(), bookmark).map((candidate) => ({
          ...candidate,
          mounted: held.has(candidate.id),
        })),
      );
      target.listingState.load("");
    } catch {
      setConnecting(toDraft(bookmark));
    }
  };

  /**
   * Open a favourite: mount the folder and show it (E27-S1).
   *
   * Deliberately the bookmark route and nothing else. A favourite that has
   * been opened therefore also appears under MOUNTED, and stays as a bookmark
   * afterwards — noted in `docs/design-gaps.md`, because the alternative is a
   * second way to hold a mount and two answers to "what is this window on".
   */
  const openFavorite = (path: string) => {
    const target = normalisePath(path);
    const favorite = favorites().find(
      (candidate) => normalisePath(candidate.path) === target,
    );
    if (!favorite) return;
    const bookmark = fromDraft(favoriteDraft(favorite));
    // Already saved means the user made it deliberately; opening the favourite
    // must not quietly turn their bookmark into a transient one.
    if (!bookmarks().some((candidate) => candidate.id === bookmark.id)) {
      setFavoriteMounts((current) => new Set(current).add(bookmark.id));
    }
    void openMount(bookmark);
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
  // The app-wide queue and the verbs over it (`app/queues.ts`).
  const {
    transfers,
    queues,
    refreshTransfers,
    createQueue,
    updateQueue,
    deleteQueue,
    moveTransferToQueue,
    reorderQueue,
    reorderQueues,
  } = createQueues({
    backend,
    onProblem: setProblem,
    onSettled: (changed) => {
      // A transfer that landed changed the share it landed on, and nothing
      // else tells the window: `listing://changed` is emitted for this app's
      // own mutations — a rename, a delete, a new folder — and the queue is
      // not one of them. Without this a file dragged in from Finder copies
      // successfully and simply never appears (reported 2026-09-06).
      //
      // Through the coalescer rather than straight to `load`: dropping twenty
      // files finishes twenty transfers, and that is one relist, not twenty.
      // The mount is checked because the queue event is a broadcast — every
      // window hears about every transfer, and relisting on someone else's
      // would be this window refreshing for a share it is not showing.
      if (changed.status === "done" && changed.mount === activeMountId()) {
        refresh.request();
      }
    },
  });
  const [showTransfers, setShowTransfers] = createSignal(false);
  /**
   * The message log, and which of the two the transfers surface is showing
   * (`twopane.md` XTP-E5-S1).
   *
   * The ring first, then the stream: a window opened later would otherwise
   * start empty and be missing exactly the lines explaining why it was opened.
   * `Verbose` is per window and not persisted — it is a way of looking at what
   * is on screen now, not a setting about the app.
   */
  const [log, setLog] = createSignal<LogLine[]>([]);
  const [logView, setLogView] = createSignal<"queue" | "log">("queue");
  const [verbose, setVerbose] = createSignal(false);
  onMount(() => {
    void backend.logLines().then(setLog);
    const stop = backend.onLogLine((line) =>
      // Capped where the engine caps its own ring, so a window left open for a
      // week holds what the engine holds rather than a week of lines.
      setLog((current) => [...current, line].slice(-LOG_CAPACITY)),
    );
    onCleanup(stop);
  });
  const clearLog = () => {
    setLog([]);
    void backend.clearLog();
  };
  /**
   * The recovery a hint line offers.
   *
   * The line names the *mount*, so this acts on the share the line is about
   * rather than on whatever the focused pane happens to be holding — a log is
   * read after the fact, and the pane has usually moved on.
   */
  const runLogAction = (action: LogAction, mount: string | undefined) => {
    if (action !== "remountRw") return;
    const id = mount ?? activeMountId();
    if (id !== null) void remountReadWrite(id);
  };
  // Open on launch only when the queue still owes work — something queued,
  // running or paused. Not "the queue is non-empty": finished rows are kept
  // until `Clear finished` takes them, so that opened the pane on every launch
  // for anyone who had ever downloaded anything.
  onMount(() =>
    void backend.transfers().then((all) => outstanding(all) && setShowTransfers(true)),
  );
  /** Destinations already taken, awaiting Replace / Keep both / Skip. */
  const [conflicts, setConflicts] = createSignal<{
    items: Conflict[];
    upload: boolean;
    sources: string[];
    destination: string;
    /** The share side of the transfer, when it is not this window's (E24-S2). */
    mount?: string;
  } | null>(null);
  /**
   * A drag is over this window, and where it would land.
   *
   * One signal for both kinds — a Finder drag (E13-S5) and an Excalibur one
   * (E24-S2) — because they light up the same rows. `message` is the drop
   * ring's line and is empty for an OS drag, which has nothing to say beyond
   * the highlight; `bookmark` is the sidebar row a drop would go to.
   */
  const [dropTarget, setDropTarget] = createSignal<{
    folder: string | null;
    bookmark?: string | null;
    /** The FAVORITES section, which highlights whole rather than by row. */
    favorites?: boolean;
    message?: string;
    refused?: boolean;
  } | null>(null);
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
  /** Every share a drawn pane is on. Two of them once two panes are drawn. */
  const mountedIds = () =>
    new Set(
      drawnPanes()
        .map((side) => panes[side].mountId())
        .filter((id): id is string => id !== null),
    );
  /**
   * The share the window's verbs act on: the focused pane's.
   *
   * Falls back to whichever bookmark is mounted, which is what a window that
   * has never named a mount has — the state every single-pane window is in
   * until a bookmark is opened into a pane.
   */
  const activeMountId = () =>
    focused().mountId() ?? bookmarks().find((bookmark) => bookmark.mounted)?.id ?? null;
  const activeBookmark = () => bookmarks().find((bookmark) => bookmark.mounted) ?? null;
  /** A pane's own mount id, falling back to the window's for a pane that has none. */
  const mountIdOf = (target: Pane) => target.mountId() ?? activeMountId();

  /**
   * Another window changed something on this window's share (E23-S1).
   *
   * Through the same coalescer a local mutation uses, so a window watching
   * twenty deletions in another window relists once rather than twenty times —
   * and so the two windows converge without either of them polling. The shell
   * only sends this to windows on the mount, and the id is checked anyway:
   * an event that arrives a moment after this window switched shares would
   * otherwise relist the new one for a change to the old.
   */
  onMount(() => {
    const stop = backend.onListingChanged((mount) => {
      if (mount !== activeMountId()) return;
      refresh.request();
    });
    onCleanup(stop);
  });

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
   * The read-only recovery path.
   *
   * A refused upgrade is not an error: the engine resolves with the mount facts
   * that came back, and the status bar re-reads the reason from them. Treating
   * it as a failure is how this button becomes a loop.
   */
  const remountReadWrite = (id: string) => reestablish(id, { upgrade: true });

  /**
   * Put a share's connection back, as it was mounted (XTP-E2-S3).
   *
   * The toolbar's `Reconnect` and the banner's. Deliberately not the upgrade
   * above: a share somebody opened read-only must not come back writable
   * because they pressed the button that recovers a dropped connection. The
   * two differ by one flag here and by one flag in the engine, and the labels
   * on the two buttons are what say which is which.
   */
  const reconnect = (id: string) => reestablish(id, { upgrade: false });

  const reestablish = async (id: string, options: { upgrade: boolean }) => {
    setRemounting(true);
    try {
      if (options.upgrade) await backend.remountReadWrite(id);
      else await backend.reconnect(id);
      // Re-list rather than patching the mount facts by hand: `writable`
      // travels with the listing, and taking it from anywhere else is how the
      // badge and the rows come to disagree. Every pane on the share, because
      // two panes can be on one (XTP-E8-S1) and the one that did not ask is
      // showing the same stale facts.
      for (const side of drawnPanes()) {
        if (mountIdOf(panes[side]) === id) panes[side].refresh.request();
      }
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
   * The same button, retrying the share the pane it sits under is holding.
   *
   * Taking the id from the window would have the right pane's Remount upgrade
   * the left pane's share — a button acting on a share the user is not looking
   * at.
   */
  const remountActionFor = (target: Pane) => {
    const id = mountIdOf(target);
    return id === null ? undefined : () => void remountReadWrite(id);
  };

  /** `Reconnect`, on the share the focused pane is holding. */
  const reconnectAction = () => {
    const id = activeMountId();
    return id === null ? undefined : () => void reconnect(id);
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
  /**
   * A share-relative path on this window's mount, as an absolute OS path.
   *
   * `null` unless the mount is a folder on this machine, which is the only
   * case where a share-relative path *has* an absolute form: everything else
   * is reachable through its mount and nowhere else, which is why a favourite
   * can only ever be local (E27-S1).
   */
  const absolutePath = (path: string): string | null => {
    const bookmark = activeBookmark();
    if (!bookmark || bookmark.protocol !== "local") return null;
    return osPath({ id: bookmark.id, protocol: "local", root: bookmark.share }, path);
  };

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
    // A menu opened on the list is never about a favourite row, so the
    // previous one's path is cleared rather than left to be acted on.
    setFavoriteMenuPath(null);
    const only = subject.length === 1 ? subject[0] : undefined;
    const absolute = only ? absolutePath(only.path) : null;
    setContextMenu({
      at,
      items: menuFor(subject, context, {
        local: activeBookmark()?.protocol === "local",
        favorite: absolute !== null && isFavorite(favorites(), absolute),
      }),
    });
  };

  /** The favourite a context menu was opened on, while that menu is open. */
  const [favoriteMenuPath, setFavoriteMenuPath] = createSignal<string | null>(null);

  /** The one-item menu on a row in FAVORITES. */
  const openFavoriteMenu = (path: string, at: { x: number; y: number }) => {
    setFavoriteMenuPath(path);
    setContextMenu({ at, items: favoriteMenu() });
  };

  /** Which bookmark the sidebar menu is open on, while it is open. */
  const [bookmarkMenuId, setBookmarkMenuId] = createSignal<string | null>(null);
  /**
   * The menu on a row in MOUNTED or BOOKMARKS (E3-S1).
   *
   * Also selects the row, so the footer's − and Edit — which act on the
   * selection — agree with what was just right-clicked rather than with
   * whatever was left selected before.
   */
  const openBookmarkMenu = (id: string, at: { x: number; y: number }) => {
    setSelectedBookmark(id);
    setBookmarkMenuId(id);
    setContextMenu({ at, items: bookmarkMenu() });
  };

  /**
   * The View menu's verbs (E17-S1).
   *
   * Every one of them is a persisted preference, so they go through
   * `changeSetting` rather than a signal — the same path Preferences uses, and
   * the reason a layout chosen here is still chosen after a relaunch.
   */
  /**
   * Turn the second pane on and off.
   *
   * Focus goes back to the left pane when the second one is put away, because
   * the focused pane is what every verb acts on and leaving focus on a pane
   * nobody can see would aim them at nothing.
   */
  const togglePanes = () => {
    const next = settings().paneMode === "dual" ? "single" : "dual";
    if (next === "single") focusPane("left");
    // A second pane that opened on nothing would be an empty column with no
    // way to tell whether it was broken. It opens where the first one is, which
    // is the folder the user is looking at, and either half can be moved from
    // there. It gets its own mount when a bookmark is opened into it
    // (`twopane.md` XTP-E8-S1).
    if (next === "dual" && panes.right.listing() === null) {
      panes.right.goTo(panes.left.path());
    }
    changeSetting("paneMode", next);
  };

  const runViewAction = (action: ViewMenuAction) => {
    switch (action) {
      case "toggleHidden":
        changeSetting("showHidden", !showHidden());
        return;
      case "toggleFoldersFirst":
        changeSetting("foldersFirst", !foldersFirst());
        return;
      case "layoutColumns":
      case "layoutNameLed":
      case "layoutMono":
      case "layoutIcons":
      case "layoutColumnView":
        changeSetting("rowLayout", LAYOUT_KEYS[action]);
        return;
      case "togglePanes":
        togglePanes();
        return;
      case "toggleTree":
        toggleTree();
    }
  };

  const runMenuAction = (action: MenuAction) => {
    setContextMenu(null);
    const chosen = selectedVisible();
    switch (action) {
      case "removeBookmark": {
        const id = bookmarkMenuId();
        setBookmarkMenuId(null);
        if (id !== null) removeSelected(id);
        return;
      }
      case "editBookmark": {
        const id = bookmarkMenuId();
        setBookmarkMenuId(null);
        const bookmark = bookmarks().find((candidate) => candidate.id === id);
        if (bookmark) setConnecting(toDraft(bookmark));
        return;
      }
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
      case "addFavorite": {
        // The menu only offers this for one folder on a local mount, so both
        // of these are already true — checked again because a menu item and
        // the action behind it disagreeing is exactly how a disabled verb
        // still runs.
        const folder = chosen.length === 1 ? chosen[0] : undefined;
        const absolute = folder?.kind === "dir" ? absolutePath(folder.path) : null;
        if (absolute === null) return;
        addFavorites([absolute]);
        setNotice(`Added ${folder?.name ?? absolute} to Favorites`);
        return;
      }
      case "removeFavorite": {
        const path = favoriteMenuPath();
        setFavoriteMenuPath(null);
        if (path !== null) persistFavorites(removeFavorite(favorites(), path));
        return;
      }
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


  /**
   * Start a transfer, asking about anything already at the destination.
   *
   * The check happens once for the whole batch, so a user dropping twenty
   * files answers at most twenty questions rather than being interrupted
   * twenty times mid-copy. `apply to all` collapses it to one.
   */
  const startTransfer = async (
    sources: string[],
    destination: string,
    upload: boolean,
    mount?: string,
  ) => {
    if (sources.length === 0) return;
    try {
      const clashes = await backend.checkConflicts(sources, destination, upload, mount);
      const standing = settings().conflictDefault;
      if (clashes.length > 0 && standing !== "ask") {
        // A standing answer, applied to the whole batch. The prompt is still
        // the default because replacing a file is not something to do on the
        // user's behalf unless they said so once, deliberately.
        setConflicts({ items: clashes, upload, sources, destination, mount });
        await resolveConflicts(standing, true);
        return;
      }
      if (clashes.length > 0) {
        setConflicts({ items: clashes, upload, sources, destination, mount });
        return;
      }
      await enqueue(sources, destination, upload, mount);
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  const enqueue = async (
    sources: string[],
    destination: string,
    upload: boolean,
    mount?: string,
    rename?: Record<string, string>,
  ) => {
    if (sources.length === 0) return;
    await (upload
      ? backend.enqueueUploads(sources, destination, mount, rename)
      : backend.enqueueDownloads(sources, destination, mount, rename));
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
    // the *destination's* suggestion, because the name has to be free where the
    // file is going and only that side knows.
    const keep = pending.sources.filter((source) => {
      const name = source.split(/[\\/]/).pop() ?? source;
      if (!clashing.has(name)) return true;
      if (!answeredNames.has(name)) return true;
      return choice !== "skip";
    });
    /**
     * What each renamed source should be called.
     *
     * This used to be computed, shown to the user as "Keep both would call it
     * …", and then thrown away — the original paths were re-sent, so the answer
     * behaved as Replace and overwrote the file it promised to keep
     * (`backlog.md:117`). The name now travels with the request.
     */
    const rename =
      choice === "keepBoth"
        ? Object.fromEntries(
            pending.sources.flatMap((source) => {
              const name = source.split(/[\\/]/).pop() ?? source;
              if (!answeredNames.has(name)) return [];
              const item = answered.find((candidate) => candidate.name === name);
              return item ? [[source, item.keepBothAs] as const] : [];
            }),
          )
        : undefined;
    try {
      await enqueue(keep, pending.destination, pending.upload, pending.mount, rename);
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

  // The View menu's Transfers item. Same toggle as ⇧⌘T and the toolbar
  // button — on macOS the menu is the only one of the three that ever sees
  // the key, because an accelerator is answered before the webview.
  onMount(() => {
    const stop = backend.onMenuTransfers(() => setShowTransfers((open) => !open));
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
      const urlFiles =
        event.paths.length > 0 && event.paths.every((candidate) => /\.txt$/i.test(candidate));
      if (urlFiles) {
        if (event.phase === "leave") {
          setDropTarget(null);
        } else if (event.phase === "over") {
          setDropTarget({ folder: null, message: "Add URLs" });
        } else {
          setDropTarget(null);
          void backend
            .downloadUrlsFromFiles(event.paths)
            .then((urls) => {
              if (urls.length > 0) void openAddDownload(urls);
            })
            .catch((error) => setProblem(engineFailure(error).message));
        }
        return;
      }
      if (!acceptsDrop(permissions())) return;
      if (event.phase === "leave") {
        setDropTarget(null);
        return;
      }
      const folder = folderUnder(event.position);
      if (event.phase === "over") {
        // Where this window was when the drag reached it, so a spring-loaded
        // descent can be backed out of. Recorded on the first `over` rather
        // than only where the drag started, because the window that springs is
        // usually not the window that was dragged from.
        if (dragOrigin() === null) setDragOrigin(path());
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

  /**
   * Dragging a selection out of the list (E24-S2).
   *
   * The gesture E8-S8 reserved: `pointerdown` on the name cell plus a few
   * pixels of movement, which is why rubber-band selection deliberately does
   * not start there. From that point AppKit owns the drag, so nothing here
   * tracks the pointer.
   */
  const dragOut = async (entry: Entry) => {
    // The dragged set is the selection when the grabbed row is in it, and just
    // that row when it is not — Finder's rule, and the one that stops a drag
    // of one file quietly moving nine others the user had forgotten about.
    const chosen = selectedVisible().some((candidate) => candidate.path === entry.path)
      ? selectedVisible()
      : [entry];
    setDragOrigin(path());
    try {
      await backend.beginDrag(
        chosen.map((item) => item.path),
        // Which of them are folders, because a file promise has to say so and
        // a path cannot: `Dune` and `Dune.mkv` are told apart by the listing,
        // not by the string. A folder promised as a file is a drop Finder
        // accepts and then writes an empty file for.
        chosen.filter((item) => item.kind === "dir").map((item) => item.path),
        await dragImage(chosen),
      );
    } catch (error) {
      setProblem(engineFailure(error).message);
    }
  };

  /**
   * The drag image, as PNG bytes.
   *
   * Rendered here rather than in Rust because this is the side that knows what
   * the row looks like, and drawn at CSS scale rather than device scale: an
   * `NSImage` built from PNG data takes its size in points from the pixel
   * dimensions, so a 2× bitmap would be a drag image twice the size of the row
   * it came from. Slightly soft on a Retina display, and noted in
   * docs/design-gaps.md.
   */
  const dragImage = async (chosen: Entry[]): Promise<Uint8Array> => {
    const label =
      chosen.length === 1 && chosen[0] ? chosen[0].name : `${chosen.length} items`;
    const canvas = document.createElement("canvas");
    const measure = canvas.getContext("2d");
    if (!measure) return new Uint8Array();
    const font = "13px -apple-system, system-ui, sans-serif";
    measure.font = font;
    const width = Math.min(320, Math.ceil(measure.measureText(label).width) + 40);
    canvas.width = width;
    canvas.height = 22;
    const paint = canvas.getContext("2d");
    if (!paint) return new Uint8Array();
    paint.font = font;
    // A glyph and a name, which is what the row shows. Deliberately not a
    // screenshot of the row: the row is 1000px wide and a drag image that size
    // hides the folder it is being dropped on.
    paint.fillStyle = "rgba(120,120,128,0.55)";
    paint.fillRect(4, 5, 12, 12);
    paint.fillStyle = "rgba(0,0,0,0.85)";
    paint.fillText(label, 24, 16);
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
    return blob ? new Uint8Array(await blob.arrayBuffer()) : new Uint8Array();
  };

  /** The folder this window was showing when the drag began, for Escape. */
  const [dragOrigin, setDragOrigin] = createSignal<string | null>(null);
  /** Set once a spring-load has descended, so leaving can undo it. */
  const [sprung, setSprung] = createSignal(false);
  let springTimer: number | undefined;
  let springOver: string | null = null;

  /** A mount as `dnd.ts` needs it, or `null` when this app is not holding it. */
  const dropMountOf = (id: string | null): DropMount | null => {
    if (id === null) return null;
    const bookmark = bookmarks().find((candidate) => candidate.id === id);
    if (!bookmark || !bookmark.mounted) return null;
    // `share` is the absolute folder for a local mount, and unused otherwise —
    // it is what turns a share-relative path into the OS path the transfer
    // queue's two verbs are the only ones that take.
    return { id, protocol: bookmark.protocol, root: bookmark.share };
  };

  /** Whether a point is over the sidebar's FAVORITES section (E27-S1). */
  const overFavorites = (position: { x: number; y: number }): boolean =>
    document.elementFromPoint(position.x, position.y)?.closest("[data-favorites]") != null;

  /**
   * Add what a drop onto FAVORITES was carrying.
   *
   * The pasteboard carries paths and a mount, never a kind — `dnd.ts` says so
   * where it has to invent one — so which of them are folders is a question
   * for the filesystem, asked with the same call the section already uses to
   * hide a favourite whose folder is gone. That answer arrives after the drop,
   * which is why the ring says `Add to Favorites` for a file too: the window
   * cannot know at drag time, and a ring that flickered between two answers
   * while the pointer sat still would be worse than one that is occasionally
   * followed by "that was not a folder".
   *
   * A file among the folders is skipped rather than refusing the whole drop:
   * dragging a folder and a file together plainly means the folder.
   */
  const addDroppedFavorites = async (paths: string[]) => {
    const folders = await backend.existingDirectories(paths).catch(() => []);
    if (folders.length === 0) {
      setNotice(
        paths.length === 1
          ? "Only folders can be added to Favorites"
          : "None of those are folders",
      );
      return;
    }
    const before = favorites();
    const after = addFavorites(folders);
    const added = after.length - before.length;
    if (added === 0) {
      setNotice("Already in Favorites");
      return;
    }
    const skipped = paths.length - folders.length;
    const what = added === 1 ? "1 folder" : `${added} folders`;
    setNotice(
      skipped === 0
        ? `Added ${what} to Favorites`
        : `Added ${what} to Favorites — ${skipped} skipped, not folders`,
    );
  };

  /** Where a point would drop: a folder row, a sidebar row, or this folder. */
  /**
   * The pane a point is over, or the focused one.
   *
   * A drop says where it goes by landing there, so it must not be answered by
   * "whichever pane has focus" — with two panes that is how a drag onto the
   * right one writes into the left.
   */
  const paneUnder = (position: { x: number; y: number }): Pane => {
    const column = document.elementFromPoint(position.x, position.y)?.closest("[data-pane]");
    const side = column?.getAttribute("data-pane");
    return side === "left" || side === "right" ? panes[side] : focused();
  };

  const targetUnder = (
    position: { x: number; y: number } | null,
  ): { mount: string | null; path: string; label: string; bookmark: string | null } | null => {
    if (!position) return null;
    const element = document.elementFromPoint(position.x, position.y);
    const row = element?.closest("[data-bookmark]");
    const id = row?.getAttribute("data-bookmark");
    if (id) {
      // A bookmark is that share's **root** — the "coming later" E13-S5 left.
      const bookmark = bookmarks().find((candidate) => candidate.id === id);
      return { mount: id, path: "", label: bookmark?.name ?? id, bookmark: id };
    }
    const over = paneUnder(position);
    const folder = folderUnder(position);
    if (folder !== null) {
      return {
        mount: over.mountId() ?? activeMountId(),
        path: folder,
        label: folder.slice(folder.lastIndexOf("/") + 1),
        bookmark: null,
      };
    }
    // The list background, or anywhere else in the window: this folder.
    return {
      mount: activeMountId(),
      path: path(),
      label: path() === "" ? (share() ?? "this share") : path().slice(path().lastIndexOf("/") + 1),
      bookmark: null,
    };
  };

  /**
   * A drag from an Excalibur window, over this one (E24-S2).
   *
   * Every window gets its own stream — the native side emits to the window the
   * pointer is over, never broadcasts — so this asks no questions about whose
   * drag it is. It only has to decide what a drop *here* would mean, which is
   * `resolveDrop`'s job, and say so.
   */
  onMount(() => {
    const stop = backend.onWindowDrag((event) => {
      if (event.phase === "leave" || !event.selection) {
        // Left without dropping: Escape, or the pointer moved to another
        // window. Either way this window backs out of anything it sprang into,
        // which is what "Escape backs out to where the drag started" is.
        endDrag(true);
        return;
      }
      // FAVORITES is answered before `resolveDrop`, which knows only about
      // verbs that touch a share: this drop moves no bytes at all, it adds
      // rows to the sidebar's own list (E27-S1).
      if (event.position && overFavorites(event.position)) {
        const drop = favoriteDrop(event.selection, dropMountOf(event.selection.mount));
        if (event.phase === "over") {
          setDropTarget({
            folder: null,
            favorites: true,
            message: describeFavoriteDrop(drop),
            refused: !drop.ok,
          });
          // Nothing to spring into: the section is a list, not a folder.
          springLoad(null);
          return;
        }
        endDrag(false);
        if (drop.ok) void addDroppedFavorites(drop.paths);
        else setProblem(drop.reason);
        return;
      }

      const target = targetUnder(event.position);
      if (!target) {
        endDrag(false);
        return;
      }
      const to = dropMountOf(target.mount);
      const plan: DropPlan = to
        ? resolveDrop({
            selection: event.selection,
            from: dropMountOf(event.selection.mount),
            to,
            path: target.path,
            // Only this window's own mount has facts to read. A sidebar row for
            // a share another window holds is taken at its word and refused by
            // the server if it was wrong — better than refusing a legal drop on
            // the strength of not having asked.
            writable:
              target.mount === activeMountId()
                ? (listing()?.mount.writable ?? false)
                : true,
            alt: event.modifiers.alt,
          })
        : { verb: "refused", reason: `Open ${target.label} first.` };

      if (event.phase === "over") {
        setDropTarget({
          folder: target.bookmark ? null : (folderUnder(event.position) ?? null),
          bookmark: target.bookmark,
          message: describeDrop(plan, target.label),
          refused: plan.verb === "refused",
        });
        springLoad(target.bookmark ? null : folderUnder(event.position));
        return;
      }
      endDrag(false);
      void runDrop(plan);
    });
    onCleanup(() => {
      stop();
      endDrag(false);
    });
  });

  /**
   * Descend into a folder held under the pointer, the way Finder does.
   *
   * The timer is restarted whenever the folder under the pointer changes, so a
   * drag *across* a column of folders springs into none of them — only a pause
   * counts. Deliberately not armed for a sidebar row: descending into a share
   * would mean mounting and switching this window mid-drag, which is a much
   * larger thing to do to somebody by hovering.
   */
  const springLoad = (folder: string | null) => {
    if (folder === springOver) return;
    springOver = folder;
    window.clearTimeout(springTimer);
    if (folder === null) return;
    springTimer = window.setTimeout(() => {
      setSprung(true);
      goTo(folder);
    }, SPRING_DELAY_MS);
  };

  /** Clear the highlight, and optionally undo a spring-load. */
  const endDrag = (restore: boolean) => {
    window.clearTimeout(springTimer);
    springOver = null;
    setDropTarget(null);
    const origin = dragOrigin();
    if (restore && sprung() && origin !== null) goTo(origin);
    setSprung(false);
    // Cleared unconditionally, which is what makes the `leave` that always
    // follows a drop harmless: the native side takes every window's drop
    // target down when the session ends, and that teardown reports a `leave`
    // to each of them — including the one that just received the drop, which
    // must not then back out of the folder it sprang into.
    setDragOrigin(null);
  };

  /**
   * Do what the drop ring promised.
   *
   * The refusal is a toast rather than a silence, for E13-S5's reason: a drop
   * that lands and does nothing is indistinguishable from a broken app.
   */
  const runDrop = async (plan: DropPlan) => {
    switch (plan.verb) {
      case "none":
        return;
      case "refused":
        setProblem(plan.reason);
        return;
      case "move": {
        let moved = 0;
        for (const { from, to } of moveTargets(plan)) {
          try {
            // `replace: false`, as everywhere else: a move that silently
            // overwrote a file is a destructive act nobody asked for, and the
            // server refuses it atomically rather than this checking first.
            await backend.rename(from, to, false, plan.mount);
            moved += 1;
          } catch (error) {
            setProblem(engineFailure(error).message);
            break;
          }
        }
        refresh.request();
        setSelection(EMPTY_SELECTION);
        if (moved > 0) {
          setNotice(
            moved === 1
              ? `Moved ${plan.items[0]?.name ?? "1 item"}`
              : `Moved ${moved} items`,
          );
        }
        return;
      }
      case "copy": {
        const upload = plan.direction === "upload";
        // The *remote* side of the transfer: the share it is going to for an
        // upload, the one it comes from for a download. That is the mount the
        // queue row records, so the worker reads or writes the share the drop
        // named rather than whichever one the app most recently touched.
        await startTransfer(
          plan.sources,
          plan.destination,
          upload,
          upload ? plan.destMount : plan.sourceMount,
        );
      }
    }
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

  /**
   * The window's own rules about which entries are visible, and in what order.
   *
   * Shared because the column view lists directories this window is not "in",
   * and a column that sorted differently, or showed dotfiles the list was
   * hiding, would be describing a different folder from the one beside it.
   */
  const visibleOf = (list: Entry[], text: string) => {
    const matching = filterEntries(list, text);
    const visible = showHidden() ? matching : matching.filter((entry) => !entry.hidden);
    // A row waiting out its undo window is already gone as far as the user is
    // concerned. It is hidden rather than deleted, which is what makes Undo
    // able to do anything at all.
    const doomed = new Set(pendingDelete()?.paths ?? []);
    const remaining = doomed.size === 0 ? visible : visible.filter((e) => !doomed.has(e.path));
    return sortEntries(remaining, sort(), foldersFirst());
  };

  /**
   * The transfers landing in one pane's directory right now (XTP-E3-S3).
   *
   * `direction` decides which space `destPath` is in, because the two commands
   * disagree on purpose: an upload's destination is share-relative on its
   * `mount`, and a download's is a local absolute folder — where `mount` names
   * where the *sources* were, not where they are going.
   */
  const landingIn = (target: Pane): Transfer[] => {
    const parentOf = (candidate: string) => {
      const cut = candidate.lastIndexOf("/");
      return cut === -1 ? "" : candidate.slice(0, cut);
    };
    const mine = mountIdOf(target);
    return transfers().filter((transfer) => {
      if (transfer.status !== "running" && transfer.status !== "queued") return false;
      const parent = parentOf(transfer.destPath);
      if (transfer.direction === "up") {
        return (transfer.mount ?? null) === mine && parent === target.path();
      }
      return isLocalPane(target) && parent === absoluteIn(target, target.path());
    });
  };

  /**
   * A row for a file that is on its way but not there yet.
   *
   * Size is the bytes that have arrived, which is the one number that is true
   * of a partial file; there is no mtime because it has not been written yet,
   * and claiming one would be inventing it.
   */
  const inFlightRows = (target: Pane): Entry[] =>
    landingIn(target).map((transfer) => ({
      name: transfer.name,
      path: target.path() === "" ? transfer.name : `${target.path()}/${transfer.name}`,
      kind: "file" as const,
      size: transfer.bytesDone,
      modifiedMs: null,
      addedMs: null,
      mode: null,
      linkTarget: null,
      kindLabel: "Transferring",
      kindCategory: "other" as const,
      hidden: false,
    }));

  /** What one pane draws: its own listing and filter, the window's own rules. */
  const listedIn = (target: Pane) => {
    const data = target.listing();
    return data ? visibleOf(data.entries, target.filter()) : [];
  };

  /**
   * The arriving rows a pane still needs to draw itself.
   *
   * A file the server has already published is a real row: it stops being
   * in-flight the moment the listing knows about it, which is what makes the
   * change to an ordinary row need no relist. Both the merge below and the
   * marker the list styles by read *this*, so a row cannot be drawn while the
   * other says it is finished.
   */
  const pendingIn = (target: Pane, listed = listedIn(target)) => {
    const known = new Set(listed.map((entry) => entry.name));
    return inFlightRows(target).filter((entry) => !known.has(entry.name));
  };

  const entriesOf = (target: Pane) => {
    const listed = listedIn(target);
    const pending = pendingIn(target, listed);
    return pending.length === 0 ? listed : [...listed, ...pending];
  };

  /** Which of a pane's rows are still arriving, for the list to draw them so. */
  const inFlightPaths = (target: Pane) =>
    new Set(pendingIn(target).map((entry) => entry.path));
  const entries = createMemo(() => entriesOf(focused()));

  /**
   * How many rows there would be with no filter.
   *
   * Counted with the same `showHidden` rule as the visible set, so the two
   * numbers describe one population — "3 of 13" must not compare a filtered
   * count against a total that includes dotfiles the list is not showing.
   */
  const totalIn = (target: Pane) => {
    const data = target.listing();
    if (!data) return 0;
    return showHidden() ? data.entries.length : data.entries.filter((e) => !e.hidden).length;
  };

  /**
   * The selection as it is on screen.
   *
   * A row that a filter has hidden is not part of what the status bar
   * describes or what a toolbar verb acts on: "1 selected" beside a list that
   * visibly contains no selected row is a claim the user cannot check, and
   * Download would then act on a file they cannot see. The underlying set is
   * left alone, so clearing the filter brings the selection back.
   */
  const selectedVisibleOf = (target: Pane) =>
    entriesOf(target).filter((entry) => target.selected().has(entry.path));
  const selectedBytesOf = (target: Pane) =>
    selectedVisibleOf(target).reduce((total, entry) => total + entry.size, 0);
  /** A pane's own crumbs, which is where two panes on one share differ. */
  const mountPathOf = (target: Pane) =>
    ["", ...rootSegments(), ...nav.segments(target.path())].join("/");

  const selectedVisible = createMemo(() =>
    entries().filter((entry) => selected().has(entry.path)),
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
    // Demo chrome, for the browser preview only — the same rule
    // `defaultBookmarkStore` uses for its demo rows. The preview has no engine
    // behind it, so without this the design's screens have no share to name.
    //
    // Gated, because this used to be the plain fallback for "a listing with no
    // bookmark behind it" and there are real windows in that state: they
    // titled themselves after a share that does not exist and drew
    // `smb://nas.local/media` in the path field. In the app, a window that
    // cannot name its mount says nothing — `null` here is already drawn as an
    // empty path field and an unnamed title bar (E1-S6).
    if (inTauri()) return null;
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

  /**
   * The window's own name, kept in step with the title bar it draws.
   *
   * `TitleBar` paints the title into the webview because the design centres it
   * over the toolbar, and the native one is hidden to make room. Nothing else
   * then tells the platform anything, so every window stayed called
   * "Excalibur" — and the macOS Window menu lists windows by exactly that
   * name. Two windows were two entries reading the same word.
   *
   * Falls back to the app's name rather than to something like "No share":
   * this string is read outside the app, in the Window menu and the app
   * switcher, where a sentence about this app's state is noise.
   */
  createEffect(() => {
    const named =
      share() !== null && host() !== null ? `${share()} — ${host()}` : "Excalibur";
    void backend.setWindowTitle(named);
  });
  const rootSegments = () => named()?.root ?? [];
  const segments = () => [...rootSegments(), ...nav.segments(path())];
  const mountPath = () => ["", ...segments()].join("/");
  /**
   * The whole location as one URI, which is what the path field's editor opens
   * on. Built from the same three parts the breadcrumbs draw, so what is shown
   * and what is edited cannot disagree.
   */
  const locationUri = () =>
    scheme() === null ? "" : `${scheme()}://${uriHost() ?? ""}${mountPath()}`;

  /**
   * A path or a URI typed into the path field (E1-S2).
   *
   * Three outcomes, because three things can be typed. A bare path moves inside
   * the mount already open. A `file://` folder needs no credentials, so it is
   * opened directly. Anything else goes to the connection dialog *prefilled*
   * rather than being connected here: a remote share may need a password or an
   * unknown host key trusted, and that conversation already exists there. The
   * dialog is one Return away, and the alternative is reimplementing both.
   */
  const goToLocation = (text: string) => {
    const parsed = parseLocation(text, home());
    if (parsed === null) return;
    if (parsed.kind === "invalid") {
      setProblem(parsed.reason);
      return;
    }
    if (parsed.kind === "navigate") {
      goTo(resolveGoto(parsed.path));
      return;
    }
    if (parsed.draft.protocol === "local") {
      void openMount(fromDraft(parsed.draft));
      return;
    }
    // A port cannot survive this route: `Bookmark` has no field for one, so
    // `fromDraft` would drop it and the mount would go to the protocol's
    // default. Connecting to the wrong port silently is worse than asking, so
    // a non-default port goes to the dialog, which has the field.
    const fallback = defaultPort(parsed.draft.protocol);
    if (parsed.draft.port !== "" && Number(parsed.draft.port) !== fallback) {
      setConnecting(parsed.draft);
      return;
    }
    // Otherwise try it. `openMount` reaches the keychain — the shell reads the
    // credential by `spec.id`, and `bookmarkId` is `protocol:host:share`, so a
    // URI typed for a share already saved finds the stored secret — and falls
    // back to the prefilled dialog by itself when the connection is refused.
    //
    // The saved bookmark is preferred over a fresh one when the URI adds no
    // username, so typing a bare `xsync://nas.local/media` cannot quietly drop
    // the username or the read-only preference configured in the dialog.
    //
    // Matched on normalised values rather than on the id: `xsync://freya.local`
    // names a host and no share, and the id of a share read back out of a URI
    // has lost its leading slash. Both used to miss, and a miss here is a
    // connection dialog for a share that would have opened by itself.
    const existing = matchBookmark(bookmarks(), parsed.draft);
    // A host with no share, and nothing saved that says which share is meant:
    // there is no location here to open. Mounting the empty share would be a
    // guess, so the dialog gets it with the host already filled in.
    if (existing === null && parsed.draft.share.trim() === "") {
      setConnecting(parsed.draft);
      return;
    }
    void openMount(
      existing && parsed.draft.username === "" ? existing : fromDraft(parsed.draft),
    );
  };

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
    if (entry.kind === "dir") {
      goTo(entry.path);
      return;
    }
    // With two panes, direction is spatial and a double-click means the
    // obvious one: send it to the other side *(handoff §Two-pane specifics)*.
    // With one pane there is no other side, and opening a file is Quick Look's
    // and Edit's job, so this stays what it was.
    if (drawnPanes().length < 2) return;
    // The click that precedes the double-click has already selected the row,
    // so the transfer is of that entry.
    void runPaneTransfer(focused(), panes[focus() === "left" ? "right" : "left"]);
  };

  /** The visible paths, in display order — what a range or a band spans. */
  const order = createMemo(() => entries().map((entry) => entry.path));

  const select = (path: string, modifiers: { shift?: boolean; toggle?: boolean }) => {
    setSelection((current) => clickSelect(current, order(), path, modifiers));
    setReveal(null);
  };

  /**
   * The rubber band's result.
   *
   * The list hands over paths rather than an index range: a band over the icon
   * grid covers a rectangle, and the cells in a rectangle are not contiguous
   * in index order, so there is no range to slice. The list already holds the
   * entries in the order it drew them, so it is the one place that can answer.
   */
  const selectBand = (paths: string[], additive: boolean) => {
    setSelection((current) => band(current, paths, additive));
  };

  /** Browser URL pasteboards complement Tauri's native path drop (XDL-E8-S1). */
  const browserDropUrls = (event: DragEvent): string[] => {
    const transfer = event.dataTransfer;
    if (!transfer) return [];
    return validDownloadDrop(
      transfer.getData("text/uri-list") || transfer.getData("text/plain"),
    );
  };

  /**
   * One pane's column: the list and its status bar (XTP-E2-S1).
   *
   * A local component rather than one in `src/components/`, because it closes
   * over the window's handlers — `select`, `open`, `runDrop` and two dozen
   * more — which all act on the *focused* pane. Passing them would mean thirty
   * props and a second copy of the question "which pane".
   *
   * It takes focus on the way in instead: a pointer down anywhere in the
   * column focuses it before the click reaches a row, so by the time a handler
   * runs the focused pane is the one that was clicked. That is what lets the
   * window's existing handlers work unchanged for two panes.
   */
  /**
   * A pane's mount facts, which travel with its listing.
   *
   * `list_dir` returns the mount alongside the entries precisely so the two
   * cannot drift, and that rule is what lets a pane answer for itself here
   * rather than asking the window, which now has two.
   */
  const mountOf = (target: Pane) => target.listing()?.mount;
  const bookmarkOf = (target: Pane) =>
    bookmarks().find((candidate) => candidate.id === mountIdOf(target));

  /**
   * A pane's mount as its own status bar should describe it (XTP-E2-S2).
   *
   * The listing is the authority on `writable` and capacity, because those
   * travel with it — which is the property that lets a pane answer for itself
   * rather than asking the window, which now has two. What is new is *whose*
   * status the event stream is talking about: `onMountChanged` and the banner
   * both follow the active mount, so they only overlay the pane holding it. A
   * reconnect on the left share reddening the right pane's dot would be the
   * drift this story exists to remove.
   */
  const mountFactsOf = (target: Pane): MountInfo | undefined => {
    const current = mountOf(target);
    if (!current) return undefined;
    if (mountIdOf(target) !== activeMountId()) return current;
    const event = mountEvent();
    // A banner and a green health dot at the same time would be the window
    // disagreeing with itself, so the dot follows whichever is worse.
    const status = event?.status ?? (problem() === null ? current.status : "error");
    return { ...current, status };
  };
  const isLocalPane = (target: Pane) => bookmarkOf(target)?.protocol === "local";
  /** A local mount's `share` is its absolute root, so a row's path hangs off it. */
  const absoluteIn = (target: Pane, relative: string) => {
    const root = bookmarkOf(target)?.share ?? "";
    return relative === "" ? root : `${root}/${relative}`;
  };

  /**
   * Whether one pane can send its selection to the other, and why not (XTP-E3-S1).
   *
   * The transfer surface is share↔local: `enqueueUploads` takes local paths and
   * a share destination, `enqueueDownloads` the reverse. So one side has to be
   * a local mount. Two remote shares is a real thing to want and it is not
   * built — the engine would have to relay, which is E22-S3 and blocked on
   * xsync having no server-side copy — so it is refused with that reason
   * rather than silently doing nothing.
   */
  const transferGate = (from: Pane, to: Pane) => {
    const chosen = selectedVisibleOf(from);
    if (chosen.length === 0) return { ready: false, reason: "Nothing selected" };
    const target = mountOf(to);
    if (!target) return { ready: false, reason: "The other pane has no share" };
    // The same `writable` the status badge reads, so a blocked write is never
    // attempted rather than attempted and refused.
    if (!target.writable) return { ready: false, reason: target.reason };
    if (!isLocalPane(from) && !isLocalPane(to)) {
      return {
        ready: false,
        reason: "Copying between two remote shares is not built yet",
      };
    }
    return { ready: true, reason: null };
  };

  const runPaneTransfer = async (from: Pane, to: Pane) => {
    if (!transferGate(from, to).ready) return;
    const chosen = selectedVisibleOf(from);
    if (isLocalPane(from)) {
      // Local paths into a share folder: an upload, whichever pane it is in.
      await startTransfer(
        chosen.map((entry) => absoluteIn(from, entry.path)),
        to.path(),
        true,
        to.mountId() ?? undefined,
      );
      return;
    }
    // Share paths into a local folder: a download.
    await startTransfer(
      chosen.map((entry) => entry.path),
      absoluteIn(to, to.path()),
      false,
      from.mountId() ?? undefined,
    );
  };

  /** One arrow in the gutter. */
  const TransferArrow = (arrow: { from: PaneSide; to: PaneSide; glyph: string }) => {
    const gate = () => transferGate(panes[arrow.from], panes[arrow.to]);
    return (
      <button
        class={`${styles.gutterArrow} ${gate().ready ? styles.primary : ""}`}
        disabled={!gate().ready}
        aria-label={arrow.from === "left" ? "Send to the right pane" : "Send to the left pane"}
        title={gate().reason ?? undefined}
        // The gutter is also the splitter; without this a press on an arrow
        // would start a drag as well as a transfer.
        onPointerDown={(event) => event.stopPropagation()}
        onClick={() => void runPaneTransfer(panes[arrow.from], panes[arrow.to])}
      >
        {arrow.glyph}
      </button>
    );
  };

  const PaneColumn = (side: { pane: Pane; side: PaneSide }) => (
    <div
      class={`${styles.paneColumn} ${
        drawnPanes().length > 1 && focus() === side.side ? styles.paneColumnFocused : ""
      }`}
      // Hit-tested by the native drop target, which has a point and no element
      // (E24-S2): a drop belongs to the pane it landed on, not to the focused
      // one, because dragging somewhere is how you say where it goes.
      data-pane={side.side}
      onPointerDown={() => focusPane(side.side)}
    >
      {/* The band names what the pane holds. The handoff says LOCAL SITE and
          REMOTE SITE; either pane here can hold either, so it says the side and
          the share — and nothing at all with one pane, where there is no second
          column to tell it apart from. */}
      <Show when={drawnPanes().length > 1}>
        <div class={styles.paneTitle}>
          <span>{side.side === "left" ? "Left" : "Right"}</span>
          <span class={styles.paneTitleUri}>{mountPathOf(side.pane)}</span>
        </div>
      </Show>
      {/* The folder tree, when the pane's band is open (XTP-E2-S4). Above the
          list and below the title, where the handoff puts it. */}
      <Show when={treeHeightOf(side.side) > 0}>
        <DirectoryTree
          rows={side.pane.tree.rows(showHidden())}
          current={side.pane.path()}
          height={treeHeightOf(side.side)}
          label={`${side.side === "left" ? "Left" : "Right"} pane folders`}
          onReveal={(target) => side.pane.tree.reveal(target)}
          onNavigate={(target) => side.pane.goTo(target)}
          onToggle={(target) => side.pane.tree.toggle(target)}
          onResize={(height) => resizeTree(side.side, height)}
          onResizeEnd={saveTreeBand}
        />
      </Show>
          <div class={`${styles.listColumn} ${problem() ? styles.listDimmed : ""}`}>
                {/* Nothing mounted is not an empty directory: there is no list to
                    draw, and the one thing to offer is the way out of it (E1-S6). */}
                <Show
                  when={!side.pane.disconnected()}
                  fallback={<EmptyState onNewConnection={() => setConnecting(true)} />}
                >
                  {/* Column view is the one layout that is not this directory
                      drawn differently — it draws the ancestors too, each its
                      own listing — so it is a component beside `FileList`
                      rather than a fifth arm inside it (E26-S4). */}
                  <Show
                    when={settings().rowLayout !== "column-view"}
                    fallback={
                      <ColumnView
                        path={side.pane.path()}
                        listDir={(target) =>
                          backend.listDir(target, mountIdOf(side.pane) ?? undefined)
                        }
                        prepare={(list) => visibleOf(list, side.pane.filter())}
                        selected={side.pane.selected}
                        onSelect={select}
                        onNavigate={goTo}
                        onOpen={open}
                      />
                    }
                  >
                  <FileList
                    layout={settings().rowLayout}
                    entries={entriesOf(side.pane)}
                  inFlight={(path) => inFlightPaths(side.pane).has(path)}
                    selected={side.pane.selected}
                    onSelect={select}
                    onBand={selectBand}
                    reveal={reveal()}
                    // A page is the list's viewport in the list's row height, so the
                    // list is what counts it (E26-S2).
                    onViewportRows={setPageRows}
                    onOpen={open}
                    onContextMenu={openContextMenu}
                    // Undefined at the share root, which is what withholds the `..`
                    // row — read from the same `canAscend` as the toolbar's arrow.
                    // The *pane's* path: read from the window, a left pane one
                    // folder deep put a `..` row on a right pane sitting at its
                    // share root, and clicking it moved the left pane.
                    onAscend={
                      nav.canAscend(side.pane.path())
                        ? () => side.pane.ascend()
                        : undefined
                    }
                    slow={side.pane.listingState.slow()}
                    sort={sort()}
                    onSort={onSort}
                    widths={widths()}
                    onResize={onResize}
                    onResizeEnd={onResizeEnd}
                    renaming={renaming()}
                    dropTarget={dropTarget()}
                    onDragOut={(entry) => void dragOut(entry)}
                    onRename={(from, name) => void commitRename(from, name)}
                  />
                  </Show>
                </Show>
                {/* Every number here is the pane's own, so two panes on two
                    shares cannot come to describe each other (XTP-E2-S2). */}
                <StatusBar
                  mount={mountFactsOf(side.pane)}
                  local={isLocalPane(side.pane)}
                  path={mountPathOf(side.pane)}
                  items={entriesOf(side.pane).length}
                  total={totalIn(side.pane)}
                  selected={selectedVisibleOf(side.pane).length}
                  remounting={remounting()}
                  onRemount={remountActionFor(side.pane)}
                  selectedBytes={selectedBytesOf(side.pane)}
                />
          </div>
    </div>
  );

  /**
   * The window's body: one pane, or two with a gutter between them.
   *
   * The gutter is the handoff's 44px, and dragging it moves the split. It is
   * empty until XTP-E3 puts the transfer arrows in it.
   */
  /**
   * The transfer queue, in whichever shape this window is wearing.
   *
   * One call site for both, so the sheet and the drawer cannot come to be
   * handed different queues, different handlers or a different category —
   * only the chrome differs, which is the whole claim of XTP-E4-S1.
   */
  const TransfersSurface = (shape: { docked: boolean }) => (
    <Transfers
      docked={shape.docked}
      height={drawerHeight()}
      onResizeHeight={resizeDrawer}
      onResizeHeightEnd={saveDrawer}
      onToggleCollapse={toggleDrawer}
      transfers={transfers()}
      log={log()}
      view={logView()}
      onView={setLogView}
      verbose={verbose()}
      onVerbose={setVerbose}
      onClearLog={clearLog}
      onLogAction={runLogAction}
      categories={categories()}
      category={category()}
      onCategory={openCategory}
      size={paneSize()}
      onResize={setPaneSize}
      onResizeEnd={onPaneResizeEnd}
      onClose={() => setShowTransfers(false)}
      onPauseAll={() => void backend.pauseAllTransfers().then(refreshTransfers)}
      onClearFinished={() => void backend.clearFinishedTransfers().then(refreshTransfers)}
      onRetryFailed={() => void backend.retryFailedTransfers().then(refreshTransfers)}
      onClearFailed={() => void backend.clearFailedTransfers().then(refreshTransfers)}
      onPause={(id) => void backend.pauseTransfer(id).then(refreshTransfers)}
      onCancel={(id) => void backend.cancelTransfer(id).then(refreshTransfers)}
      onRemove={(id) => void backend.removeTransfer(id).then(refreshTransfers)}
      onRetry={(id) => void backend.retryTransfer(id).then(refreshTransfers)}
      onOpenFolder={(path) => void backend.openTransferFolder(path)}
      onCopyUrl={(url) => void navigator.clipboard?.writeText(url)}
      onRemount={remountAction()}
      queues={queues()}
      onCreateQueue={(name, color, concurrency) => void createQueue(name, color, concurrency)}
      onUpdateQueue={(id, name, color, concurrency) => void updateQueue(id, name, color, concurrency)}
      onDeleteQueue={(id) => void deleteQueue(id)}
      onMoveToQueue={(id, queueId) => void moveTransferToQueue(id, queueId)}
      onReorderQueue={(queueId, ids) => void reorderQueue(queueId, ids)}
      onReorderQueues={(ids) => void reorderQueues(ids)}
    />
  );

  const PaneRow = () => (
    <Show
      when={drawnPanes().length > 1}
      fallback={<PaneColumn pane={panes.left} side="left" />}
    >
      <div class={styles.paneRow} ref={paneRow}>
        <div
          class={styles.paneSlot}
          style={{ "flex-grow": `${splitRatio()}`, "flex-basis": "0" }}
        >
          <PaneColumn pane={panes.left} side="left" />
        </div>
        <div
          class={styles.paneGutter}
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize panes"
          onPointerDown={startSplitDrag}
        >
          <TransferArrow from="left" to="right" glyph="▶" />
          <TransferArrow from="right" to="left" glyph="◀" />
        </div>
        <div
          class={styles.paneSlot}
          style={{ "flex-grow": `${1 - splitRatio()}`, "flex-basis": "0" }}
        >
          <PaneColumn pane={panes.right} side="right" />
        </div>
      </div>
    </Show>
  );


  return (
    <div
      class={styles.window}
      onDragOver={(event) => {
        const types = [...(event.dataTransfer?.types ?? [])];
        if (!types.includes("text/uri-list") && !types.includes("text/plain")) return;
        event.preventDefault();
        event.stopPropagation();
        if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
      }}
      onDrop={(event) => {
        const urls = browserDropUrls(event);
        if (urls.length === 0) return;
        event.preventDefault();
        event.stopPropagation();
        void openAddDownload(urls);
      }}
    >
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
        onRefresh={() => listingState.load(path())}
        onCrumb={onCrumb}
        uri={locationUri()}
        onGo={goToLocation}
        filterRef={(element) => (filterInput = element)}
        onQuickLook={toggleQuickLook}
        onEdit={() => void editSelected()}
        onReconnect={reconnectAction()}
        reconnecting={remounting()}
        onDownload={() => void downloadSelected()}
        onUpload={() => void uploadHere()}
        viewItems={viewMenu({
          showHidden: showHidden(),
          foldersFirst: foldersFirst(),
          rowLayout: settings().rowLayout,
          paneMode: settings().paneMode,
          treeOpen: treeHeightOf(focus()) > 0,
        })}
        onViewAction={runViewAction}
        onAddDownload={() => void openAddDownload()}
        dual={drawnPanes().length > 1}
        onTransfers={() => setShowTransfers((open) => !open)}
        transfersOpen={showTransfers()}
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
            listDir={(at) => backend.listDir(at, mountIdOf(focused()) ?? undefined)}
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
            onReconnect={reconnectAction()}
            onDismiss={() => setProblem(null)}
          />
        )}
      </Show>
      <Show when={showTransfers() && !dockedTransfers()}>
        {TransfersSurface({ docked: false })}
      </Show>
      <div class={styles.body}>
        <Sidebar
          collapsed={settings().sidebarCollapsed}
          bookmarks={sidebarBookmarks()}
          // No mount is what the window shows on downloads, so none highlights
          // as active there — the session behind is untouched (XDL-E2-S1).
          activeId={bookmarks().find((bookmark) => bookmark.mounted)?.id ?? ""}
          selectedId={selectedBookmark()}
          dropBookmark={dropTarget()?.bookmark ?? null}
          favorites={shownFavorites()}
          userFolders={folders()}
          dropFavorites={dropTarget()?.favorites ?? false}
          onOpenFavorite={openFavorite}
          onFavoriteMenu={openFavoriteMenu}
          onBookmarkMenu={openBookmarkMenu}
          onNewConnection={() => setConnecting(true)}
          onSelect={setSelectedBookmark}
          onOpen={(id) => void mountBookmark(id)}
          onRemove={removeSelected}
          onEdit={(id) => {
            const bookmark = bookmarks().find((candidate) => candidate.id === id);
            if (bookmark) setConnecting(toDraft(bookmark));
          }}
        />
        {/* The panes and, below them, the drawer: docked to the bottom of the
            work area rather than the window, so the sidebar keeps its own
            footer and the drawer keeps the width it is describing. */}
        <div class={styles.workspace}>
          <PaneRow />
          <Show when={showTransfers() && dockedTransfers()}>
            {TransfersSurface({ docked: true })}
          </Show>
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
      <Show when={goingTo() !== null}>
        <GoToFolder
          initial={goingTo() ?? ""}
          shareName={share() ?? ""}
          listDir={(target) => backend.listDir(target, mountIdOf(focused()) ?? undefined)}
          onGo={(target) => {
            setGoingTo(null);
            goTo(target);
          }}
          onCancel={() => setGoingTo(null)}
        />
      </Show>
      <Show when={showPreferences()}>
        <Preferences
          settings={settings()}
          onChange={changeSetting}
          onClose={() => setShowPreferences(false)}
          // The browser preview has no folder picker, so the field stays
          // typeable and the button simply is not offered.
          onPickFolder={inTauri() ? () => void pickDownloadFolder() : undefined}
          downloadLimits={downloadLimits()}
          keychainAvailable={keychainAvailable()}
          onSaveHttpCredential={backend.saveHttpCredential}
          onForgetHttpCredential={backend.forgetHttpCredential}
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
      <Show when={addDownload.showing()}>
        <AddDownloadDialog
          folder={addDownload.destination()}
          initialUrls={addDownload.urls()}
          backend={backend}
          pickFolder={inTauri() ? pickShareFolder : undefined}
          busy={addDownload.busy()}
          onCancel={addDownload.close}
          onAdd={(urls, folder, options) => void addDownload.submit(urls, folder, options)}
        />
      </Show>
    </div>
  );
}
