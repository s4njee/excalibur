import { For, Show, createSignal } from "solid-js";
import { ContextMenu } from "./ContextMenu";
import type { MenuItem } from "../state/menu";
import type { ViewMenuAction } from "../state/viewMenu";
import styles from "./shell.module.css";
import type { MountInfo } from "../state/types";
import { NO_SHARE_REASON, gate, type Affordance, type Context } from "../state/permissions";
import * as shortcuts from "../state/shortcuts";
/**
 * The verbs act on the selection; the write ones are disabled from
 * `mount.writable` alone.
 */
export function Toolbar(props: {
  /** Absent when nothing is mounted; every verb is then refused (E1-S6). */
  mount?: MountInfo;
  scheme: string | null;
  host: string | null;
  segments: string[];
  hasSelection: boolean;
  filter: string;
  onFilter: (value: string) => void;
  canBack: boolean;
  canForward: boolean;
  canAscend: boolean;
  onBack: () => void;
  onForward: () => void;
  onAscend: () => void;
  /** Relist the folder now. The same verb ⌘R runs, uncoalesced. */
  onRefresh: () => void;
  onCrumb: (depth: number) => void;
  /** The whole location as a URI, for the path field's editor. */
  uri: string;
  /** A path or URI typed into the path field (E1-S2). */
  onGo: (text: string) => void;
  onQuickLook?: () => void;
  onEdit?: () => void;
  onDownload?: () => void;
  onUpload?: () => void;
  /** The View menu's rows, and what choosing one means. */
  viewItems: MenuItem<ViewMenuAction>[];
  onViewAction: (action: ViewMenuAction) => void;
  /** Open the Add Download dialog (XDL-E3-S1). */
  onAddDownload: () => void;
  /** Open the transfers pane (⇧⌘T, and the View menu's Transfers item). */
  onTransfers: () => void;
  /** Whether the pane is open, so the button can read as pressed. */
  transfersOpen: boolean;
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  onPreferences: () => void;
  /** So ⌘F can put the caret here. */
  filterRef?: (element: HTMLInputElement) => void;
}) {
  const [viewMenuAt, setViewMenuAt] = createSignal<{ x: number; y: number } | null>(null);
  let viewButton!: HTMLButtonElement;

  // With no mount there is nothing writable and nothing selected, so every
  // verb comes back disabled from the same gate the mounted case uses — no
  // second rule for the empty window to disagree with (E1-S6).
  const context = (): Context => ({
    writable: props.mount?.writable ?? false,
    reason: props.mount?.reason ?? NO_SHARE_REASON,
    hasSelection: props.hasSelection,
  });

  return (
    <div class={styles.toolbar}>
      {/* The design says the sidebar is collapsible and that the flag persists
          (E1-S3), but not where the control lives. Here because the sidebar's
          own footer is three equal buttons at a specified size, and a fourth
          would break that. Recorded in docs/design-gaps.md. */}
      <button
        class={styles.iconButton}
        aria-label={props.sidebarCollapsed ? "Show sidebar" : "Hide sidebar"}
        aria-pressed={!props.sidebarCollapsed}
        onClick={props.onToggleSidebar}
      >
        ▚
      </button>
      <button
        class={styles.iconButton}
        aria-label="Preferences"
        title={shortcuts.tooltip("preferences")}
        onClick={props.onPreferences}
      >
        ⚙
      </button>
      {/* The View menu. Owned here rather than by the window because the menu
          needs the button as its `anchor` — without one the press that ought to
          close it is dismissed, then re-opened by the button's own click, and
          it could never be closed by pressing it again. */}
      <button
        class={styles.iconButton}
        ref={viewButton}
        aria-label="View options"
        aria-haspopup="menu"
        aria-expanded={viewMenuAt() !== null}
        onClick={() => {
          if (viewMenuAt() !== null) {
            setViewMenuAt(null);
            return;
          }
          // Under the button rather than at the pointer: opened from a fixed
          // control, so it should appear in the same place every time.
          const box = viewButton.getBoundingClientRect();
          setViewMenuAt({ x: box.left, y: box.bottom + 2 });
        }}
      >
        ☰
      </button>
      <Show when={viewMenuAt()}>
        {(at) => (
          <ContextMenu
            items={props.viewItems}
            at={at()}
            label="View options"
            anchor={viewButton}
            onChoose={(action) => {
              setViewMenuAt(null);
              props.onViewAction(action);
            }}
            onClose={() => setViewMenuAt(null)}
          />
        )}
      </Show>
      <div class={styles.segmented}>
        <button
          class={styles.segment}
          disabled={!props.canBack}
          aria-label="Back"
          title={shortcuts.tooltip("back")}
          onClick={props.onBack}
        >
          ‹
        </button>
        <button
          class={styles.segment}
          disabled={!props.canForward}
          aria-label="Forward"
          title={shortcuts.tooltip("forward")}
          onClick={props.onForward}
        >
          ›
        </button>
      </div>
      <button
        class={styles.button}
        disabled={!props.canAscend}
        aria-label="Enclosing folder"
        title={shortcuts.tooltip("ascend")}
        onClick={props.onAscend}
      >
        ↑
      </button>
      {/* ⌘R has existed since E16-S1 with nothing on screen to say so, and a
          listing can go stale for reasons this app never hears about — another
          machine writing to the share, a transfer landing somewhere it is not
          watching. Disabled from `mount` rather than a `can` prop: relisting
          is not a permission, it is only meaningless with nothing mounted. */}
      <button
        class={styles.button}
        disabled={props.mount === undefined}
        aria-label="Refresh"
        title={shortcuts.tooltip("refresh")}
        onClick={props.onRefresh}
      >
        ⟳
      </button>
      {/* An empty field rather than `://` over a blank crumb: with nothing
          mounted there is no path, and half a URI would read as one that
          failed to load (E1-S6). */}
      <Show
        when={props.scheme !== null && props.host !== null}
        fallback={<div class={styles.pathField} />}
      >
        <PathField
          scheme={props.scheme ?? ""}
          host={props.host ?? ""}
          segments={props.segments}
          onCrumb={props.onCrumb}
          uri={props.uri}
          onGo={props.onGo}
        />
      </Show>
      <div class={styles.verbs}>
          {/* Every verb goes through `gate`. None of them reads `writable`
              directly: an inline condition here is exactly how a write affordance
              ends up gated on something that is not the mount. */}
          <Verb
            affordance="quickLook"
            context={context()}
            label="Quick Look"
            shortcut="quickLook"
            onClick={props.onQuickLook}
          />
          <Verb
            affordance="edit"
            context={context()}
            label="Edit"
            shortcut="edit"
            onClick={props.onEdit}
          />
          <Verb
            affordance="download"
            context={context()}
            label="Download"
            shortcut="download"
            onClick={props.onDownload}
          />
          <Verb
            affordance="upload"
            context={context()}
            label="Upload"
            shortcut="upload"
            primary
            onClick={props.onUpload}
          />
      </div>
      {/* A filename is not prose. WKWebView capitalises, autocorrects and
          spell-checks a text input by default, and a filter that quietly
          became `Arrival` after `arrival` matches nothing (E3-S5). */}
      <input
        ref={props.filterRef}
        class={styles.filter}
        placeholder="Filter"
        title={shortcuts.tooltip("focusFilter")}
        value={props.filter}
        autocapitalize="none"
        autocorrect="off"
        autocomplete="off"
        spellcheck={false}
        onInput={(event) => props.onFilter(event.currentTarget.value)}
      />
      {/* The queue's two controls, grouped at the end: one adds to it, one
          shows it. Add URL is where the reference puts it — it briefly lived
          in the pane's own header, which meant the only route to the HTTP
          downloader was to open the pane first and notice a button there. */}
      <button
        class={`${styles.button} ${styles.primary}`}
        aria-label="Add URL"
        onClick={props.onAddDownload}
      >
        ＋ Add URL
      </button>
      {/* The queue's own control. It opens the pane rather than switching the
          window onto it: the window always browses now, and the transfers
          surface is the sheet. `aria-pressed` because it toggles a thing that
          stays on screen, which is not what a plain action button means. */}
      <button
        class={styles.button}
        aria-label="Transfers"
        aria-pressed={props.transfersOpen}
        title={shortcuts.tooltip("transfers")}
        onClick={props.onTransfers}
      >
        Transfers
      </button>
    </div>
  );
}
/**
 * The path field, rendered as clickable breadcrumb segments.
 *
 * Segments rather than a text field by default: the design wants an ancestor
 * to be one click away, and only reveals an editable field when the user
 * clicks the empty space (not yet wired — E1-S2 finishes it).
 */
function PathField(props: {
  scheme: string;
  /** Empty for a `file:` mount, which has no authority. */
  host: string;
  segments: string[];
  onCrumb: (depth: number) => void;
  /** The whole location as a URI, which is what the editor starts from. */
  uri: string;
  /** A typed path or URI (E1-S2). Parsed by `state/location.ts`. */
  onGo: (text: string) => void;
}) {
  const [editing, setEditing] = createSignal(false);
  let input: HTMLInputElement | undefined;

  const edit = (event: MouseEvent) => {
    // Only the field's own empty space. A click that landed on a crumb is a
    // navigation, and turning it into an edit would make the breadcrumbs
    // unusable — which is the trap this whole field is trying to avoid.
    if (event.target !== event.currentTarget) return;
    setEditing(true);
  };

  const commit = () => {
    const text = input?.value ?? "";
    setEditing(false);
    props.onGo(text);
  };

  return (
    <Show
      when={!editing()}
      fallback={
        <input
          ref={(element) => {
            input = element;
            // Selected, not just focused: the field opens on where you already
            // are, and the common case is replacing it rather than editing it.
            queueMicrotask(() => {
              element.focus();
              element.select();
            });
          }}
          class={styles.pathInput}
          value={props.uri}
          aria-label="Location"
          spellcheck={false}
          autocapitalize="none"
          autocorrect="off"
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commit();
            } else if (event.key === "Escape") {
              // Stopped here so the window's own Escape — which clears the
              // filter, then the selection — does not also fire.
              event.preventDefault();
              event.stopPropagation();
              setEditing(false);
            }
          }}
          // Leaving without pressing Enter abandons the edit. Committing on
          // blur would connect to whatever was half-typed when the user
          // clicked away.
          onBlur={() => setEditing(false)}
        />
      }
    >
    <div class={styles.pathField} onClick={edit} title="Click to type a path or URI">
      <span class={styles.crumbScheme}>{props.scheme}://</span>
      {/* The host is the share root: depth 0. A local mount has none — the
          field reads `file:///Users/you/Documents` — and an empty crumb button
          would be a zero-width click target sitting between `//` and the first
          segment. The segments that name the mount are still clickable and
          still go to the share root (`onCrumb`), so nothing is lost with it. */}
      <Show when={props.host !== ""}>
        <button class={styles.crumb} onClick={() => props.onCrumb(0)}>
          {props.host}
        </button>
      </Show>
      <For each={props.segments}>
        {(segment, index) => {
          const last = () => index() === props.segments.length - 1;
          return (
            <>
              <span class={styles.crumbSeparator}>/</span>
              {/* The last crumb is where we already are. Rendering it as a
                  button would offer a click that does nothing. */}
              <Show
                when={!last()}
                fallback={<span class={styles.crumbCurrent}>{segment}</span>}
              >
                <button class={styles.crumb} onClick={() => props.onCrumb(index() + 1)}>
                  {segment}
                </button>
              </Show>
            </>
          );
        }}
      </For>
    </div>
    </Show>
  );
}
/**
 * One toolbar verb, drawn from the permission gate.
 *
 * The tooltip is the server's own reason, verbatim. Paraphrasing it loses the
 * detail that tells a user whether it is their export or their credentials —
 * "Read-only — export mapped `ro,root_squash`" is actionable; "not allowed" is
 * not.
 */
function Verb(props: {
  affordance: Affordance;
  context: Context;
  label: string;
  primary?: boolean;
  onClick?: () => void;
  /** Named from the same table that binds the key (E16-S1). */
  shortcut?: shortcuts.ShortcutId;
}) {
  const state = () => gate(props.affordance, props.context);
  // A disabled button's tooltip owes the user the reason it is disabled;
  // teaching a shortcut for something they cannot do would be worse than
  // saying nothing. So the refusal wins, and the hint fills the rest.
  const title = () =>
    state().reason ?? (props.shortcut ? shortcuts.tooltip(props.shortcut) : undefined);
  return (
    <button
      class={`${styles.button} ${props.primary ? styles.primary : ""}`}
      disabled={state().disabled}
      title={title()}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}
