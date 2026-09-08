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

Grounded in `../../xsync/legacy/xsync-downloader.md`, updated against the
provider checkout on 2026-09-07. This table is the
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
| Standalone advisory capability probe | XDL-S3 |
| Ordered mirror fallback and caller-selected first source | XDL-S8 |

**Still to integrate or not yet available — and what it blocks here**

| Missing | Story | Blocks |
|---|---|---|
| **Per-download progress detail** | XD-E6-S4, XDL-S2 | The provider now emits per-chunk progress and exposes snapshots; Excalibur still has to carry stream/range/rate/stall fields through its transfer events and state before it can draw connection lanes. A completed-piece bitfield for the optional block map is still absent. |
| **Control while running** | XDL-S1 | The provider's owned transfer handle now supports pause/resume/cancel. Excalibur still drives `sync_endpoints` as a blocking call and must adopt and associate that handle with its persisted queue rows. |
| **Rate limiting** | XD-E7-S1, XDL-S4 | The provider's live rate handle is available; Excalibur has not threaded it through active HTTP jobs or the weekly policy yet. |
| Resume across process death | XD-E3-S2 | Resuming a download after a quit or crash (XDL-E5-S3). |
| Segments split across mirrors, Metalink pieces, measured mirror choice | XD-E5-S1, XD-E5-S3 | Multi-source transfer. XDL-E3-S4 uses XDL-S8's pre-transfer fallback and does not claim these semantics. |
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

- [x] **Do.** The reference's status footer: aggregate speed, a sparkline, and
  a count of what is in flight. *(Built 2026-09-07 as the downloads view's own
  status bar — the footer the tab already had, enriched rather than a second
  bar. Proven as `excalibur 49a22b1`.)*

**AC**
- [x] Total speed, active count, connection count and remaining bytes, in the
  window's existing status bar rather than a second one. *(Done for the three
  the queue can truthfully name — active count of running rows, remaining
  bytes over known sizes, aggregate rate — in the tab's existing status bar,
  with the numbers in mono beside the accent aggregate. **Connection count is
  the seam:** the queue's `Transfer` carries no per-download connection state
  — that is xsync's `XD-E2-S4`, unplumbed to the row — so the footer says
  nothing rather than invent one, the same honesty rule S3 used. Noted in
  `docs/design-gaps.md`.)*
- [x] The sparkline is the last 60 samples of aggregate throughput, drawn from
  data the app already has. It is decorative and is not the only place a number
  appears. *(Done: a one-second sampler keeps a 60-sample ring of the same
  aggregate the footer's number reads, drawn as an accent `aria-hidden` SVG
  line beside that number. Pure `pushSample`/`sparklinePath` in
  `state/throughput.ts`, unit-tested: capped ring, empty-under-two, scaled to
  the samples' own peak.)*

---

## XDL-E3 — Adding a download

#### XDL-E3-S1 — The dialog

- [x] **Do.** The reference's screen 2, in Excalibur's dialog chrome. *(Built
  2026-09-07: the toolbar's Add URL opens the dialog only where the queue is;
  the queue field the reference draws next to Save to does not exist yet
  (XDL-E6-S1), so Save to carries a per-window remembered folder instead —
  the "rememberable per queue" of the AC, recorded to the prefs store under
  `download-dest:<label>`.)*

**AC**
- [x] One or more URLs, one per line. Destination folder, defaulting to the
  configured download folder and rememberable per queue. *(Done: each line
  validated before Start with the engine's own `validate_http_url` rule
  mirrored in `state/downloads.ts` — the queue refuses credentialed URLs,
  queries and fragments, so the dialog says so rather than sending a row the
  engine throws back. The default folder is the last-used destination, else
  the configured download folder, else the OS one; validated in
  `state/downloads.test.ts` and `addDownload.test.tsx`.)*
- [x] Built on the existing dialog and field components — the same focus ring,
  the same button shapes, the same overlay stacking rule
  (`components/overlays.test.ts` covers why that matters). *(Done: the dialog
  is `dialog.module.css`'s scrim/dialog/titlebar/form/actions — the connection
  dialog's own chrome — with `aria-modal`, an Escape capture that closes one
  thing, and an autofocused URL box. The URL box is a textarea, which exposed
  a window keyboard rule that only counted `INPUT` as typing; a Backspace
  there deleted the file selection behind the dialog. That rule now counts
  `TEXTAREA` too, and a test types into the box and presses keys to say so.)*
- [x] Paste-to-add: a URL on the clipboard when the tab is focused offers to
  fill the field, and never acts without the user. *(Done: the dialog reads
  the clipboard when it opens and when the window refocuses, and offers — with
  the origin host redacted — a single usable URL under a "Use it" button that
  fills rather than starts. A credentialed or multi-line clipboard offers
  nothing.)*

#### XDL-E3-S2 — What the server says before we start

- [x] **Do.** Probe the URL and report what came back. *(Built 2026-09-07:
  `probe_http_download` runs XDL-S3's standalone probe off the webview thread;
  the dialog debounces edits, probes a pasted batch sequentially, and rejects
  late answers from an older edit or a disposed dialog.)*

**AC**
- [x] Resolving, resolved, or failed — with size and whether ranges are
  supported. *(Done through XDL-S3: nullable facts stay "unknown" in the
  TypeScript contract and fixture backend; a probe warning is a failed probe,
  not a resolved one. `addDownload.test.tsx` covers all three states and
  bounded batch probing.)*
- [x] A server with no range support locks the connection control to one and
  says why, rather than offering a choice that will be ignored. *(Done: a
  single URL or selected mirror locks the whole picker; a mixed batch keeps
  the override for range-capable rows and writes `connections: 1` only on the
  rows that cannot range.)*
- [x] A failed probe does not block Start. The engine retries better than a
  dialog can, and a HEAD that 405s is common and harmless. *(Done for rejected
  probes and advisory warnings; both leave Start enabled and carry no invented
  size or range fact.)*

#### XDL-E3-S3 — Connections

- [x] **Do.** The reference's bar-array connection picker, which doubles as its
  own scale. *(Built 2026-09-07 as an accessible radio group whose increasing
  bar count is decoration beside the numeric value.)*

**AC**
- [x] The control sets a per-download connection count within the engine's own
  cap. `xsync-download` *chooses* a count already (XD-E2-S4), so this is an
  override and is labelled as one — the default is "let it choose". *(Done:
  the native limits command supplies the scale; the optional override crosses
  Tauri, persists in `Transfer.http_options`, survives queue JSON, and reaches
  `HttpSource::with_segments`. Absent remains the engine choice.)*
- [x] The per-origin cap (XD-E6-S1, `--max-per-host`, default 4) is honoured
  and visible. Asking for 16 connections to a host capped at 4 must not read as
  16. *(Done: the dialog ends its scale at `min(maxConnections, maxPerHost)`
  and names both limits. The engine reserves the selected source's actual
  `read_concurrency` from one process-wide semaphore per origin, and the wait
  observes cancellation. A loopback regression starts two 16-request jobs at
  once and observes no more than four active requests at their shared origin.)*

#### XDL-E3-S4 — Mirrors

- [x] **Do.** Several URLs for one object. *(Built 2026-09-07: mirror mode
  sends one enqueue with a primary URL, ordered alternatives and the user's
  preferred starting index. The shape persists with the queue row.)*
- [x] Use XDL-S8's available failover without claiming XD-E5-S1/S3's unfinished
  multi-source behavior. *(Done: the engine tries the selected source first,
  falls through the remaining sources during pre-transfer enumeration, then
  downloads from the chosen `HttpSource`. The dialog says the sources are not
  mixed during transfer. Loopback tests cover fallback, preferred selection,
  one output row and the chosen source's bytes; segment mixing, Metalink pieces
  and measured runtime mirror selection remain provider work.)*

---

## XDL-E4 — Seeing inside one download

*The reference offers three treatments and recommends its connection-lane view
because it is the only one that reflects the real split model. That reasoning
holds here, and so does its converse: none of these may be drawn from invented
geometry (Decision 5).*

#### XDL-E4-S1 — Connection lanes

- [x] **Do.** One row per connection, each lane starting at its own byte offset,
  with a playhead at its leading edge and a per-lane rate.
- [x] **Integrate XDL-S2.** The provider emits per-chunk progress and exposes
  transfer snapshots with stream/range/rate/stall fields. Excalibur now carries
  them through `Tally`/`Moved`, queue events and TypeScript transfer state; the
  lanes do not approximate them from aggregate progress.

**AC**
- [x] Each lane's fill begins where that segment's range begins, not at zero.
- [x] A stalled lane is visibly stalled and says so in words as well as colour.
- [x] The summary strip reports fastest lane, stalled count and retries from
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

**Blocker.** xsync exposes segment snapshots but no completed-piece bitfield,
so Excalibur has no real mosaic geometry to render.

---

## XDL-E5 — Control

#### XDL-E5-S1 — Pause, resume, remove

- [ ] **Do.** The toolbar verbs, on a selection.
- [ ] Pause/resume of a *running* download must adopt **XDL-S1**. Cancel works
  today through `CancelToken`; xsync's owned transfer handle now supplies live
  pause and resume.

**Blocker for live pause/resume.** Excalibur still runs `sync_endpoints` as one
blocking call and does not retain XDL-S1's handle by queue-row/session id. The
provider capability is available; the consumer refactor is still required.

Current bounded slice (2026-09-07): each queue row exposes Pause/Cancel and
Resume for paused work in both the compact sheet and Downloads table. Remove
asks whether to discard a partial or completed row and routes through the
native queue; selection based toolbar actions remain open, as does the
provider work needed for pausing a running HTTP stream.

**AC**
- [ ] Selection follows the file list's rules — click, shift-click, cmd-click —
  and the verbs act on the selection, disabled with nothing selected.
- [x] Remove asks before deleting a partial file, and never deletes a completed
  one without saying so.
- [ ] A paused download's partial file is kept, and resuming continues rather
  than restarting, or the row says plainly that it will restart.

#### XDL-E5-S2 — The context menu

- [x] **Do.** Pause, Resume, Move to queue, Open folder, Copy URL, Remove.
  *(Built 2026-09-07: download rows use the shared `ContextMenu` and the
  `downloadMenu` pure rule in `src/state/menu.ts`; status-refused actions stay
  visible with a reason. Move to queue targets XDL-E6-S1's named queues. Remove
  confirms before asking the queue to delete the
  row, Open folder calls the native shell for the local destination's parent,
  and Copy URL strips any credentials from legacy rows.)*

**AC**
- [x] Built through the existing `state/menu.ts` gate, so what a row offers is
  one pure function and testable — not a second menu implementation.
- [x] Copy URL copies the URL the user gave, redacted of credentials.

#### XDL-E5-S3 — Surviving a restart

- [ ] **Do.** Downloads come back after a quit or a crash and continue.
- [ ] Needs **XD-E3-S2**. The queue's own restore already re-queues unfinished
  rows; what is missing is HTTP-side resume against a validator.

**Blocker.** xsync's resume work still lacks the accepted validator semantics:
changed or absent validators must refuse unsafe continuation, and the
`If-Range`/kill-and-restart evidence is incomplete. Excalibur cannot promise
crash-safe continuation until that provider contract is closed.

---

## XDL-E6 — Queues and scheduling

*aria2 has no queue concept either — the reference says so, and builds queues as
an app-level layer. So does this.*

#### XDL-E6-S1 — Named queues

- [x] **Do.** Queues with a colour, a concurrency limit and an order; rows
  belong to one.

**AC**
- [x] A queue decides which of its rows are allowed to run; the worker's
  existing concurrency limit stays the global ceiling.
- [x] Drag to reorder sets priority.

Current bounded slice (2026-09-07): the engine and Downloads UI persist named
queue definitions (name, colour, concurrency and order), assign each transfer
a queue and optional position, migrate the former flat queue file, enforce each
queue's running-row ceiling below the worker's global semaphore, and expose
native commands/events for queue management, movement and ordering. The queue
picker supports create/edit/delete and row movement; selecting a queue enables
drag and Arrow-key reorder, which sends the resulting ids back to native for
persistent priority. The global worker ceiling remains authoritative.

#### XDL-E6-S2 — The weekly window

- [ ] **Do.** The reference's 7×24 paintable grid: full speed, throttled, paused.
- [ ] The *throttled* state must integrate **XDL-S4**. Until Excalibur retains
  and updates xsync's live rate handle, the grid is two-state and says so,
  rather than offering a throttle that does nothing.

**Blocker for Throttled cells.** xsync's live rate cap is available, but the
active HTTP route exposes no rate handle to Excalibur's scheduler yet.

**AC**
- [ ] Policy applies at the hour boundary and does not interrupt a running
  download mid-chunk.
- [ ] The grid is keyboard-reachable. A paint-drag-only control is unusable
  without a pointer, which E16-S2 does not permit.

Current bounded slice (2026-09-07): `transfer::schedule` defines and validates
the Monday-first 168-slot policy with only Full speed and Paused states. It
rejects malformed persisted grids and makes the hour-boundary rule a policy on
starting queued work. Runtime enforcement, persistence and the keyboard
reachable grid remain open; Throttled remains blocked on the missing consumer
integration with XDL-S4.

---

## XDL-E7 — Settings

#### XDL-E7-S1 — Download defaults

- [x] **Do.** A Downloads page in the existing Preferences: default folder,
  default connections, global concurrency, retry policy. *(Built 2026-09-07:
  the connection default and retry policy travel with each HTTP queue row and
  are applied by `HttpSource`; global concurrency still configures the shared
  worker, and the folder remains the native/default destination.)*

**AC**
- [x] Every field goes through `state/settings.ts`'s sanitiser, whose
  `default:` arm is a `never` binding — a new setting without a sanitiser is a
  compile error, and that property is kept.
- [x] Settings that are also engine options name the flag they map to, the way
  the reference does. That is the one part of its Engine screen worth keeping:
  it teaches the user what the thing underneath is called.
- [x] **No RPC endpoint, no secret token, no Restart engine.** There is no
  daemon (Decision 1).

#### XDL-E7-S2 — Proxies and credentials

- [ ] **Do.** Surface `ClientConfig`'s proxies, headers and `.netrc`.

Current bounded slice (2026-09-07): Preferences configures environment/disabled/
explicit proxies, safe headers and a `.netrc` path. Rust builds the provider's
`ClientConfig`; Basic passwords travel directly to the existing secret store
under `http-download`, remain process-only, and have no read command. Unsafe
credential headers, `Range`, proxy userinfo and malformed values are refused.

**Validation blocker.** The implementation and secret-shape tests pass, but an
actual save/forget cycle against the native OS keychain has not been exercised
manually, so the story and its keychain acceptance criterion remain open.

**AC**
- [ ] A credential entered here goes to the keychain by the route E3-S4 already
  built, and **no command reads it back** — the standing rule that no secret
  leaves Rust.
- [x] A credential is dropped when a redirect leaves the origin the user named.
  `xsync-download` does this already; Excalibur supplies credentials once to
  the Rust-owned `ClientConfig` and does not re-attach them in UI or queue code.

---

## XDL-E8 — Getting URLs in

#### XDL-E8-S1 — Drag and drop

- [x] **Do.** A URL or a `.txt` of URLs dropped on the window opens the dialog
  pre-filled. The drop machinery exists (E24-S2); this is a new pasteboard type.
  *(Built 2026-09-07: browser `text/uri-list`/plain URL drops and native `.txt`
  path drops converge on the existing dialog. Text files are limited to 16 at
  1 MiB each before IPC reads them; comments and blank lines are ignored, and
  invalid URL lines remain visible for the dialog to explain. A drop only
  opens the dialog and never starts a transfer.)*

#### XDL-E8-S2 — Batch input

- [x] **Do.** Many URLs at once, `xsync-download`'s XD-E6-S2 shape. *(Built
  2026-09-07: the Add Download dialog accepts one URL per line, validates and
  probes them in a bounded sequence, and enqueues one persisted row per URL.)*

#### XDL-E8-S3 — Watch the clipboard

- [x] **Do.** Offer to add a URL copied while the app is running. **Offer**, never
  act — a manager that starts downloads because something was copied is a
  manager nobody trusts. *(Built 2026-09-07: the dialog checks on mount and
  window focus, shows a redacted-host offer, and fills only after “Use it”.)*

#### XDL-E8-S4 — Browser capture

- [ ] Deliberately last, and possibly never (Decision 7). A browser extension is
  a separate product with its own review, packaging and threat model.

**Blocker.** This needs a product decision plus a separately packaged browser
extension, native-messaging host, security review and store-review plan. None
of those artifacts or ownership decisions exists in this repository.

---

## XDL-E9 — Integrity and safety

#### XDL-E9-S1 — Checksums

- [ ] **Do.** Accept an expected digest and verify what landed.

**Blocker.** xsync accepts expected digests, but its current workaround can
publish and then move a bad final file to `.unverified`. This story requires a
pre-commit verification hook so a mismatch can never receive the final name.

**AC**
- [ ] A file that fails verification is never published under its final name.
  `xsync-download`'s XD-E3-S3 notes this needs an engine hook; if that hook is
  absent, the row must fail loudly and the partial must not be renamed.
- [ ] The algorithm is named in the UI. "Checksum ✓" without saying which is a
  claim the user cannot check.

#### XDL-E9-S2 — What a server is allowed to do to us

- [ ] **Do.** Bound it. Needs **XD-E7-S4/S5/S6**.

**Blocker.** xsync has part of redirect handling, but consistent resolved URLs
for segmented requests, classified TLS trust failures and hostile-response
resource caps remain open in XD-E7-S4/S5/S6.

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
| **XDL-M3 It shows its work** | XDL-E4-S1/S2, XDL-E3-S2/S3 | The lane view over real segment offsets; the add dialog reporting what the server actually supports. *(E4-S1's real segment lanes are built; the shared-sample throughput strip remains.)* |
| **XDL-M4 It is trustworthy** | XDL-E9, XDL-E7-S2, XDL-E5-S3 | Checksums, hostile-response handling, credentials that never leak, and downloads that survive a crash. *(Gated on XD-E3-S2, XD-E7-S4/S5/S6.)* |
| **XDL-M5 It is patient** | XDL-E6, XDL-E7-S1, XDL-E8 | Queues, a weekly window, and every way of getting a URL in. *(Throttling needs Excalibur's XDL-S4 live-rate integration.)* |

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
