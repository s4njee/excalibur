// What the toolbar's View menu offers (E17-S1, E8-S3/S4).
//
// *Asked for directly, 2026-09-09: "there should be a view hamburger style
// menu in the toolbar that lets you hide dotfiles or make the view icons
// instead of details".*
//
// A pure function over the settings, for the same reason `menu.ts` and
// `transferMenu.ts` are: what a menu offers, and which of its rows is the one
// you are on, is a rule — far cheaper to state and test here than by opening a
// menu and reading it. The toolbar draws these through the same `ContextMenu`
// the file list uses, so Escape, the arrow keys and the focus ring come free.
//
// Two things worth knowing about the rows:
//
//   1. **The tick is in the label.** `MenuItem` has no checked state, and
//      giving it one would touch every other menu for the sake of this one.
//      A leading tick reads correctly to a screen reader in a `menuitem` and
//      keeps the labels aligned because the space is always there.
//   2. **Icons is a fourth layout the design does not draw.** The handoff has
//      `1f`, `1g` and `1h` and none of them is a grid; this is E26-S4, built
//      2026-09-09 on a direct ask. Recorded in `docs/design-gaps.md`.

import type { MenuItem } from "./menu";
import type { PaneMode, RowLayout } from "./types";

export type ViewMenuAction =
  | "toggleHidden"
  | "toggleFoldersFirst"
  | "layoutColumns"
  | "layoutNameLed"
  | "layoutMono"
  | "layoutIcons"
  | "layoutColumnView"
  | "togglePanes";

/** The settings the menu reflects; a subset, so callers cannot pass the world. */
export interface ViewState {
  showHidden: boolean;
  foldersFirst: boolean;
  rowLayout: RowLayout;
  paneMode: PaneMode;
}

/** `✓ Label` or `  Label`, so the rows line up whether or not they are on. */
function tick(on: boolean, label: string): string {
  return `${on ? "✓" : " "} ${label}`;
}

export function viewMenu(view: ViewState): MenuItem<ViewMenuAction>[] {
  return [
    {
      action: "toggleHidden",
      // Named for the state, not the verb: a menu that says "Hide" when they
      // are hidden cannot be read without first knowing the answer.
      label: tick(view.showHidden, "Show Hidden Files"),
      disabled: false,
      reason: null,
    },
    {
      action: "toggleFoldersFirst",
      label: tick(view.foldersFirst, "Folders First"),
      disabled: false,
      reason: null,
    },
    {
      action: "layoutColumns",
      label: tick(view.rowLayout === "columns", "Columns"),
      disabled: false,
      reason: null,
      separated: true,
    },
    {
      action: "layoutNameLed",
      label: tick(view.rowLayout === "name-led", "Name-led"),
      disabled: false,
      reason: null,
    },
    {
      action: "layoutMono",
      label: tick(view.rowLayout === "mono", "Mono grid"),
      disabled: false,
      reason: null,
    },
    {
      action: "layoutIcons",
      label: tick(view.rowLayout === "icons", "Icons"),
      disabled: false,
      reason: null,
    },
    {
      action: "layoutColumnView",
      // Named for what it is rather than for `1f`, which is also "Columns":
      // one is a row with columns in it, the other is a folder per pane.
      label: tick(view.rowLayout === "column-view", "Column View"),
      disabled: false,
      reason: null,
    },
    {
      action: "togglePanes",
      // Ruled off from the layouts because it answers a different question:
      // how many panes, not how a listing is drawn. A pane still uses one of
      // the layouts above (`twopane.md` XTP-E7-S1).
      label: tick(view.paneMode === "dual", "Two Panes"),
      disabled: false,
      reason: null,
      separated: true,
    },
  ];
}
