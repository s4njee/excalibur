# xdl — a download manager in Excalibur

A plan for a **Downloads** tab in Excalibur: paste a URL, watch it arrive over
many connections, control it, and keep the record. Written 2026-09-06.

`design_handoff_aria2_download_manager/` is the visual reference. It is a
reference and not a specification — the brief for this work was "loosely based
off", and this document says at each point what is taken and what is not.

## What this is built on, and what that changes

The reference is an **aria2 front end**: a separate daemon reached over JSON-RPC,
with an endpoint, a secret token and a Restart Engine button. Excalibur has no
such thing and should not grow one. The engine here is the sibling repo's
`xsync-download`, whose whole design is one sentence from its own module
comment:

> **a download is a `xsync_endpoint::Source`.**

`read_range` is already a ranged GET; `ReadHandle::read_at` is already a ranged
read on a kept-alive connection; the sink's `stage_open`/`stage_write`/
`stage_commit` are already a `.part` file published by rename. Hand an
`HttpSource` and a `LocalSink` to `sync_endpoints` and the planner, the worker
pool, the verified write path, cancellation and the event stream all work
unchanged.

Excalibur *already drives `sync_endpoints`* — `crates/engine/src/transfer/
endpoint.rs`, built for E22-S2. So a download is not a new subsystem. It is a
third route beside the two that exist, and most of this plan is user interface
rather than machinery.

Three consequences worth stating before any story:

- **There is no RPC to configure.** The reference's screen 6 (host, port, secret
  token, "Connected to aria2 1.37.0", Restart engine, `aria2.changeGlobalOption`)
  has no counterpart and is not being built. What survives from it is the *idea*
  of download defaults, which becomes a page in Excalibur's existing
  Preferences.
- **There is no connection pill.** The titlebar's "RPC connected" says nothing
  when the engine is a library in the same process. Dropped rather than faked.
- **The queue already exists.** E13 built one, E22-S2 made it fast, and it
  persists, restores, and has pause/resume/cancel/retry. A second queue for
  downloads would mean two schedulers, two persistence files and two answers to
  "what is running". See Decision 2.

## Decisions taken

| # | Decision | Why |
|---|---|---|
| 1 | The engine is `xsync-download` through `sync_endpoints`, not aria2, and not a new downloader. | It is already a `Source`, and Excalibur already drives that seam. A second engine would be a second resume journal, a second verification story and a second set of bugs. |
| 2 | **One queue.** A download is a `Transfer` row; the Downloads tab is a *view* over the existing queue filtered to HTTP sources. | Two queues means two answers to "is anything running", two concurrency limits fighting for the same disk, and two things to restore at launch. The row already carries `direction`, `bytes_total`, `status`, `verified` and now `files_total`. |
| 3 | **Excalibur's greys, not the reference's white.** | Asked for directly. The reference's `#FFFFFF` surfaces and `#ECEEEF` canvas are replaced by the tokens in `src/tokens/tokens.css`; nothing in this plan introduces a colour that is not already a token, in either theme. |
| 4 | The reference's *structure* is taken; its chrome is not. | Its titlebar, traffic lights and OS-neutral shell are a standalone app's. This is a tab inside a window that already has a titlebar, a sidebar and a status bar. |
| 5 | Segment visualisation shows **real byte ranges**, or it is not shown. | The reference's own note admits its inline bar fakes the geometry by mapping overall percent onto slices. A progress bar that invents where the bytes are is worse than one that says only how many there are. Gated on XD-E6-S4 (below). |
| 6 | A URL is untrusted input, and so is everything a server says about it. | Filenames, redirects, lengths and content types all come from the other end. `xsync-download` already has `name::` for safe naming and `redact_url` for credentials; this plan never renders a raw URL without it. |
| 7 | Browser capture is a separate product, not a story here. | The reference's screen 4 is an extension popup. It needs a browser extension, a native messaging host and a review process; it is scoped as XDL-E8-S4 and deliberately left last. |

## What `xsync-download` already gives us, and what it does not

Grounded in `../xsync/xsync-downloader.md` as of 2026-09-06. This table is the
honest dependency list; several screens in the reference are gated on the right
column.

**Available**

| Capability | Story |
|---|---|
| `HttpSource`, one URL, one connection | XD-E1-S1 |
| `xs get` and the CLI surface | XD-E1-S3 |
| Safe output naming from URL and headers | XD-E1-S4 |
| Streaming reads, and servers with no `Range` | XD-E1-S6 |
| Parallel segments within one file | XD-E2-S2 |
| Connection count chosen rather than asked for | XD-E2-S4 |
| Preallocation under a parallel writer | XD-E2-S5 |
| One contiguous region per worker, streamed | XD-E2-S7 |
| Silent under-delivery bounded | XD-E3-S4 |
| Checksums (`Algorithm`, `Expected`) | XD-E3-S3 *(needs an engine hook for "never publishes")* |
| Many downloads at once, per-origin cap | XD-E6-S1 |
| Batch input, one URL per line | XD-E6-S2 |
| Session save and restore *(library)* | XD-E6-S3 |
| Retry, backoff, giving up *(library)* | XD-E7-S2 |
| Proxies, auth, headers, cookies *(library)* | XD-E7-S3 |
| Metalink parsing *(pieces not wired)* | XD-E5-S2 |

**Not available — and what it blocks here**

| Missing | Story | Blocks |
|---|---|---|
| **Per-download progress detail** | XD-E6-S4 | The connection-lane and block-map detail views (XDL-E4). Without per-segment offsets there is nothing honest to draw. |
| **Control while running** | XD-E6-S5 | Pausing and resuming a *running* download (XDL-E5-S1). Cancel already works through the engine's `CancelToken`. |
| **Rate limiting** | XD-E7-S1 | The speed-limiter card and every throttled cell in the scheduler (XDL-E6-S2). |
| Resume across process death | XD-E3-S2 | Resuming a download after a quit or crash (XDL-E5-S3). |
| Server capability probe | XD-E2-S1 | The add dialog's "server supports ranges" line (XDL-E3-S2). |
| Several URLs for one object, mirror choice | XD-E5-S1, XD-E5-S3 | Mirrors (XDL-E3-S4). |
| TLS trust, redirects, hostile responses | XD-E7-S4/S5/S6 | XDL-E9. |
| Downloads with no declared length | XD-E1-S5 | An indeterminate row (XDL-E2-S3). |

Nothing in this plan may be built by reaching into `../xsync` and changing it.
Where a story needs one of the above, it says so and stops at the seam.

---

## XDL-E1 — A download is a transfer

#### XDL-E1-S1 — The HTTP route

- [x] **Do.** A third route in `Worker::attempt`, beside E22-S1's wire protocol
  and E22-S2's `ShareSource`/`ShareSink`: an `HttpSource` from
  `xsync-download` paired with the `ShareSink` that already exists, so a
  download lands through the same staged, verified, resumable write path as
  every other transfer. *(Built 2026-09-07; the destination is xsync's
  `LocalSink`, which keeps the resume journal's restart proof. Committed as
  `excalibur 72ef808`.)*

**AC**
- [x] A `Transfer` can name an HTTP(S) URL as its source. `Transfer` gains the
  shape for it without breaking the queue file — `#[serde(default)]`, the way
  `mount`, `verified`, `files_done` and `files_total` were all added.
- [x] The row is driven by the same `sync_endpoints` session, so cancellation,
  the resume journal, BLAKE3 verification and the progress stream are the ones
  already tested — not a second implementation.
- [x] The destination filename comes from `xsync-download`'s `name::`, never
  from the URL's last path segment directly. A server that offers
  `../../../.ssh/authorized_keys` in `Content-Disposition` writes a file called
  something safe inside the chosen folder, and a test says so.
- [x] Nothing about an existing share transfer changes. The endpoint tests and
  the throughput harness both still pass, and the harness gains a download row.

#### XDL-E1-S2 — Where a download comes from and goes to

- [x] **Do.** Give `Transfer` an origin that is not a mount. Today a row is
  `direction` plus one `mount`; a download has no mount at all. *(Built
  2026-09-07: the `from`/`to: Endpoint` shape landed with S1 (`excalibur
  72ef808`); this story proves both ACs and changes no routing — `http.rs`
  keeps matching explicitly-set endpoints, and share rows keep upgrading
  lazily through `from_endpoint`/`to_endpoint`. The `Direction` removal stays
  E22-S3's. Proven as `excalibur a5fb184`.)*

**AC**
- [x] A row can be *from a URL, to a local folder* without pretending to be a
  `Direction::Down` off a share. E22-S3's `from: Endpoint` / `to: Endpoint`
  change is the same change; this story either does it or waits for it, and
  does not invent a parallel field. *(Done: `Transfer::http`/`new_http` carry
  `direction: None` with `Http`/`Local` endpoints; the worker drains such rows
  with no share connected — `tests/http_download.rs` drives them through a
  `BackendSource` that panics on backend resolution — and an endpoint-native
  row round-trips through JSON with `direction` null,
  `tests/transfer_model.rs: endpoint_native_rows_round_trip_with_direction_absent`.)*
- [x] The existing rows migrate without a queue file rewrite, and a queue
  written by today's build still loads. *(Done: `from`/`to`/`direction`/`mount`
  and the rest are `#[serde(default)]`; legacy down and up rows upgrade lazily
  (`tests/transfer_model.rs`), and a pre-endpoint `transfers.json` opens
  through `TransferQueue::open` with rows re-queued and both ends derived
  (`tests/queue.rs: a_queue_written_before_endpoints_still_loads_and_names_share_ends`);
  an endpoint-native download row persists and restores with its absent
  direction intact (`: an_endpoint_native_download_row_survives_a_restart_without_a_direction`).)*

---

## XDL-E2 — The Downloads tab

#### XDL-E2-S1 — Where the tab lives

- [x] **Do.** Decide and build how a window shows downloads instead of a share.
  The reference is a whole app; this is a tab in a window that already has a
  sidebar, a toolbar, a path field and a status bar. *(Built 2026-09-07: the
  window has a view — browse or downloads — carried in `WindowParams`,
  switched from the sidebar, and remembered per window label. The downloads
  view shows the same `Transfers` queue as the sheet (rows, bulk menu, footer)
  filtered to the chosen category; E2-S2 restyles it into the reference's
  table. Placement recorded in `docs/design-gaps.md`.)*

**AC**
- [x] A window can be *on downloads* the way it is on a mount, and the choice
  survives a restart per window (E23-S2's job, same mechanism). *(Done for the
  windows the shell creates: an explicit `?view=` wins, a ⌘N mount wins, and a
  window opened onto nothing restores its remembered view and category from
  `window-view:<label>` in the prefs store. Restoring every window's mount and
  path is still E23-S2's — a share needs its session, downloads need none —
  and the view rides the same `WindowParams` record it will persist.)*
- [x] The toolbar's share verbs (Quick Look, Edit, Download, Upload) are not
  merely disabled but *absent* — a toolbar of greyed-out buttons that can never
  apply is a worse answer than a toolbar that changes. *(Done: the verbs, the
  path field crumbs and the filter render only in the browse view; the
  downloads bar keeps Refresh (re-reads the queue), the sidebar toggle and
  preferences. The matching shortcuts return early on downloads.)*
- [x] The sidebar keeps FAVORITES and MOUNTED, and gains the reference's
  CATEGORIES section — All / Downloading / Paused / Completed / Failed, each
  with a live count. Recorded in `docs/design-gaps.md`: the reference puts
  categories at the top of its own sidebar, and here they share one with
  favourites and shares. *(Done: CATEGORIES sits after MOUNTED, counts come
  live off the window's transfer signal, cancelled rows belong to All alone.)*
- [x] Nothing about the browsing window regresses; the multi-window tests still
  pass. *(Done: 528 vitest green, tsc clean — the browse tree renders exactly
  as before when the view is browse, and `multiWindow.test.tsx` is untouched
  apart from the new required `view` field.)*

#### XDL-E2-S2 — The list

- [x] **Do.** The reference's table, in Excalibur's palette: file, size,
  progress, speed, ETA, status. *(Built 2026-09-07 as `DownloadsTable`: the
  tab's rows, while the sheet keeps its compact rows — two densities sharing
  the header, bulk menu and footer. Proven as `excalibur 4e3abf3`.)*

**AC**
- [x] Columns are the reference's set, and the *type rule* is taken wholesale
  because it is the best thing in the reference: **language is sans, machine
  values are mono.** Sizes, speeds, ETAs, percentages, hostnames and status
  badges are all mono, right-aligned where numeric. *(Done: file/size/progress/
  speed/ETA/status grid plus a narrow actions column for the row's own
  Pause/Cancel/Resume/Retry — the reference's right-click menu has no
  equivalent here, and losing the actions was not an option. Vitest resolves
  CSS to empty, so classes are asserted by token use and the contrast test,
  not by name: every machine value reads `var(--font-mono)` and every numeric
  column aligns right.)*
- [x] Surfaces are Excalibur's greys. The reference's white card, `#FAFBFB`
  header and `#ECEEEF` canvas map onto existing tokens, and the tab is legible
  in both themes with no colour introduced that is not already a token.
  *(Done: list bg, statusbar bg header, hairline rows, segment-track fills.
  Seven badge tokens added with dark values — success and warning fills plus
  per-state text — because the global `--success` tops out at 3.34:1 on white
  and `--error` fails on its own dark fill; the premise needed saying out
  loud, so it is written on the tokens and held in `contrast.test.ts`.)*
- [x] Status badges carry the reference's five states, in Excalibur's own
  success/warning/danger tokens rather than its hexes. *(Done: Downloading/
  accent, Queued/warning, Paused/muted, Completed/success, Failed/error, plus
  Cancelled/muted. Every text-on-fill pair is held to 4.5:1 in both themes.)*
- [x] A row shows the origin host under the filename, redacted — a URL with
  userinfo never renders its credentials, and a test with
  `https://user:pw@host/f` says so. *(Done: `originHost()` — HTTP hosts via
  `URL.hostname`, the share's host from the mount id, This Mac for local
  origins, legacy rows through direction and mount. The engine refuses
  credentialed URLs, but display does not trust the file.)*
- [x] 10,000 rows scroll at the same frame rate the file list manages (E8-S2's
  bar), which means the same virtualisation and not a second one. *(Done: the
  table calls the file list's `visibleRange` over its own 52px rows —
  `downloadWindow()` is the whole of it. A 10,000-row unit case windows to
  under 40 rows, and a 10,000-row render commits a bounded slice. Frame rate
  itself is not assertable in vitest; sharing the arithmetic is the guarantee
  the story asked for.)*

#### XDL-E2-S3 — A row that does not know its size

- [x] **Do.** The indeterminate case, which the reference does not draw and
  which this app already got wrong once. *(Built 2026-09-07: one rule shared
  by both bars, proven as `excalibur 5c9da99`.)*

**AC**
- [x] A download whose length the server never declared shows an indeterminate
  bar, not a bar at zero. `Transfer::fraction()` already returns `Option<f64>`
  and says why in its doc comment; the frontend uses it rather than
  reimplementing it — the bug fixed for folder scans, not repeated here.
  *(Done: `fractionOf()` is that rule — `null` while `bytes_total` is 0,
  clamped `0.0..=1.0` otherwise — and both the downloads table and the sheet
  rows read it. Unknown renders as a 40%-width block sliding across the
  track, transform-only so it stays on the compositor, with a
  `prefers-reduced-motion` rest. The engine has no sizeless path yet
  (`XD-E1-S5`, xsync refuses rather than guesses), so the row is drawn for
  the day the engine grows one — the frontend already reads the shape it
  will emit.)*
- [x] The status line reads a growing byte count with no denominator, and does
  not claim a percentage it cannot compute. *(Done: the table's right-hand
  figure is `formatSize(bytes_done)` and the status line joins a plain count
  with the rate — asserted to contain no percentage and no size separator;
  the only `/` is the rate's own.)*

#### XDL-E2-S4 — The footer

- [ ] **Do.** The reference's status footer: aggregate speed, a sparkline, and
  a count of what is in flight.

**AC**
- [ ] Total speed, active count, connection count and remaining bytes, in the
  window's existing status bar rather than a second one.
- [ ] The sparkline is the last 60 samples of aggregate throughput, drawn from
  data the app already has. It is decorative and is not the only place a number
  appears.

---

## XDL-E3 — Adding a download

#### XDL-E3-S1 — The dialog

- [ ] **Do.** The reference's screen 2, in Excalibur's dialog chrome.

**AC**
- [ ] One or more URLs, one per line. Destination folder, defaulting to the
  configured download folder and rememberable per queue.
- [ ] Built on the existing dialog and field components — the same focus ring,
  the same button shapes, the same overlay stacking rule
  (`components/overlays.test.ts` covers why that matters).
- [ ] Paste-to-add: a URL on the clipboard when the tab is focused offers to
  fill the field, and never acts without the user.

#### XDL-E3-S2 — What the server says before we start

- [ ] **Do.** Probe the URL and report what came back.

**AC**
- [ ] Resolving, resolved, or failed — with size and whether ranges are
  supported. **Needs XD-E2-S1**; until then the dialog says what it knows and
  does not draw the line at all rather than guessing.
- [ ] A server with no range support locks the connection control to one and
  says why, rather than offering a choice that will be ignored.
- [ ] A failed probe does not block Start. The engine retries better than a
  dialog can, and a HEAD that 405s is common and harmless.

#### XDL-E3-S3 — Connections

- [ ] **Do.** The reference's bar-array connection picker, which doubles as its
  own scale.

**AC**
- [ ] The control sets a per-download connection count within the engine's own
  cap. `xsync-download` *chooses* a count already (XD-E2-S4), so this is an
  override and is labelled as one — the default is "let it choose".
- [ ] The per-origin cap (XD-E6-S1, `--max-per-host`, default 4) is honoured
  and visible. Asking for 16 connections to a host capped at 4 must not read as
  16.

#### XDL-E3-S4 — Mirrors

- [ ] **Do.** Several URLs for one object.
- [ ] Blocked on **XD-E5-S1** and **XD-E5-S3**; `xsync-download` has failover
  only, and Metalink pieces are not wired. Not started here.

---

## XDL-E4 — Seeing inside one download

*The reference offers three treatments and recommends its connection-lane view
because it is the only one that reflects the real split model. That reasoning
holds here, and so does its converse: none of these may be drawn from invented
geometry (Decision 5).*

#### XDL-E4-S1 — Connection lanes

- [ ] **Do.** One row per connection, each lane starting at its own byte offset,
  with a playhead at its leading edge and a per-lane rate.
- [ ] **Needs XD-E6-S4.** Until per-segment progress crosses the seam there is
  nothing to draw. Do not approximate it.

**AC**
- [ ] Each lane's fill begins where that segment's range begins, not at zero.
- [ ] A stalled lane is visibly stalled and says so in words as well as colour.
- [ ] The summary strip reports fastest lane, stalled count and retries from
  real counters.

#### XDL-E4-S2 — Throughput strip

- [ ] **Do.** The reference's 60-second throughput chart, under whichever detail
  view is shown.

**AC**
- [ ] Built from the same samples the footer sparkline uses, so two charts
  cannot disagree.

#### XDL-E4-S3 — Block map

- [ ] **Do.** The IDM-style mosaic, as an option.
- [ ] Lowest priority in this epic, and honestly optional: it needs a bitfield
  the engine does not currently expose, and the lane view already answers the
  question better.

---

## XDL-E5 — Control

#### XDL-E5-S1 — Pause, resume, remove

- [ ] **Do.** The toolbar verbs, on a selection.
- [ ] Pause/resume of a *running* download needs **XD-E6-S5**. Cancel works
  today through `CancelToken`.

**AC**
- [ ] Selection follows the file list's rules — click, shift-click, cmd-click —
  and the verbs act on the selection, disabled with nothing selected.
- [ ] Remove asks before deleting a partial file, and never deletes a completed
  one without saying so.
- [ ] A paused download's partial file is kept, and resuming continues rather
  than restarting, or the row says plainly that it will restart.

#### XDL-E5-S2 — The context menu

- [ ] **Do.** Pause, Resume, Move to queue, Open folder, Copy URL, Remove.

**AC**
- [ ] Built through the existing `state/menu.ts` gate, so what a row offers is
  one pure function and testable — not a second menu implementation.
- [ ] Copy URL copies the URL the user gave, redacted of credentials.

#### XDL-E5-S3 — Surviving a restart

- [ ] **Do.** Downloads come back after a quit or a crash and continue.
- [ ] Needs **XD-E3-S2**. The queue's own restore already re-queues unfinished
  rows; what is missing is HTTP-side resume against a validator.

---

## XDL-E6 — Queues and scheduling

*aria2 has no queue concept either — the reference says so, and builds queues as
an app-level layer. So does this.*

#### XDL-E6-S1 — Named queues

- [ ] **Do.** Queues with a colour, a concurrency limit and an order; rows
  belong to one.

**AC**
- [ ] A queue decides which of its rows are allowed to run; the worker's
  existing concurrency limit stays the global ceiling.
- [ ] Drag to reorder sets priority.

#### XDL-E6-S2 — The weekly window

- [ ] **Do.** The reference's 7×24 paintable grid: full speed, throttled, paused.
- [ ] The *throttled* state needs **XD-E7-S1**. Until rate limiting exists the
  grid is two-state, and says so, rather than offering a throttle that does
  nothing.

**AC**
- [ ] Policy applies at the hour boundary and does not interrupt a running
  download mid-chunk.
- [ ] The grid is keyboard-reachable. A paint-drag-only control is unusable
  without a pointer, which E16-S2 does not permit.

---

## XDL-E7 — Settings

#### XDL-E7-S1 — Download defaults

- [ ] **Do.** A Downloads page in the existing Preferences: default folder,
  default connections, global concurrency, retry policy.

**AC**
- [ ] Every field goes through `state/settings.ts`'s sanitiser, whose
  `default:` arm is a `never` binding — a new setting without a sanitiser is a
  compile error, and that property is kept.
- [ ] Settings that are also engine options name the flag they map to, the way
  the reference does. That is the one part of its Engine screen worth keeping:
  it teaches the user what the thing underneath is called.
- [ ] **No RPC endpoint, no secret token, no Restart engine.** There is no
  daemon (Decision 1).

#### XDL-E7-S2 — Proxies and credentials

- [ ] **Do.** Surface `ClientConfig`'s proxies, headers and `.netrc`.

**AC**
- [ ] A credential entered here goes to the keychain by the route E3-S4 already
  built, and **no command reads it back** — the standing rule that no secret
  leaves Rust.
- [ ] A credential is dropped when a redirect leaves the origin the user named.
  `xsync-download` does this already; the UI must not undo it by re-attaching.

---

## XDL-E8 — Getting URLs in

#### XDL-E8-S1 — Drag and drop

- [ ] **Do.** A URL or a `.txt` of URLs dropped on the window opens the dialog
  pre-filled. The drop machinery exists (E24-S2); this is a new pasteboard type.

#### XDL-E8-S2 — Batch input

- [ ] **Do.** Many URLs at once, `xsync-download`'s XD-E6-S2 shape.

#### XDL-E8-S3 — Watch the clipboard

- [ ] **Do.** Offer to add a URL copied while the app is running. **Offer**, never
  act — a manager that starts downloads because something was copied is a
  manager nobody trusts.

#### XDL-E8-S4 — Browser capture

- [ ] Deliberately last, and possibly never (Decision 7). A browser extension is
  a separate product with its own review, packaging and threat model.

---

## XDL-E9 — Integrity and safety

#### XDL-E9-S1 — Checksums

- [ ] **Do.** Accept an expected digest and verify what landed.

**AC**
- [ ] A file that fails verification is never published under its final name.
  `xsync-download`'s XD-E3-S3 notes this needs an engine hook; if that hook is
  absent, the row must fail loudly and the partial must not be renamed.
- [ ] The algorithm is named in the UI. "Checksum ✓" without saying which is a
  claim the user cannot check.

#### XDL-E9-S2 — What a server is allowed to do to us

- [ ] **Do.** Bound it. Needs **XD-E7-S4/S5/S6**.

**AC**
- [ ] A redirect to another origin drops credentials, and to a non-HTTP scheme
  is refused.
- [ ] A `Content-Disposition` filename cannot escape the destination folder,
  cannot be absolute, and cannot be a dotfile the user did not ask for.
- [ ] A declared length that the body then exceeds is an error, not a disk
  filled silently.
- [ ] A TLS failure is refused and explained, never "continue anyway" by
  default.

---

## Milestones

| Milestone | Stories | Demo |
|---|---|---|
| **XDL-M1 It downloads** | XDL-E1-S1, XDL-E1-S2, XDL-E3-S1, XDL-E2-S2 | Paste an ISO URL, watch it arrive over many connections into a chosen folder, verified, in Excalibur's own list. |
| **XDL-M2 It is a manager** | XDL-E2-S1/S3/S4, XDL-E5-S1/S2 | A Downloads tab with categories, counts, a footer, selection, and pause/remove that work. |
| **XDL-M3 It shows its work** | XDL-E4-S1/S2, XDL-E3-S2/S3 | The lane view over real segment offsets; the add dialog reporting what the server actually supports. *(Gated on XD-E6-S4, XD-E2-S1.)* |
| **XDL-M4 It is trustworthy** | XDL-E9, XDL-E7-S2, XDL-E5-S3 | Checksums, hostile-response handling, credentials that never leak, and downloads that survive a crash. *(Gated on XD-E3-S2, XD-E7-S4/S5/S6.)* |
| **XDL-M5 It is patient** | XDL-E6, XDL-E7-S1, XDL-E8 | Queues, a weekly window, and every way of getting a URL in. *(Throttling gated on XD-E7-S1.)* |

## Definition of done

- [ ] A download runs through the same planner, verified write path, resume
      journal and cancellation as every other transfer — no second engine.
- [ ] Nothing in the tab renders a colour that is not a token, and it is legible
      in both themes.
- [ ] No progress indicator draws geometry the engine did not report.
- [ ] No credential is rendered, logged, persisted in plaintext, or readable
      back out of Rust.
- [ ] A hostile server cannot write outside the destination folder, fill the
      disk past the declared length, or downgrade the transport.
- [ ] `docs/design-gaps.md` carries an entry for every place this went beyond
      the reference — starting with the tab's own location, the shared sidebar,
      and the greys.

## Open questions for the designer

The reference does not draw these, and this plan should not settle them alone.

- **Where the tab lives.** A mode switch, a sidebar section, or a window that is
  either a browser or a manager. The reference cannot help — it is a whole app.
- **Whether downloads and share transfers share one list.** Decision 2 makes
  them one *queue*; whether they are one *view* is a drawing question. Today's
  Transfers sheet would show both.
- **The empty and first-run states**, which the reference explicitly leaves out.
- **What a completed download's row becomes** — a history list, or a row that
  stays until dismissed.
- **The paper/amber and dark skins** (`1i`, `1j`) are alternative visual
  treatments of the same table. Excalibur already has a light and a dark theme;
  these are not a third and a fourth, and are recorded here only so the question
  is not reopened later.
