// What *this* window is, as opposed to what the app is (E23-S1).
//
// The app can show several browser windows over one engine. Sessions,
// bookmarks, the keychain and the transfer queue are the app's; the mount, the
// path, the history, the selection, the filter and the row layout are the
// window's. Everything in `src/state` below this file is already per-window by
// construction — each window is its own webview running its own `App` — so the
// only thing that needs saying out loud is which window this one *is*, and
// what it was opened on.
//
// It comes in the URL rather than over IPC because both answers are needed
// before the first command: the label goes into every `excalibur-stream://`
// URL, and the mount decides which bookmark the sidebar draws as mounted. A
// round trip for either would mean a frame of the empty state in a window that
// is not empty.

/** How a browser window was opened. */
export interface WindowParams {
  /**
   * The Tauri window label, which is the name the engine keys this window's
   * mount by. `main` is the window `tauri.conf.json` creates; every other one
   * is named by `new_window`.
   */
  label: string;
  /**
   * The mount this window opened on, when it was opened onto one.
   *
   * `null` for the first window and for a browser preview, both of which start
   * with nothing mounted. It is *not* read from the bookmark store: a `mounted`
   * flag on disk is from a previous run and describes a share the shell is not
   * holding (E1-S6), so the only trustworthy source is the shell that just set
   * it.
   */
  mount: string | null;
  /** The folder to open in. Empty is the share root. */
  path: string;
}

/** The first window, and what a browser preview is. */
export const FIRST_WINDOW: WindowParams = { label: "main", mount: null, path: "" };

/**
 * Read a window's parameters out of its query string.
 *
 * Anything missing falls back to the first window's answer, so the main
 * window — which is created from the config and carries no query at all — and
 * the browser preview both come out of here without a special case.
 */
export function parseWindowParams(search: string): WindowParams {
  const parameters = new URLSearchParams(search);
  const label = parameters.get("label");
  const mount = parameters.get("mount");
  return {
    label: label === null || label === "" ? FIRST_WINDOW.label : label,
    // An empty `mount` is "no mount", not a share whose id is the empty
    // string: the query is built by formatting, and a formatter that had
    // nothing to say leaves the value empty rather than the key out.
    mount: mount === null || mount === "" ? null : mount,
    path: parameters.get("path") ?? FIRST_WINDOW.path,
  };
}

let current: WindowParams | null = null;

/**
 * This window's parameters, read once.
 *
 * Cached because the label is read on every preview URL and the query string
 * cannot change under a window — Tauri gives each window its own URL at
 * creation and nothing here ever navigates.
 */
export function windowParams(): WindowParams {
  if (current) return current;
  current =
    typeof window === "undefined"
      ? FIRST_WINDOW
      : parseWindowParams(window.location.search);
  return current;
}

/** This window's label, for the URLs and commands that have to name it. */
export function windowLabel(): string {
  return windowParams().label;
}
