// The sidebar's small pictures.
//
// **Drawn here rather than taken from the system.** Finder's sidebar uses SF
// Symbols, and Apple's licence does not permit shipping those outside an Apple
// framework — so these are our own drawings of the same shapes: a house, an app
// grid, a display, a page, a photo, a film frame. The goal is that a Mac user
// recognises the row without looking, not that the curves match Apple's.
//
// One line weight, `currentColor` throughout, and no fills, so a row inherits
// its colour from the theme and the active state rather than carrying a second
// palette that has to be kept in step with the first.

import { Switch, Match } from "solid-js";
import type { Glyph as Kind } from "../state/favorites";

/** One 16px glyph, coloured by whatever it is sitting in. */
export function Glyph(props: { kind: Kind; class?: string }) {
  return (
    <svg
      class={props.class}
      viewBox="0 0 16 16"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.25"
      stroke-linecap="round"
      stroke-linejoin="round"
      // Decorative: the row's name is already the accessible label, and a
      // second reading of "Documents" helps nobody using a screen reader.
      aria-hidden="true"
      // Which picture this is, for a test to read. The shapes are paths with
      // no text in them, so without this the only assertable fact would be
      // "an svg is present" — which stays true when every row draws the same
      // one, the exact regression worth catching.
      data-glyph={props.kind}
    >
      <Switch fallback={<FolderGlyph />}>
        <Match when={props.kind === "home"}>
          <path d="M2.5 7.6 8 3.2l5.5 4.4" />
          <path d="M4.2 7.4v5.4h7.6V7.4" />
        </Match>
        <Match when={props.kind === "applications"}>
          <rect x="2.7" y="2.7" width="4.5" height="4.5" rx="1.2" />
          <rect x="8.8" y="2.7" width="4.5" height="4.5" rx="1.2" />
          <rect x="2.7" y="8.8" width="4.5" height="4.5" rx="1.2" />
          <rect x="8.8" y="8.8" width="4.5" height="4.5" rx="1.2" />
        </Match>
        <Match when={props.kind === "desktop"}>
          <rect x="2.2" y="3.2" width="11.6" height="7.7" rx="1.3" />
          <path d="M8 10.9v2.3" />
          <path d="M6.1 13.2h3.8" />
        </Match>
        <Match when={props.kind === "documents"}>
          <path d="M4.2 2.7h4.9l2.9 2.9v7.7H4.2z" />
          <path d="M9.1 2.7v2.9h2.9" />
        </Match>
        <Match when={props.kind === "pictures"}>
          <rect x="2.4" y="3.4" width="11.2" height="9.2" rx="1.3" />
          <circle cx="5.9" cy="6.7" r="1.05" />
          <path d="M3.1 11.5 6.3 8.4l2.4 2.2 2.1-1.9 2.1 1.9" />
        </Match>
        <Match when={props.kind === "movies"}>
          <rect x="2.4" y="3.4" width="11.2" height="9.2" rx="1.3" />
          <path d="M5.6 3.4v9.2" />
          <path d="M10.4 3.4v9.2" />
        </Match>
      </Switch>
    </svg>
  );
}

/** Anything with no special meaning — including every folder a user adds. */
function FolderGlyph() {
  return (
    <path d="M2.5 5.2a1 1 0 0 1 1-1h3.2l1.4 1.5h4.4a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1z" />
  );
}
