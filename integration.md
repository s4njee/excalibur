# Excalibur — cross-project integration

Work that involves another project in the `xcalibur/` suite. Anything Excalibur
can finish alone lives in `backlog.md`. Suite-level context is `../ROADMAP.md`.

Suite: `xcalibur/{xsync, xc, excalibur, xebra, xstat}`. Excalibur's path deps
(`../../../xsync/crates/...`) resolve to the same place under the superproject as
they did in `~/projects/`, so the layout is what makes the build work at all.

---

## xsync

Excalibur's primary backend, its transfer engine, and — via `xsync-download` —
the engine behind the planned Downloads tab.

### 1. Promote `legacy/xsyncv3.md` into xsync's own docs

- **What / why.** Move the v3 protocol spec into `xsync/docs/protocol-v3.md`; the
  spec should live with the implementation. It is a contract document Excalibur
  happens to have written, because Excalibur was the reason v3 exists.
  `legacy/xsyncv3.md:11` names Excalibur as first consumer and Kestrel
  (`~/projects/sftp`) as second — a second consumer means it is no longer one
  client's design note. xsync's `docs/protocol-ownership.md` is the change process
  it belongs under.
- **Current state.** 1557 lines against xsync commit `1bdcfd71` (2026-09-02),
  Phase 1 marked Done 2026-09-03. It has drifted from xsync's tree since.
  (`legacy/xsyncv3.md:1`; `../ROADMAP.md` §3 Phase 2, seam 1.)

### 2. A protocol-version handshake that fails loudly

- **What / why.** A mismatched `xs` should be refused with a clear message rather
  than silently degraded or failing on the first unknown frame. Excalibur can be
  pointed at any `xs` on any host; the negotiation machinery exists (`CAP_FS_V3` is
  bit 7, v3 selected iff both peers advertise `CAP_VERSION_NEGOTIATION` and
  `CAP_FS_V3`) and the fallback is *supposed* to be reduced mode over browse v2.
- **Current state.** `negotiate_protocol_version` selects 3/2/1 in
  `xsync-core/src/protocol.rs`. What is missing on this side is **E7-S1**, the
  reduced-mode fallback, which is unbuilt. So a too-old `xs` today degrades into
  an unhandled shape rather than either a working reduced mode or a loud refusal.
  Pick one and build it. (`legacy/xsyncv3.md:508`, `:1449`; `legacy/plan.md:1644`.)

### 3. E7-S0 — a pinned git dep instead of a path dep

- **What / why.** `xsync-client`, `xsync-core` and `xsync-endpoint` become a
  pinned git revision with a documented `[patch]` override for development (the
  Kestrel XS-U2 pattern). `Cargo.lock` records **no revision** for them today: a
  build depends on files whose history lives in a sibling working copy. Also
  `cargo fmt --all` from here rewrites the whole xsync workspace, including crates
  Excalibur does not depend on and someone else's uncommitted work.
- **Current state.** Blocked on xsync having a pushed branch to pin. `cargo deny`'s
  `bans` gate was red over a wildcard path dep in `xsync-core` and went green when
  xsync added `version` beside `path`; Excalibur's `xsync-endpoint` line copies
  that shape so it cannot reintroduce the failure.
  (`crates/engine/Cargo.toml:35`, `:39`, `:55`; `docs/supply-chain.md:138`;
  `legacy/plan.md:1628`.)

### 4. xsync Phase 14 — the engine as a library (E22-S0)

- **What.** Endpoint traits (`Source`/`Sink`) with the planner and scheduler
  generic over them, cancellation, an event stream with stable per-file and job
  ids, library push/pull over a caller-owned session, and remote↔remote resolved
  as **relay** (the process owning both endpoints relays).
- **Why.** DoD v2: every transfer between any two mounts runs xsync's planner,
  scheduler, verification and resume journal, and Excalibur's own worker is gone.
- **Current state.** Filed in `../xsync/backlogv4.md` as "Phase 14 — The engine as
  a library", items 4.73–4.78, commit `66227f7bd6`, against xsync `v3` @
  `599d9d4529`. `xsync-endpoint` exists; E22-S1/S2 are done here. Still open
  there: **the P0 in xsync's `BUGS.md`** — the paranoid *pull* path sends an
  all-zero `LargeFileFinish` digest — which must be fixed before this GUI can
  honestly render "verified". (`legacy/plan.md:4187`, `:5809`.)

### 5. Deferred xsync features that gate Excalibur UI

Each is an Excalibur story stopped at the seam, not an Excalibur bug.

| Excalibur story | Waiting on |
|---|---|
| E8-S9 live refresh (`:2228`) | `Notify`/`Watch`, wire types 110–115, xsync Phase 4 |
| E7-S3 export picker (`:1724`) | `ListExports`, types 44–49, xsync Phase 2 |
| E3-S1 xsync auth fields, E3-S4 TLS pins (`:845`) | `xs serve` over TLS, xsync Phase 2 |
| E13-S6 resume across a link drop (`:1167`) | Session `Resume`, types 52–55, Phase 3 |
| E22-S2 hash cache by mount id + path (`:4460`) | a cache-key change inside xsync |
| E26-S6 owner/group in Properties (`:5750`) | the `OWNER_NAMES` feature bit |
| xsync↔xsync on one host round-trips through us | a server-side `Copy` |
| ~~XTP-E5-S1 the message log~~ — done at verb resolution; frames need the wiring | ~~a frame tap on `xsync-client`~~ — landed, §8 |
| XTP-E9-S5 cancel in flight (`twopane.md` XTP-E9-S5) | `Cancel` (type 18) reachable from the client — §8 |

### 6. `xsync-download` — the whole Downloads tab

- **What / why.** `legacy/xdl.md` builds the Downloads tab on `xsync-download`,
  not aria2 and not a new downloader, because a download is a
  `xsync_endpoint::Source` and Excalibur already drives `sync_endpoints`
  (`crates/engine/src/transfer/endpoint.rs`, E22-S2). A second engine would mean a
  second resume journal, a second verification story, a second scheduler and two
  answers to "is anything running" (Decisions 1 and 2).
- **Current state.** The library stories xdl asked for are **done on xsync's side
  as of 2026-09-07**, with remaining resume, mosaic and hostile-response work,
  and now live in
  `../xsync/legacy/xsync-downloader.md`. xdl's
  dependency table (`legacy/xdl.md:58`) marks as available: `HttpSource`
  (XD-E1-S1), `xs get` (XD-E1-S3), safe output naming (XD-E1-S4), streaming and
  no-`Range` servers (XD-E1-S6), parallel segments (XD-E2-S2), connection count
  chosen not asked (XD-E2-S4), preallocation (XD-E2-S5), bounded under-delivery
  (XD-E3-S4), per-origin cap (XD-E6-S1), batch input (XD-E6-S2), session
  save/restore (XD-E6-S3), retry/backoff (XD-E7-S2), proxies/auth/headers
  (XD-E7-S3), the standalone probe (XDL-S3), and ordered mirror fallback with
  an explicit first source (XDL-S8). XDL-S1/S2/S4 add an owned transfer handle,
  coalesced segment snapshots and a live rate limit.
- **Consumed here.** XDL-E3-S2–S4 now use XDL-S3 and XDL-S8 without changing
  xsync. The Excalibur IPC/queue shape is
  `HttpDownloadOptions { connections, mirrors, preferredMirror, retry }`; it is
  optional and `#[serde(default)]`, so older rows and callers keep the engine
  choice. Real adapter tests cover the IPC shape, and loopback engine tests
  cover fallback, preferred selection, safe output, and an aggregate
  four-request per-origin cap across concurrent jobs. XDL-E4-S1 now consumes
  XDL-S2's real stream offsets/ranges/rates/stall state through queue events and
  renders them in the selected-download detail view.
- **App-owned policy.** XDL-E6 now has an engine/native foundation for named
  queue definitions, per-row queue membership and ordering, with each queue's
  concurrency enforced beneath the existing worker-wide ceiling. Its 7×24
  schedule currently has a validated two-state domain model only; no runtime
  or schedule-runtime claims are made yet. XDL-E6-S1's UI now manages those
  queues and persists row ordering. XDL-E7-S1 carries default connections and
  bounded retry settings through persisted HTTP options into `HttpSource`;
  E7-S2 configures xsync's proxy, safe-header and `.netrc` support while keeping
  the Basic password in Rust and the existing keychain store.
- **Still missing at the seam:** xsync has no completed-piece bitfield for
  XDL-E4-S3's optional mosaic. XDL-S1 control while running is available but
  Excalibur has not retained its owned handle per row/session for XDL-E5-S1.
  XDL-S4 live rate limiting is available but is not yet connected to
  XDL-E6-S2's schedule. XD-E3-S2 resume
  across process death → XDL-E5-S3; XD-E5-S1/S3 cross-mirror segments and
  measured runtime mirror choice → future multi-source behavior beyond E3;
  XD-E7-S4/S5/S6 TLS trust and hostile responses → XDL-E9; XD-E1-S5 no declared
  length → XDL-E2-S3.
- **The rule.** Nothing in the xdl plan may be built by reaching into `../xsync`
  and changing it; a story that needs one of the above says so and stops at the
  seam. (`legacy/xdl.md:100`)

### 7. `scripts/stage-xs.sh` → a Tauri sidecar

- **What / why.** Turn the staging script into a Tauri `externalBin` sidecar so
  the bundle is self-contained and pins an exact xsync version. Today it
  cross-compiles `xs` from the sibling checkout and installs it on a *remote*
  Linux host over ssh — a deployment aid, not packaging — and a GUI release must
  name the exact xsync version it ships.
- **Current state.** Script works and is verified (E18-S4, `root@orion.local`).
  No sidecar, no version stamp in the bundle. Interacts with seam 2: a sidecar
  `xs` and a user-supplied remote `xs` are different version stories, and only the
  handshake covers the second. (`scripts/stage-xs.sh:1`; `../ROADMAP.md` §3 Phase 4.)

### 8. A frame tap and a reachable `Cancel` on `xsync-client`

- **What / why.** The re-issued share-browser handoff specs a protocol message log
  as a first-class surface — "The log is where protocol truth lives: mount command
  and its export flags, listing timings, and failures verbatim"
  (`design_handoff_share_browser 2/README.md`, §Screens/8) — and states the
  mechanism: every protocol command and response emits a `log://line`, with the log
  pane a view onto that stream rather than a separate thing. Excalibur cannot
  produce that stream. **Proposed:** an opt-in observer on `xsync-client` — an
  `mpsc` tap or an `Fn(&V3Message, Direction)` installed at `Client::from_split` —
  off by default, allocation-free when unarmed, dropping frames and reporting the
  drop rather than applying back-pressure to the protocol. It belongs at
  `Connection::request` (`../xsync/crates/xsync-client/src/lib.rs:286`) rather than
  `Connection::call` (`:322`), because `call` converts a server `Error` reply into
  an `Err` and loses the frame the log's `Error:` category needs verbatim. Second and
  smaller: `Cancel` (type 18) is defined and the server handles it, but the client
  crate offers no way to send one, so an in-flight request can only be abandoned by
  dropping the `OpenFile`, `Stage` or `Client`. **Proposed:** expose it, so a
  drawer's per-row cancel need not wait for a chunk boundary.
- **Done 2026-09-09, both halves except the wiring.** Excalibur has the pane, the
  `log://line` event and the ring (XTP-E5-S1), recording at the `ShareBackend`
  seam — one line per engine verb, with the pane saying so. xsync has the tap
  (`xsync` `0851e6d`): `Client::from_split_tapped` installs an
  `Fn(Direction, id, &V3Message)` at the send and at the reader, armed before
  the opening handshake, handed a borrow rather than a copy because a
  `V3Message` can carry a read's payload. **What remains** is Excalibur's
  `XsyncBackend` passing one in and turning frames into `LogLine`s, at which
  point the pane's `Engine operations — not protocol frames` note becomes
  conditional on which backend the mount is.
- **Still missing:** `Cancel` (type 18) is defined and the server handles it, but
  the client crate offers no way to send one.
- **Current state before that.** Neither existed. `xsync-client` has no `tracing` or `log`
  dependency and no callback, channel or observer anywhere; `Connection::request` is
  private and every frame passes through it. `V3Message` already derives `Debug`, so
  no new formatting contract is needed in xsync. On the Excalibur side `log://line`
  appears nowhere in the tree, and `SshStderr` (`crates/engine/src/backend/xsync.rs:450`)
  drains the remote's stderr into a bounded ring — lifecycle text, not frames, and the
  nearest thing that exists today. Backends that cannot produce frames would log at the
  `ShareBackend` seam (`crates/engine/src/backend/mod.rs:295`) at engine-verb
  resolution, which the pane must say it is showing rather than imply frames it does
  not have. (`twopane.md` XTP-E5-S0, XTP-E9-S5.)

---

## xc

No code dependency today, and `../ROADMAP.md` records that xsync↔xc separation as
deliberate. Two places Excalibur touches it, both speculative.

- **Transferring into a compressed archive.** xsync's `pack` feature writes
  tar+zstd and stays byte-compatible with what `xc check`/`xc unpack` read. If
  Excalibur ever offers "copy this tree into an archive" or "browse inside an
  archive as a mount", it wants that format and should go through whichever of the
  two owns the codec — not a third implementation here. The suite plans a
  cross-project fixture test (`xs` packs, `xc check` verifies, `xc unpack`
  round-trips) in the superproject because it needs both binaries. Excalibur is a
  consumer of that guarantee, not a party to it. (`../ROADMAP.md` §2 Phase 2.)
- **The codec seam.** Open suite question: whether xc exposes a small `codec`
  crate (no engine baggage) that xsync depends on. If it lands, a compressed
  transfer here is a flag on the xsync pipeline, not a feature Excalibur builds.
- **State.** Nothing in `legacy/plan.md` or `legacy/xdl.md` mentions xc or
  compression. A placeholder for a decision not yet asked of Excalibur — do not
  build toward it.

---

## xebra

The other Tauri GUI in the suite. No code is shared and none should be forced —
SolidJS and React stay separate (`../ROADMAP.md` §2.1). What *is* shared is a
discipline both arrived at independently:

- A **Tauri-free engine crate** where correctness lives and CI can test it without
  a desktop runner.
- A **headless CLI** driving it (`crates/cli`, `xcal`) — here it also generates
  `src/state/fixture.json`, so the frontend fixture cannot drift from what the
  engine produces (`legacy/plan.md:306`). It cannot drift from the engine; it
  can and did fall behind the *type*, when `Entry` gained `addedMs` and nobody
  regenerated — the preview drew `NaN undefined NaN` in a whole column, and
  TypeScript could not see it because the JSON is imported as a module.
  `src/state/fixture.test.ts` now lists the fields, so a new one fails until
  the fixture carries it.
- A **thin `src-tauri`** owning DTOs, letting no engine enum cross IPC unmapped
  (`legacy/plan.md:630`).
- A `design_handoff_*/` directory as the visual source of truth, plus a
  `docs/design-gaps.md` recording every place the build went beyond the mocks, and
  a `plan.md` story format where a landed story keeps its unticked ACs with the
  reason (`legacy/plan.md:115`).

**The action:** extract these into a root `docs/gui-conventions.md`. Do **not**
extract a shared crate — the roadmap's rule is that a common progress/event DTO is
worth it only if a third GUI appears.

**What Excalibur has that xebra does not:** a `deny.toml` and a
`docs/supply-chain.md` (guarantees, audit, accepted advisories, licence review,
per-dependency justification, and a register of the one `unsafe` module). Both are
cheap to copy and are the thing most likely to catch a licence problem before a
release — and xebra links xc, which vendors non-OSS `unrar` sources. The suite's
root `deny.toml` is to be seeded from Excalibur's. (`../ROADMAP.md` §3 Phase 1.)

---

## xstat

No interaction today, none planned. xstat asks whether a shared filesystem-walk
engine beats `walkdir` (xc) and `ignore::WalkParallel` (xsync); Excalibur does no
walking of its own — every tree walk it triggers happens inside xsync's planner
(E22-S1). If the gate opens and xsync adopts `crates/xstat`, Excalibur inherits
the result through xsync with no change here. **Nothing to do.**
(`../ROADMAP.md` §3 Phase 5.)

---

## Suite-wide

- **Excalibur has no remote**, one of the repos blocking suite Phase 0. Branch
  here is `xcalibur`, cut from `main`, pre-move snapshot `55f1d94`.
- **Which xsync does the suite pin?** Excalibur is written against v3 @ `1bdcfd71`
  and its Downloads tab needs the `xdl` branch's library work. The roadmap wants
  `v3` and `xdl` merged into xsync's `main` before pinning, so the suite never
  points at a feature branch. Excalibur cannot become a submodule with a working
  build until this is settled. (`../ROADMAP.md` §3 Phase 0, §5.)
- **Toolchain.** xc pins `rustc 1.98.0` deliberately; Excalibur has no
  `rust-toolchain.toml` and CI uses `@stable`. Adding a pin here is a one-file
  Phase 0 change.
- **Shared crate versions.** `zstd`, `tar`, `clap`, `tokio`, `blake3` appear in
  more than one project; Excalibur names `blake3` directly (E12-S1's staged commit
  digest) and gets the rest through xsync. Divergent `zstd` across xc and xsync is
  the real risk — not one Excalibur can fix, but it reaches here through
  `Cargo.lock`. (`../ROADMAP.md` §3 Phase 3.)
- **Licence.** The `smb` feature is off by default because turning it on makes the
  binary GPLv3 (`pavao`) — one of the two entries the suite's root `deny.toml`
  must rule on explicitly, alongside xc's vendored `unrar`.
  (`docs/supply-chain.md:62`.)
- **Naming.** `xcalibur` (suite) vs `excalibur` (this project) differ by one
  letter; expect confusion in CI job names, issue titles and shell history.
