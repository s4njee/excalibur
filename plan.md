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
| **Linux Secret Service variance** | Headless/no-keyring sessions. | E3-S4 falls back to "don't save" with an explicit message, never to plaintext. |

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
| M0 | E0-S3 — Icon set | Next |
| M0 | E2-S2 — Session manager | Next |
| M1 | E7-S2, E3-S1/S2/S4, E8, E9, E10 | Not started |
| M2+ | everything else | Not started |

M0's demo criterion — the design's `1a` and `1b` rendered from fixed data, with the
theme switch and the sidebar — is met. `?screen=1b` is the one worth looking at: it
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

#### E2-S2 — Session manager
**AC**
- `mounts: Map<bookmarkId, Mount>` lifecycle `mounting → ready → error |
  reconnecting`; one backend instance per mount; `activeMountId` switching does not
  disconnect others.
- Health loop: `ping` every N seconds; failure flips status to `reconnecting`, emits an
  event, and attempts reconnect with backoff; on success `mount_info` is re-read and
  `writable` re-published *(handoff §Permissions gating: "re-read on reconnect")*.

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

#### E3-S1 — New Connection dialog *(handoff §Screens/3)*
**AC**
- 460px dialog, title bar 34px "New Connection", body `#f4f4f5` padding `16px 18px 18px`.
- Protocol segmented control on a 2px-padded `#e2e2e5` track, radius 6px, 24px tabs,
  active white + shadow + 600; tabs **SMB · NFS · SFTP · xsync**.
- Form grid `96px 1fr`, gap `9px 12px`, right-aligned 11.5px `#6a6a70` labels. Field
  sets by protocol:
  - SMB: Server (mono), Share (mono), Username, Password (masked), Domain (optional),
    "Save in keychain", Mount as RW/RO.
  - NFS: Server, Export path (mono), Version (auto/3/4 — v4 disabled with tooltip until
    E5-S5), Security (`sys` | `krb5` | `krb5i` | `krb5p`, Kerberos options disabled
    until E5-S5), optional principal; no password; Mount as RW/RO.
  - SFTP: Server, Port (22), Path (mono), Username, Auth (Password | Private key |
    Agent), key file picker, passphrase (masked), "Save in keychain", Mount as RW/RO.
  - xsync: Server, Port (default from `xsyncv3.md` §7), Export (mono, with a
    "Browse exports…" picker that calls `ListExports` when connected), Auth (SSH key |
    Password | Token | Guest) *or* "via SSH" toggle that switches to host/user fields;
    Mount as RW/RO.
- Live URI preview (`smb://user@host/share`, `nfs://host/export`,
  `sftp://user@host:port/path`, `xsync://user@host:port/export`) in the white mono
  box, updating on every keystroke.
- `Test` runs `test_connection`; result inline: `#2f9e5f` "Reachable · SMB3 · signing
  on" (protocol-specific label from `MountInfo`) or `#c0392b` with the error text.
- `Cancel` / primary `Connect` 26px, right-aligned; `Connect` disabled until required
  fields are valid.
- "Mount as" is a *preference* passed to the backend (used as SMB/NFS/xsync mount hint
  where the protocol supports requesting RO); the resulting `writable` still comes from
  the mount, never from the radio *(handoff §Permissions gating)*.

#### E3-S2 — Bookmarks store
**AC**
- `Bookmark { id, name, protocol, host, port?, share, username?, authMethod?,
  saveCredentials, mountPreference }` persisted via `tauri-plugin-store`; never
  contains a secret.
- Sidebar `+` opens E3-S1; `−` removes the selected bookmark after a 1-step undo
  toast (confirmations are "not yet designed" — this is the least-invasive
  interpretation and is flagged for the designer); `Edit` reopens the dialog
  pre-filled.
- Reachability probe (TCP connect with 2s timeout, on app start and every 60s while
  idle) drives the "unreachable" state.

#### E3-S3 — Keychain
**AC**
- `keyring` crate: service `excalibur`, account `<bookmarkId>`; passwords, passphrases
  and tokens stored only when "Save in keychain" is on; deleted when the bookmark is
  deleted or the box is unticked.
- Secrets are `zeroize`d after use in Rust; redacted from `Debug`; never logged; never
  sent to the webview.
- Linux without a Secret Service: the checkbox is disabled with the tooltip "No
  keychain available — credentials will be asked for each time".

#### E3-S4 — SSH host keys and TLS pins (SFTP, xsync)
**AC**
- Unknown host key / server fingerprint → trust prompt showing algorithm and
  fingerprint; accepted keys stored in the app data dir (`known_hosts`-compatible for
  SSH); changed key hard-fails with a red banner and no "accept anyway" in the
  banner itself (a deliberate second step in the bookmark editor).

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

#### E7-S2 — v3 backend
**AC**
- `xsync-client` `Fs` → `ShareBackend` adapter; `MountInfo` maps from xsync
  `MountInfo` (`effective_writable`, `reason`, `options` → status-bar restatement,
  `SecurityInfo` → `xsync3 · tls1.3-aes-128-gcm`).
- Range reads/writes, staged resumable uploads (`StageOpen/Write/Commit`), CAS on
  edit write-back (`expect_cookie`), `StatFs` → capacity.
- `watch` → live listing refresh (E8-S9) when the server advertises `NOTIFY`.
- Session `Resume` within grace is surfaced as a `reconnecting` blip, not a banner.

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
