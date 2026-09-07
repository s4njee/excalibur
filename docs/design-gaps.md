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

**Amended 2026-09-05 (E1-S6): the sheet now opens below the toolbar.** It was
anchored to the bottom of the title bar, and its own 560px then covered the
right half of the toolbar — `Quick Look`, `Edit` and `Download`. Selecting a
file and pressing Download with the queue open did nothing at all: the click
landed on the sheet. The mock has no opinion here, because it draws the queue
as a separate window and a separate window overlaps nothing. Moving it down
44px is the smallest change that keeps the drawn panel intact and stops it
swallowing the verbs it sits over. It still covers the right end of the
connection banner when both are up, which is worth a look if the separate
window is not coming.

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

## No mock for a window with nothing mounted *(E1-S6, 2026-09-05)*

**The mock** has no first-run screen. Every screen in the handoff has a share
in it — `1a` through `1h` all draw `smb://nas.local/media` with rows, an `RW`
badge and `2.4 TB free of 16 TB`.

**The problem** is that the app took that literally. The Tauri shell fell back
to a mock share whenever no session was active, so a user who had just
installed Excalibur opened on a share that does not exist, with verbs that
acted on it — Download queued a row for a file nobody has. That is worse than
an unstyled empty window, because nothing on screen said it was not real.

**What was built:** with no mount, the list area carries a centred
`No share connected`, one line of explanation, and a `New connection…` button
that opens the same dialog the sidebar's `+` does. The title bar names nothing,
the breadcrumb is an empty field, the status bar reads `Not connected` with no
badge, no capacity and no protocol, and every verb is disabled through the same
permission gate the mounted window uses — there is no second rule for the empty
case to disagree with. The sidebar still lists bookmarks, because a saved
connection is worth seeing whether or not it is mounted.

The typography is the design's own — the toolbar's control size for the button,
the row size for the heading — rather than a new scale invented for one screen.

**Sign-off:** the state is live in the browser preview at `?screen=empty`,
which renders it from a backend that refuses listings the way the shell does.
Same substitution as the connection banner above: a reproducible route rather
than a screenshot that goes stale.

**A designer might instead** want an illustration, the bookmark list promoted
into the empty area (a "pick one of these" screen rather than a "connect
something" one), or the window to open straight into the New Connection dialog
on a true first run. All three are changes to this one component; nothing else
depends on its shape.

## A `..` row in the file list *(E8-S10, 2026-09-05)*

**What the handoff shows.** Nothing. Every screen `1a`–`1h` draws a listing of
files and folders, and the only way up the tree the design gives is the
toolbar's `↑` and the breadcrumb.

**What was built.** A `..` row pinned above the listing in every folder below
the share root, with a folder glyph and the name in `--text-secondary`, at the
current layout's row height. It ascends on a double-click, exactly as a folder
row descends. It is not selectable and not counted.

**Why.** Asked for directly by the user: "in the files view there should be a
'..' entry to go to the parent folder". Every file browser that lists a
directory has one, and its absence sends a user back to the toolbar for the
most common navigation there is.

**What a designer still owns.** Whether the row is styled as a row at all or as
something quieter (a thinner strip, no glyph); whether it should respond to a
single click, given nothing else in the list does; and whether it belongs in
the `1h` mono layout, where `ls -a` would print `..` as an ordinary entry with
a mode and a date rather than as a control.

## A context menu *(E14-S2, 2026-09-05)*

**What the handoff shows.** Nothing. The design gives a toolbar and a keyboard
table, and no right-click behaviour anywhere.

**What was built.** A menu at the pointer with `Quick Look`, `Edit`, `Download`,
then `Rename`, `Move to…`, `Delete`, then `Properties`; on the list background,
`New Folder` and `Upload…`. It reuses nothing visual from the mocks because
there is nothing to reuse: 4px padding, the control background, the window
radius and shadow, and the selection fill for the active row. Refused items are
`--text-tertiary` and carry the server's reason as their tooltip, matching how a
disabled toolbar verb behaves.

**Why.** Asked for directly by the user, including that it "work for a selection
of files/folders as well".

**What a designer still owns.** Whether the item order is right; whether the
verbs that act on several rows should say so in the label (`Delete 3 items`) or
stay bare; whether `Properties` belongs at the bottom or the top; and whether a
refused item should be shown greyed at all rather than omitted — greying keeps
the menu's shape stable between mounts, which is why it is greyed here, but the
design has no precedent either way.

## Move to… *(E14-S3, 2026-09-05)*

**What the handoff shows.** No destination picker.

**What was built.** The New Connection dialog's chrome wholesale — same scrim,
same 460px sheet, same title bar and footer buttons — plus two things with no
counterpart there: a breadcrumb whose root crumb is the share name, and a
fixed-height list of folders with a `..` row. The model is "walk to the folder,
then Move here", not "select a row and confirm", so a single click enters a
folder and the destination is always the one the breadcrumb ends on. Folders
being moved are listed and refused by name rather than hidden, and the refusal
is a sentence beside the disabled button in `--error`.

**What a designer still owns.** Whether walking is the right model or a tree
with disclosure triangles would be; whether the picker should offer New Folder;
and how a destination the user cannot write to should look, given that a listing
does not say.

## Properties sheet *(E14-S4, 2026-09-05)*

**What the handoff shows.** Nothing — "Get Info / POSIX mode inspector" is
listed under §Not yet designed. It was asked for directly as the last item of
the context menu, so the shape is ours.

**What was built.** A modal on the same dialog chrome, with the entry's name as
a heading and a two-column definition list whose label column is the 96px the
other dialogs use, so labels do not jump between sheets. One entry shows Kind,
Size, Modified, Path and — when there is one — Links to; a selection shows
Items, Size, Last modified and Where. Size appears twice, rounded as the list
shows it and exact: `48.2 GB (48,234,496,000 bytes)`.

**The judgement call worth a designer's eye** is what a folder's size looks
like. Walking a folder to size it is forbidden by the story, so the sheet shows
the same `—` the Size column shows plus a sentence saying the contents were not
measured. The alternative, `0 B`, is a number that reads as "empty".

**What a designer still owns.** Whether Properties should be a modal at all or
an inspector alongside the list that stays open as the selection changes;
whether the exact byte count deserves its own row; and whether POSIX mode and
owner belong here once the engine sends them.

## A fifth protocol tab: Local *(E21-S2, 2026-09-05)*

**The mock** draws four protocol tabs in the New Connection dialog — SMB, NFS, SFTP
and xsync — and a window whose chrome always names a server: `<share> — <host>` in the
title bar, `<scheme>://<host>/<path>` in the path field, and a protocol and a security
mode in the status bar's right-hand corner.

**The problem** is that a folder on this machine has none of those. It has no server,
no account and no password; `file:` has no authority to put between the slashes; and
the engine returns `local` for both of the status bar's two labels, which would have
rendered as `local · local`.

**What was built:** a fifth tab, last in the row, with a single **Folder** field and a
`Choose…` button beside it, plus the Mount-as radio the other tabs have. The field set
swaps as it does everywhere else — the Server, Port, Username and Password fields are
absent rather than disabled. The URI preview reads `file:///Users/you/Documents`. In
the window the title bar says `Documents — This Mac`, the path field drops the host
crumb so the breadcrumb reads as a path, and the status bar shows a single `local`.
The sidebar row names the folder's path where a network bookmark names its host, and
is never dimmed: reachability is not a question a directory answers.

**For the designer:** three judgement calls. Whether Local belongs in this dialog at
all, or in a separate "Open Folder…" command — it is here because the window's whole
model is one mount at a time, and a folder is a mount. Whether it belongs last in the
tab row; it is last because the dialog opens on xsync and leading with the one tab that
reaches nothing would misread. And whether `This Mac` is the right thing in the title
bar — the alternative is the folder's parent path, which is more informative and less
readable.

## Two windows, side by side *(E23-S1, 2026-09-05)*

**What the handoff shows.** One window. Every screen `1a`–`1h` draws a single
browser with a single share in it, and §Screens/4 draws the transfer queue as a
separate window beside it. Nothing in the handoff says what a *second* browser
window is, or what the two of them share.

**The problem** is that everything about the app is currently written as though
there were one of it: the session manager has one active mount, the stream
protocol serves "the active mount", and the window's chrome names it. A second
window on a second share would have made both windows show the second one.

**What was built** — one engine, many windows. Each window is a *whole* browser,
not a pane of one:

- **Duplicated in every window:** title bar (`<share> — <host>`), toolbar with
  its breadcrumb and filter, sidebar, file list, status bar, Quick Look panel,
  the connection banner, and the empty state. A window with nothing mounted sits
  beside a window on `smb://nas.local/media` and looks exactly like first run,
  because for that window it is.
- **Per-window state:** the active mount, the path, the back/forward history,
  the selection, the filter text, the row layout, the Quick Look panel and the
  sheets. Two windows on one share can be in different folders with different
  rows selected, which is the entire reason to open the second one.
- **Shared, app-wide, one copy:** the mounted sessions themselves, the bookmark
  list, the keychain, the preferences, and the transfer queue with its worker. A
  share mounted in two windows is **one** connection, not two logins; it is
  disconnected when the last window looking at it closes.
- **The sidebar is duplicated rather than shared.** It is not a palette that
  belongs to the app: it is how a window changes *its own* mount, so it has to
  be in the window whose mount it changes. Its contents are the same list in
  both, and a bookmark added in one appears in the other.
- **Two windows on one mount are titled identically**, with no `(2)` suffix.
  macOS does not number windows and the mock has nowhere to put a suffix; the
  window's own path field is what tells them apart.
- **A mutation in one window shows in the other.** Creating, renaming or
  deleting sends `listing://changed` to every window on that mount, which each
  window feeds into the 250 ms refresh coalescer it already had. The two lists
  agree within a quarter-second without either window polling.
- **⌘N** opens a new window on the same mount and the same folder, offset 24px
  down and right from the one that opened it.
- **Closing the last window does not quit.** The app stays in the Dock with no
  windows, as a macOS app does; ⌘Q quits, and asks first if transfers are still
  moving.

**The macOS menu bar is now this app's own, for one item.** Tauri already
installed a default menu bar — the E8-S4 entry above, which says the app has no
menu, is about the *window* chrome the handoff draws, and was never true of the
system menu. That default menu's `Quit` sends `terminate:` straight to
`NSApplication`, which no part of this app can stand in front of; a ⌘Q with a
transfer half-finished would have killed it silently. The menu is now built here
instead, item for item the same, with `Quit` as an ordinary item that goes
through the code that asks. Nothing visible changed.

**The row layout is the one setting that is per-window and persisted at the same
time.** ⌘1/⌘2/⌘3 changes the window you pressed it in, and writes the value as
the app's default for the next window opened. It is not pushed sideways into
windows already open, because a layout is a property of the view you are looking
at, not of the app.

**What a designer still owns.** Four things:

- **The transfers sheet.** It is still a sheet inside each window, showing the
  one app-wide queue — so with two windows open and the sheet up in both, the
  same rows are drawn twice. The handoff draws the queue as a separate window,
  which is now the obviously right answer and is the change this story makes
  worth doing rather than the one it does.
- **Whether a second window on the same share should be distinguishable at all**
  — a subtitle, a tint, a numbered title — or whether identical titles are
  correct because the folder is the difference that matters.
- **What the sidebar's `MOUNTED` section means with two windows.** It currently
  marks the mount *this* window is looking at. The alternatives are marking
  every mount the app holds (accurate about the connections, silent about which
  window is where) or showing both states differently.
- **Where ⌘N should land.** It opens on the same mount and folder, on the
  reading that a second window is for comparing two places in one share. Opening
  on the empty state instead — a fresh window for a different share — is the
  other defensible answer, and is a one-line change.
- **Whether `File ▸ New Window` should exist in the menu bar.** It deliberately
  does not: a menu accelerator is handled before the webview sees the key, and
  the folder ⌘N opens the new window on lives in the webview — so the item could
  only ever open a window at the share root, which is not what ⌘N does. It
  becomes possible once a window's path is somewhere the Rust side can read,
  which is what E23-S2's restore needs anyway.

## Go to Folder *(E26-S2, 2026-09-06)*

**The mock** has no Go to Folder sheet. It draws a path field in the toolbar
that is a breadcrumb — a row of clickable crumbs, not an editable line — so
there is nowhere in the design to type a path that is not already on screen.

**The problem** is that ⌘⇧G is in the story's AC and is the one keyboard route
to a folder the user cannot see: deep paths, a folder whose name they know, a
path pasted from a terminal. Making the breadcrumb editable was the alternative
and was rejected — the crumbs are also the drop target and the navigation
control, and a field that is sometimes a row of buttons has two behaviours in
one place.

**What was built:** a small modal borrowing `dialog.module.css` the way
Preferences and Move to… do — a title bar, one mono field prefixed with the
share's name, a hint line, and Cancel / Go. It opens seeded with the folder the
window is showing plus a trailing slash, so it is a place to edit a path rather
than an empty box to retype one into. Tab and → complete the next component from
the folder being typed into; several candidates extend as far as they agree and
draw the rest as chips underneath. Enter lists the resolved path and navigates,
or shows the engine's refusal inline. A leading `/` means the share root, and
`..` cannot climb above it.

**For the designer:** three judgement calls. Whether this belongs in a modal at
all, or as a mode of the path field, which is where a Finder user would look for
it. Whether the chips are worth their space, or whether an ambiguous Tab should
say nothing the way a shell's first Tab does. And whether the share name should
be a fixed prefix beside the field — it is there to say the path is *inside* the
mount, which is the one thing a user typing `/` is likely to get wrong.

## Drag between windows: the ring, the caption and the drag image *(E24-S2, 2026-09-06)*

**The mock** has a drop ring — the 2px accent inset E13-S5 already draws when
Finder files are over the list — and nothing else. It says nothing about a drag
that starts *inside* the app, and a drag between two Excalibur windows is a
gesture the design has never had to describe, because until E23-S1 there was
only one window.

**The problem** is that the ring alone cannot say what the drop will do. Same
mount is a **move**; a different mount is a **copy**; ⌥ forces a copy. Those are
different enough that a user who guesses wrong loses a file off a share, and the
AC asks for the ring to say which. There is also a third answer the design has
no vocabulary for at all: a **refusal**, for the pairs the transfer queue cannot
do yet (share to share, which arrives with E22) and for a folder dropped into
itself.

**What was built:** three things past the mock.

1. **A caption inside the ring** — a small accent pill, bottom-centre of the
   list, reading `Move to Documentaries` or `Copy 3 items to raid6`. It is
   `position: sticky` so it survives a spring-loaded descent scrolling the list
   under it. A refusal turns the ring and the pill grey rather than accent, and
   the pill carries the reason verbatim: an accent ring that then refuses the
   drop is the failure E13-S5's Results already call out.
2. **A sidebar row as a drop target** — the "coming later" tooltip E13-S5 left
   is gone, and a mounted share row now highlights and takes a drop to that
   share's root. Only *mounted* rows: an unmounted bookmark has no session to
   copy into and says `Open <name> first.`
3. **A drag image** — the row's glyph and name in a small pill, drawn to a
   canvas in the webview and handed to AppKit as PNG bytes. It is drawn at CSS
   scale rather than device scale, because an `NSImage` built from PNG data
   takes its point size from the pixel dimensions and a 2× bitmap would be a
   drag image twice the size of the row it came from. It is therefore slightly
   soft on a Retina display.

**For the designer:** four judgement calls. Whether the caption belongs in the
ring at all or in the status bar, which is already the app's live region and
would not cover a row. What a refused drop should look like — grey is a guess,
and "this cannot happen" and "this is disabled" are currently the same colour.
Whether a sidebar drop should highlight the row or the whole sidebar, since the
drop lands at a share's *root* and not in anything the row itself shows. And
whether the drag image should be the row (name and glyph, as built), a stack for
a multiple selection the way Finder draws one, or a count badge.

## M7 verification, 2026-09-06 — three defects found in the bundled app

Driving the packaged app with two windows on two mounts turned up three things
the tests could not see, because each needs a *second* window to exist.

**1. A window's chrome does not follow a share mounted into it.** Mounting a
second share through the sidebar's `+` (New Connection) while a share is already
mounted moves the listing and the window's active mount to the new share, but
the title, the breadcrumb and the sidebar's selected row all keep naming the old
one. Only this route: opening a *bookmark* replaces the mount and updates the
chrome correctly, so the two paths disagree. It matters more than a cosmetic lag
— the window is naming a share that its next delete or upload will not go to,
and the standing rule is that a mount's identity comes from the session and is
never inferred. **For the designer:** whether mounting a second share should
move the window to it at all, or open it alongside and leave the window where it
is; the current behaviour is halfway between the two and reads as a bug either
way.

**2. A completed transfer does not refresh the destination listing.** ~~A file
dropped into the folder a window is showing lands on disk and reaches `done` in
the queue, but the window keeps the listing it had.~~ **Fixed 2026-09-06.** The
window now relists when a transfer reaches `done` on the mount it is showing,
through the same coalescer a local mutation uses, so twenty dropped files are
one relist. The mount is checked because queue progress is a deliberate
broadcast — every window hears about every transfer — and relisting on all of
them would be each window refreshing for shares it is not showing. A *running*
transfer deliberately does not relist: those arrive per chunk, and a listing
round trip behind every progress update would be aimed at the share already busy
moving the bytes.

**3. The Window menu has no window list.** It offers Minimize, Zoom and Close
only. Every macOS app with more than one window lists the open windows there and
offers *Bring All to Front*, and with two Excalibur windows overlapping there is
no keyboard or menu route to the one behind. This is E26 (Finder parity) work
rather than a defect in E23-S1, but it is what makes two windows awkward to use
rather than merely possible.

A fourth observation, not a defect: a **debug** bundle takes several seconds to
first paint, showing an empty window meanwhile. That is what was misread for a
day as "the bundle renders nothing" (see the withdrawn note in `plan.md` at
E22-S3). Worth knowing before anyone reaches the same conclusion again; worth
measuring on a release bundle before treating it as a real startup cost.

## A folder is one transfer row *(E22-S2, 2026-09-06)*

**The mock** (§Screens/4) draws the transfers panel as a list of files: a name,
a size, a bar and a rate on each row. Every row in it is a file.

**What was built:** a folder dropped onto a share, or dragged out of one onto
another window, is **one row** that names the folder. Its bar fills as the
whole tree moves and its rate is the tree's rate; the files inside it never
appear.

This is not a preference. Since E22-S2 every transfer runs xsync's planner, and
the planner is the thing that walks the tree — it is what turned a dropped
folder from an `EISDIR` failure into a working copy. Pre-walking the folder here
to make per-file rows would mean statting every file over the browse session
before the transfer had sent a byte, and then having the planner walk it again;
the size, the rate and the resume point on those rows would all be this app's
guesses rather than the engine's answers. A download of a folder *selected in
the list* still expands to one row per file, because that walk (E13-S4) already
existed and reports a subdirectory it could not read as a failed row, which is
worth keeping until there is somewhere better to put it.

So the panel currently shows two shapes at once, and that is the honest
description of a half-migrated queue rather than a design decision.

**For the designer:** E22-S3 is where this is meant to settle — its AC says "a
directory row stops being one row". The question worth answering before then is
what a folder row *should* look like while it is running. Three candidates: one
row that says `Show — 412 files, 8.1 GB` and expands on click; one row per file
as the mock draws, with the folder as a group header; or the folder row plus a
single "current file" line underneath it, which is what most file managers do
and which the panel has no vocabulary for. The engine can supply any of the
three — the planner names and sizes every file before the first byte moves —
so this is a drawing question, not a plumbing one.

**Amended 2026-09-06 (E22-S3): both shapes are now the folder row, and the row
learned to say what it holds.** `enqueue_downloads` no longer pre-walks a
selection, so a folder is one row whichever way it was started — dragged,
dropped, or picked in the list and downloaded. What settled it was not the
argument above but a measurement: every row is run as its own `sync_endpoints`
session, and a folder of 1002 files took 21.7 s as one row per file against
0.31 s as one row, with only 0.30 s of the 21.7 spent talking to the share.
The per-file shape was 70× of pure scheduling.

The first of the three candidates is what was built, minus the expansion. A
running row now reads `41.2 / 61.7 GB · 412 / 1002 files · 112 MB/s · 3m 04s`
and a finished one `verified · 1002 files · 8.1 GB`. The count is the planner's
`Planned` event, which the adapter already received for its byte total and
threw the file count away from; it appears the moment the scan ends, so a row
briefly shows a size and no count. A row that names a single file shows no
count at all, because `1 file` repeats the row's own title.

**What this gave up, stated rather than hidden:** a subdirectory the walk could
not read used to be its own failed row beside the ones that worked. It is now a
per-entry failure inside the session, and the *folder's* row fails with the
count and the first reason — `3 of 1002 entries failed — season 2: permission
denied`. Nothing is silently omitted, which was E13-S4's actual rule, but the
detail is one sentence instead of one row per problem.

**Still for the designer**, and now the only part left: **clicking a folder row
expands to nothing.** The engine names and sizes every file before the first
byte moves and could feed a disclosure triangle, and the panel has no
vocabulary for one. The other open question is the "current file" line most
file managers show under a folder row — `season 1/e04.mkv` — which the engine
emits on every `Transferred` event and this app currently drops on the floor.

## Dragging out to Finder is invisible in this app *(E24-S1, 2026-09-06)*

**The mock** draws no state for a drag that leaves the window. §Screens/4's
transfers panel is drawn for copies the user started *here* — Download, Upload,
a drop onto a list — and the handoff's drag material is all about the ring on a
destination row, which by definition is not what happens when the destination is
the Desktop.

**What was built:** dragging a selection out puts one file promise per item on
the pasteboard. Nothing is fetched until Finder asks; when it does, the copy is
an ordinary transfer, so it appears in the panel as rows like any other and can
be paused, cancelled and retried there. Meanwhile **Finder** shows its own copy
progress, because it is Finder that asked. So one copy is reported twice, in two
different vocabularies, in two different places — and the window the user
dragged *from* shows nothing at all at the moment of the drop.

Three things are undesigned, and only the first is urgent:

1. **Where the promise's rows go.** ~~They are indistinguishable from a
   Download in the panel: same direction, same shape, and a destination path in
   a staging directory the user never sees, renamed into place at the end. A
   person who drags a film onto the Desktop and then opens the sheet sees a
   download to a folder whose name has a dot in front of it.~~ **Half-answered
   by accident, 2026-09-06 (E22-S3).** A promise is now **one** row rather than
   one per file, and the row is *taken back out of the queue* when the drag
   ends, whatever happened to it — so the sheet no longer accumulates downloads
   to dotted folders, and a person who opens it after the drop sees nothing.
   That was not a design decision, it was a correctness one: a row naming a
   staging directory that survives into `transfers.json` is re-queued on the
   next launch and copies a tree into a folder that was deleted when the drag
   ended, which is how three abandoned drags left 42 MB of `.excalibur-*`
   directories in a real Downloads folder. The row still needs to say *dragged
   to Desktop* while it is there; the question of whether the panel should show
   these at all is now the only one left, and "no" is closer to what the code
   does.
2. **A failure has nowhere good to land.** A promise that cannot be fulfilled
   reports an `NSError`, and Finder renders it as its own alert — which is the
   right place for it, but it means one error appears in Finder's words and the
   same error on a queue row in ours.
3. **The drag image for many items.** One rendered row with AppKit's own count
   badge on it, because only the first dragging item carries a picture; the
   design has no drawing for a multi-item drag out.

**For the designer:** the question to answer first is whether a promise is a
transfer *the user should see in this app at all*. Both answers are defensible
and the plumbing supports either — the rows exist because the queue is what
moves the bytes, not because anybody decided they should be visible.

## A FAVORITES section in the sidebar *(E27-S1, 2026-09-06)*

**The mock** draws two sidebar sections and no more: `MOUNTED` and `BOOKMARKS`,
both of them lists of *shares*. Nothing in the handoff draws a folder on this
machine in the sidebar, and §"Not yet designed" does not mention one.

**The problem** is that E21-S2 made a folder on this machine a first-class
mount, and then left the only route to one as: open the dialog, pick the Local
tab, type or browse to an absolute path. Every file manager the app is being
compared against puts Home, Documents and the rest one click away, and the
sidebar is where they live.

**What was built:** a `FAVORITES` section above `MOUNTED`, seeded on a first run
with Finder's own set — Home, Applications, Desktop, Documents, Pictures and the
platform's video folder — and added to from the file list's context menu (`Add
to Favorites`, offered for one folder on a local mount) or by dropping folders
onto the section. A favourite's own context menu offers `Remove from Favorites`.
The list is JSON in the app's data directory beside `bookmarks.json`, and
opening one goes through the same `mount` a local bookmark does.

Seven judgement calls, in the order they will be argued about:

1. **Single click opens a favourite**, where a bookmark needs a double. The two
   rows are doing different things: a bookmark is *selected* first — the
   sidebar footer's − and Edit act on the selection — and mounting a share over
   the network is not something to do by brushing past a row. A favourite has
   no selected state and opens a folder on this disk.
2. **A favourite whose folder is missing is not drawn.** Clicking it could only
   fail. It stays in the saved list, so an ejected disk coming back brings the
   row back — hidden, never forgotten.
3. **The video folder is labelled by the folder that is actually there**:
   `Movies` on macOS, `Videos` (or whatever `XDG_VIDEOS_DIR` says) on Linux.
   Labelling it "Videos" everywhere would name a folder a Mac does not have.
   By the same rule Linux gets no `Applications` row at all, because it has no
   such folder anyone browses.
4. **Opening a favourite also leaves a bookmark behind.** The window's model is
   one mount at a time and a mount is a bookmark, so a favourite that has been
   opened appears under `MOUNTED` and afterwards under `BOOKMARKS`. The
   alternative is a second way to hold a mount, and two answers to "what is
   this window on".
5. **A drop highlights the whole section**, not a row: the drop adds to a list
   rather than landing in any one folder. The ring says `Add to Favorites` even
   when the drag is carrying a file, because the pasteboard carries no kind
   (see the E24-S2 entry above) — the filesystem is asked after the release,
   files are skipped, and a toast says how many were.
6. **A favourite row is one line**, name only, with the path as its tooltip.
   The two-line sidebar row has a host or a protocol to put underneath; a
   favourite has only its path, which is both too long for the column and the
   thing least worth reading at a glance.
7. **Removing a favourite has no undo**, where removing a bookmark does. The
   bookmark's undo exists because a removal also forgets a keychain password;
   a favourite is one path and can be dragged back.
8. **Every favourite is connected at startup, invisibly** *(2026-09-07, asked
   for directly: "all of the local favorites should be preconnected on
   startup")*. The app holds each of these folders from launch, before any row
   is clicked. Nothing on screen says so: the section looks the same, the row
   does not appear under `MOUNTED`, and the window is not put on any of them —
   opening one still goes through the bookmark route and still leaves a
   bookmark behind (item 4 above). What it buys is the transfer queue: a queue
   restored from the previous run names shares nobody has mounted yet, its rows
   now wait for them rather than failing, and this is what they wait for. A
   favourite whose disk has been ejected fails silently — no banner, no failed
   row — which is the same rule item 2 states for drawing it.

**For the designer:** the section itself needs sign-off before any of the
above. Then: whether a favourite should be reorderable by dragging within the
section (it is currently append-only, in seed order); whether the row wants an
icon, which every other file manager gives it and which this sidebar has no
vocabulary for; whether `Add to Favorites` belongs in the context menu, on a
toolbar, or on ⌘T the way Finder has it; and whether removing should confirm,
undo, or neither. Copy note: the section reads `Favorites` because that is what
the platform calls it, while this repo's prose is otherwise British — if the UI
should be `Favourites` everywhere, it is one string in `Shell.tsx` and two in
`menu.ts`.

**Favourites carry glyphs the handoff does not draw.** The mock's sidebar is
text only, and a text-only Favorites section is legible but slow to scan: six
rows of similar-length words, where every OS the user came from gives each of
these folders a picture. So each row now carries a 16px glyph — a house, an app
grid, a display, a page, a photo, a film frame, and a plain folder for anything
added.

Two things a designer should rule on. **They are our drawings, not Apple's**:
Finder uses SF Symbols, whose licence does not permit shipping them outside an
Apple framework, so these approximate the shapes rather than reproduce them —
close enough to recognise, not identical, and they will not track Apple's when
those change. And **the glyph is chosen by path, not by name**: only the
platform's real Documents folder draws a page, while a project directory called
`Documents` draws a plain folder. That is what Finder does, but it means two
rows reading the same word can carry different pictures, which is either
exactly right or confusing depending on how it reads on screen.


## The transfers header is a menu, not three buttons *(E13-S1, 2026-09-07)*

**The mock** (§Screens/4) draws a 34px header reading `Transfers` with two
buttons — `Pause all`, `Clear finished` — and a `✕`. Three controls, and the
queue it is drawn over is a handful of rows.

**What was built:** `Pause all` and `Clear finished` moved behind a `☰` beside
the `✕`, joined by two verbs the mock has no reason to imagine — `Retry failed`
and `Clear failed` — and the menu is the same `ContextMenu` the file list uses,
so Escape, the arrow keys, the focus ring and the greyed-with-a-reason rows are
the ones already designed.

*Asked for directly, 2026-09-07: "there should probably be a hamburger menu or
similar that has clear failed, retry failed".* The queue that prompted it held
261 rows, 260 of them failed for one reason, and there was no way to act on all
of them at once: a `Retry` per row and a `Clear finished` that deliberately does
not take failures. Five buttons in a 34px header is not the answer either.

Three decisions inside it, each of which a designer could reverse:

1. **`Clear failed` is a separate verb from `Clear finished`.** Folding `Failed`
   into "finished" would have been one item instead of two, and it would sweep
   away rows that still carry a `Retry` the user has not pressed — the argument
   `crates/engine/tests/queue.rs` has held since E13-S1, and having an explicit
   `Clear failed` does not weaken it.
2. **A verb with nothing to act on is greyed, not hidden**, with the count in
   the label — `Retry failed (260)`, or `Retry failed` in grey with
   `Nothing has failed` as its tooltip. A menu whose rows move between openings
   has to be re-read each time; one with a greyed row can be learned once.
3. **`✕` stays a button.** It acts on the panel; everything in the menu acts on
   the queue.

**For the designer:** whether `☰` is the right mark for it (Finder would use a
`⋯` in a circle, and this app has no vocabulary for either yet); whether
`Pause all` deserves to stay in a header it has been in since the first mock
rather than joining the others; and whether the counts belong in the label at
all, given the footer three rows below already reads `2 active · 4 queued ·
260 failed`.

## A queued transfer can be waiting for its share *(E13-S3, 2026-09-07)*

**The mock** gives a queued row one string: `queued`. It has no state between
"waiting for a worker" and "moving".

**What was built:** a row whose share is not connected reads
`queued · waiting for its share`, and stays that way until the mount arrives.

It is a bug fix rather than an invention, and the bug is worth writing down
because the design has no room for it. Mounts are made when a window connects,
so at launch there are none — and the queue restored from the previous run is
full of rows naming shares from it. The worker started them anyway and settled
every one as `Failed` with `connection lost: … is not connected`, which is how a
real `transfers.json` on this machine came to hold 261 rows of which 260 had the
same error. A share that has not been mounted *yet* is a wait; the row now says
which.

**For the designer:** the string is the row's own, sharing the slot a finished
row uses for `unchanged`. Whether a waiting row should look different from a
merely queued one — dimmed, or with the share's name in it — is open. So is
whether the panel's footer should count them separately: `260 waiting` and
`260 queued` are different situations and it currently says only the second.
