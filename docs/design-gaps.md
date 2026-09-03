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
