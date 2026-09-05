// The context menu (E14-S2).
//
// It renders a list someone else decided on: `state/menu.ts` builds the items
// and `state/permissions.ts` decides which are refused, so this file is only
// about where the menu appears, how it is dismissed, and how a keyboard gets
// through it. That split is deliberate — the interesting rules are then
// testable without opening a menu, and this stays small enough to read.

import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import styles from "./contextmenu.module.css";
import type { MenuAction, MenuItem } from "../state/menu";

/** How close to an edge the menu may come before it flips. */
const MARGIN = 8;

export function ContextMenu(props: {
  items: MenuItem[];
  /** Where the pointer was, in viewport coordinates. */
  at: { x: number; y: number };
  onChoose: (action: MenuAction) => void;
  onClose: () => void;
}) {
  let menu!: HTMLDivElement;
  // Placed after measuring, so the menu never paints in the wrong corner
  // first: it is hidden for exactly one frame rather than jumping.
  const [box, setBox] = createSignal<{ left: number; top: number } | null>(null);
  /** Which item the keyboard is on. `-1` until an arrow is pressed. */
  const [active, setActive] = createSignal(-1);

  const enabled = () => props.items.filter((item) => !item.disabled);

  onMount(() => {
    const size = menu.getBoundingClientRect();
    // Flip rather than clamp. A menu clamped against the bottom edge sits
    // under the pointer, so the first item is whatever the release lands on;
    // flipped, it opens upward from the same corner and nothing is under the
    // cursor.
    const left =
      props.at.x + size.width + MARGIN > window.innerWidth
        ? Math.max(MARGIN, props.at.x - size.width)
        : props.at.x;
    const top =
      props.at.y + size.height + MARGIN > window.innerHeight
        ? Math.max(MARGIN, props.at.y - size.height)
        : props.at.y;
    setBox({ left, top });
    menu.focus();
  });

  onMount(() => {
    // Dismissal. `pointerdown` rather than `click`, so the menu is gone before
    // whatever is underneath reacts to the press — otherwise the click that
    // closes the menu also selects a row.
    const away = (event: PointerEvent) => {
      if (!menu.contains(event.target as Node)) props.onClose();
    };
    // Anything that moves the list out from under the menu closes it: a menu
    // pinned to viewport coordinates while the rows scroll past is pointing at
    // a different file than the one it was opened on.
    const gone = () => props.onClose();
    window.addEventListener("pointerdown", away, true);
    window.addEventListener("resize", gone);
    window.addEventListener("blur", gone);
    // Capture, because the list scrolls inside its own container and a scroll
    // event does not bubble.
    window.addEventListener("scroll", gone, true);
    onCleanup(() => {
      window.removeEventListener("pointerdown", away, true);
      window.removeEventListener("resize", gone);
      window.removeEventListener("blur", gone);
      window.removeEventListener("scroll", gone, true);
    });
  });

  // A refused item can never be the active one, including after the items
  // change underneath (a mount going read-only while the menu is open).
  createEffect(() => {
    const item = props.items[active()];
    if (item?.disabled) setActive(-1);
  });

  /** Move to the next selectable item, wrapping. */
  const step = (delta: number) => {
    const items = props.items;
    if (enabled().length === 0) return;
    let index = active();
    for (let tries = 0; tries < items.length; tries += 1) {
      index = (index + delta + items.length) % items.length;
      if (!items[index]?.disabled) {
        setActive(index);
        return;
      }
    }
  };

  const choose = (item: MenuItem) => {
    if (item.disabled) return;
    props.onChoose(item.action);
  };

  const onKey = (event: KeyboardEvent) => {
    // Nothing here reaches the window's own shortcut table: while a menu is
    // open, Delete means "the menu item", not "delete the selection".
    event.stopPropagation();
    switch (event.key) {
      case "Escape":
        event.preventDefault();
        props.onClose();
        return;
      case "ArrowDown":
        event.preventDefault();
        step(1);
        return;
      case "ArrowUp":
        event.preventDefault();
        step(-1);
        return;
      case "Home":
        event.preventDefault();
        setActive(-1);
        step(1);
        return;
      case "End":
        event.preventDefault();
        setActive(0);
        step(-1);
        return;
      case "Enter":
      case " ": {
        event.preventDefault();
        const item = props.items[active()];
        if (item) choose(item);
        return;
      }
      case "Tab":
        // A menu is modal enough that tabbing out of it is a dismissal, not a
        // way to reach the rest of the window through it.
        event.preventDefault();
        props.onClose();
        return;
      default:
    }
  };

  const id = (index: number) => `context-menu-item-${index}`;

  return (
    <div
      class={styles.menu}
      ref={menu}
      role="menu"
      tabindex={-1}
      aria-label="File actions"
      aria-activedescendant={active() >= 0 ? id(active()) : undefined}
      style={{
        left: `${box()?.left ?? props.at.x}px`,
        top: `${box()?.top ?? props.at.y}px`,
        visibility: box() ? "visible" : "hidden",
      }}
      onKeyDown={onKey}
      // The menu is opened by a right-click; a right-click *on* it should not
      // open a second one.
      onContextMenu={(event) => event.preventDefault()}
    >
      <For each={props.items}>
        {(item, index) => (
          <>
            <Show when={item.separated && index() > 0}>
              <div class={styles.separator} role="separator" />
            </Show>
            <div
              id={id(index())}
              class={`${styles.item} ${item.disabled ? styles.itemDisabled : ""} ${
                active() === index() ? styles.itemActive : ""
              }`}
              role="menuitem"
              aria-disabled={item.disabled}
              // The reason a verb is refused is the server's, and it is the
              // only place a user learns why — the same tooltip the toolbar's
              // disabled verbs carry.
              title={item.reason ?? undefined}
              onPointerEnter={() => setActive(item.disabled ? -1 : index())}
              onClick={() => choose(item)}
            >
              {item.label}
            </div>
          </>
        )}
      </For>
    </div>
  );
}
