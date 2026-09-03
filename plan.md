# Excalibur — implementation plan

A desktop share browser for **SMB, NFS, SFTP and xsync** shares. Rust + Tauri v2 shell,
SolidJS frontend, all protocol and filesystem I/O in Rust. Single-pane, Cyberduck-style,
bookmarks in a permanent sidebar, the mount's effective permission (RW/RO) first in the
status bar and gating every write verb.

The visual and behavioural specification is
`design_handoff_share_browser/README.md` (the "handoff") and its canvas
`Share Browser Mockups.dc.html`, options `1a`–`1h`. This plan does not restate pixel
values that document already fixes; acceptance criteria reference handoff sections and
call out only the metrics that are easy to get wrong. When the two disagree, the
handoff wins on look and this plan wins on architecture.

## Decisions taken

| Decision | Choice | Consequence |
|---|---|---|
| Access model | **Userspace protocol clients in Rust.** No kernel mounts, no FUSE, no admin rights. A "mount" is a logical session. | Portable; RW/RO comes from protocol flags; each backend is implemented, not shelled out to. No Finder/Explorer visibility (out of scope). |
| Platforms (v1) | **macOS + Linux.** Windows kept buildable but not packaged, tested or credential-integrated. | Keychain via `keyring` crate (macOS Keychain, Secret Service/libsecret). Two packaging pipelines. |
| xsync backend | **xsync is the primary backend.** `xsyncv3.md` Phase 1 is built *first*, then Excalibur's first real share is xsync (M1). SFTP, SMB and NFS follow. | The `ShareBackend` trait and `xsync-client::Fs` (xsyncv3 E9-S1) are co-designed by the same person at the same time; the trait is shaped by what xsync actually returns, and the other three backends adapt to it. Development runs against `xs --server` from day one — no Docker fixtures needed until M3. |
| Row layout | **`1f` (24px columns) is the default**, `1g`/`1h` selectable and persisted, as the handoff's state model specifies. | All three are built; `1f` is the one that gets the resizable columns. |
| Protocols added beyond the handoff | **SFTP** and **xsync** join SMB and NFS behind one trait. | The New Connection dialog gets four protocol tabs; the design's "swap the field set" rule extends to them. |
| Engine provenance | New crate `excalibur-engine`, **porting** from the sibling `~/projects/sftp` (Kestrel) engine where it fits: `fs/sftp.rs` (russh-sftp), `transfer/worker.rs` (durable queue, conflicts, pause/resume), `fs/xs.rs` (browse v2 adapter), keychain and TOFU host-key handling. | Same author, same licence, proven against real servers. Ported, not depended on: Kestrel's trait is dual-pane/Svelte-shaped and drags FTP/WebDAV/S3 along. |
| Frontend styling | CSS Modules + CSS custom properties for the handoff tokens; no runtime CSS-in-JS. | Tokens live in one file per theme; components never hard-code a hex. |
| Icons / fonts | Lucide (monochrome) at 12–16px; JetBrains Mono 400/500 bundled as woff2; sans stack `"Helvetica Neue", Helvetica, Arimo, system-ui, sans-serif`. | No runtime font fetch (Tauri CSP). |
| Persistence | `tauri-plugin-store` for bookmarks, prefs, transfer-queue state; secrets only in the OS keychain. | Nothing secret is ever in a JSON file or in frontend state. |
| `unsafe` code | **Denied by default, not forbidden** *(2026-09-03)*. Preference order: a safe crate, then a vetted FFI crate (`rustix`, `nix`, `cap-std`), then hand-written `unsafe` — which is now permitted where none of the above is adequate. | The allow goes on the one module that needs it, never the crate or workspace; every block carries a `// SAFETY:` note; a tested safe wrapper is what the rest of the tree sees; `docs/supply-chain.md` records it like a dependency. Held as an absolute, this lint had already deferred a feature out of xsync (owner names) rather than deciding anything about the feature, and would have forced a weaker path-confinement design. Mirrors `xsyncv3.md` §7 Decision 9. |

## Architecture

```
excalibur/
  Cargo.toml                 workspace
  crates/
    engine/                  excalibur-engine — NO tauri dependency
      src/
        backend/mod.rs       ShareBackend trait, Attrs, Entry, Capacity, Writability
        backend/smb.rs       SMB2/3 client (Epic 4)
        backend/nfs.rs       NFSv3 (+v4 later) client (Epic 5)
        backend/sftp.rs      russh + russh-sftp (Epic 6)
        backend/xsync.rs     xsync-client v3 adapter; browse-v2 reduced mode (Epic 7)
        session/             SessionManager: mount lifecycle, health, reconnect
        listing/             list_dir + kind detection + sort/filter helpers (server-side cache)
        transfer/            queue, worker, conflicts, resume, progress events
        preview/             Quick Look readers (range reads, mime sniffing)
        edit/                edit-in-place: temp file, watcher, write-back
        secrets/             keyring facade
        error.rs             EngineError with transient/fatal + errno mapping
    cli/                     xcal — thin CLI over the engine for headless testing
  src-tauri/                 Tauri v2: commands, DTOs, event bridge, plugin config
  src/                       SolidJS app
    tokens/                  light.css, dark.css, geometry.css
    components/              TitleBar, Toolbar, Sidebar, PathField, FileList{Columns,NameLed,Mono}, StatusBar, Banner, dialogs/*
    state/                   stores mirroring the handoff's State Management section
    windows/                 browser, transfers, quicklook
  design_handoff_share_browser/   the spec (read-only)
  xsyncv3.md, plan.md
```

Two boundaries, borrowed from Kestrel and stated as rules:

1. **File bytes never cross IPC.** Tauri commands carry paths, ids and metadata; Rust
   does every read and write. Previews stream to the webview via a local
   `asset`-protocol handler backed by the engine, not via command return values.
2. **The engine has no Tauri dependency.** Every backend and the transfer worker are
   testable headlessly against in-process or containerised servers.

### The `ShareBackend` trait (engine)

Every protocol implements this; the UI only ever sees it through Tauri commands.

```rust
#[async_trait]
pub trait ShareBackend: Send + Sync {
    async fn connect(spec: &ConnectionSpec, secrets: &dyn SecretSource) -> Result<Self> where Self: Sized;
    fn mount_info(&self) -> MountInfo;              // writable, reason, protocol_label, security_label, options, capacity hints
    async fn capacity(&self) -> Result<Capacity>;   // free/total/available
    async fn list_dir(&self, path: &str) -> Result<Listing>;   // entries + MountInfo snapshot (handoff: writability rides with listings)
    async fn stat(&self, path: &str) -> Result<Attrs>;
    async fn open_read(&self, path: &str, offset: u64) -> Result<BoxRead>;
    async fn open_write(&self, path: &str, mode: WriteMode) -> Result<BoxWrite>;  // Create | Resume{offset}
    async fn mkdir(&self, path: &str) -> Result<()>;
    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()>;
    async fn remove(&self, path: &str, recursive: bool) -> Result<()>;
    async fn set_mtime(&self, path: &str, mtime: SystemTime) -> Result<()>;
    fn capabilities(&self) -> Caps;                 // random_access, resume_write, atomic_rename, symlinks, xattrs, watch, locks
    async fn watch(&self, path: &str) -> Result<Option<WatchStream>>;  // None when unsupported
    async fn ping(&self) -> Result<Health>;
}
```

`MountInfo.writable` is the **single source of truth** the handoff demands: computed by
the backend from protocol facts (SMB tree-connect `MaximalAccess`/share flags; NFS
export `ro` via `FSINFO`/`ACCESS` probe; SFTP `statvfs` `ST_RDONLY` + a probe; xsync
`MountInfo.effective_writable`), re-read on every reconnect, and returned alongside
every `list_dir` so the status bar can never drift.

### Risk register (read before estimating)

| Risk | Why | Mitigation story |
|---|---|---|
| **Pure-Rust SMB** | `smb` (0.11, pure Rust SMB2/3, signing/encryption) is young; `pavao` wraps libsmbclient (C, LGPL, mature). Neither is battle-tested in a GUI. | E4-S1 spike compares both against Samba, macOS `smbd`, Synology, Windows; pick one, wrap it behind the trait so the choice is reversible. |
| **NFSv4 / Kerberos in userspace** | `nfs3_client` (pure Rust) covers NFSv3 + AUTH_SYS well; NFSv4.x and `krb5p` (the handoff's `1b` status line) have no mature pure-Rust client. | E5 ships NFSv3 first; NFSv4 via `libnfs` bindings or a v4.0 minimal client is a later story; the UI labels honestly (`NFSv3 · sys`). |
| **Privileged NFS source ports** | Many servers default to `secure` (client port < 1024). | E5-S2 detects `EACCES`/`PROG_MISMATCH`, explains `insecure` export option, and offers no privilege escalation. |
| **xsync Phase 1 scope creep** | It is on the critical path (M-1) and is protocol work under `docs/protocol-ownership.md` review, which is slow by design. | Keep Phase 1 to the story list in E7-S0 — no daemon, no TLS, no locks, no notify; those are M6. Freeze the v3 table only for the types Phase 1 uses; later phases add types, never change them. |
| **Streaming media previews** | Video needs HTTP range semantics into the webview. | E11-S3 uses a Tauri custom protocol with `Range` support over `open_read(offset)`. |
| **Linux Secret Service variance** | Headless/no-keyring sessions. | E3-S3 falls back to "don't save" with an explicit message, never to plaintext. |

---

## Epics and stories

Story ids `E<n>-S<m>`. **AC** = acceptance criteria. Handoff references look like
*(handoff §Screens/1, Status bar)*. A story that has landed is marked **Done** with
its date, its AC boxes ticked (or left unticked with the reason), and a **Results** /
**Next steps** pair recording what actually happened and what it leaves for later —
the same convention `xsyncv3.md` uses, so a reader coming back cold does not have to
reconstruct it from git history.

## Progress

| Milestone | Story | Status |
|---|---|---|
| M0 | E0-S1 — Workspace and toolchain | **Done** 2026-09-03 |
| M0 | E0-S2 — Design tokens and typography | **Done** (fonts outstanding) |
| M0 | E0-S4 — Headless CLI (`xcal`) | **Done** (mock only) |
| M0 | E1-S1 — Window chrome and four-row layout | **Done** 2026-09-03 |
| M0 | E1-S2 — Toolbar | **Done** (navigation inert) |
| M0 | E1-S3 — Sidebar | **Done** (fixed rows) |
| M0 | E2-S1 — `ShareBackend` trait and mock | **Done** 2026-09-03 |
| M0 | E2-S3 — Tauri commands and events | **Done** (`list_dir`, `mount_info`) |
| M1 | E7-S2 — xsync v3 backend | **Done** 2026-09-03 |
| M0 | E2-S2 — Session manager | **Done** 2026-09-03 |
| M1 | E3-S1 — New Connection dialog | **Done** 2026-09-03 |
| M1 | E3-S2 — Bookmarks store | **Done** 2026-09-03 |
| M1 | E3-S3 — Keychain | **Done** 2026-09-03 |
| M0 | E0-S3 — Icon set | Not started |
| M1 | E3-S4 — SSH host keys | **Done** 2026-09-03 |
| M1 | E8, E9, E10 | Not started |
| M2+ | everything else | Not started |

M0's demo criterion — the design's `1a` and `1b` rendered from fixed data, with the
theme switch and the sidebar — is met. M1's backend talks to a real `xs --server`, and the session manager holds it. A user can
open the dialog, pick a protocol, connect, and have the bookmark persist. The dialog's
password field now reaches the OS keychain when "Save in keychain" is ticked, though no
backend authenticates with it yet (E4/E6). `?screen=1b` is the one worth looking at: it
is the same component tree with `mount.writable` false.

---

### E0 — Foundations and scaffolding

#### E0-S1 — Workspace and toolchain — **Done**

*2026-09-03.*

**AC**
- [x] Cargo workspace with `crates/engine`, `crates/cli`, `src-tauri`;
  `unsafe_code = "deny"` and clippy `pedantic` at warn, matching the sibling projects.
- [x] Tauri v2 app configured for macOS and Linux; `cargo check --workspace` builds the
  shell, including `generate_context!`. *(Not yet launched as a window here — that
  needs a desktop session; the frontend is verified in a browser instead.)*
- [x] SolidJS + Vite + TypeScript strict; `vitest`.
- [ ] ESLint + Prettier. **Not yet:** `pnpm build` runs `tsc --noEmit` first, so type
  errors already fail the build; a formatter and linter are worth adding with the
  first contributor rather than before one.
- [x] CI on `macos-latest` and `ubuntu-latest` with fmt, clippy `-D warnings`, tests,
  `pnpm build`, `pnpm test`, and `cargo deny`/`cargo audit` gates.

**Results**

- `pnpm` needed `allowBuilds: esbuild` in `pnpm-workspace.yaml` — the same file and
  the same reason as Kestrel, which is where the convention came from. Without it
  `pnpm install` refuses esbuild's postinstall and Vite cannot start.
- Tauri rejects an icon that is not RGBA, with a message that says so only at
  `generate_context!` time. The placeholder icons are generated RGBA; the real mark
  arrives with E18-S1.
- `deny.toml` is written now rather than at release: it is the file that makes adding
  a dependency a deliberate act, and the first dependency is always the cheapest time
  to establish that.

**Next steps**

- Launch the shell as a window on a desktop session and screenshot it, which is the
  half of this story a headless check cannot cover.
- ESLint and Prettier with the first contributor.

#### E0-S1b — Frontend linting *(carved out of E0-S1)*
**AC**
- ESLint with the Solid plugin and Prettier, run in CI alongside `pnpm build`.
- Rules chosen to catch what `tsc` cannot: unused JSX props, reactivity misuse,
  accidental `any` at boundaries.

#### E0-S2 — Design tokens and typography — **Done (fonts outstanding)**

*2026-09-03.*

**AC**
- [x] Every token in *(handoff §Design Tokens)* as a CSS custom property with the
  exact hex value, plus radii, control heights, bar heights, sidebar width and
  shadow. *(One `tokens.css` rather than three files: light is the base and dark
  redefines only what changes, which is what keeps a token added without a dark
  value visibly wrong rather than silently empty. Split it if it outgrows a screen.)*
- [ ] JetBrains Mono 400/500 bundled as woff2. **Not yet:** the stack falls back to
  `ui-monospace`, which on macOS is SF Mono and looks close but is not the design.
  Bundling is a licence check and a build step, not a token change, so it is its own
  story.
- [ ] A `/tokens` dev route rendering every token as a swatch. **Not yet.**
- [x] `data-theme` on `<html>` switches the palette.
- [ ] `system` following `prefers-color-scheme`. **Not yet:** there is no preferences
  store to hold the choice until E17-S1.

**Results**

- `body` sets `user-select: none`. A file browser is not a document, and text
  selection on chrome and rows makes drag-to-select feel broken.
- The dark palette redefines only what changes. Writing it as a full second palette
  would double the places a new colour has to be added and halve the chance both get
  updated.

**Next steps**

- Bundle JetBrains Mono, then verify no network font request under the Tauri CSP.
- The `/tokens` swatch route is worth building the first time someone reviews a
  colour against the mock and has to open dev tools to do it.

#### E0-S2b — Bundle JetBrains Mono *(carved out of E0-S2)*
**AC**
- 400 and 500 weights as woff2 in the repo, with `@font-face` and the licence file.
- No network font request at runtime, verified against the shipped CSP.

#### E0-S3 — Icon set
**AC**
- Lucide icons vendored as Solid components; a `kindToIcon(kind, isDir)` map covers the
  kinds produced by E8-S5; all monochrome, sized 12–16px, colour from tokens
  (`#8a8f98` folder fill / `#a8adb5` file stroke in light).

#### E0-S4 — Headless CLI — **Done (mock only)**

*2026-09-03.*

**AC**
- [x] Drives the engine without Tauri. `xcal ls|stat|mount|df`, with `--json` and
  `--read-only`.
- [ ] `get|put|mkdir|rm|mv`, and URI arguments naming a real share. **Deferred:** every
  one of those needs a backend that exists, and none does yet. They join as their
  protocols land.

**Results**

- `xcal ls --json` generates `src/state/fixture.json`, which the frontend uses when it
  runs in a browser. That makes the fixture impossible to drift from what the engine
  actually produces — regenerate it rather than editing it, and a change to the
  engine's shape shows up as a diff in the frontend's data.
- It earned its place immediately: the first `xcal ls` is what showed the listing was
  right before any pixel existed.

**Next steps**

- Take a URI and dispatch to a backend, once there is one to dispatch to.

### E1 — App shell and layout

#### E1-S1 — Window chrome and four-row layout — **Done** *(handoff §Screens/1 Layout, Title bar)*

*2026-09-03.*

**AC**
- [x] Title bar 34px, centred 12px/600 `#4a4a50` title `<share> — <host>`; the mock's
  three grey circles are *not* rendered. `titleBarStyle: Overlay` with `hiddenTitle`
  is configured for macOS.
- [x] Toolbar 44px, body `flex: 1`, fixed 206px sidebar + list column; list column =
  24px header, scrollable rows, 24px status bar.
- [x] Minimum window 820×480 in `tauri.conf.json`.
- [ ] Window corner radius 7px. **Not applied:** the token exists, but the radius
  belongs to the OS window, not the web root, and applying it to the root would round
  the content inside a square window.

**Results**

- The title follows the mount rather than being fixed. It was hardcoded first, and the
  read-only screen immediately showed the bug — it still read `media — nas.local`
  while displaying an NFS export.
- The mock's three grey circles are stand-ins for whatever the platform draws. Not
  rendering them is deliberate and worth restating, because they are the most obviously
  copyable thing in the design.

**Next steps**

- Verify the macOS traffic lights sit correctly over the 34px title bar; `Overlay`
  places them over the web content and the design leaves no inset for them.

#### E1-S2 — Toolbar *(handoff §Screens/1 Toolbar)*
**AC**
- Back/forward segmented pair (two 30×24 buttons, shared 5px-radius border, 1px
  divider), enabled `#55555c` / disabled `#b0b0b6`; `↑` enclosing-folder button
  `padding: 0 9px`.
- Path field `flex: 1`, 24px, mono 11px breadcrumb: scheme dim, host/intermediates
  `#3a3a40`, separators `#c0c0c6`, current `#1a1a1c` weight 500; segments clickable;
  clicking empty space switches to an editable input that accepts a pasted URI and
  navigates on Enter, reverts on Escape.
- Verbs `Quick Look`, `Edit`, `Download` secondary; `Upload` the single primary;
  gap 5px between verbs, 8px between groups; each verb has a tooltip showing its
  shortcut (E16).
- Filter field fixed 150px, placeholder "Filter" `#a0a0a6`.

#### E1-S3 — Sidebar *(handoff §Screens/1 Sidebar)*
**AC**
- Section header "MOUNTED" (9.5px/600, `.09em`, uppercase) then one row per live
  session; active row `#dfe7fb` bg with 3px `#2f6df6` left border and compensated
  padding `0 12px 0 9px`; subtitle mono 9.5px `#5c76a8` e.g. `SMB3 · nas.local`.
- "BOOKMARKS" header with 16px top padding; 36px rows, gap 9px, 13×10 glyph, name
  12px, subtitle mono 9.5px `#8a8a90` protocol + host; unreachable rows `opacity: .5`
  with subtitle "unreachable".
- Footer `margin-top: auto`, 1px top border, three equal 28px buttons `+`, `−`, `Edit`.
- Single-click selects; double-click mounts (if needed) and navigates to the root.
- Collapsible; the collapsed flag persists; it never auto-hides *(handoff §Responsive)*.
- Dark theme uses the dark sidebar tokens (`#26272a`, active `#1f3358`/`#4f86ff`,
  subtitle `#8fa8d8`).

#### E1-S4 — Responsive column dropping *(handoff §Responsive)*
**AC**
- Below ~900px list width the Kind column is removed, then Modified; header and rows
  stay aligned; no horizontal scroll.

#### E1-S5 — Transitions *(handoff §Transitions)*
**AC**
- 120ms ease on hover/disabled colour changes; 140ms opacity + 4px rise on Quick Look
  open/close; **no** layout animation in the list (verified by a test asserting no
  `transition` on row geometry properties).

### E2 — Engine core and Tauri bridge

#### E2-S1 — `ShareBackend` trait and shared types — **Done**

*2026-09-03.*

**AC**
- [x] The trait, `Attrs`, `MountInfo`, `Caps`, and `EngineError` with a
  transient/fatal split. *(`Caps` is a struct of named booleans rather than bitflags:
  it crosses IPC as JSON, where a bitfield would be an opaque number on the other
  side. `EngineError` carries no `errno` yet — no backend produces one.)*
- [x] A `MockBackend` from an in-memory tree with fault injection: latency, read-only,
  and failure after N calls.

**Results**

- `list_dir` returns a `Listing` — entries *and* the `MountInfo` as of that listing —
  rather than entries alone. Fetching writability separately would let two round trips
  disagree, and the badge is the thing a user trusts. This is the shape the handoff
  asks for and it is worth the slightly odd return type.
- `EngineError::surface()` says where a failure belongs on screen: transfer row,
  banner, or inline field. That routing is a property of the error, so deriving it at
  each call site would be three chances to get it wrong.
- The mock refuses mutations on a read-only mount itself, not only in the UI. The UI
  gating is what a user sees; this is what makes a bug in it harmless.
- The mutation test enumerates every write verb rather than spot-checking one. A verb
  missing from that list is exactly a verb the UI would also forget to disable.

**Next steps**

- **E2-S2 (session manager)** is the next piece: several mounts, lifecycle, health
  polling and reconnect.
- `connect()` is not on the trait yet. Each backend's constructor differs enough
  (keychain, host keys, ports) that the shape should come from the second real
  backend, not be guessed before the first.

#### E2-S2 — Session manager — **Done**

*2026-09-03.*

**AC**
- [x] `mounts: Map<bookmarkId, Mount>` with the lifecycle `mounting → ready → error |
  reconnecting`; one backend instance per mount; switching the active mount does not
  disconnect the others.
- [x] Health loop: `ping` on an interval; failure flips the status to `reconnecting`,
  emits an event, and reconnects with exponential backoff; on success `mount_info` is
  re-read and `writable` re-published.

**Results**

- **Reconnecting is not restoring a connection, it is re-reading the facts.** A share
  can come back *different* — an export flipped to read-only while the app was
  running — and a manager that only restored the socket would leave the badge and every
  write verb wrong. The test that matters knocks a mount over, waits for the
  `Reconnecting` event, brings it back read-only, and asserts the published facts say
  so. A simpler design passes every other test in this file and fails that one.
- `Connector` is a trait rather than a `match` on the protocol, so a test can supply a
  connector that refuses on demand. The reconnect path most needs testing and a real
  server is worst at reproducing it.
- Testing it also needed a backend whose health flips *after* it has been handed out:
  a connector flag is not enough, because the session already holds the backend and the
  case under test is a mount that stops answering while held. The first version of the
  test set a connector flag, and passed while proving nothing.
- A refused connection keeps its row in `Error` rather than dropping it. The bookmark
  vanishing on a failed connect would be worse than the failure.
- Events are a bounded `broadcast`, and every event carries the whole state rather than
  a delta. A slow subscriber loses old events instead of growing a queue, and losing one
  costs latency rather than correctness.
- `SessionManager` aborts its health tasks on drop. Without that they hold `Arc`s to the
  shared state and keep reconnecting to shares nobody holds; there is a test for it.
- The shell now holds a real `SessionManager` and forwards `mount://changed` to the
  window. It falls back to the mock share when nothing is mounted, so the window has
  something to draw on first run without pretending to be connected.

**Next steps**

- The frontend still reads its fixture; **E3-S1 (the connection dialog)** and the
  bookmark store are what let a user mount something from the UI rather than from
  `xcal`.
- `ProtocolConnector` answers SMB, NFS and SFTP with `Unsupported` until E4, E5 and E6.
  That is honest, and it is what a UI reads to grey out a protocol tab.
- The health interval is 15s with a 1s→30s backoff, chosen rather than measured. Worth
  revisiting against a real flaky link.

#### E2-S3 — Tauri commands and events
**AC**
- Commands: `connect`, `disconnect`, `test_connection`, `list_dir`, `stat`,
  `read_text_preview`, `download`, `upload`, `mkdir`, `rename`, `remove`, `capacity`,
  `bookmarks_*`, `prefs_*`, `transfers_*`, `quicklook_open`, `edit_open`.
- Events: `mount://changed`, `listing://changed`, `transfer://progress` (carries
  `transfer id`), `connection://health`, `edit://written`.
- DTOs are `serde` structs owned by `src-tauri`; no engine enum crosses IPC unmapped.
- `list_dir` returns `{ entries, mount: MountInfo }` — writability rides with every
  listing.

#### E2-S4 — Error taxonomy to UI
**AC**
- Every `EngineError` maps to one of: per-item (→ transfer queue row), connection-level
  (→ banner, E15), or validation (→ inline field error). Nothing maps to a modal
  alert *(handoff §Errors)*.

### E3 — Connections, credentials, bookmarks

#### E3-S1 — New Connection dialog — **Done (the protocols that exist)** *(handoff §Screens/3)*

*2026-09-03.*

**AC**
- [x] 460px dialog, 34px title bar "New Connection", body `#f4f4f5` padding
  `16px 18px 18px`.
- [x] Protocol segmented control on a 2px-padded `#e2e2e5` track, radius 6px, 24px
  tabs, active white + shadow + 600; tabs **SMB · NFS · SFTP · xsync**.
- [x] Form grid `96px 1fr`, gap `9px 12px`, right-aligned 11.5px labels, and a field
  set that changes with the protocol: SMB gains Domain and the keychain checkbox, NFS
  drops username, password and port entirely, SFTP and xsync carry a port.
- [ ] NFS's Version and Security controls, and SFTP's auth-method picker and key file.
  **Not built:** every one of them selects between things that do not exist — NFSv4
  and Kerberos are E5-S5, key and agent auth are E6-S1. A disabled control that will
  never enable in this build is a promise, and the notice under the form is a
  straighter way to say the same thing.
- [ ] xsync's "Browse exports…" picker. **Deferred** with E7-S3, which needs
  `ListExports`; `xs --server` has one unnamed export.
- [x] Live URI preview, built from a partial draft so it moves as fields change.
- [x] `Test` runs `test_connection`, with the result inline in green or red.
- [x] `Cancel` and primary `Connect`, 26px, right-aligned; `Connect` disabled until the
  required fields are valid.
- [x] "Mount as" is a preference passed to the backend; the mount's actual writability
  still comes from the server. There is a test that the spec carries the preference and
  no secret.

**Results**

- **The field set changes rather than greying out.** An inert password box on the NFS
  tab would tell a user that NFS has a password, which it does not — its identity is a
  `sec=` mode. Which fields a protocol has is a fact about the protocol, so it lives in
  `state/connection.ts` next to the validation, not in the component.
- `Test` connects for real and reports the mount facts, including writability, rather
  than probing the port. The questions a user is actually asking are "are my credentials
  right, does this export exist, will I be able to write", and none of them is
  answerable by a successful TCP connect. Reporting "reachable" from one would be
  answering an easier question than the one asked.
- The dialog says plainly which protocols have a backend. `ProtocolConnector` answers
  the other three with `Unsupported`, and letting someone fill in a form that cannot
  connect is worse than telling them first.
- The URI preview is built from a partial draft, so it moves while typing rather than
  appearing once the form validates. `sftp://host:22` collapses to `sftp://host`,
  because a default port is noise and a non-default one is information.
- Nine tests cover the rules — the URI, the per-protocol field sets, validation, and
  that a password never reaches the spec — without rendering a dialog.
- Switching protocol keeps the host, username and mount preference. Re-typing a
  hostname because you changed tab is the kind of small rudeness that makes a dialog
  feel hostile.

**Next steps**

- **E3-S2 (bookmarks store)** is next: `Connect` currently mounts and closes, but
  nothing persists, so the sidebar's rows are still fixed.
- The browser build refuses `Test` and `Connect` with "not connected to the engine".
  That is honest about what a browser preview is, but it means the dialog's happy path
  is only exercisable in the desktop app.
- Password and passphrase fields collect a secret that currently goes nowhere. **They
  must not be wired up before E3-S3 (keychain)** — an interim path that keeps a
  password in frontend state or a JSON file is exactly what the design forbids.

#### E3-S2 — Bookmarks store — **Done (probe runs once)**

*2026-09-03.*

**AC**
- [x] `Bookmark { id, name, protocol, host, port?, share, username?,
  mountPreference }` persisted via `tauri-plugin-store`; **never contains a secret**.
  There is a test for that specifically, because the file is plain JSON in the app's
  data directory and anything written there should be assumed readable.
- [x] Sidebar `+` opens the dialog; `−` removes the selected bookmark behind a
  one-step undo toast; `Edit` reopens the dialog pre-filled.
- [x] Reachability probe (TCP connect, 2s timeout) drives the "unreachable" state.
- [ ] Re-probing every 60s while idle. **Not built:** the probe runs once on start.
  A timer that fires regardless of whether the window is visible would wake a sleeping
  laptop to ask four servers whether they are up, so this wants a visibility check
  first, and that is worth doing deliberately rather than as a `setInterval`.

**Results**

- **The store never sees a secret**, and that is the property the tests lead with. A
  password reaches the OS keychain (E3-S3) or nowhere; the bookmark carries a username
  because that is not a secret, and nothing else.
- The id is derived from protocol, host and share, lower-cased. Connecting to a share
  a second time updates its row rather than growing an identical one, and re-typing the
  host with different capitalisation does not produce a duplicate.
- **Undo restores to the original index, not the end.** An undo that appends silently
  reorders the sidebar, which is exactly the kind of thing that makes undo feel
  untrustworthy. The test asserts the order, not just the membership.
- A bookmark is saved on a *successful* connect, not on opening the dialog: a
  connection that never worked is not worth remembering.
- The probe is a TCP connect rather than a mount. It runs for every saved bookmark, and
  mounting each one would ask servers for credentials nobody typed. It answers a
  deliberately weaker question — "is something listening" — and the design uses it for
  exactly that, dimming a row rather than claiming the share is usable.
- The browser preview seeds the design's four sample rows when nothing is saved,
  including the unreachable one. Without them the preview's sidebar is empty and the
  states worth looking at cannot be seen at all; the desktop app starts empty, as it
  should.
- A corrupt or unreadable store loads as empty rather than failing. The bookmarks are a
  convenience, and an empty sidebar is recoverable where a window that will not open is
  not.

**Next steps**

- **E3-S3 (keychain)** is the gate on the dialog's password field, which currently
  collects a secret that goes nowhere. It must stay that way until the keychain exists.
- The 60s idle re-probe wants a document-visibility check so it does not poll a hidden
  window.
- Mounting from a bookmark — clicking a saved row to connect it — is not wired yet; the
  row selects. It belongs with E8-S7's navigation, which is what decides where the
  listing goes afterwards.

#### E3-S3 — Keychain — **Done (stored, not yet consumed)**

*2026-09-03.*

**AC**
- [x] `keyring` crate: service `excalibur`, account `<bookmarkId>`; passwords, passphrases
  and tokens stored only when "Save in keychain" is on; deleted when the bookmark is
  deleted or the box is unticked.
- [x] Secrets are `zeroize`d after use in Rust; redacted from `Debug`; never logged; never
  sent to the webview.
- [x] Linux without a Secret Service: the checkbox is disabled with the tooltip "No
  keychain available — credentials will be asked for each time".

**Results**

- **"Never sent to the webview" is structural, not a rule anyone has to remember.**
  There are commands to save and to forget a credential, and deliberately **no command
  to read one back** — the window collects a password once, as the user types it, and
  never sees it again. The engine will fetch it from the keychain itself when it
  connects. A getter here would undo the property in one line, so its absence is the
  design and `src-tauri/src/lib.rs` says so in a comment.
- `Secret` wraps `Zeroizing<String>` and its accessor is called `expose()` rather than
  `get()` or `as_str()`, so every point where a credential leaves the type is greppable
  by one word. Its `Debug` prints `Secret(<redacted>)`, which is what keeps it out of a
  log line written by someone who only derived `Debug` on a surrounding struct.
- **Availability is answered by a probe round-trip, not by asking the platform.** A
  Linux desktop with no Secret Service still has a keyring API that accepts every call
  and stores nothing; a capability check that trusts it would offer "Save in keychain"
  and silently lose the password. `KeyringStore::available()` writes, reads back and
  deletes a value on a reserved account, so the answer is what the machine actually
  does. It cost a real round trip to learn that, which is the right trade for not lying
  to the user.
- The keychain commands are `async` over `spawn_blocking`. Tauri runs a non-`async`
  command on the main thread, and a macOS Keychain call can put up a system prompt and
  sit there until the user answers — which would freeze the window behind it. Clippy's
  `needless_pass_by_value` on the first draft is what surfaced this.
- Verified against the **real macOS Keychain**, not a mock: `crates/engine/tests/keychain.rs`
  round-trips a credential and is gated on `EXCALIBUR_KEYCHAIN_TEST=1` so CI, which has
  no unlocked keychain, does not fail on it. Run it with
  `EXCALIBUR_KEYCHAIN_TEST=1 cargo test -p excalibur-engine --test keychain`.
- The browser preview reports no keychain, so the disabled state is the one on screen
  there: the checkbox greys out, gains the inline hint "— none on this machine" and the
  AC's tooltip verbatim. That is the Linux-without-Secret-Service case, visible on a Mac.
- **The credential is stored and deleted correctly, but nothing reads it yet.** xsync
  over SSH authenticates through OpenSSH and never asks for a password; SMB and SFTP,
  which will, arrive with E4 and E6. So the write path is done and the read path has no
  caller — worth stating plainly rather than implying the feature is closed.
- Housekeeping this story forced, both worth having: `cargo deny check` had been failing
  since M0 (sixteen `unmaintained` notices, all reached through Tauri, plus the
  workspace's own path deps read as wildcards) — now accepted individually with reasons
  in `deny.toml`, so a genuinely new one still fails the build. And `docs/supply-chain.md`
  now exists, matching xsync's, recording the audit and why each direct dependency is
  there.

**Next steps**

- **A connect path that reads the secret** is what closes the loop, and it arrives with
  the first backend that authenticates with a password (E4 SMB or E6 SFTP). That is also
  when the account key gets exercised against a real bookmark id.
- **E3-S4 (host keys and TLS pins)** is the other half of connection trust. **Done**,
  below.
- Re-prompting when a stored credential is rejected — the share's password changed —
  has no path yet. It belongs with whichever backend first reads a secret.

#### E3-S4 — SSH host keys and TLS pins (SFTP, xsync) — **Done (SSH; TLS has no transport yet)**

*2026-09-03.*

**AC**
- [x] Unknown host key / server fingerprint → trust prompt showing algorithm and
  fingerprint; accepted keys stored in the app data dir (`known_hosts`-compatible for
  SSH); changed key hard-fails with a red banner and no "accept anyway" in the
  banner itself (a deliberate second step in the bookmark editor).
- [ ] TLS pins. **Not built:** there is no TLS transport to pin. `xs serve` over TLS is
  `xsyncv3.md` E2-S1 and arrives with M6; SFTP has no TLS at all. Building a pin store
  against no connection would be guessing at its shape.

**Results**

- **This fixed a live bug, not just an absent feature.** `xs` is reached by spawning
  `ssh`, and `ssh` verifies host keys by asking on a terminal. A GUI has no terminal,
  so the question could not be answered: every first connection to a new host died with
  `Host key verification failed.` on an inherited stderr nobody was reading. The
  decision now happens before `ssh` runs, where it can be shown.
- The flow is: `ssh-keyscan` asks the host what keys it has → `verify` compares them
  against the app's own `known_hosts` → the window prompts or refuses → the connection
  itself then runs with `StrictHostKeyChecking=yes` against that same file. **The last
  step is what makes the first one safe.** `ssh-keyscan` opens its own connection, so an
  attacker could answer the scan with the real key and the session with theirs — and
  gain nothing, because the session is checked against what the scan stored. The scan
  decides *what to ask about*; it never decides what to trust.
- **A changed key and an unknown key are deliberately different shapes on screen**, and
  that difference is enforced in the engine rather than in the markup: `HostKeyUnknown`
  has `Surface::TrustPrompt` and `HostKeyChanged` has `Surface::Banner`, with a test
  asserting it. An unknown host is a question with a fingerprint to check and a button
  that answers it. A changed key is not a question — no fact available in that dialog
  distinguishes an impersonated host from a rebuilt one — so it renders with **zero
  buttons inside the banner**, verified in the browser.
- **`~/.ssh/known_hosts` is deliberately neither read nor written.** Writing it would
  mean an app silently editing the file every other SSH tool depends on. Reading it
  means implementing hashed entries (`HashKnownHosts` is on by default on most Linux
  distros), which is HMAC-SHA1 over a salt — and getting that subtly wrong would either
  hard-fail a host whose key never changed or fail to notice one that did. Asking the
  user once more is a much cheaper mistake than either.
- **Testing against a real host found a bug that would have hit every macOS user.**
  OpenSSH reads `UserKnownHostsFile` as a *space-separated list*, and the default data
  directory on macOS is `~/Library/Application Support`. Unquoted, the path split into
  two filenames that both did not exist, and `ssh` reported "No host key is known" for a
  key it had just been handed. The path is quoted now. No unit test would have caught
  this; running `xcal` against `github.com` did, on the first try.
- The stored file is genuinely `known_hosts`-compatible, not merely similar:
  `ssh-keygen -lf` reads it and prints the same fingerprint the prompt showed.
- `BatchMode=yes` goes on the ssh command line for the same reason as the strict
  checking: a password or passphrase prompt would go to a terminal that is not there.
  It converts a hang into an immediate, reportable failure. Key and agent auth, which is
  what `xs` over SSH is set up for, are unaffected.
- A revoked key (`@revoked` in the file) is refused even when the host also offers a good
  one, and a host that merely *added* an algorithm stays trusted rather than alarming —
  the case that would otherwise train users to click through key warnings.
- `xcal --xsync-ssh HOST:PATH [--trust]` runs the same verification on the terminal,
  which is how the whole path was exercised against a real server. `--trust` accepts an
  unknown key and, by design, does nothing at all to a changed one.
- The frontend now parses the engine's error object instead of `String(error)`-ing it.
  That had been quietly turning every Tauri-side failure into `[object Object]`;
  `src/state/errors.ts` is the one place that knows the shape, with tests.
- Both states are viewable without a server: `?screen=trust` and `?screen=keychanged`.

**Next steps**

- **Forgetting a key from the bookmark editor** is the other half of the changed-key
  path. `forget_host_key` exists and is registered; the editor has no control wired to
  it yet, so today the recovery is to edit the file. That control belongs with the
  bookmark editor's own story rather than being bolted onto the banner — which is
  exactly the separation the AC asked for, but it does mean the second step is currently
  manual.
- **TLS pinning** waits on a TLS transport (`xsyncv3.md` E2-S1, M6).
- `ssh` stderr is still inherited, so an *auth* failure is as invisible as a host-key
  failure used to be. Capturing it is E15-S2's job (the auth failure loop guard), and it
  wants the same treatment this story gave host keys.
- SFTP (E6-S1) reaches SSH through `russh` rather than a subprocess, so it will need the
  same `KnownHosts` store driven from a host-key callback instead of from `ssh-keyscan`.
  The store was written with that in mind; `verify` is the part that will not transfer.

### E4 — SMB backend

#### E4-S1 — Client library spike and decision
**AC**
- Both `smb` (pure Rust) and `pavao` (libsmbclient) are wrapped behind `ShareBackend`
  and run the same matrix: Samba 4.x (Docker), macOS built-in `smbd`, a Synology or
  TrueNAS, Windows 11. Matrix records: dialect negotiated, signing, encryption,
  auth (NTLMv2, Kerberos), RO-share detection, large-dir listing time, 1 GiB
  read/write throughput, rename-with-replace.
- Decision recorded in `docs/decisions/0001-smb-client.md`; the loser is deleted, not
  left as dead code.

#### E4-S2 — Connect, tree connect, writability
**AC**
- SMB 3.1.1 preferred, 3.0.2/3.0/2.1 negotiable; SMB1 never.
- `MountInfo.protocol_label` = `SMB3` / `SMB2`; `security_label` = cipher when
  encrypted (`aes-128-gcm`, `aes-256-gcm`) or `signing on`/`signing off`.
- `writable` = share `MaximalAccess` includes `FILE_WRITE_DATA` ∧ share not
  `READ_ONLY` ∧ user did not request RO; `reason` explains the failing term
  (e.g. "share grants read-only access").

#### E4-S3 — Listing and attributes
**AC**
- `QUERY_DIRECTORY` with `FileIdBothDirectoryInformation`; hidden bit → `Attrs.hidden`;
  10k-entry directory listed in < 300 ms on LAN against Samba (benchmarked).
- Kind detection per E8-S5 uses name + attributes.

#### E4-S4 — Range I/O, transfers
**AC**
- `open_read(offset)` uses `READ` with multi-credit pipelining (≥ 4 × 1 MiB in
  flight); `open_write(Resume{offset})` writes at offset; `FLUSH` on close.
- 1 GiB read and write ≥ 80% of the SMB throughput `smbutil`/`mount_smbfs` achieves
  on the same host pair (published in `docs/benchmarks.md`).

#### E4-S5 — Mutations and capacity
**AC**
- `mkdir`, `rename(replace)` via `SET_INFO FileRenameInformation`, `remove` (file and
  recursive directory via client-side walk), `set_mtime`; `capacity` from
  `QUERY_FS_INFO FsFullSizeInformation`.

#### E4-S6 — Reconnect
**AC**
- On `STATUS_NETWORK_NAME_DELETED`/socket drop, session manager reconnects and
  re-tree-connects; `writable` re-read; open transfers resume at offset (E13-S6).

### E5 — NFS backend

#### E5-S1 — NFSv3 client
**AC**
- `nfs3_client` (pure Rust) wrapped behind `ShareBackend`: `MOUNT` → root fh,
  `LOOKUP`, `GETATTR`, `READDIRPLUS`, `READ`, `WRITE`+`COMMIT`, `CREATE`, `MKDIR`,
  `RENAME`, `REMOVE`/`RMDIR`, `SETATTR`, `FSSTAT`, `FSINFO`, `ACCESS`, `READLINK`.
- Tested against Linux `nfsd` (Docker, privileged) and macOS `nfsd` with
  `ro`/`rw`/`root_squash` variants.

#### E5-S2 — Identity and ports
**AC**
- AUTH_SYS with configurable uid/gid (default: current user's), shown in the dialog
  as "Identity"; `insecure`-less exports produce a clear message ("server requires
  a privileged client port; add `insecure` to the export") — never an attempt to
  bind < 1024.
- TCP only; UDP not offered.

#### E5-S3 — Writability *(handoff §Screens/2)*
**AC**
- `writable` = `ACCESS` on root grants `MODIFY|EXTEND` ∧ `FSINFO`/`PATHCONF` not
  read-only ∧ a create-probe in a temp name succeeds when the user did not request
  RO (probe is skipped and result is `false` if the user asked RO). `reason` mirrors
  the mock: "Read-only — export mapped `ro,root_squash`" is produced when `ACCESS`
  denies write and the uid maps to `nobody` (detected via `GETATTR` on a probe or
  `uid == 65534`).
- `protocol_label` = `NFSv3`; `security_label` = `sys`.

#### E5-S4 — Range I/O and transfers
**AC**
- `rsize`/`wsize` negotiated from `FSINFO` (`rtmax`/`wtmax`, cap 1 MiB); ≥ 4 reads in
  flight; `COMMIT` on close; throughput ≥ 80% of the kernel client on the same pair.

#### E5-S5 — NFSv4 and Kerberos (later)
**AC**
- Spike `libnfs` bindings vs. a minimal v4.0 client for `krb5/krb5i/krb5p` via
  `gssapi`; the dialog's Version/Security controls light up when a backend passes the
  E5-S1 matrix; `security_label` becomes `krb5p` etc. Not a v1 blocker; UI is honest
  until then.

### E6 — SFTP backend

#### E6-S1 — Port Kestrel's SFTP backend
**AC**
- `russh` 0.63 + `russh-sftp` 2.4, exact-pinned; auth: password, private key
  (+passphrase), `ssh-agent`, keyboard-interactive; TOFU host keys (E3-S4).
- `ShareBackend` methods map 1:1 to SFTPv3 ops; `open_read(offset)` uses `read` at
  offset with ≥ 8 outstanding 256 KiB requests; `open_write(Resume)` opens
  `WRITE|APPEND`-free positional writes.
- Test against the bundled in-process russh server fixture (ported) and OpenSSH in
  Docker.

#### E6-S2 — Writability and capacity
**AC**
- `capacity` via `statvfs@openssh.com`; `writable` = `f_flag` lacks `ST_RDONLY` ∧
  create-probe succeeds (skipped if RO requested); `reason` e.g. "filesystem is
  read-only" or "permission denied on `/srv`".
- `protocol_label` = `SFTP`; `security_label` = KEX/cipher from russh (`aes256-gcm`).

#### E6-S3 — Mutations
**AC**
- `rename(replace)` uses `posix-rename@openssh.com` when advertised; otherwise
  remove-then-rename and `Caps.atomic_rename = false` (transfer worker uses the
  safe path).

### E7 — xsync backend *(primary; first real backend)*

#### E7-S0 — Prerequisite: `xsyncv3.md` Phase 1
**Status (2026-09-03):** in progress on xsync branch `v3`, uncommitted. Done: E11-S1
(Phase 1 wire freeze), E3-S6 (negotiation + `Features` exchange), E3-S7 (error model),
E3-S1 (concurrent dispatch with per-handle ordering). Next: E1-S4 `Mount`, then E4
open/read/write. See `xsyncv3.md` §Progress.

**AC**
- `../xsync` has merged the Phase 1 stories (E11-S1 table, E3-S1, E3-S6/7, E4-S1–S3,
  E4-S8, E5-S1–S3, E1-S4, E9-S1, E9-S4, E9-S6, E10-S1, E10-S4) with vectors frozen and
  `CAP_FS_V3` advertised by `xs --server`.
- `xsync-client::Fs` and Excalibur's `ShareBackend` were designed together: every
  `ShareBackend` method has a direct `Fs` counterpart, and the adapter in E7-S2 is
  mechanical (no buffering, no emulation).
- Dependency is a pinned git revision of `xsync-client`, with a documented local
  `[patch]` override for development (Kestrel XS-U2 pattern).

#### E7-S1 — Reduced-mode fallback over browse v2 *(deferred to M4)*
For remotes running an older `xs` that only speaks browse v2 (xsyncv3 E11-S2 downgrade).

**AC**
- Over `ssh host xs --server <root>`, implement `list_dir`, `stat`, `mkdir`, `rename`,
  `remove`, `set_mtime` via browse v2; `open_read`/`open_write` implemented as
  whole-file `Fetch`/`Publish` behind `Caps.random_access = false`.
- UI shows a status-bar chip "reduced mode — upgrade remote xsync" and disables
  Quick Look for files > 8 MiB and all video previews; transfers are whole-file.
- `writable` = `true` unless the probe returns `permission denied` on a create-probe;
  `protocol_label` = `xsync v2`; `security_label` = `ssh`.

#### E7-S2 — v3 backend — **Done (what v3 offers today)**

*2026-09-03.*

**AC**
- [x] `xsync-client` → `ShareBackend` adapter; `MountInfo` maps from xsync's
  (`effective_writable` → `writable`, `reason`, `options`). *(No `SecurityInfo` to map:
  that arrives with the TLS transport, `xsyncv3.md` E2-S1. Over SSH the label is `ssh`,
  which is the honest amount to claim — the security is OpenSSH's and this backend
  cannot describe it.)*
- [x] Range reads and writes; `StatFs` → capacity.
- [ ] Staged resumable uploads and CAS on write-back. **Deferred:** `StageOpen/Write/
  Commit` and `expect_cookie` are `xsyncv3.md` E4-S6/E4-S7 and do not exist yet. Plain
  positional writes are what v3 has, so `Caps::atomic_rename` is false and the transfer
  worker will use the safe path.
- [ ] `watch` → live listing refresh. **Deferred** with E7 on the xsync side; `Caps::watch`
  reads the `NOTIFY` support bit, which no server sets yet, so it is already wired to
  turn on by itself.
- [ ] Session `Resume` as a `reconnecting` blip. **Deferred** with E3-S2 on the xsync
  side; there is no session identity to resume against.

**Results**

- **This is the first real protocol backend, and it works end to end.** `xcal ls
  --xsync-local <dir> --xs <binary>` spawns `xs --server`, negotiates v3, mounts, lists,
  reads with digest verification, and writes. Three integration tests cover it against a
  real server, skipped unless `EXCALIBUR_XS` names an `xs` binary — mocking the protocol
  here would only test the mock.
- **The adapter earned its place immediately by finding two bugs**, which is the reason
  `xsyncv3.md` E9-S1 deliberately left the `Fs` trait unbuilt until something consumed
  the client:
  - `Mount::open` sent `mode: 0`, so a created file landed as `----------` and the next
    read of it was `EACCES`. The server was doing exactly as asked; the *convenience*
    method should not ask for that. It now passes `0o666` before umask when the flags
    create, like an ordinary `open(2)`. Fixed upstream, because every consumer would
    have hit it.
  - The client had no way to drive a session over separate read and write halves, which
    is what a child process and an SSH channel both are. `Client::from_split` is now
    public; `connect_ssh` was already doing this internally.
- **Three verbs are refused as `Unsupported` rather than emulated**: `mkdir`, `rename`
  and `remove` have no v3 message (`xsyncv3.md` E5-S4). A UI reads that and disables the
  verb, which is better than offering it and reporting a confusing failure. It is also
  the clearest possible statement of what E5-S4 is for.
- A write-class open is refused locally *and* by the server. The local check saves a
  round trip; the server's is what makes a bug in the local one harmless.
- `capacity` reports `available_bytes`, not `free_bytes`: the root-only reserve is not
  space this session can use, and showing it would overstate what a user can write.
- Handles are closed on the failure path as well as the success path. A leaked handle
  per failed listing would exhaust the session's table, and the failure that leaks is
  the one that repeats.
- The listing asks for one optional attribute block (`SYMLINK_TARGET`), not all of them.
  Each block costs the server work per entry, so asking for everything would make a
  large directory pay for columns nothing shows.

**Next steps**

- **Extract `xsync_client::Fs` now.** This adapter is the evidence E9-S1 was waiting
  for: the methods it actually needed are list/stat/read/write/statfs/capacity, and the
  ones it could not use say as much about the trait as the ones it could.
- The dependency is a path dep to a sibling checkout. E7-S0 wants a pinned git revision
  with a documented `[patch]` override; that needs xsync's `v3` branch pushed somewhere
  first, so it is blocked rather than skipped.
- **E2-S2 (session manager)** is what lets the app hold this backend rather than the
  mock, and is the next thing between here and a GUI showing a real share.

#### E7-S3 — Export picker
**AC**
- The dialog's "Browse exports…" lists `ListExports` results with access badges
  (RW/RO) and comments; picking one fills the Export field.

### E8 — Browsing

#### E8-S1 — Listing state and loading *(handoff §Loading)*
**AC**
- `listing: { entries, loading, error }`; navigation cancels the previous request;
  listings < 200 ms show no indicator; past 200 ms a 2px indeterminate accent bar
  under the column header appears and the previous listing stays visible at
  `opacity: .5`.

#### E8-S2 — Column layout `1f` *(handoff §Screens/1 Column header, Rows; §6 `1f`)*
**AC**
- Header grid `1fr 96px 156px 128px`, gap 16px, padding `0 12px`, bg `#f4f4f5`,
  border-bottom `#dcdcde`, 10.5px/600 `#6a6a70`, sort indicator 8px `#2f6df6` ▲/▼.
- Rows 24px, border-bottom `#f2f2f3`; name cell flex gap 8px, glyph `flex: none`,
  ellipsis truncation; Size mono 11px right `#6a6a70` (`—` `#8a8a90` for folders);
  Modified `31 Aug 2026, 18:24`; Kind human-readable.
- Selected row: bg + border `#2f6df6`, white text, glyph stroke `rgba(255,255,255,.7)`.
- Dotfiles `opacity: .55`, Kind "Hidden", shown only when `showHidden`.
- Virtualised: 100k rows scroll at 60fps (measured with the mock backend).
- Columns click-to-sort and drag-resizable; widths persisted per layout.

#### E8-S3 — Layout `1g` name-led *(handoff §6 `1g`)*
**AC**
- `display: flex; gap: 9px; padding: 6px 10px`; glyphs 16×13 / 13×16; name 12.5px;
  second line mono 10px `48.2 GB · mkv · 31 Aug 18:24`; soft selection `#e8effd` with
  `#4a6ba8` metadata; names never truncate; no column resize.

#### E8-S4 — Layout `1h` mono grid *(handoff §6 `1h`)*
**AC**
- Card `#1a1b1d`, header `#26272a` mono 10px uppercase `.05em`, 22px rows, all mono
  11px; leading `d` in `#4f86ff` / dim `-`; sizes `48.2G`/`2.0K`; dates `MM-DD HH:MM`.
- `rowLayout` persisted; switch via View menu and a shortcut.

#### E8-S5 — Kind detection
**AC**
- Extension → kind table (≥ 150 common types, "Matroska Video", "SubRip Text",
  "Folder", "Hidden", fallback "<EXT> File" / "Document"); sniffing is *not* done in
  listings (no reads); the same table drives icons (E0-S3) and Quick Look routing.

#### E8-S6 — Sort and filter
**AC**
- `sort: { column, ascending }`; folders-first is a preference (default on);
  natural sort for names; stable.
- Filter is client-side substring (case-insensitive, diacritic-insensitive) over the
  current listing; updates per keystroke without re-listing; counts in the status bar
  reflect the filtered set.

#### E8-S7 — Navigation and history *(handoff §Navigation)*
**AC**
- Double-click folder descends; `↑` ascends; breadcrumb click jumps; back/forward keep
  a per-window `{stack, index}`; sidebar double-click mounts then opens the root.
- Cmd/Ctrl+↓ descends into the selected folder, Cmd/Ctrl+↑ ascends.

#### E8-S8 — Selection model *(handoff §Selection)*
**AC**
- Click selects; shift-click extends; Cmd/Ctrl-click toggles; click-drag rubber-bands
  (rectangle intersects rows); type-to-select jumps to the first match with a 700 ms
  typing buffer; Escape clears filter first, then selection.
- `selection: Set<string>` of paths; verbs and status counts derive from it.

#### E8-S9 — Live refresh
**AC**
- Cmd/Ctrl+R refreshes; when the backend offers `watch` (xsync v3) the listing
  refreshes on events, coalesced to at most one relist per 250 ms; an overflow event
  triggers a full relist.

### E9 — Permission gating

#### E9-S1 — One derived value *(handoff §Permissions gating)*
**AC**
- `mount.writable` (from the latest `list_dir`/`mount_info`) is the *only* input to:
  status badge, disabled state of Edit/Upload/rename/delete/new-folder, drop-target
  acceptance, Quick Look footer copy, and the `Remount as RW` recovery button.
- A property-based test enumerates every write affordance and asserts it is disabled
  when `writable = false`; no write command is ever *sent* to the engine in that state
  (the engine also refuses with `EROFS` as defence in depth).

#### E9-S2 — Disabled styling *(handoff §Screens/2)*
**AC**
- Dark: bg `#2e2f32`, border `#3a3b3f`, text `#63646a`, `cursor: not-allowed`; light
  equivalent derived from tokens; tooltip explains the reason string.

#### E9-S3 — Remount as RW
**AC**
- Re-runs `connect` with `mountPreference = 'rw'`; on success re-reads `writable`,
  and if still read-only, the queue row error text updates to the server's reason
  rather than looping.

### E10 — Status bar *(handoff §Screens/1 Status bar)*

#### E10-S1 — Groups and dividers
**AC**
- 24px, bg `#f7f7f8`, 10px gaps, 1×12px `#dcdcde` dividers. Groups: permission badge
  (mono 9.5px/500 `.04em`, `RW` white-on-`#2f6df6`, `RO` `#e8e8ea`-on-`#4a4b50` in
  dark), plain-language restatement with mono 10.5px path, counts
  `13 items · 1 selected · 48.2 GB` (selected size), capacity `2.4 TB free of 16 TB`,
  right-aligned mono 10px protocol/cipher + 6px health dot (`#2f9e5f` healthy,
  `#c0392b` failed).
- Restatement text: RW → "Read & write on `<path>`"; RO → `reason` from `MountInfo`.
- Capacity refreshes on mount, on reconnect, and after every transfer completes.

### E11 — Quick Look *(handoff §Screens/5)*

#### E11-S1 — Panel
**AC**
- Separate always-dark 440px panel window; header 30px `#2a2b2e` with filename
  11.5px/500, mono 10px `#9a9ba0` `2 KB · text/plain`, `Open in editor`, `✕`; body
  `#1e1f21` padding 14px; footer 24px with RW/RO chip and copy per handoff;
  right-aligned "Space to close".
- Space toggles; Escape closes; arrow keys move selection and the preview follows.

#### E11-S2 — Text
**AC**
- Mono 11px, `line-height: 1.8`, `#c4c4c8`, non-selectable 1-based line numbers
  `#63646a`; reads the first 1 MiB via `open_read(0)` and lazily continues on
  scroll; encoding sniff (UTF-8, UTF-16 BOM, Latin-1 fallback); binary detection →
  "unpreviewable" card.

#### E11-S3 — Images, video, audio
**AC**
- Images render centred, contained; video/audio use the platform `<video>`/`<audio>`
  fed by a Tauri custom protocol (`excalibur-stream://mount/path`) that honours HTTP
  `Range` by calling `open_read(offset)`; seeking a 40 GB MKV starts playback within
  2 s on LAN.
- Disabled (with the reduced-mode chip) when `Caps.random_access = false`.

#### E11-S4 — Unpreviewable
**AC**
- Shows kind, size, mtime and a `Download` button that enqueues a transfer.

### E12 — Edit in place *(handoff §Screens/5, "Edit")*

#### E12-S1 — Round trip
**AC**
- `Edit` (toolbar or `Open in editor`) downloads to `~/Library/Caches/excalibur/edit/<id>/<name>`
  (XDG cache on Linux), records the remote `Attrs`/cookie, opens the OS default
  editor (`open`/`xdg-open`), and watches the file with `notify`.
- Each save uploads via `open_write(Create)` to a temp name and atomic rename (or
  xsync `StageCommit`/`Write` with `expect_cookie`); a toast and a queue row record
  each write-back *(handoff: "Show a toast or a queue entry")*.
- Remote changed underneath (cookie/mtime mismatch) → conflict prompt Replace / Keep
  both / Cancel; never a silent overwrite.
- Disabled on RO mounts; the Quick Look footer reads "Read-only — download a copy to
  edit".
- Temp files are removed when the editor's file handle count drops to zero for 10
  minutes, or on app quit with no unsaved write-back pending.

### E13 — Transfers *(handoff §Screens/4, §Transfers)*

#### E13-S1 — Queue model and window
**AC**
- `Transfer { id, direction, name, sourcePath, destPath, bytesDone, bytesTotal, rate,
  etaSeconds, status, error }`; queue is **per-app**, persisted, survives navigation
  and restart (unfinished items restored as `queued`).
- 560px panel as a separate window (default) or slide-down sheet (preference); header
  34px "Transfers" with 22px `Pause all` / `Clear finished`; footer 24px `#f7f7f8`
  counts + right-aligned aggregate rate.

#### E13-S2 — Rows
**AC**
- Direction chips exactly per handoff (`↑ UP`, `↓ DOWN`, failed red); filename 11.5px,
  500 while active; right mono 10.5px status strings `41.2 / 61.7 GB · 112 MB/s ·
  3m 04s` / `queued` / `done · 84 KB` green; 4px progress bar `#e8e8eb`/`#2f6df6`;
  finished rows drop the bar and go `opacity: .6`; destination path mono 9.5px.
- Failed rows: error in `#c0392b`, inline 20px `Retry` and `Remount as RW` (the
  latter only when the error is a permission error on a RO mount).

#### E13-S3 — Worker
**AC**
- Port Kestrel's worker: N concurrent transfers (pref, default 3), pause/resume/cancel,
  retry with backoff for transient errors, throughput and ETA smoothing, progress
  events at ≤ 10 Hz per transfer via `transfer://progress`.
- Downloads write `<name>.part`, fsync, atomic rename; uploads use the backend's
  atomic path when `Caps.atomic_rename`, else the documented safe fallback.

#### E13-S4 — Download / Upload verbs
**AC**
- `Download` writes the selection (files and folders, recursively) to the configured
  download folder; `Upload` opens a file/folder picker targeting the cwd; both create
  one queue row per file with folder rows aggregating.

#### E13-S5 — Drag and drop *(handoff §Transfers)*
**AC**
- OS → list: accent 2px inset ring while dragging over the list (only when
  `writable`); drop uploads to cwd (or the hovered folder row, highlighted).
- List → OS: drag out initiates downloads to the drop location (Tauri drag-out on
  macOS; Linux via XDG portal/`text/uri-list` after download to a temp then move).
- Drop onto a sidebar row is explicitly a no-op with a "coming later" tooltip
  (handoff marks server-to-server as future).

#### E13-S6 — Conflicts and resume
**AC**
- Conflict prompt Replace / Keep both / Skip with "apply to all remaining"; Keep both
  appends ` 2`, ` 3`… before the extension.
- Downloads resume from `.part` length; uploads resume where `Caps.resume_write`
  (SFTP offset writes, xsync staging) and restart otherwise; a reconnect mid-transfer
  continues from the last acknowledged byte.

### E14 — File operations

#### E14-S1 — New folder, rename, delete
**AC**
- New folder (Cmd/Ctrl+Shift+N) creates `untitled folder` and enters rename; Enter
  renames the selected item inline (RW only); Delete/Backspace deletes.
- Rename/delete confirmations are **"not yet designed"** in the handoff: implement
  rename inline with no confirmation, delete with a single undoable toast for ≤ 5
  items and a minimal confirm sheet above that; both flagged in `docs/design-gaps.md`
  for the designer.
- All refused client-side when `!writable` (E9-S1).

### E15 — Connection health and errors *(handoff §Errors — "not mocked yet")*

#### E15-S1 — Banner
**AC**
- Connection-level failures (unreachable, auth rejected, stale handle, TLS/host-key
  changed) show a strip under the toolbar `#fdeaea`/`#c0392b` with the message and a
  `Reconnect` button; list dims to `opacity: .5`; health dot flips to `#c0392b`.
- Banner clears on successful reconnect; `writable` and capacity re-read.
- Built to the handoff's prose; screenshot attached to `docs/design-gaps.md` for
  sign-off.

#### E15-S2 — Auth failure loop guard
**AC**
- A rejected credential opens the connection editor pre-filled, never a retry loop
  that locks an account; keychain entry offered for update.

### E16 — Keyboard, shortcuts, accessibility *(handoff §Keyboard)*

#### E16-S1 — Shortcut map
**AC**
- Space Quick Look; Enter rename; Cmd/Ctrl+↓/↑ descend/ascend; Cmd/Ctrl+R refresh;
  Cmd/Ctrl+F filter; Escape filter→selection; Cmd/Ctrl+D download; Cmd/Ctrl+U upload;
  Cmd/Ctrl+E edit; Cmd/Ctrl+[ / ] back/forward; Cmd/Ctrl+Shift+N new folder;
  Cmd/Ctrl+, preferences; Cmd/Ctrl+Shift+T transfers; Cmd/Ctrl+1/2/3 row layouts.
- Every toolbar verb's tooltip shows its shortcut; the map is one table in code that
  generates both the bindings and the tooltips.

#### E16-S2 — Accessibility
**AC**
- Focus rings visible (2px accent, offset) on every control; list is an ARIA grid with
  row/cell roles; status bar is a live region for permission and health changes;
  contrast of every token pair used for text meets 4.5:1 (checked by a test over the
  token file, with the documented exceptions listed).

### E17 — Preferences and persistence

#### E17-S1 — Stored state
**AC**
- `theme`, `rowLayout`, `sidebarCollapsed`, `showHidden`, `foldersFirst`,
  `downloadFolder`, `transferConcurrency`, `conflictDefault`, column widths — persisted
  and restored on launch.
- Preferences UI is "not yet designed": a minimal native-feeling sheet with these
  fields only, flagged in `docs/design-gaps.md`.

### E18 — Packaging and release

#### E18-S1 — macOS
**AC**
- Universal `.dmg`, hardened runtime, notarised; auto-update via Tauri updater with a
  signed manifest; app icon set.

#### E18-S2 — Linux
**AC**
- `.AppImage` and `.deb` (x86_64, arm64); desktop entry and icon; Secret Service
  optional dependency documented.

#### E18-S3 — Release process
**AC**
- Tag → CI builds both platforms, runs the full test matrix, attaches artifacts and
  SHA-256 sums, and publishes release notes from `CHANGELOG.md`.

### E19 — Testing and quality

#### E19-S1 — Engine integration matrix
**AC**
- Docker Compose fixtures: Samba (`rw` and `ro` shares), Linux `nfsd` (`rw`, `ro`,
  `root_squash`), OpenSSH, `xs --server`/`xs serve`; each backend passes one shared
  conformance suite over `ShareBackend` (list, stat, range read, resume write,
  mkdir/rename/remove, capacity, writability on RO, reconnect).
- Runs in CI on Linux; macOS CI runs the in-process fixtures only.

#### E19-S2 — Frontend tests
**AC**
- `vitest` component tests against `MockBackend` for every screen state in the
  handoff (`1a`–`1h` plus banner and reduced mode); a visual regression suite
  (Playwright screenshots) compares each state to a rendered reference of the mock
  at 1060px.

#### E19-S3 — End to end
**AC**
- `tauri-driver`/WebdriverIO smoke: connect to Docker SFTP, browse, Quick Look a text
  file, upload, download, disconnect — on Linux CI.

#### E19-S4 — Performance gates
**AC**
- 100k-row list scroll ≥ 55 fps; listing → first paint ≤ 50 ms after data; app cold
  start ≤ 1.5 s on the reference Mac; gates fail CI at a 15% regression.

### E20 — Not yet designed (parking lot — do not build from imagination)

Per handoff §Not yet designed, these need mocks before implementation. Each has a
placeholder story that only records the request:

- Search across a share (vs. filtering one directory).
- Get Info / POSIX mode inspector.
- Disconnected/reconnecting banner *(E15 builds to the prose; visual sign-off pending)*.
- Preferences *(E17 ships a minimal sheet; sign-off pending)*.
- Rename and delete confirmations *(E14 ships minimal; sign-off pending)*.
- Multi-window behaviour (today: one browser window, one transfers window, one Quick
  Look panel).

---

## Milestones

| Milestone | Epics / stories | Demo |
|---|---|---|
| **M-1 xsyncv3 Phase 1** *(in `../xsync`)* | `xsyncv3.md` §5 Phase 1; E7-S0 here | `xs --server` speaks v3: open/read/write at offset, full attrs, statfs, advertised writability; `xsync-client` crate with a conformance fixture. |
| **M0 Skeleton** | E0, E1-S1–S3, E2-S1–S3 (mock backend) | Pixel-matched `1a` and `1b` with fake data; theme switch; sidebar. |
| **M1 First real share (xsync)** | E7-S2, E3-S1 (xsync tab, "via SSH"), E3-S2, E3-S4, E8-S1/S2/S5–S8, E9, E10 | Connect to `ssh host xs --server`, browse, RW/RO from `MountInfo`, capacity from `StatFs`, status bar live. |
| **M2 Read, edit, move** | E11, E12, E13, E14, E15 | Quick Look (incl. video over range reads), edit-in-place with CAS, transfer queue with staged resumable uploads, banner — all on xsync. |
| **M3 Other protocols** | E6 (SFTP, ports Kestrel), E4-S1–S6 (SMB), E5-S1–S4 (NFS), E3-S3 keychain | The handoff's SMB/NFS plus SFTP behind the trait xsync shaped; Docker fixtures arrive here. |
| **M4 Polish** | E8-S3/S4/S9, E16, E17, E1-S4/S5, E19, E7-S1 (reduced mode), E7-S3 | Row variants, shortcuts, a11y, prefs, test matrix green, older-`xs` fallback. |
| **M5 Release** | E18 | Signed macOS + Linux builds. |
| **M6 xsync daemon** | E3-S1 xsync auth fields, E7-S3 export picker (after `xsyncv3.md` Phase 2) | Connect to `xs serve` over TLS with principals and exports, no SSH. |

## Definition of done (v1)

- [ ] Every screen `1a`–`1h` matches the mock within 1px on the metrics the handoff
      calls literal, in both themes, verified by the visual suite.
- [ ] `mount.writable` is provably the only input to every write affordance (E9-S1
      test passes) for SMB, NFS, SFTP and xsync.
- [ ] No file bytes cross IPC; no secret leaves Rust; no plaintext credential is ever
      on disk.
- [ ] The engine conformance suite passes against real Samba, nfsd, OpenSSH and xsync
      servers in CI.
- [ ] Signed, notarised macOS build and Linux AppImage/deb from a tagged CI run.
- [ ] `docs/design-gaps.md` lists every place this plan built beyond the mocks, with a
      screenshot, awaiting design sign-off.
