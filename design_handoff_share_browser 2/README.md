# Handoff: NFS/Samba Share Browser

## Overview
Desktop file browser for NFS and SMB/CIFS shares, targeting Rust + Tauri with a SolidJS
frontend. The app ships **two browsing modes over one shared engine**, switchable by the
user and persisted:

- **Single pane** (Cyberduck-style, options `1a`/`1b`) — one remote file list at a time,
  bookmarks in a permanently visible left sidebar, transfers implicit via toolbar verbs.
  The default mode.
- **Two pane** (transfer-client style, options `2a`/`2b`) — local tree + list on the left,
  remote tree + list on the right, transfer arrows between them, and a docked drawer at
  the bottom holding the transfer queue and the protocol message log.

Both modes read the same mounts, listings, selection and transfer queue; only the layout
differs. The mount's effective permission (read-write vs read-only) is always visible and
always gates the write verbs — in single pane it is the first item in the one status bar,
in two pane it is on the remote pane's own status bar.

Core features covered: connect (SMB or NFS), browse, filter, Quick Look, Edit in place,
Download, Upload, and a transfer queue with failure recovery.

## About the Design Files
`Share Browser Mockups.dc.html` is a **design reference created in HTML** — a prototype
showing intended look, layout, and behavior. It is not production code to copy.

The task is to **recreate these designs in the target codebase** (SolidJS components +
Tauri commands in Rust) using that project's established patterns, state primitives, and
styling approach. The HTML uses inline styles purely because the prototype tooling
requires it — do not carry that pattern into the app; use whatever the codebase already
uses (CSS modules, vanilla-extract, plain CSS, etc.).

If no frontend scaffolding exists yet, follow the stated stack: Tauri v2 shell, SolidJS
frontend, Rust backend for the SMB/NFS clients and all filesystem I/O.

## Fidelity
**High-fidelity.** Colors, typography, spacing, and row metrics are final and should be
matched closely. Every hex value, font size, and pixel dimension in this document is
literal and taken from the mock. Iconography is the exception: the mock uses plain
rectangles as stand-ins for file/folder icons — substitute a real icon set (see Assets).

Form controls are also drawn rather than native (checkbox = 12px box, radio = 12px circle,
text fields = styled divs with sample values) because the prototype runtime strips
controlled-input state. Use real `<input>` elements in the app; match the drawn dimensions
and colors.

## Design Tokens

### Type
| Role | Value |
| --- | --- |
| UI sans | `"Helvetica Neue", Helvetica, Arial, sans-serif` |
| Mono | `"JetBrains Mono", ui-monospace, monospace` (Google Fonts, weights 400/500) |
| Body / row text | 12px / 400 |
| Row text (roomier variants) | 12.5px |
| Button + field text | 11.5px |
| Column headers | 10.5px / 600, color `#6a6a70` |
| Sidebar section headers | 9.5px / 600, `letter-spacing: .09em`, uppercase |
| Status bar | 11px; mono runs inside it 10–10.5px |
| Mono metadata (size, paths, protocol) | 9.5–11px |

Mono is used for anything the user might copy or compare character-by-character: sizes,
paths, URIs, protocol strings, mount options, transfer rates. Never for prose.

### Light theme
| Token | Hex |
| --- | --- |
| Title bar | `linear-gradient(#f7f7f8, #eeeef0)` |
| Toolbar bg | `#f4f4f5` |
| Sidebar bg | `#eeeef0` |
| List bg | `#ffffff` |
| Status bar bg | `#f7f7f8` |
| Chrome hairline | `#dcdcde` |
| Row hairline | `#f2f2f3` |
| Control border | `#c9c9cd` |
| Control bg | `#ffffff` |
| Text primary | `#1a1a1c` |
| Text secondary | `#6a6a70` |
| Text tertiary / disabled | `#8a8a90` |
| Placeholder | `#a0a0a6` |
| Folder glyph fill | `#8a8f98` |
| File glyph stroke | `#a8adb5` |
| Selection row bg | `#2f6df6`, text `#ffffff` |
| Soft selection bg (variant 1g) | `#e8effd`, text accent `#4a6ba8` |
| Sidebar active bg | `#dfe7fb`, 3px left border `#2f6df6` |

### Dark theme
| Token | Hex |
| --- | --- |
| Title bar | `#303135` |
| Toolbar bg | `#2a2b2e` |
| Sidebar bg | `#26272a` |
| List bg | `#1e1f21` |
| Field / inset bg | `#1c1d1f` |
| Control bg | `#35363a` |
| Control border | `#45464b` |
| Chrome hairline | `#1a1b1d` |
| Row hairline | `#26272a` |
| Text primary | `#e8e8ea` |
| Text secondary | `#9a9ba0` |
| Text tertiary | `#8a8b90` |
| Disabled text | `#63646a` |
| Disabled control bg / border | `#2e2f32` / `#3a3b3f` |
| Folder glyph fill | `#7d838c` |
| Sidebar active bg | `#1f3358`, left border `#4f86ff`, subtitle `#8fa8d8` |
| Accent (dark) | `#4f86ff` |

### Accent + semantic
| Token | Hex |
| --- | --- |
| Accent | `#2f6df6` |
| Accent hover/pressed | `#1d4fc4` |
| Success / connected | `#2f9e5f` |
| Error text | `#c0392b`, error chip bg `#fdeaea` |
| Accent chip bg (light) | `#e8effd` |
| Neutral chip bg (light) | `#f0f0f2` |
| RO badge (dark) | bg `#4a4b50`, text `#e8e8ea` |

### Geometry
- Border radius: 5px controls and fields, 4px small/inline buttons, 3px status chips,
  1.5–2px file glyphs, 2px progress bars, 6px segmented-control track, 7px window corner.
- Control heights: 24px standard toolbar/form control, 26px dialog action button,
  22px compact header button, 20px inline row button.
- Bars: title 34px, toolbar 44px, column header 24px, status 24px, transfer footer 24px.
- Sidebar width 206px, fixed (`flex: none`), 1px right border.
- Shadow: window `0 2px 10px rgba(0,0,0,.10)`; active segmented tab
  `0 1px 2px rgba(0,0,0,.12)`. Nothing else casts a shadow.
- Spacing: toolbar `padding: 0 10px`, `gap: 8px` between toolbar groups and `gap: 5px`
  between adjacent verb buttons; list rows `padding: 0 12px`; sidebar rows `padding: 0 12px`.

## Screens / Views

### 1. Browser — light, read-write mount (option `1a`, the primary mock)
Window 1060px wide; the mock's content area is 520px tall but should fill the real window.

**Purpose:** the app's home. Browse one directory of one mounted share, act on the selection.

**Layout:** four stacked rows — title bar (34px), toolbar (44px), body (flex: 1), and the
body splits into fixed 206px sidebar + flexible list column. The list column is itself a
vertical flex: column header (24px), scrollable rows (flex: 1), status bar (24px).

**Title bar:** three 9px `#c8c8cc` circles at left (platform-neutral window controls —
replace with real Tauri decorations or the host OS's), centered 12px/600 `#4a4a50` title
in the form `<share> — <host>`.

**Toolbar, left to right:**
1. Back/forward segmented pair — two 30×24px buttons in one 5px-radius `#c9c9cd` bordered
   group split by a 1px `#dcdcde` divider. Enabled glyph `#55555c`, disabled `#b0b0b6`.
2. Enclosing-folder button, 24px tall, `padding: 0 9px`, `↑`.
3. Path field — `flex: 1`, 24px, white, mono 11px. Rendered as breadcrumb segments:
   scheme `smb://` in `#8a8a90`, host and intermediate segments `#3a3a40`, separators `/`
   in `#c0c0c6`, current directory `#1a1a1c`/500. Segments are clickable (navigate to
   that ancestor); clicking empty space in the field should switch it to an editable
   text input for pasting a URI.
4. Verb buttons: `Quick Look`, `Edit`, `Download` as secondary (white bg, `#c9c9cd`
   border, `#1a1a1c` text), then `Upload` as the single primary (`#2f6df6` bg + border,
   white 500 text). Only one primary in the toolbar.
5. Filter field, fixed 150px, placeholder "Filter" in `#a0a0a6`. Filters the current
   directory client-side as the user types.

**Sidebar:**
- Section header "MOUNTED", then one row per live connection. Active row: `#dfe7fb` bg,
  3px `#2f6df6` left border (padding becomes `0 12px 0 9px` to compensate), name
  12px/600, subtitle mono 9.5px `#5c76a8` reading `SMB3 · nas.local`.
- Section header "BOOKMARKS" (16px top padding), then saved-but-unmounted entries: 36px
  tall, `gap: 9px`, 13×10px `#a8adb5` glyph, name 12px `#1a1a1c`, subtitle mono 9.5px
  `#8a8a90` with protocol and host. Unreachable bookmarks render the whole row at
  `opacity: .5` with subtitle text "unreachable".
- Footer pinned with `margin-top: auto`, 1px top border, three equal 28px buttons:
  `+` (new connection → dialog `1c`), `−` (remove selected bookmark), `Edit`.
- Rows respond to double-click (mount + navigate) and single-click (select). Drag a file
  selection onto a sidebar row to queue a server-to-server copy (future; not mocked).

**Column header:** grid `1fr 96px 156px 128px`, `column-gap: 16px`, `padding: 0 12px`,
bg `#f4f4f5`, 1px bottom border `#dcdcde`. Columns: Name (left), Size (right-aligned),
Modified, Kind. The sorted column shows an 8px `#2f6df6` `▲`/`▼` after its label. Headers
are click-to-sort and should be drag-resizable.

**Rows:** same grid and gap as the header. Height 24px, 1px bottom border `#f2f2f3`.
- Name cell: `display: flex; gap: 8px; align-items: center`. Folder glyph 11×9px filled
  `#8a8f98`; file glyph 9×11px 1px-stroked `#a8adb5`, both `border-radius: 1.5px` and
  `flex: none`. Name truncates with ellipsis.
- Size: mono 11px, right-aligned, `#6a6a70`; folders show an em dash `—` in `#8a8a90`.
- Modified: 12px `#6a6a70`, format `31 Aug 2026, 18:24`.
- Kind: 12px `#6a6a70`, human-readable ("Matroska Video", "SubRip Text", "Folder").
- Selected row: bg and bottom border both `#2f6df6`, all text white, file glyph stroke
  `rgba(255,255,255,.7)`, size text loses its dim color.
- Dotfiles render at `opacity: .55` with Kind "Hidden" when "show hidden" is on.

**Status bar** (the permission surface), left to right with 10px gaps and 1px×12px
`#dcdcde` dividers between groups:
1. Permission badge — `RW`, mono 9.5px/500, `letter-spacing: .04em`, white on `#2f6df6`,
   `padding: 2px 6px`, radius 3px.
2. Plain-language restatement: "Read & write on `/media`" with the path in mono 10.5px
   `#3a3a40`.
3. Counts: "13 items · 1 selected · 48.2 GB" (selected size, not total).
4. Capacity: "2.4 TB free of 16 TB".
5. Right-aligned (`margin-left: auto`): mono 10px protocol/cipher line
   "SMB3 · aes-128-gcm" then a 6px `#2f9e5f` dot for a healthy connection.

### 2. Browser — dark, read-only mount (option `1b`)
Same structure and metrics, dark tokens, and one behavioral difference that is the point
of the screen: on a read-only mount, `Edit` and `Upload` render disabled (bg `#2e2f32`,
border `#3a3b3f`, text `#63646a`, `cursor: not-allowed`) and the status badge is `RO` on
`#4a4b50` with the reason spelled out — "Read-only — export mapped `ro,root_squash`".
Right side reads "NFSv4.2 · krb5p". Anything that mutates the share (rename, delete, new
folder, drop-to-upload, save-from-editor) is disabled from the same single source of
truth. Never let a write be attempted and fail where it could have been prevented.

### 3. New Connection (option `1c`)
460px dialog. Title bar 34px with centered "New Connection"; body `#f4f4f5`,
`padding: 16px 18px 18px`.

- Protocol segmented control: 2px-padded `#e2e2e5` track, radius 6px, two 24px tabs.
  Active tab white with the small shadow and 600 weight; inactive transparent `#6a6a70`.
  Switching to NFS swaps the field set (export path instead of share; no password —
  NFS auth is `sec=` mode + optional Kerberos principal).
- Form grid: `96px 1fr`, `gap: 9px 12px`, labels right-aligned 11.5px `#6a6a70`.
  Fields: Server (mono), Share (mono), Username, Password (masked), "Save in keychain"
  checkbox on the unlabeled row, then "Mount as" radio pair — "Read & write" (default) /
  "Read-only".
- Live URI preview below the form: white, 1px `#dcdcde`, radius 5px, `padding: 8px 10px`,
  mono 10.5px `#6a6a70` — `smb://dorian@nas.local/media`. Updates as fields change.
- Action row: `Test` button at left, then its result inline — 11px `#2f9e5f` "Reachable ·
  SMB3 · signing on" on success, `#c0392b` with the error on failure. `Cancel` and
  primary `Connect` pushed right with `margin-left: auto`, both 26px tall.

### 4. Transfer queue (option `1d`)
560px panel; in the app this is a separate window or a slide-down sheet from the browser.

Header 34px: "Transfers" plus right-aligned 22px `Pause all` and `Clear finished`.

One block per transfer, `padding: 10px 12px`, 1px `#f2f2f3` separators:
- Direction chip, mono 9.5px/500, radius 3px: uploads `↑ UP` accent-on-`#e8effd`,
  downloads `↓ DOWN` `#6a6a70`-on-`#f0f0f2`, failed `#c0392b`-on-`#fdeaea`.
- Filename 11.5px (500 weight while active).
- Right-aligned mono 10.5px status: `41.2 / 61.7 GB · 112 MB/s · 3m 04s` when running,
  `queued` when waiting, `done · 84 KB` in `#2f9e5f` when finished.
- Progress bar 4px, `#e8e8eb` track, `#2f6df6` fill, radius 2px. Queued items show an
  empty track; finished items drop the bar and render the whole block at `opacity: .6`.
- Destination path, mono 9.5px `#8a8a90`.
- Failed items replace the bar with the error in `#c0392b` and two 20px inline buttons:
  `Retry` and `Remount as RW`. The second is the recovery path for the read-only case —
  it re-mounts the share read-write and resumes the transfer.

Footer 24px `#f7f7f8`: "1 active · 1 queued · 1 failed", right-aligned mono
"112 MB/s aggregate".

### 5. Quick Look + Edit handoff (option `1e`)
440px floating panel, always dark regardless of app theme (previews read better on dark).

- Header 30px `#2a2b2e`: filename 11.5px/500, then mono 10px `#9a9ba0` "2 KB ·
  text/plain", then right-aligned 20px `Open in editor` and a 20px `✕`.
- Body `#1e1f21`, `padding: 14px`, mono 11px, `line-height: 1.8`, text `#c4c4c8`, with
  non-selectable 1-based line numbers in `#63646a`. Text files preview as-is; images
  render centered contained; video/audio show a player; unpreviewable types show kind +
  size + a Download button.
- Footer 24px: `RW` badge (9px mono chip) then "Editable in place · saves back over SMB",
  right-aligned mono "Space to close". On a read-only mount this reads `RO` +
  "Read-only — download a copy to edit", and `Open in editor` is disabled.
- Quick Look is read-only preview; `Edit` (toolbar or "Open in editor") downloads to a
  temp file, opens the OS default editor, watches the file, and uploads on save. Show a
  toast or a queue entry on each write-back.

### 6. Row-layout variants (options `1f`, `1g`, `1h`)
Three treatments of the same list, presented for a pick. `1f` is what `1a` uses.

- **`1f` Columns, 24px** — densest. Grid `1fr 88px 120px` + `column-gap: 16px`,
  22px header, 24px rows, short dates ("31 Aug 18:24"). Best for sorting and scanning;
  long names truncate.
- **`1g` Name-led, two-line** — `display: flex; gap: 9px`, `padding: 6px 10px`, larger
  glyphs (16×13 folder, 13×16 file). Name 12.5px on line one, mono 10px metadata on
  line two: `48.2 GB · mkv · 31 Aug 18:24`. Selection uses the soft `#e8effd` fill with
  `#4a6ba8` metadata. About half the rows per screen; names never truncate and there are
  no columns to drag.
- **`1h` Mono grid, terminal-adjacent** — dark card `#1a1b1d`, header `#26272a` with
  mono 10px uppercase `NAME / SIZE / MTIME` and `letter-spacing: .05em`, 22px rows, all
  mono 11px. Kind collapses to a leading type character (`d` in `#4f86ff` for
  directories, dim `-` for files), sizes abbreviate (`48.2G`, `2.0K`), dates are
  `MM-DD HH:MM`. Everything aligns; least approachable.

### 7. Two-pane browser — light, RW remote (option `2a`)
Window 1240px wide; mock body 480px tall plus the drawer. Structure top to bottom: title
bar (34px), toolbar (44px), pane row (flex: 1), docked drawer.

**Toolbar** differs from single pane because direction is now spatial, so there are no
Download/Upload buttons:
1. `Site Manager` and `Reconnect` (the sidebar's job in single pane moves into a dialog).
2. Divider (1px × 18px `#dcdcde`), then `Quick Look` and `Edit` — these still act on
   whichever pane has focus.
3. Divider, then two checkboxes: **Sync browsing** (on by default) and **Show hidden**.
4. Right-aligned protocol line + health dot, same as single pane.

**Pane row** = local pane (`flex: 1`) + 44px transfer gutter + remote pane (`flex: 1`),
each pane separated by a 1px `#dcdcde` border. The split should be user-draggable and
persisted. Each pane is a vertical stack:

| Band | Height | Notes |
| --- | --- | --- |
| Pane title | 26px | `#eeeef0`, 10px/600 uppercase `.07em` `#8a8a90` — "LOCAL SITE" / "REMOTE SITE". The remote one carries the full URI right-aligned in mono 9.5px `#6a6a70`. |
| Path field | 24px | white, mono 11px, current segment `#1a1a1c`/500, ancestors `#3a3a40`. Editable on click. |
| Directory tree | 104px, **optional** | Collapsible to zero and persisted per pane — `2a` shows it expanded, `2b` shows both panes with it collapsed, which is a legitimate state and not an omission. When expanded: `#fafafb` (dark `#26272a`), 20px rows, 16px indent per level, 8px `#8a8a90` disclosure `▶`/`▼`, selected row `#e8effd` with a `#7f9fe0` glyph. Resizable. |
| Column header | 24px | Local: `1fr 88px 116px` (Name/Size/Modified). Remote: `1fr 88px 116px 104px` (adds Kind). `column-gap: 16px`, `padding: 0 10px`. |
| File list | flex: 1 | 24px rows, identical row spec to `1f`. First row is always `..` (parent). |
| Pane status bar | 24px | See below. |

**Per-pane status bars** — this is the two-pane answer to the permissions requirement:
- Local: `LOCAL` badge (mono 9.5px on `#e2e2e5`, text `#3a3a40`), then
  "6 items · 2 selected · 61.8 GB", then right-aligned "318 GB free".
- Remote: `RW` badge (white on `#2f6df6`), then "Read & write on `/media`", divider,
  item count, then right-aligned "2.4 TB free of 16 TB".

**Transfer gutter** (44px, `#f4f4f5`): two 30×30px buttons centered vertically with an
8px gap. `▶` uploads the local selection to the remote cwd and is the primary (accent
fill) when a local selection exists; `◀` downloads the remote selection and is primary
when the remote pane has the selection instead. The inactive direction renders secondary,
and both disable when their target is not writable.

**In-flight rows.** A file mid-upload appears in the remote list before it exists there:
row bg `#fafafb`, name `#6a6a70` with a dashed `#7f9fe0` glyph, Size showing bytes
transferred in accent, Modified reading "transferring…". It becomes a normal row on
completion.

**Docked drawer** (bottom, collapsible, height persisted):
- Tab strip 26px on `#eeeef0`: `Queued (2)`, `Failed (1)`, `Finished (8)`, `Log`. Active
  tab gets a 2px `#2f6df6` bottom border and 600 weight; inactive `#6a6a70`. Counts live
  in the labels; a non-zero Failed count may instead render as a red pill badge (see
  `2b`). Right-aligned 20px `Pause all` / `Clear finished`.
- Queue table: header 22px `#f4f4f5` 10px/600, grid `52px 1fr 1fr 84px 148px` with
  `column-gap: 16px`. Columns: Dir (mono 9.5px `↑ UP` accent / `↓ DOWN` `#6a6a70`),
  Local file, Remote file (both mono 10.5px, ellipsized), Size (mono, right), Progress
  (4px bar + mono 9.5px "67% · 112 MB/s"; queued rows show an empty track and the word
  "queued").
- Drawer footer 24px `#f7f7f8`: "1 active · 1 queued · 1 failed", divider, "ETA 3m 04s",
  right-aligned mono "112 MB/s aggregate".

### 8. Two-pane browser — dark, RO remote (option `2b`)
Same structure with dark tokens, showing the read-only path end to end:
- Toolbar `Edit` disabled; **Sync browsing** off, **Show hidden** on.
- The gutter's `▶` upload button is disabled (`#2e2f32` / `#3a3b3f` / `#63646a`,
  `cursor: not-allowed`, tooltip "Upload blocked — remote is read-only"); `◀` stays live.
- Remote pane status bar: `RO` badge on `#4a4b50`, "Read-only — `ro,root_squash`".
- Drawer has the **Log** tab active, so this option doubles as the log spec:
  `padding: 7px 12px`, mono 10.5px, `line-height: 1.7`, timestamp `#9a9ba0`, then a
  colored category label — `Command:` `#4f86ff`, `Response:` / `Listing:` / `Hint:`
  `#9a9ba0`, `Status:` `#2f9e5f`, `Error:` `#c0392b` — then the message in `#c4c4c8`.
  Lines are selectable and copyable. Right-aligned drawer controls: a `Verbose` toggle,
  `Copy`, `Clear`.
- The log is where protocol truth lives: mount command and its export flags, listing
  timings, and failures verbatim (`EROFS on /export/backups/patch-notes.txt`). An error
  line may carry an inline recovery button — here `Remount as RW` on the hint line.
- Failed tab label uses the badge form: label "Failed" + a `#c0392b` pill, radius 7px,
  9.5px/600 white.

Note the log was originally specced above the panes and was moved into the drawer; do not
reintroduce a top log strip.

## Interactions & Behavior

**Navigation.** Double-click folder → descend. Back/forward maintain per-window history.
`↑` goes to the enclosing folder. Breadcrumb segment click → jump to ancestor. Sidebar
double-click → mount if needed, then open at the share root.

**Selection.** Single click selects; click+drag rubber-bands; shift-click extends;
cmd/ctrl-click toggles. Type-to-select jumps to the first name matching typed characters.
Toolbar verbs and the status-bar selection counts all read from the selection.

**Keyboard.** Space toggles Quick Look. Enter renames (RW only). Cmd/Ctrl+↓ descends,
Cmd/Ctrl+↑ ascends. Cmd/Ctrl+R refreshes. Cmd/Ctrl+F focuses the filter. Escape clears
the filter, then clears selection. The brief called for a keyboard-driven feel — every
toolbar verb needs a shortcut and a tooltip showing it.

**Transfers.** Drag files from the OS onto the list → upload to the current directory
(show an accent 2px inset drop ring on the list while dragging). Drag from the list to
the OS → download. `Download` writes to the configured download folder. Conflicts prompt
Replace / Keep both / Skip, with "apply to all remaining". Transfers are resumable where
the protocol allows and survive navigation; the queue is per-app, not per-window.

**Permissions gating.** One derived value — `mount.writable` — drives the status badge,
the disabled state of Edit/Upload/rename/delete/new-folder, drop-target acceptance, and
Quick Look's footer copy. It comes from the actual mount (SMB tree connect flags,
NFS export options), not from the user's dialog choice, and is re-read on reconnect.

**Errors.** Per-item failures live in the transfer queue, not in modals. Connection-level
failures (host unreachable, auth rejected, stale handle) show a banner strip under the
toolbar in `#fdeaea` / `#c0392b` with a `Reconnect` button, dim the list to
`opacity: .5`, and flip the status-bar health dot from `#2f9e5f` to `#c0392b`. Not
mocked yet — build to this description or ask for the mock.

**Loading.** Directory listings under ~200ms should not flash a spinner; past that, show
a thin indeterminate accent bar under the column header and keep the previous listing
visible and dimmed.

**Transitions.** Sparse and fast: 120ms ease for hover and disabled-state color changes,
140ms for Quick Look open/close (opacity + 4px rise). No layout animation in the list —
it fights scanning.

**Two-pane specifics.** Double-click in either pane transfers that item in the obvious
direction (local → upload, remote → download) rather than opening it; double-click a
folder still descends. Drag between panes queues a transfer; drag within a pane moves.
With **Sync browsing** on, navigating one pane navigates the other to the same relative
subpath — if the mirrored path does not exist, keep the other pane put and show a single
inline warning in the pane title band rather than a modal, and offer to create the folder
when the target is writable. Focus follows the last-clicked pane and drives Quick Look,
Edit and the keyboard shortcuts; give the focused pane a visible 1px accent inner border
on its list. Tab switches panes. The drawer collapses to just its tab strip and remembers
that state; a new failure auto-selects the Failed tab only if the drawer is already open.

**Mode switch.** Single pane ↔ two pane is a View menu item plus a shortcut, and preserves
the current mount, cwd and selection. The transfer queue is shared: transfers started in
one mode remain visible in the other (single pane surfaces them in the separate Transfers
window `1d`, two pane in the docked drawer). The message log is always recording; only
its surfacing differs.

**Responsive.** Minimum useful window ~820×480. Below ~900px the Kind column drops, then
Modified. The sidebar is collapsible (persist the flag); it does not auto-hide.

## State Management
Frontend (SolidJS signals/stores):
- `bookmarks: Bookmark[]` — `{ id, name, protocol: 'smb' | 'nfs', host, share, username,
  saveCredentials, mountPreference: 'rw' | 'ro' }`; persisted.
- `mounts: Map<bookmarkId, Mount>` — `{ status: 'mounting' | 'ready' | 'error' |
  'reconnecting', writable, protocolLabel, securityLabel, mountOptions, capacity: { free,
  total } }`.
- `activeMountId`, `cwd: string`, `history: { stack, index }`.
- `listing: { entries: Entry[], loading, error }` where `Entry = { name, isDir, size,
  modified, kind, hidden }`.
- `sort: { column: 'name' | 'size' | 'modified' | 'kind', ascending }`, `filter: string`,
  `selection: Set<string>`, `showHidden: boolean`.
- `quickLook: { path, mimeType, content } | null`.
- `transfers: Transfer[]` — `{ id, direction, name, sourcePath, destPath, bytesDone,
  bytesTotal, rate, etaSeconds, status: 'queued' | 'active' | 'paused' | 'done' |
  'failed', error }`.
- `rowLayout: '1f' | '1g' | '1h'`, `viewMode: 'single' | 'dual'` and
  `theme: 'light' | 'dark' | 'system'`; persisted.
- Two-pane only: `localPane: { cwd, listing, selection, sort, treeExpanded, freeBytes }`
  mirroring the remote pane's shape, `syncBrowsing: boolean`, `focusedPane: 'local' |
  'remote'`, `splitRatio: number`, `drawer: { open, height, tab: 'queued' | 'failed' |
  'finished' | 'log' }`.
- `log: LogLine[]` — `{ at, category: 'command' | 'response' | 'status' | 'listing' |
  'hint' | 'error', text, action? }`; capped ring buffer, `verbose` flag filters it.
  Recorded in both modes.

Rust/Tauri side owns all I/O: `connect`, `disconnect`, `list_dir`, `stat`, `read_file`,
`write_file`, `download`, `upload`, `mkdir`, `rename`, `remove`, `capacity`. Transfers
report progress by event stream (`transfer://progress`) rather than polling, and each
carries the transfer id. Credentials go to the OS keychain, never to app state or disk.
`list_dir` should return the mount's writability alongside the entries so the status bar
can never drift from reality. Local-side listing for the two-pane view goes through the
same command surface (a `local` protocol) so both panes share one code path. Every
protocol command and response emits a log line (`log://line`) — the log pane is a view
onto that stream, not a separate mechanism.

## Assets
None shipped. Substitute in the app:
- **Icons.** The mock's file/folder glyphs are placeholder rectangles. Use a real
  monochrome set (Lucide, Phosphor, or the platform's native file icons) at 12–16px, and
  map kind → icon. Keep them monochrome at sidebar/list scale; the design has no
  multicolor icons anywhere.
- **Fonts.** JetBrains Mono must be bundled locally (Tauri apps should not fetch Google
  Fonts at runtime). Helvetica Neue is a system font; ship a grotesk fallback stack for
  Linux and Windows (e.g. Helvetica Neue → Arimo → system-ui).
- **Sample data** in the mock (nas.local, 10.0.0.4, movie filenames) is illustrative only.

## Files
- `Share Browser Mockups.dc.html` — all six screens plus the three row variants, as an
  anchored option canvas. Option ids `1a`–`1h` match the section headings above.

## Not yet designed
Ask for these before implementing them from imagination: search across a share (as
opposed to filtering one directory), Get Info / POSIX mode inspector, the
disconnected/reconnecting banner state, preferences, rename and delete confirmations,
multi-window behavior, the sync-browsing mismatch warning, and the collapsed state of the
two-pane drawer.
