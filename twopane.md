# Excalibur — the two-pane browser

*Asked for directly 2026-09-09: "add epics and stories to build out the design
reference, so that it uses the xsync protocol and that it is used like filezilla".*

The design handoff was re-issued on 2026-09-09 with two new artboards. `design_handoff_share_browser 2/`
is a strict superset of the folder beside it: options `1a`–`1h` are unchanged, and
options `2a` (light, read-write remote) and `2b` (dark, read-only remote) are new,
along with the *Two-pane specifics* and *Mode switch* paragraphs in §Interactions and
the two-pane half of §State Management.

Those two artboards are the whole of this document. Everything in `1a`–`1h` is either
built or already owned by a story in `backlog.md`; nothing here re-specifies it.

Story ids `XTP-E<n>-S<m>`, in their own namespace, on the `XDL-` precedent
(`../AGENTS.md:86`) — the plan namespace is full to `E27` and this is a body of work
of its own, not a stripe across the existing epics. **AC** = acceptance criteria. Handoff references
look like *(handoff §Screens/7, Transfer gutter)*.

---

## Decisions taken

- **Two panes, not two windows.** Excalibur already has real multi-window
  (`new_window`, `src/state/window.ts`) and M7 shipped a drag between two windows.
  The handoff's `2a` is not that: it is one window whose body is split, with a shared
  drawer and one focused pane driving the toolbar. Windows stay; this is a view of
  one of them, chosen from the View menu beside the row layouts and column view
  (XTP-E7).

- **The local pane is a mount, not a special case.** `Protocol::Local` and
  `LocalBackend` already exist and implement the same `ShareBackend` as `XsyncBackend`
  (`crates/engine/src/backend/local.rs`, `backend/mod.rs:295`). The handoff asks for
  exactly this — "Local-side listing for the two-pane view goes through the same command
  surface (a `local` protocol) so both panes share one code path" *(handoff §State
  Management)*. So the left pane is an ordinary pane pointed at a local mount, and
  "local vs remote" is a label on the pane, not a branch in the code.

- **xsync is the transport this view is designed against.** The handoff's prose is
  SMB/NFS throughout — "SMB tree connect flags, NFS export options", "SMB3 · aes-128-gcm",
  `ro,root_squash`. That language is a placeholder for a fact xsync already reports
  better: `MountInfo.effective_writable` and its `reason`, computed server-side from
  `--read-only`, requested access, and a probed write barrier
  (`../xsync/crates/xsync-core/src/server.rs:2140`), and enforced on every write-class
  request (`:5075`). XTP-E9 restates each SMB/NFS-shaped requirement in xsync's terms
  rather than inventing an SMB-flavoured equivalent. The other backends keep working
  because they are behind the same trait; they simply supply a thinner truth.

- **No new transfer engine.** The queue, worker, named queues, conflict handling,
  pause/cancel/retry and persistence are built and live in Rust
  (`crates/engine/src/transfer/`). The drawer is a second view onto the same queue, and
  the gutter is a second way to enqueue. `Backend` is already pane-agnostic — `mount` is
  an explicit optional argument on `rename`, `enqueueDownloads`, `enqueueUploads` and
  `checkConflicts`, added for cross-window drag (E24-S2) — so pane-to-pane transfer
  plumbing largely exists.

- **The message log needs a change in xsync.** Nothing in `xsync-client` can observe
  wire frames: the crate has no `tracing` dependency and no callback, channel or observer
  anywhere. `log://line` is specced by the handoff and implemented nowhere. XTP-E5-S0
  files that against `../xsync`; see `integration.md` §8.

- **Everything here is past the mocks in at least one respect** — the handoff itself
  lists the collapsed drawer state, the sync-browsing mismatch warning and the
  disconnected banner as *Not yet designed*. Per the Part II rule
  (`legacy/plan.md:4157`), each epic opens with a design spike where it goes beyond
  `2a`/`2b`, and anything shipped past them gets an entry in `docs/design-gaps.md`.

---

## XTP-E1 — The pane as a thing

`src/App.tsx` is one 2828-line root that owns exactly one of everything the handoff
wants two of: one `history` signal (`:160`), one `listing`, one `selection` (`:466`),
one sort, one filter, one column-width set (`:266`). The body is a fixed
`Sidebar + listColumn` (`:2596`). There is no dual-pane, split-view or splitter code
anywhere in `src/`, `src-tauri/src/` or `crates/engine/src/`.

This epic is the long pole and blocks every other epic here. It ships no visible
feature: done means single-pane behaves exactly as it does today, with its state
living in one pane instance.

#### XTP-E1-S1 — Extract a `Pane` model
**AC**
- A pure module `src/state/pane.ts`, with a sibling test, owning one pane's
  `{ mountId, cwd, history, listing, selection, sort, filter, columnWidths, showHidden,
  treeExpanded, freeBytes }` and the transitions over them.
- The existing `src/state/{navigation,listing,selection,sort,filter,columns}.ts` modules
  are composed by it, not duplicated or rewritten — they are already pure and tested.
- `App.tsx` holds a `Pane` instance instead of the loose signals, and drives the same
  single-pane UI from it.
- No behavioural change: the existing suite passes untouched, and the single-pane
  screenshots are unchanged.

#### XTP-E1-S2 — Two instances and a focused pane
**AC**
- `panes: { local: Pane, remote: Pane }` and `focusedPane: 'local' | 'remote'`.
- Toolbar verbs, Quick Look, Edit, the keyboard shortcuts in `src/state/shortcuts.ts`
  and the filter all read the focused pane; nothing reads a global "current" anything.
- Focus follows the last-clicked pane. Tab switches panes. The focused pane's list
  carries a 1px accent inner border *(handoff §Two-pane specifics)*.
- One pane is the same code with one pane instance rendered.

#### XTP-E1-S3 — A pane on a local mount
**AC**
- The local pane opens a `Protocol::Local` mount through the ordinary mount path and
  lists through the same `Backend.listDir`; no local-only listing code is added.
- Its status bar reads its own free space from the mount's capacity, which
  `LocalBackend` already probes via `statvfs`.
- The `..` parent row, sorting, filtering, rename, delete and drag behave as they do in
  a remote pane, because they are the same code.

---

## XTP-E2 — The two-pane window

*(handoff §Screens/7)*

#### XTP-E2-S1 — Pane shell and the splitter
**AC**
- Window body is local pane (`flex: 1`) + 44px gutter + remote pane (`flex: 1`), 1px
  `#dcdcde` between, matching `2a`'s metrics.
- Each pane is the handoff's vertical stack: pane title 26px, path field 24px, optional
  tree, column header 24px, list (`flex: 1`), status bar 24px.
- Pane title band: 10px/600 uppercase `.07em` — "LOCAL SITE" / "REMOTE SITE" — with the
  remote pane carrying its full URI right-aligned in mono 9.5px.
- The split is user-draggable and the ratio persists.
- Local column header is `1fr 88px 116px` (Name/Size/Modified); remote adds Kind at
  `104px`. Below ~900px Kind drops, then Modified, reusing the existing responsive rule
  in `src/state/columns.ts`.
- Minimum useful window ~820×480 *(handoff §Responsive)*.

#### XTP-E2-S2 — Per-pane status bars
**AC**
- Local: a `LOCAL` badge (mono 9.5px on `#e2e2e5`), item and selection counts with
  selected size, and free space right-aligned.
- Remote: the `RW`/`RO` badge, the plain-language restatement of the reason, a divider,
  item count, and "free of total" right-aligned.
- Both read from the pane, so neither can drift from the other or from the listing —
  `list_dir` already returns mount facts alongside entries, which is the property that
  makes this true. The event stream and the banner are the window's and describe the
  *active* mount, so they overlay only the pane holding it: a reconnect on one share
  must not redden the other pane's dot.
- **Capacity stays inline and the protocol line keeps the right edge** until XTP-E2-S3
  moves it to the toolbar; two things cannot claim that corner at once. Recorded in
  `docs/design-gaps.md`.
- `RO` outranks `LOCAL`, which the handoff never has to decide because it never draws a
  read-only local folder. A folder you cannot write is a warning; which machine it is on
  is not.

#### XTP-E2-S3 — The two-pane toolbar
**AC**
- Direction is spatial here, so `Download` and `Upload` are absent from the
  toolbar entirely *(handoff §Screens/7, Toolbar)*.
- Left to right: `Site Manager`, `Reconnect`, divider, `Quick Look`, `Edit` (both acting
  on the focused pane), divider, `Sync browsing` and `Show hidden` checkboxes, then the
  right-aligned protocol line and health dot.
- Every verb keeps its existing shortcut and gains a tooltip naming it. A verb that is
  refused shows the reason instead: teaching a shortcut for something the user cannot do
  is worse than saying nothing.
- **No `Site Manager`** — it stands in for the sidebar the handoff's two-pane window does
  not have, and this one keeps its sidebar in both modes (XTP-E8-S1).
- **No `Sync browsing` checkbox** until XTP-E6 builds the feature; a checkbox that does
  nothing is worse than its absence. **No `Show hidden` checkbox** either: it has been in
  the View menu since E17-S1, and two controls for one setting have to agree forever.
- **The protocol line and health dot stay in each pane's status bar.** The handoff's
  toolbar can carry one because its window has exactly one connection; either pane here
  can be remote and both can be, so one line above two panes would describe one of them.
  Capacity is right-aligned beside it, which is what XTP-E2-S2 asked for.
- **`Reconnect` keeps the access the share was mounted with.** It shared
  `remount_read_write` with `Remount as RW`, which asks for read-write whatever the mount
  was opened as — so recovering a dropped connection silently un-protected a share opened
  read-only. `reconnect` is its own engine call now, and the banner uses it too.

#### XTP-E2-S4 — The directory tree
**AC**
- A per-pane tree band, 104px default, `#fafafb` light / `#26272a` dark, 20px rows, 16px
  indent per level, 8px disclosure `▶`/`▼`, selected row `#e8effd` with a `#7f9fe0` glyph.
- Collapsible to zero and resizable; the state persists per pane. Both panes collapsed
  is a legitimate state, which is what `2b` draws — not an omission.
- Expanding a node lists that directory through the same `Backend.listDir` and caches it
  for the session; selecting a node navigates the pane.
- This is the first tree in the app — `MoveTo.tsx:18` explicitly declined one — so it is
  a new component with its own tests rather than a generalisation of anything existing.
- **Two panes only, and reopened from the View menu.** The handoff draws the tree in its
  two-pane screens alone; a single-pane window already has the sidebar, and a second
  navigator beside it would be two places to click the same folder. Nothing in the mocks
  reopens a band collapsed to zero, so `Directory Tree` joins the View menu beside
  `Two Panes` — it acts on the focused pane, and is greyed with `Two panes only` where
  there is no band to open. Recorded in `docs/design-gaps.md`.
- **The folder the pane is in comes from the pane's own listing.** `list_dir` has already
  returned it; asking again would be a second round trip for an answer on screen, and a
  second answer is how a tree comes to disagree with the list beside it. So the tree
  fetches only the folders the pane has not listed, and a window costs no extra request
  to draw one.
- A fixed band above a list is what turns CSS's flex-column trap into a visible bug: the
  list would not shrink, so it overflowed the pane and pushed the status bar out of a
  short window. `scrolling.test.ts` states the rule for every growing column.
- Selecting a row navigates; the selected row is the pane's folder rather than a
  selection of the tree's own. Navigating *into* a folder reveals it — its ancestors
  open, it does not — which is what `2a` draws, and what stops a band five rows tall
  unfolding under the user.

---

## XTP-E3 — Moving files between panes

*(handoff §Screens/7, Transfer gutter; §Two-pane specifics)*

#### XTP-E3-S1 — The transfer gutter
**AC**
- 44px column, two 30×30px buttons, 8px gap, vertically centred.
- `▶` uploads the local selection into the remote cwd; `◀` downloads the remote
  selection into the local cwd. Each enqueues through the existing
  `enqueueUploads`/`enqueueDownloads` with an explicit `mount`.
- Whichever direction has a selection renders primary (accent fill); the other renders
  secondary; a direction whose target is not writable is disabled with the tooltip
  "Upload blocked — remote is read-only".
- Disabled state derives from the same `mount.writable` value as the status badge,
  through `src/state/permissions.ts`, so a blocked write is never attempted.

#### XTP-E3-S2 — Double-click and drag between panes
**AC**
- Double-click a file transfers it in the obvious direction; double-click a folder still
  descends *(handoff §Two-pane specifics)*.
- Drag a selection from one pane to the other queues a transfer; drag within a pane
  moves. `src/state/dnd.ts` already models drops and spring-loaded folders; this extends
  `resolveDrop` rather than adding a second drag path.
- Dragging from the OS onto either pane uploads to that pane's cwd, with the existing
  accent inset drop ring.
- Conflicts use the built prompt (Replace / Keep both / Skip). **Note:** `Keep both` is
  currently broken — it computes a free name, shows it, then re-sends the original paths
  and behaves as `Replace` (`backlog.md:117`). That bug is in the shared path this story
  depends on and should be fixed with it, not around it.

#### XTP-E3-S3 — In-flight rows
**AC**
- A file mid-upload appears in the destination list before it exists there: row bg
  `#fafafb`, name `#6a6a70`, dashed `#7f9fe0` glyph, Size showing bytes transferred in
  accent, Modified reading "transferring…".
- It becomes an ordinary row on completion, without a full relist.
- Driven by the existing `transfer://progress` event, which already carries the whole
  `Transfer` on every tick — so a dropped event costs latency, not correctness.

---

## XTP-E4 — The drawer

*(handoff §Screens/7, Docked drawer)*

Today the transfer queue is a floating sheet anchored under the toolbar, or a full-column
embedded view reached from a sidebar category (`src/components/Transfers.tsx`). Nothing is
docked to the bottom and there is no tab strip. The queue itself does not change.

#### XTP-E4-S1 — Dock, tabs and collapse
**AC**
- A bottom-docked drawer whose height persists, collapsible to just its 26px tab strip.
- Tabs `Queued (n)`, `Failed (n)`, `Finished (n)`, `Log`; active tab gets a 2px accent
  bottom border and 600 weight. A non-zero Failed count may render as a `#c0392b` pill
  instead, as `2b` draws it.
- Right-aligned `Pause all` and `Clear finished`, bound to the existing commands.
- A new failure auto-selects the Failed tab **only if the drawer is already open** — it
  never opens the drawer on its own.
- The collapsed state is one of the handoff's *Not yet designed* items; ship to this
  description and file a `docs/design-gaps.md` entry.
- **Docked with two panes, floating with one.** Same component, same queue, same
  handlers — only the chrome differs, so the two shapes cannot come to show different
  rows. The handoff draws the drawer in its two-pane screens alone, and the reason
  generalises: a sheet anchored under the toolbar covers one of the two columns the
  transfer is between.
- **The tabs are the CATEGORIES already there** (XDL-E2-S1: All, Downloading, Paused,
  Completed, Failed) rather than the handoff's `Queued`/`Failed`/`Finished`. They name
  the same rows — "Queued" there includes what is running, which is "Downloading" here —
  and two strips naming one filter differently is the drift this epic keeps removing.
  A non-zero Failed count becomes a pill, as `2b` draws it.
- **No `Log` tab** until XTP-E5 builds the message log; a tab with nothing behind it is
  worse than its absence. **`Pause all` and `Clear finished` stay behind the `☰`**, which
  is where a direct ask put them (2026-09-07) and where two more verbs since have joined
  them.
- Collapse is one number, `0` meaning the strip alone: a height and a flag could
  disagree. It reopens at the height it was, which the window remembers because the store
  cannot hold both at once.

#### XTP-E4-S2 — The queue table
**AC**
- Header 22px, grid `52px 1fr 1fr 84px 148px`, `column-gap: 16px`: Dir, Local file,
  Remote file, Size, Progress.
- Dir is mono 9.5px `↑ UP` accent / `↓ DOWN` secondary; the two file columns are mono
  10.5px and ellipsize; Progress is a 4px bar plus mono 9.5px "67% · 112 MB/s", and a
  queued row shows an empty track and the word "queued".
- Rows virtualise, reusing `src/state/virtual.ts`; the existing `DownloadsTable` is
  extended or a sibling is added, not forked wholesale.
- Drawer footer 24px: "1 active · 1 queued · 1 failed", divider, "ETA 3m 04s",
  right-aligned mono aggregate rate.
- **A `Transfer` has no `Local file` / `Remote file` to render yet.** It still carries
  `Direction::{Up,Down}`; the `from`/`to: Endpoint` change that gives a row two named
  ends is E22-S3, untouched (`backlog.md:107`). This story either lands after it or
  renders source and destination from the pane pair that created the row and says so.
- **Taken the second way.** `endsOf` reads the endpoint pair where there is one — HTTP
  rows have had one since XDL — and derives the two ends from `direction` where there is
  not, which is the same fallback `originHost` has always made. A row that names neither
  gets a dash rather than a path that might be the wrong way round.
- **Two column sets, one table.** `layout="sites"` in the drawer, `"downloads"` in the
  sheet: same rows, same context menu, same virtual window, different cells. The queue
  holds two kinds of work and the two references answer differently — for a download the
  far end is a URL and what matters is speed, ETA and status; for a transfer between two
  panes the row is between two named places. The trailing actions column is a fixed
  width, because an `auto` that is empty in the header and full of buttons in the rows
  puts every column out of line with its own heading.
- The row's queue picker is dropped in the drawer — queues are the download manager's
  idea, and the row's menu still carries `Move to queue`.
- The footer's counts and ETA are shared with the sheet: they are better numbers there
  too. The ETA is bytes-remaining over aggregate rate, not a sum of the rows' own — those
  run in parallel, so adding them over-reports by the number of workers.

---

## XTP-E5 — The message log

*(handoff §Screens/8, Log tab)*

The handoff is specific that this is not a convenience: "The log is where protocol truth
lives: mount command and its export flags, listing timings, and failures verbatim". It is
also the one thing in `2a`/`2b` that cannot be built inside Excalibur.

#### XTP-E5-S0 — Prerequisite in `../xsync`: a frame tap on the client
**Done 2026-09-09** (`xsync` `0851e6d`). `Client::from_split_tapped` takes an
`Fn(Direction, id, &V3Message)`, armed before the opening handshake. It sits at the send
and at the reader rather than at `Connection::request`'s reply: that catches frames
nobody is waiting for as well, and keeps a server `Error` verbatim, which is what the AC
was protecting. The observer is handed a borrow — a `V3Message` can carry a read's
payload — so back-pressure and dropping are the observer's, which is where the AC's
"reports the drop" belongs. Excalibur does not install one yet.
`xsync-client` has no `tracing` or `log` dependency and exposes no callback, channel or
observer; every frame goes through the private `Connection::request`
(`../xsync/crates/xsync-client/src/lib.rs:286`). Filed as `integration.md` §8.

**AC**
- `xsync-client` gains an opt-in observer — an `mpsc` tap or an `Fn(&V3Message, Direction)`
  installed at `Client::from_split` — through which every request and reply passes.
- The tap sits at `Connection::request`, not `Connection::call` (`:322`): `call` turns a
  server `Error` reply into an `Err` and loses the frame, and the design's `Error:`
  category needs it verbatim.
- Off by default and allocation-free when unarmed; a slow consumer drops frames and
  reports the drop rather than applying back-pressure to the protocol.
- `V3Message` already derives `Debug`; no new formatting contract is invented in xsync.

#### XTP-E5-S1 — `log://line` and the log pane
**AC**
- A `log://line` Tauri event carrying `{ at, category, text, action? }` with category in
  `command | response | status | listing | hint | error`, and a capped ring buffer behind
  it, per *(handoff §State Management)*.
- The log pane renders `padding: 7px 12px`, mono 10.5px, `line-height: 1.7`, timestamp
  `#9a9ba0`, coloured category label (`Command:` accent, `Response:`/`Listing:`/`Hint:`
  secondary, `Status:` `#2f9e5f`, `Error:` `#c0392b`), message `#c4c4c8`.
- Lines are selectable and copyable; drawer controls are a `Verbose` toggle, `Copy`,
  `Clear`.
- An error line may carry an inline recovery button — `2b` draws `Remount as RW` on the
  hint line, and `remount_read_write` is already a command.
- The log records in both views; only its surfacing differs. One pane keeps using the
  separate Transfers window.
- Backends that cannot produce wire frames log at the `ShareBackend` seam instead, and
  the pane says which resolution it is showing rather than implying frames it does not
  have.
- **Built at the seam, without XTP-E5-S0.** Every backend is wrapped in `LoggedBackend`,
  so a verb reaches the log by crossing the trait rather than by somebody remembering to
  write a line beside the call. The pane carries a pinned `Engine operations — not
  protocol frames`; when the tap lands, frame lines join the same stream and the note is
  what changes.
- **Verbose is a category filter, not a second stream.** Off leaves out `Response` and
  `Listing` — the chatty half, one of each per directory browsed — and keeps what was
  asked for, what the mount is doing, what failed and what to do about it.
- Byte-moving verbs (`read_range`, `write_range`, `open_read`) are deliberately not
  logged: one transfer makes thousands, and a log they fill is one nobody can read.
- The hint's recovery acts on the **line's** mount, not the focused pane's: a log is read
  after the fact and the pane has usually moved on.
- The ring is capped at 500 lines in the engine and again in the window, so a window left
  open for a week holds what the engine holds.

---

## XTP-E6 — Sync browsing

#### XTP-E6-S1 — Mirror one pane's navigation onto the other
**AC**
- On by default in `2a`, off in `2b`; the flag persists.
- Navigating one pane navigates the other to the same relative subpath.
- Where the mirrored path does not exist: keep the other pane put, show a single inline
  warning in that pane's title band — never a modal — and offer to create the folder when
  the target is writable.
- The warning's appearance is *Not yet designed*; ship to this description and file a
  `docs/design-gaps.md` entry.

---

## XTP-E7 — Two panes as a view, not a mode

*Revised 2026-09-09, asked directly: "i am wondering whether it should be just a
togglable view like columns/icons and share the same bookmarks".*

Yes to the menu, and the answer got better since this was written: the View menu
now exists and holds five layouts, and column view already established that a
"layout" may be a component of its own drawing more than one listing. Two panes
belongs in that menu, chosen the same way, and there is no case for a separate
mode with its own switch.

**But it is not a `rowLayout` value, and that is the thing worth getting right.**
Icons and column view *replace* how a listing is drawn. Two panes does not —
each pane still draws rows, in one of the row layouts. So the window needs both
answers at once, and folding two panes into `rowLayout` would make
"two panes, name-led rows" unsayable. It is a second axis, shown in the same
menu under its own rule.

That also settles the open question this document used to carry: `1g`/`1h`
apply *per window*, not per pane, because they stay what `rowLayout` means.

#### XTP-E7-S1 — The View menu's second axis
**AC**
- `paneMode: 'single' | 'dual'` in `state/settings.ts`, beside `rowLayout` and
  persisted the same way — not a member of `RowLayout`.
- One more row in the View menu, ruled off from the layouts because it answers a
  different question, with the tick treatment the other rows use
  (`state/viewMenu.ts`).
- A shortcut in `state/shortcuts.ts` continuing ⌘1–⌘5.
- Switching preserves the focused pane's mount, cwd and selection; switching
  back leaves the window as it was found.
- Every row layout stays available in both, and applies to both panes at once.
- Column view and icons in a pane are refused rather than nested: a pane is
  narrow and a column view inside one is columns inside a column. The menu says
  so rather than silently drawing something else — one of the few places this
  design has to state a rule the handoff never faced.

#### XTP-E7-S2 — What the toolbar becomes
**AC**
- Direction is spatial with two panes, so `Download` and `Upload` leave the
  toolbar *(handoff §Screens/7, Toolbar)* and the gutter's arrows take over
  (XTP-E3-S1).
- `Quick Look`, `Edit`, the filter and the View menu keep working and act on the
  focused pane.
- `＋ Add URL` and `Transfers` are app-wide and do not move.
- **One editable path field, in the toolbar, belonging to the focused pane.**
  The handoff draws one per pane, having no toolbar field to compare against;
  this window has one that already takes a typed path or URI (E1-S2), and two
  editable fields would be two places to type the same thing. Each pane's title
  band shows its own path, read-only, so both are legible at once and only the
  focused one is editable. Recorded in `docs/design-gaps.md`.

---

## XTP-E8 — One set of bookmarks

*Revised 2026-09-09, same ask: "share the same bookmarks".*

The handoff moves the sidebar's job into a `Site Manager` dialog for the
two-pane screens, and this document used to plan one. That was the handoff
solving a problem it had given itself by drawing two panes as a separate
application; with two panes as a view of *this* window, the sidebar is already
there and already right. **The Site Manager epic is dropped** — one bookmark
store, one connection dialog, one keychain path, and one less surface to keep in
step.

What the sidebar does not yet answer is which pane a row acts on. That is the
whole of this epic, and it is a question the handoff never had to ask.

#### XTP-E8-S1 — A bookmark opens into the focused pane
**AC**
- Double-clicking a bookmark mounts it into the focused pane, leaving the other
  untouched. Focus is the same value XTP-E1-S2 already tracks and the same value
  the toolbar's verbs read, so there is one answer to "which pane" in the window
  rather than two.
- The focused pane is visibly so before the click — the 1px accent inner border
  from XTP-E1-S2 — because a bookmark that opens in the wrong half is a
  destination the user did not choose.
- With one pane the behaviour is what it is today, since the focused pane is the
  only pane.

#### XTP-E8-S2 — MOUNTED with two mounts
**AC**
- `MOUNTED` lists both, because the window holds both; the sidebar's active
  treatment marks the focused pane's mount.
- `activeId` stops being "the one mounted bookmark" — it is the focused pane's,
  and `App.tsx`'s current derivation (`bookmarks().find(b => b.mounted)`) cannot
  express that.
- Unmounting the focused pane's share leaves the other pane alone.

#### XTP-E8-S3 — Dropping onto a sidebar row, with somewhere to drop from
**AC**
- A drop onto a share row copies to that share's root — the E24-S2 behaviour
  that exists — and now has an obvious source: a selection in the other pane.
- The drop is refused, with the reason, when the target is not writable; the
  same `mount.writable` value everything else reads (XTP-E9-S1).
- A drop onto the row of a share that is already one of the two panes is a
  copy into it, not a no-op.

---

## XTP-E9 — xsync as the two-pane transport

The handoff's transport-specific language, restated against what xsync actually reports.
Each of these is a place where building the mock literally would produce something less
truthful than what the protocol already offers.

#### XTP-E9-S1 — Writability from the mount, not the dialog
**AC**
- The `RW`/`RO` badge, its reason string, the disabled state of `Edit`, the gutter's
  upload arrow, rename, delete, new folder and drop-target acceptance all derive from one
  value, as the handoff requires *(handoff §Permissions gating)*.
- Over xsync that value is `MountInfo.effective_writable` with its `reason`, which the
  server computes from `--read-only`, the requested access and a probed write barrier
  (`../xsync/crates/xsync-core/src/server.rs:2140`) and enforces on every write-class
  request (`:5075`) — so the UI's claim and the server's behaviour cannot disagree.
- The reason renders in the handoff's plain-language form. `ro,root_squash` is NFS's
  vocabulary; xsync's `ExportReadOnly` / `FilesystemReadOnly` / `NoPermission` /
  `Unwritable` are shown in their own words rather than translated into export flags.
- Other backends supply what they have; the surface is identical.

#### XTP-E9-S2 — Capacity and the protocol line
**AC**
- "free of total" comes from `Mount::statfs()`, using `available_bytes` — not
  `free_bytes`, which counts reserved blocks the user cannot have.
- `FsInfo.total_inodes`, `free_inodes` and `fs_type` are hardcoded to zero and empty by
  the server (`../xsync/crates/xsync-core/src/server.rs:2790`); nothing in the UI may
  render them until that is fixed.
- The right-aligned protocol line reads xsync's negotiated truth — version, transport,
  and the negotiated feature set — in place of the mock's "SMB3 · aes-128-gcm". The
  server advertises `STAGE_RESUME | PATH_LISTING | SUBTREE_LISTING | PATH_READ`
  (`../xsync/crates/xsync-core/src/server.rs:4535`), so what the line can honestly say is
  known.

#### XTP-E9-S3 — Listing latency in a view that lists twice as often
**AC**
- `XsyncBackend::list_dir` opens a directory handle, pages it, then closes it
  (`crates/engine/src/backend/xsync.rs:812`). `READ_DIR_PATH` (type 98) answers the same
  question in one round trip and the server advertises `PATH_LISTING`; switch to
  `Mount::read_dir_path`, falling back to the handle path when the feature is absent or
  the directory exceeds `max_entries` (in which case it returns `None` rather than paging).
- The directory tree in XTP-E2-S4 uses `READ_TREE` (type 128) where `SUBTREE_LISTING` is
  negotiated, so expanding a node costs one round trip rather than one per level.
- Sync browsing doubles the listing rate, so this is measured: a listing that took two
  round trips takes one, recorded in `docs/benchmarks.md`.

#### XTP-E9-S4 — Refresh without Notify
**AC**
- Two panes and sync browsing make a stale listing twice as visible, and xsync has no
  change notification: types 110–115 are reserved for Phase 4 with no constants, no codec
  and no handler (`../xsync/legacy/xsyncv3.md:1657`), the server never sets
  `supports::NOTIFY`, and E8-S9 is already parked on exactly this (`backlog.md:51`).
- Until Notify exists: relist a pane on focus and after any transfer completes into its
  cwd, through the built 250 ms coalescer (`src/state/refresh.ts`). No polling timer.
- A listing under ~200ms shows no spinner; past that, a thin indeterminate accent bar
  under the column header with the previous listing kept and dimmed *(handoff §Loading)*.
- When Notify lands, this story's polling is deleted rather than kept alongside it.

#### XTP-E9-S5 — Cancel at a chunk boundary
**AC**
- The drawer's per-row cancel stops a transfer promptly without dropping the session.
- `CANCEL` (type 18) exists and the server handles it, but `xsync-client` exposes no way
  to send one — over the client crate a request is abandoned only by dropping the
  `OpenFile`, `Stage` or `Client`. So cancel operates at chunk boundaries in the transfer
  loop, via the existing `CancelToken`, and the UI does not promise to interrupt a read
  already in flight.
- Exposing `CANCEL` on the client is filed as `integration.md` §8; when it lands, this
  story tightens rather than changes shape.
- Resume is unaffected and already strong: `STAGE_RESUME` is advertised, and a stage's
  resume token survives both the connection and the server process.

---

## Milestones

| Milestone | Stories | Demo |
|---|---|---|
| **XTP-M1 One pane, factored** | XTP-E1-S1, S2 | Single pane behaves exactly as today, with every piece of its state living in one `Pane` instance and the suite green |
| **XTP-M2 It has two sides** | XTP-E1-S3, XTP-E2-S1/S2/S3, XTP-E9-S1 | `~/Downloads` on the left, an `xs --server` share on the right, both listing, both status bars honest about what is writable |
| **XTP-M3 It moves files** | XTP-E3, XTP-E4-S1/S2, XTP-E9-S5 | Select four files, hit `▶`, watch them appear as in-flight rows on the right and progress in the docked drawer; cancel one mid-flight |
| **XTP-M4 It shows its work** | XTP-E5-S0, XTP-E5-S1, XTP-E9-S2/S3 | The Log tab shows the mount, its writability reason and every listing as it happens; a read-only upload fails with the server's own words and a `Remount as RW` button that works |
| **XTP-M5 It is a client you keep open** | XTP-E2-S4, XTP-E6, XTP-E7, XTP-E8, XTP-E9-S4 | Trees expanded on both sides, sync browsing on, two panes chosen from the View menu and switched back with the queue intact, a bookmark opened into the focused pane |

**Order.** XTP-E1 is the long pole and everything waits on it; it ships no feature, so it
should not be split across a release. XTP-E5-S0 lives in another repo, so it starts in
parallel with XTP-E1 and is the only thing here with an external dependency. XTP-E9-S1 is
wanted early because the gutter's disabled states in XTP-E3-S1 read from it. XTP-E4-S2
wants E22-S3's `from`/`to: Endpoint` change (`backlog.md:107`) and either follows it or
ships with the narrower rendering named in its AC. XTP-E6 and E7 are independent of each
other.

**Corrected 2026-09-09, found while starting XTP-E3-S1: XTP-E8-S1 comes before it.** The
gutter's arrows enqueue through `enqueueUploads`/`enqueueDownloads`, and those are
share↔local — so a transfer between panes is only expressible once the two panes are on
*different* mounts, which is what E8-S1 gives them. `list_dir` could not even ask for a
second share until then. E7-S1's toggle likewise had to land with E2-S1: a shell behind a
switch that does not exist is unreachable, and the switch without the shell blanks the
window. Two *remote* shares stays refused whatever the order: relaying is E22-S3, blocked
on xsync having no server-side copy.

---

## Definition of done

- [ ] `2a` and `2b` are reproducible from the running app at the metrics in the handoff,
      in both themes, at 1240px and at the ~820px minimum.
- [ ] One derived `mount.writable` gates the badge, `Edit`, the gutter arrow, rename,
      delete, new folder, drop acceptance and Quick Look's footer copy — verified by a
      test that flips it and asserts every affordance.
- [ ] No write is attempted against a read-only remote from either view; `writeGuarded()`
      refuses it before IPC and the server's barrier is never reached.
- [ ] The queue is one queue: a transfer started in two-pane mode is visible, pausable and
      retryable from single-pane mode and survives a restart.
- [ ] The log shows a mount, a listing, a failure and a recovery for an xsync remote, and
      says honestly what resolution it has for a remote that is not xsync.
- [ ] Every story that went past `2a`/`2b` has a `docs/design-gaps.md` entry.
- [ ] Both panes list through one `Backend.listDir`; no local-only listing path exists.

---

## Open questions for the designer

The handoff names these as *Not yet designed*, and each one is reachable in this mode:

- The collapsed state of the drawer (XTP-E4-S1).
- The sync-browsing mismatch warning in the pane title band (XTP-E6-S1).
- The disconnected / reconnecting banner, which in two-pane mode has to say *which* pane
  lost its connection — a per-pane banner, or one banner that names the side.
- Whether the read-only path in each pane's title band is enough, or whether a
  pane wants its own editable field as the handoff draws (decided for now in
  XTP-E7-S2: one editable field, in the toolbar).
- What the protocol line reads for a non-xsync backend, now that it is the only place the
  transport is named in two-pane mode.
- Whether a read-only folder on this machine wants a badge of its own (XTP-E2-S2): `RO`
  alone loses the side, and `RO LOCAL` is two chips where the bar has room for one.
