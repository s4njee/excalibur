# Excalibur — backlog

Unfinished work only. Everything marked **Done** in `legacy/plan.md`'s Progress
table is out of scope here; this file is what is left. Citations are story ids
(`legacy/plan.md E7-S1`) or `file:line`.

## Status snapshot

- **Milestone:** M4 per `README.md:10`; M4.5 and M7 stories have landed on top of
  it (`legacy/plan.md:124` Progress table). Remaining milestones: **M5 Release**
  (E18), **M6 xsync daemon** (E3-S1 auth fields, E7-S3), **M8 Finder in/out**
  (E24-S1/S3/S4, E25), **M9 xsync everywhere** (E22-S2/S3, E23-S2),
  **M10 The window you keep open** (E26, E27). (`legacy/plan.md:5775`)
- **Branch:** `xcalibur`, cut from `main`. No remote yet (suite Phase 0 blocker,
  `../ROADMAP.md` §3).
- **Last 5 commits** (`git log --oneline -5`):
  - `55f1d94` snapshot in-flight work (icons, aria2 design handoff, logo) before the suite move
  - `266634d` transfers: an unmounted share is a wait, not a failure (E13-S3 gap)
  - `2b16451` boot: fix a blank window from the startup sweep and an unbounded panel
  - `39859aa` docs: the xdl download-manager plan (now `legacy/xdl.md`)
  - `5093750` transfers: one session per folder, queue stops rewriting itself (E22-S3)
  So: the transfer queue and startup path are the live edge of the tree.
- **Test gate** (`.github/workflows/ci.yml`): `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace`
  on macOS + Linux; `pnpm build` (tsc) + `pnpm test` (vitest); an unsigned
  `tauri build --bundles app` that asserts the `.icns` is in the bundle; and
  `cargo deny check`. ~248 Rust tests in `crates/engine` + `src-tauri`.
- **Not gated:** no ESLint (E0-S1b), no Docker backend matrix (E19-S1), no
  Playwright visual suite (E19-S2), no e2e (E19-S3), no perf gates (E19-S4).

---

## Share browser core

- **E0-S1b — ESLint + Prettier.** Never started; `tsc --noEmit` is the only
  frontend static check. (`legacy/plan.md:232`, `:256`)
- **E0-S2b — Bundle JetBrains Mono** 400/500 as woff2 with the licence; today
  the stack falls back and the shipped CSP forbids a network font.
  (`legacy/plan.md:272`, `:295`)
- **E0-S3 — Icon set.** Glyphs are still placeholders tinted per category;
  `kindToIcon` over vendored Lucide is unwritten (`legacy/plan.md:300`). The
  window's 7px corner radius token is likewise unapplied (`:343`).
- **E0-S4 — `xcal` mutating verbs.** `get|put|mkdir|rm|mv` and URI arguments
  naming a real share are deferred; the CLI is `ls|stat|mount|df` on the mock.
  Backends exist now, so the deferral's reason has expired. (`legacy/plan.md:313`)
- **E2-S4 — Error taxonomy to UI.** No story marked done: every `EngineError`
  should map to per-item / connection-level / validation and never to a modal.
  (`legacy/plan.md:641`)
- **E3-S2 — Re-probe bookmarks every 60s while idle.** The probe runs once on
  start. (`legacy/plan.md:722`)
- **E8-S9 — Live refresh.** Coalescer built; nothing produces `watch` events, so a
  listing never refreshes from the server side. Blocked on xsync Notify (types
  110–115, Phase 4). (`legacy/plan.md:2228`, `:1672`)
- **E11-S2 — Text preview lazy continuation on scroll.** Not built.
  (`legacy/plan.md:2527`)
- **E11-S3 — Video seek latency.** "40 GB MKV plays within 2s on LAN" never
  measured (`legacy/plan.md:2551`). **E12-S1** write-back keyed on the editor's
  handle count is not portable; the heuristic stands (`:2609`).
- **E14-S4 — Properties: POSIX mode/owner/group.** Parked until xsync sends
  names (`OWNER_NAMES` bit). Completed as E26-S6. (`legacy/plan.md:5750`)
- **Window chrome does not follow a share mounted via the sidebar `+`.** Title,
  breadcrumb and sidebar row keep naming the old mount; the bookmark route is
  correct, so the two paths disagree. (`docs/design-gaps.md:540`)
- **The Window menu has no window list** and no *Bring All to Front*; with two
  overlapping windows there is no route to the one behind. E26/E27 work.
  (`docs/design-gaps.md:568`)

## Protocols

- **SMB (E4)** — done but behind an off-by-default `smb` feature, because the
  `pavao` path makes the binary GPLv3 when on. The pure-Rust `smb` crate was the
  decision (ADR 0001) and is young and thinly maintained; that is the standing
  risk. (`legacy/plan.md:102` risk register, `docs/supply-chain.md:62`)
- **E4 — `protocol_label`/`security_label`.** `SMB3`/`SMB2` and the cipher name
  are unfilled. (`legacy/plan.md:1077`)
- **E5-S4 — NFS range I/O.** Partly done: 1 MiB read cap; throughput is poor and
  the reasons are written up rather than fixed. (`docs/benchmarks.md:5`)
- **E5-S5 — NFSv4 and Kerberos.** Spike done 2026-09-04, implementation deferred
  (ADR 0002). The UI labels honestly as `NFSv3 · sys` meanwhile.
  (`legacy/plan.md:1396`, `docs/decisions/0002-nfsv4-and-kerberos.md`)
- **E6-S2 — SFTP `security_label`.** Open: russh 0.63 does not expose the
  negotiated KEX/cipher. (`legacy/plan.md:1575`)
- **E3-S1 — NFS Version/Security controls and SFTP's auth-method picker + key
  file** are not in the dialog. (`legacy/plan.md:661`)
- **E7-S1 — Reduced-mode fallback over browse v2** for remotes running an older
  `xs`: whole-file `Fetch`/`Publish` behind `Caps.random_access = false`, a
  "reduced mode — upgrade remote xsync" chip, Quick Look capped at 8 MiB, no
  video. Deferred to M4, still not started. (`legacy/plan.md:1644`)
- **E7-S3 — Export picker** ("Browse exports…" over `ListExports` with RW/RO
  badges). Blocked on xsync Phase 2. (`legacy/plan.md:1724`, `:666`)
- **E7-S4 — `ssh-keyscan` still swallows** the failure distinction that the rest
  of the SSH path now reports. (`legacy/plan.md` Progress, M4.5 row)
- **E3-S4 — TLS pins.** Nothing to pin: `xs serve` over TLS is xsync Phase 2.
  (`legacy/plan.md:845`)


## xsync transfer engine

The bet in `legacy/plan.md` is that every transfer eventually routes through
xsync's planner, scheduler, BLAKE3 verification and resume journal, and that
Excalibur's own byte-moving worker goes away (DoD v2, `legacy/plan.md:5809`).

- **E22-S2 — three blocked ACs.** remote↔remote with Excalibur relaying (needs
  E22-S3's `Endpoint` change and xsync 4.78); a hash cache keyed by mount id +
  path (not doable without changing xsync); and a conformance case for "copy a
  tree between every pair of backends". (`legacy/plan.md:4454`–`:4473`)
- **E22-S3 — `Transfer` loses `Direction::{Up,Down}`, gains `from`/`to:
  Endpoint`.** Untouched, and it is the change that unblocks remote↔remote. A
  data-model change across the queue, the worker, both adapters, the persisted
  file and the panel. (`legacy/plan.md:4627`)
- **E22-S3 / E13-S6 — pause/resume/retry/cancel per transfer surviving a restart
  *and a reconnect of either end*.** The reconnect half is missing: a row whose
  mount came back is not re-pointed at it. (`legacy/plan.md:4634`, `:2886`)
- **E22-S3 — the app-wide Transfers window** the handoff drew, with the sheet as
  the compact view. Untouched; resolves the E13-S1 design gap.
  (`legacy/plan.md:4641`, `docs/design-gaps.md:106`)
- **E22-S3 — "Apply to all remaining" as a choice** on the conflict prompt.
  Untouched. **Bug found while reading that path:** `Keep both` computes a free
  name, shows it, then does not use it — `resolveConflicts` re-sends the original
  paths, so it behaves as `Replace`. (`legacy/plan.md:4644`, `:2603`)
- **E13-S6 — open transfers resume at offset**, and a reconnect mid-transfer
  continuing from the last acknowledged byte. (`legacy/plan.md:1167`, `:2886`)
- **E23-S2 — Restore.** Every window comes back on its mount and path, a failed
  share reconnects into the banner, and a password prompts once, not once per
  window. Not started. (`legacy/plan.md:5068`)

## Downloads tab (xdl)

1 of the 25 stories in `legacy/xdl.md` is built — **E1-S1** (the HTTP route,
2026-09-07, `72ef808`); the rest are unchecked. The engine is
`xsync-download` through `sync_endpoints`, one queue shared with transfers
(Decisions 1 and 2, `legacy/xdl.md:46`).

- **XDL-E1** — a download is a `Transfer`: a third route in `Worker::attempt`
  (S1) and an origin that is not a mount (S2), migrating existing rows without a
  queue-file rewrite. (`legacy/xdl.md:103`)
  - **E1-S1 done** (2026-09-07, `72ef808`): HTTP → local routes through
    `sync_endpoints` with `HttpSource` + `LocalSink`; `from`/`to: Endpoint`
    land on `Transfer` with `#[serde(default)]`; hostile `Content-Disposition`
    names sanitised; permanent HTTP failures (404, refused URL) fail the row
    fast instead of spending the worker's 30s of backoff.
    (`legacy/xdl.md:105`)
- **XDL-E2** — the tab: where it lives in the window (S1), the table with its
  five status badges and redacted origin host (S2), the indeterminate row (S3,
  blocked on xsync XD-E1-S5), the footer sparkline (S4). 10,000 rows must scroll
  at E8-S2's frame rate. (`legacy/xdl.md:142`)
- **XDL-E3** — add dialog (S1), server probe (S2, blocked on XD-E2-S1),
  connection picker honouring `--max-per-host` (S3), mirrors (S4, blocked on
  XD-E5-S1/S3). (`legacy/xdl.md:212`)
- **XDL-E4** — connection lanes and block map. Blocked on XD-E6-S4; Decision 5
  says a faked segment geometry is not drawn at all. (`legacy/xdl.md:261`)
- **XDL-E5** — pause/resume of a *running* download needs XD-E6-S5; surviving a
  restart needs XD-E3-S2. Cancel already works. (`legacy/xdl.md:299`)
- **XDL-E6** — named queues (S1) and the 7×24 weekly window (S2); the throttled
  state needs XD-E7-S1 rate limiting. Grid must be keyboard-reachable.
  (`legacy/xdl.md:332`)
- **XDL-E7** — Downloads preferences page and proxies/credentials via the
  keychain. Explicitly **no RPC endpoint, no secret token, no Restart engine**.
  (`legacy/xdl.md:362`)
- **XDL-E8** — URL drag-drop, batch input, clipboard *offer*; browser capture (S4)
  is deliberately last and possibly never (Decision 7). (`legacy/xdl.md:392`)
- **XDL-E9** — checksums (never publish an unverified file under its final name)
  and bounding what a server may do: redirect credential-dropping, scheme
  refusal, `Content-Disposition` path escape. Needs XD-E7-S4/S5/S6.
  (`legacy/xdl.md:416`)

## macOS native integration

- **E24-S1 — drag out to Finder.** Built and unit-tested; the one open AC is a
  real drag onto Finder through an accessibility harness asserting digest and
  that a cancelled drag queued nothing. No such harness exists in this repo.
  (`legacy/plan.md:5101`)
- **E24-S3 — Copy and paste** (⌘C/⌘X/⌘V as the keyboard twin of the drag, cut is
  a move completed on paste). Not started. (`legacy/plan.md:5542`)
- **E24-S4 / E13-S5 — Linux drag.** GTK `text/uri-list` source that must download
  to `$XDG_RUNTIME_DIR` *before* the drag begins, plus an XDG portal drop target.
  (`legacy/plan.md:5548`, `:3371`)
- **E25-S1 — Open / Open With…** through a per-share cache, making E12's
  edit-in-place the general case. (`legacy/plan.md:5557`)
- **E25-S2 — Reveal / Show in Excalibur** plus `smb://`/`nfs://`/`sftp://`/
  `xsync://` URL scheme registration in `Info.plist`. (`legacy/plan.md:5569`)
- **E26-S1 — This Mac and `/Volumes`** in the sidebar. Untouched.
  (`legacy/plan.md:5607`)
- **E26-S3 — Undo** (⌘Z over move/rename/copy-into, folding in the deferred
  delete; never across a restart). (`legacy/plan.md:5734`)
- **E26-S4/S5 — Icon and column views** (a design spike each), and **search
  across a share** via xsync `ReadTree` where available, a bounded walk
  elsewhere. (`legacy/plan.md:5740`, `:5745`)
- **E27-S1 — Real menu bar, Dock transfer badge, completion notification.**
  (`legacy/plan.md:5756`)
- **E27-S2 — Launch at login and stay resident**; quitting with active
  transfers asks. (`legacy/plan.md:5763`)
- **E27-S3 — Trash for shares.** A decision to make, not code to write; the
  default answer is no. (`legacy/plan.md:5769`)

## Packaging, release, CI, supply chain

- **E18-S1 — universal binary.** `aarch64` only; no Intel Mac here to verify a
  slice on. (`legacy/plan.md:3554`)
- **E18-S1 — hardened runtime and notarisation.** Both need a Developer ID. The
  bundle today is *inconsistently* signed (the linker's ad-hoc signature, no
  `_CodeSignature`), so Gatekeeper offers no right-click→Open override;
  `xattr -dr com.apple.quarantine` is the workaround. `APPLE_SIGNING_IDENTITY=-`
  is better locally, still not distributable, and trips Homebrew's `xattr -r`.
  (`legacy/plan.md:3558`)
- **E18-S1 — auto-update** via the Tauri updater with a signed manifest; blocked
  on E18-S3's release channel. (`legacy/plan.md:3563`)
- **E18-S2 — Linux `.AppImage` and `.deb`** (x86_64 + arm64), desktop entry,
  Secret Service documented as optional. Not started. (`legacy/plan.md:3782`)
- **E18-S3 — release process.** Tag → both platforms → full matrix → artifacts +
  SHA-256 + notes from `CHANGELOG.md`. Not started; no `CHANGELOG.md` exists.
  (`legacy/plan.md:3787`)
- **The xsync dependency is a working copy on one machine.** `Cargo.lock` records
  no revision for `xsync-client`/`-core`/`-endpoint`. E7-S0 wants a pinned git
  rev with a documented `[patch]` override. See `integration.md`.
  (`crates/engine/Cargo.toml:35`, `docs/supply-chain.md:138`)
- **`cargo fmt --all` reaches into the sibling xsync workspace** and rewrites
  files there. Use `cargo fmt -p excalibur-engine -p excalibur-cli -p excalibur`;
  worth a `justfile` recipe or a CI wrapper. (`crates/engine/Cargo.toml:39`)
- **`scripts/stage-xs.sh` is the only packaging glue** for shipping `xs`; it
  cross-compiles and installs to a *remote* host. It is not a bundle story. See
  `integration.md`.

## Tests

- **E19-S1 — engine integration matrix.** Docker Compose fixtures (Samba rw/ro,
  `nfsd` rw/ro/root_squash, OpenSSH, `xs --server`) all passing one shared
  `ShareBackend` conformance suite: list, stat, range read, resume write,
  mkdir/rename/remove, capacity, writability on RO, reconnect. Linux CI; macOS
  runs in-process fixtures only. **Not started** — this is the largest single
  hole in the gate. (`legacy/plan.md:3794`)
- **E19-S2 — frontend tests.** vitest component tests for every handoff state
  `1a`–`1h` plus banner and reduced mode, and a Playwright visual regression
  suite at 1060px. Not started. (`legacy/plan.md:3802`)
- **E19-S3 — end to end.** `tauri-driver`/WebdriverIO smoke on Linux CI.
  (`legacy/plan.md:3809`)
- **E19-S4 — performance gates.** 100k-row scroll ≥ 55 fps, listing → first paint
  ≤ 50 ms, cold start ≤ 1.5 s, failing CI at a 15% regression. The throughput
  harness that exists (`crates/engine/tests/throughput.rs`) is `#[ignore]`d and
  prints a table rather than asserting. (`legacy/plan.md:3814`, `:4660`)
- **An accessibility test harness** for E24-S1's real-Finder drag. A story, not
  a step. (`legacy/plan.md:5101`)

## Awaiting design sign-off

`docs/design-gaps.md` holds 24 entries, each waiting on a designer and none meant
to stand permanently (`docs/design-gaps.md:5`); DoD v1 also wants a screenshot on
each and none has one (`legacy/plan.md:5803`). Most likely to move under sign-off:
the FAVORITES sidebar (glyphs are our drawings, not SF Symbols, and are chosen by
path not name — `:691`), transfers as a sheet vs. a window (`:106`), and the
three-grey contrast exception (`:167`).

## Parking lot (E20 — do not build from imagination)

Needs mocks first: search across a share; Get Info / POSIX mode inspector;
disconnected/reconnecting banner sign-off; Preferences sign-off; rename/delete
confirmation sign-off; multi-window behaviour. (`legacy/plan.md:3819`)
