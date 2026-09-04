# Design gaps

Places where this implementation went beyond the mocks in
`design_handoff_share_browser/`, and what it did instead. Each entry is waiting
on a designer's sign-off; none is a decision meant to stand permanently.

The handoff's own §"Not yet designed" list is the other half of this: those are
things nobody has drawn *yet*. The entries here are things that were drawn, but
only in a place that does not exist in the app yet, or not at all.

## Remount as RW, in the status bar *(E9-S3, 2026-09-03)*

**The mock** puts `Remount as RW` on a failed row in the transfer queue
(§Screens/4), beside `Retry`. That is the only place it appears.

**The problem** is that the transfer queue is E13, and until it exists a
read-only mount has no recovery path anywhere in the window. The status bar
states the mount is read-only and gives the server's reason, and then the user
has nowhere to go.

**What was built:** a 16px `Remount as RW` button inline in the status bar,
after the read-only reason, shown only when `writable` is false. It disappears
the moment the mount is read-write, so a healthy window looks exactly like the
mock.

**For the designer:** the question is whether this belongs in the status bar at
all once the queue exists, or whether the read-only case should stay recoverable
only from a transfer that failed because of it. The engine call
(`remount_read_write`) is shared either way; removing the button is deleting one
element.

## Connection banner *(E15-S1, 2026-09-03)*

**The mock** does not draw it. §Errors describes it in prose and says "build to
this description or ask for the mock".

**What was built**, to that description exactly: a strip under the toolbar,
`#fdeaea` on a `#c0392b` line with `#c0392b` text, carrying the failure message,
a `Reconnect` button and a dismiss. The list dims to `opacity: .5` and the
status-bar health dot flips from `#2f9e5f` to `#c0392b`.

**Sign-off:** rather than a static screenshot, the state is live at
`?screen=banner` — it renders from a real dropped-mount event through the same
path a real failure takes, so what a designer sees is what the app does. The
AC asked for a screenshot; a reproducible route seemed more useful than an
image that goes stale, and that substitution is itself worth a nod.

**For the designer:** two judgement calls. The banner is full width, so it sits
above the sidebar as well as the list rather than only over the list. And it is
dismissible — a user who knows the share is down should be able to get the strip
out of the way without reconnecting.

## Rename and delete confirmations *(E14-S1, 2026-09-03)*

**The mock** does not cover either. The handoff lists "rename and delete
confirmations" under §Not yet designed.

**What was built**, per `plan.md`'s instruction to implement something minimal
and flag it:

- **Rename** is inline, in the row, with no confirmation. Enter opens it on the
  one selected row; Enter or clicking away commits; Escape cancels. The initial
  selection covers the name but not the extension, because renaming
  `Arrival.2016.mkv` almost never means renaming the `.mkv`.
- **Delete of five items or fewer** is *deferred*, not undone. The rows
  disappear at once and the files are removed when the toast expires after
  eight seconds; Undo cancels before anything is sent. An "Undo" offered after
  the files were already gone would be a button that cannot do what it says,
  and there is no filesystem operation that would make it honest.
- **Delete of more than five** shows a small confirm sheet with a red Delete.
  Above that count the undo toast stops being a safety net anybody would
  actually catch.

**For the designer:** the deferral is the part worth a second opinion. It makes
Undo real, but it means a file the user "deleted" still exists for eight
seconds, and closing the window inside that window cancels the deletion rather
than completing it. The alternative — delete immediately, offer no undo — is
simpler and more predictable, and is a one-line change if that is preferred.

## Quick Look as a floating panel, not a window *(E11-S1, 2026-09-03)*

**The mock** (§Screens/5) calls it a "440px floating panel, always dark
regardless of app theme".  read that as a separate OS window.

**What was built:** a floating panel inside the main window, matching the
handoff's own wording. A second Tauri window would need its own entry point and
IPC bridge, and the mock describes no behaviour that requires one — it is drawn
overlapping the file list, which is what an in-window panel is.

**For the designer:** if it should be a real window that can be moved outside
the app and left open beside it, that is a different thing and worth saying so.

## Quick Look as a floating panel, not a window *(E11-S1, 2026-09-03)*

**The mock** (§Screens/5) calls it a "440px floating panel, always dark
regardless of app theme". `plan.md` read that as a separate OS window.

**What was built:** a floating panel inside the main window, matching the
handoff's own wording. A second Tauri window would need its own entry point and
IPC bridge, and the mock describes no behaviour that requires one — it is drawn
overlapping the file list, which is what an in-window panel is.

**For the designer:** if it should be a real window that can be dragged outside
the app and left open beside it, that is a different thing and worth saying so.

## Transfers as a sheet, not a second window *(E13-S1, 2026-09-03)*

**The mock** (§Screens/4) draws a 560px panel. `plan.md` reads it as "a
separate window (default) or slide-down sheet (preference)".

**What was built:** the sheet only. The preference that would offer the other
has nowhere to live until E17 brings a preferences store with a UI, and a
second Tauri window costs an entry point and an IPC bridge for a choice nobody
can currently make. The panel is the same 560px and the same content either way.

**For the designer:** if the separate window is the important one — because it
can be left open beside the browser while working — that changes the priority
and is worth saying. Same question as Quick Look's panel, and probably the same
answer for both.

## The preferences sheet, and where the sidebar toggle lives *(E17-S1, 2026-09-04)*

**The mock** has no preferences screen at all. The handoff's §"Not yet designed"
list names preferences explicitly, and E17-S1's own AC says as much: "a minimal
native-feeling sheet with these fields only".

**The problem** is that nine settings now persist and a user has no way to
change most of them. Shipping the persistence without a way in would mean the
values could only be edited by hand in `prefs.json`.

**What was built** is a sheet borrowing the New Connection dialog's chrome —
same scrim, same 460px card, same titlebar, same two-column form grid — with
one control per setting that does something today. It is deliberately plain
rather than an invention: reusing an approved container seemed a smaller
liberty than designing a new one.

Two settings persist without a control in the sheet, on purpose:

- **`rowLayout`** has no picker, because `1g` and `1h` do not exist yet
  (E8-S3/S4). A control offering one real layout and two absent ones would be
  the promise E3-S1 refused to make.
- **`sidebarCollapsed`** is toggled from the toolbar instead, where the user
  can see what it does.

**The sidebar toggle's placement is the second gap.** E1-S3's AC says the
sidebar is "collapsible; the collapsed flag persists", but not where the control
is. The sidebar's own footer is specified as three equal 28px buttons, so a
fourth would break a pixel-level spec. It sits at the head of the toolbar
instead, beside the preferences gear, as a `⚙`-sized icon button.

**A designer might instead** want a proper preferences window rather than a
sheet, the standard macOS toolbar segmented control for the sidebar, or the
toggle in the titlebar. All three are cheap to move to; nothing here depends on
the current placement.

## Three grey tiers cannot all meet 4.5:1 *(E16-S2, 2026-09-04)*

**The mock** uses a three-tier grey scale for text — `--text-primary`,
`--text-secondary`, `--text-tertiary` — with the third used for subtitles,
section headers and status-bar detail, much of it at 9.5px.

**The problem** is arithmetic. Measured against the surfaces they actually
appear on:

| | light | dark |
|---|---|---|
| `--text-primary` | 15.0–17.4:1 | 9.9–13.8:1 |
| `--text-secondary` | 4.6–5.4:1 | 5.1–6.0:1 |
| `--text-tertiary` | **2.96–3.43:1** | **4.16–4.85:1** |

Tertiary fails 4.5:1 on every light surface and on two of the dark ones. It
cannot simply be darkened: the light value that would clear 4.5:1 is `#6c6c72`,
against secondary's `#6a6a70` — indistinguishable. In dark it would have to
become *lighter* than secondary and invert the hierarchy. **The design's three
tiers and the 4.5:1 threshold cannot both hold on these surfaces.**

**What was done** is the part that has no hierarchy cost:

- `--text-placeholder` darkened `#a0a0a6` → `#76767c` (light) and `#63646a` →
  `#83848a` (dark). It carries no tier; it was at 2.60:1 and 2.86:1.
- `--glyph-file` darkened `#a8adb5` → `#90959d` (light). At 2.26:1 it failed
  even the 3:1 floor WCAG sets for non-text, and it stays lighter than the
  folder glyph so the two remain distinguishable.

`--text-tertiary` is left alone and recorded as the one exception in
`src/tokens/contrast.test.ts`, which still holds it to 3:1 so it cannot quietly
get worse.

**A designer might instead** drop to two tiers and distinguish the third by
weight or size rather than lightness, darken the surfaces tertiary sits on, or
accept the ratio for text that is genuinely decorative. All three are choices
about the design's intent, which is why the code did not make one.

## No View menu for the row layouts *(E8-S4, 2026-09-04)*

**The mock** presents `1f`, `1g` and `1h` as a pick between three treatments,
and E8-S4's AC says the choice is made "via View menu and a shortcut".

**The problem** is that this application has no menu bar. It is a single Tauri
window with a custom titlebar, and the handoff draws no menus anywhere — so
there is no View menu to add an item to, and adding a whole menu bar for three
items would be a much larger liberty than the one taken here.

**What was built** is the shortcut half (⌘1/⌘2/⌘3, from the same table that
labels the toolbar) plus a picker in the preferences sheet, which is where a
user who does not know the shortcut will look. The picker's options name their
shortcuts — "Columns (⌘1)" — so the sheet teaches the keys rather than
duplicating them.

**A designer might instead** want a real menu bar, a segmented control in the
toolbar beside the sidebar toggle, or a right-click menu on the column header.
The state and the shortcuts are already in place for any of them; only the
control would change.
