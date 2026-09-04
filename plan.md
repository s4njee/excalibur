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
| **Pure-Rust SMB** | `smb` (0.11, pure Rust SMB2/3, signing/encryption) is young and thinly maintained. *(This row used to call libsmbclient LGPL. It is **GPL-3.0-or-later**, which would have made the shipped binary GPLv3 as a whole — see [ADR 0001](docs/decisions/0001-smb-client.md). The error was the premise the whole comparison rested on.)* | **Settled by E4-S1: `smb`.** It sits behind `ShareBackend`, so the choice stays reversible — which is the mitigation that matters, since maintenance is the live risk. |
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
| M0 | E0-S3 — Icon set | Not started (glyphs are placeholders, tinted per category) |
| M0 | E0-S4 — Headless CLI (`xcal`) | **Done** |
| M0 | E1-S1 — Window chrome and four-row layout | **Done** 2026-09-03 |
| M0 | E1-S2 — Toolbar | **Done** |
| M0 | E1-S3 — Sidebar | **Done** |
| M0 | E2-S1 — `ShareBackend` trait and mock | **Done** 2026-09-03 |
| M0 | E2-S2 — Session manager | **Done** 2026-09-03 |
| M0 | E2-S3 — Tauri commands and events | **Done** |
| **M1** | E3-S1 — New Connection dialog | **Done** 2026-09-03 |
| **M1** | E3-S2 — Bookmarks store | **Done** 2026-09-03 |
| **M1** | E3-S3 — Keychain | **Done** 2026-09-03 |
| **M1** | E3-S4 — SSH host keys | **Done** 2026-09-03 |
| **M1** | E7-S2 — xsync v3 backend | **Done** 2026-09-03 |
| **M1** | E8-S1 — Listing state and loading | **Done** 2026-09-03 |
| **M1** | E8-S2 — Column layout `1f` | **Done** 2026-09-03 |
| **M1** | E8-S5 — Kind detection | **Done** 2026-09-03 |
| **M1** | E8-S6 — Sort and filter | **Done** 2026-09-03 |
| **M1** | E8-S7 — Navigation and history | **Done** 2026-09-03 |
| **M1** | E8-S8 — Selection model | **Done** 2026-09-03 |
| **M1** | E9-S1/S2/S3 — Permission gating | **Done** 2026-09-03 |
| **M1** | E10-S1 — Status bar | **Done** 2026-09-03 |
| **M2** | E11-S1/S2/S3/S4 — Quick Look | **Done** 2026-09-03 |
| **M2** | E12-S1 — Edit in place | **Done** 2026-09-03 |
| **M2** | E13-S1/S2/S3/S4 — Transfers | **Done** 2026-09-03 |
| **M2** | E13-S5 — Drag and drop | **Partly done** (inbound only) |
| **M2** | E13-S6 — Conflicts and resume | **Partly done** |
| **M2** | E14-S1 — New folder, rename, delete | **Done** 2026-09-03 |
| **M2** | E15-S1 — Banner | **Done** 2026-09-03 |
| **M2** | E15-S2 — Auth failure loop guard | **Done** 2026-09-03 |
| **M3** | E6-S1 — SFTP backend | **Done** 2026-09-03 |
| **M3** | E6-S2 — Writability and capacity | **Done** (bar `security_label`) |
| **M3** | E6-S3 — Mutations | **Done** 2026-09-03 |
| **M3** | E4-S1 — SMB client decision | **Done** 2026-09-03 (`smb`; see ADR 0001) |
| **M3** | E4-S2…S6 — SMB backend | **Blocked** — `smb`'s `sspi` pins conflict with `russh` |
| **M3** | E5-S1 — NFSv3 client | **Done** 2026-09-03 |
| **M3** | E5-S2 — Identity and ports | **Done** 2026-09-03 |
| **M3** | E5-S3 — Writability | **Done** 2026-09-03 |
| **M3** | E5-S4 — Range I/O and transfers | **Partly done** (512 KiB cap; throughput unmeasured) |
| M3+ | E5-S5 NFSv4/Kerberos, polish, packaging | Not started |

Every "partly done" and every unchecked AC bullet above is written up under its own
story, with the reason it was left. Nothing here is done-but-unmentioned.

M0's demo criterion — the design's `1a` and `1b` rendered from fixed data, with the
theme switch and the sidebar — is met. M1's backend talks to a real `xs --server`, and the session manager holds it. A user can
open the dialog, pick a protocol, connect, and have the bookmark persist. The dialog's
password field now reaches the OS keychain when "Save in keychain" is ticked, though no
backend authenticates with it yet (E4/E6). Browsing is live: double-click descends,
Back/Forward/↑ and the breadcrumb all work, and a slow listing dims rather than blanks.
Preview screens: `?screen=1a` / `1b` / `trust` / `keychanged` / `slow` / `huge` / `banner` /
`transfers`.
`?screen=1b` is now the permission-gating screen too: Edit and Upload disabled with the
server's reason as their tooltip, Quick Look and Download still live, and a
`Remount as RW` recovery button in the status bar. `?screen=1b` is the one worth looking at: it
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

#### E4-S1 — Client library spike and decision — **Done (decision made; see the blocker below)**

*2026-09-03.*

**AC**
- [~] Both candidates run the same four-target matrix. **Partly**: only Samba 4.x in
  Docker was reachable here. macOS's `smbd`, a Synology/TrueNAS and Windows 11 were
  not, and are recorded as untested rather than assumed.
- [x] Decision recorded in [`docs/decisions/0001-smb-client.md`](docs/decisions/0001-smb-client.md).
  **`smb` (pure Rust) wins.** `pavao` was never added to the workspace, so "the loser
  is deleted" is satisfied by it never landing.

**Results**

- **The matrix did not decide this; three things did, and the first was a mistake in
  this very file.** The risk register described `pavao` as wrapping "libsmbclient (C,
  LGPL, mature)". **libsmbclient is GPL-3.0-or-later.** Samba's LGPL parts are only
  `lib/{talloc,tdb,tevent,ldb}`; `source3/libsmb/` is plain GPL with no linking
  exception. That is not a licence *conflict* — MIT and Apache-2.0 flow one-way into
  GPLv3, so nothing would have errored. It is absorption: the shipped binary becomes
  a combined work conveyable only under GPL-3.0-or-later, and every recipient gets
  GPLv3 rights over Excalibur's own code too. It would also have closed the Mac App
  Store permanently (GPLv3 §6/§7, the VLC precedent). The row has been corrected.
- **`pavao` cannot answer E4-S2 at any effort level.** Negotiated dialect, signing
  state, encryption state and cipher name are absent from `pavao`, from `pavao-sys`,
  *and from `libsmbclient.h` itself* — `smbc_getOptionSmbEncryptionLevel` echoes back
  what you set, not what was negotiated. Share writability is equally undetectable:
  Samba's `SMBC_fstatvfs_ctx` never sets `SMBC_VFS_FEATURE_RDONLY`. This project's
  rule that `writable` comes from the server and is never inferred from what the user
  asked for would have had to be broken.
- **`pavao` 0.3.1 serialises every libsmbclient call in the process behind one static
  mutex** — not per client, not per file. For a browser that lists while previewing
  while transferring, that is the defining workload, and `spawn_blocking` does not
  help: calls queue on the mutex while holding blocking-pool threads.
- **`smb` was validated against real Samba 4.13.7 before the decision, not after**: SMB
  3.1.1 negotiated, `Aes128Gcm` readable, listing with the DOS hidden bit, positional
  reads, capacity matching the host, and — the E4-S2 term — `file_write_data: false`
  on a read-only share against `true` on a writable one, with a real
  `STATUS_ACCESS_DENIED` on create. It is MIT, has zero `unsafe` with
  `#![forbid(unsafe_code)]` in five of its eight crates, and needs no C library.
- A known `smb` bug — a `read_block` pre-check that can spuriously deny reads on a
  server returning a restrictive MxAc context, fixed at head but unreleased — was
  tested for against both shares. Samba does not trigger it.

**Blocker: `smb` cannot currently be linked into this binary at all**

Adding `smb = "=0.11.2"` fails to resolve against the SFTP backend already shipped in
E6:

- `russh` 0.63.2 needs `crypto-bigint ^0.7.3` (resolves to 0.7.5).
- `smb` 0.11.2 pins `sspi =0.18.7`, which pins `crypto-bigint =0.7.0-rc.8`.

Both are in the same 0.7.x compatibility range, so cargo must unify them and cannot.
This was investigated rather than assumed:

- `sspi` is not optional in `smb`, and `crypto-bigint` is not optional or
  feature-gated in `sspi` — it is used unconditionally in `sspi::utils`.
- Every `smb` release back to 0.10.4 requires `sspi 0.18.x`, and **every** `sspi`
  0.18.x pins the same release candidate. Choosing an older version does not help.
- `russh` 0.63.2 is the newest release (published the same day), so there is no
  upstream version to move to. It *declares* `crypto-bigint` but never uses it — no
  Rust source in the crate references it.
- Patching that unused declaration out was tried and gets one step further before
  hitting `curve25519-dalek =5.0.0-pre.1`. **`sspi` 0.18.7 carries 27 exact pins on
  pre-release RustCrypto crates**, and russh is built against the stable stack. This
  is systemic, not a one-line fix, and several of those crates russh genuinely uses.

The root cause is upstream: a stable `sspi` release pinned to release-candidate
dependencies. Forking `sspi` onto the stable stack would mean carrying a patched
security-critical authentication crate across exactly the rc→stable API drift where
subtle cryptographic bugs live, which is worse than not having SMB.

**Next steps**

- **E4-S2 … E4-S6 are blocked on this and are not started.** The backend itself is
  understood — the probe exercised nine of the ten operations against a real server —
  so this is a dependency problem, not a design one.
- Three ways out, in order of preference: (1) `sspi` ships a release against stable
  RustCrypto, or `smb` moves off `sspi` — worth filing upstream, since a stable crate
  pinned to release candidates breaks any consumer with a normal crypto stack;
  (2) run the SMB backend out of process with its own dependency graph, which is a
  real option because `ShareBackend` is already the seam, but it is a large story;
  (3) drop `russh`, which is not on the table — E6 ships and works.
- E5 (NFS) was checked for the same problem and `nfs3_client` 0.9.0 resolves cleanly
  alongside `russh`, so M3 continues there.

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

#### E5-S1 — NFSv3 client — **Done**

*2026-09-03.*

**AC**
- [x] `nfs3_client` wrapped behind `ShareBackend`: `MOUNT` → root fh, `LOOKUP`,
  `GETATTR`, `READDIRPLUS`, `READ`, `WRITE`+`COMMIT`, `CREATE`, `MKDIR`, `RENAME`,
  `REMOVE`/`RMDIR`, `FSSTAT`, `FSINFO`, `ACCESS`. *(`SETATTR` is used through
  `CREATE`'s attributes; no caller needs a standalone `set_mtime` yet, and
  `READLINK` waits for a listing that reports link targets — `Attrs.link_target`
  is `None` here, as it is in the SFTP backend.)*
- [x] Tested against Linux `nfsd` in Docker with `rw`/`ro`/`root_squash` variants,
  plus a fourth the AC did not ask for. **Not** tested against macOS `nfsd`, which
  needs root on this machine.

**Results**

- **The crate is a wire codec, not a client.** It maps RFC 1813 one-to-one and
  stops: no DNS, no connection pool, no retry, no path resolution, no attribute
  translation, no `READDIRPLUS` paging. All of that is `backend/nfs.rs`, which is
  why it is the longest backend in the tree despite NFS being the simplest
  protocol of the four.
- **Every call takes `&mut self`, so one connection answers one request.** Not by
  convention — the socket is borrowed mutably across send-then-receive and the
  transaction id is a bare counter with no in-flight map. Concurrency therefore
  cannot come from a connection; it comes from *four* of them, which is what the
  pool is. The pool fills lazily, so a mount that only lists a directory opens one
  socket.
- **NFS has no paths.** Every operation takes a file handle, so `resolve_handle`
  walks from the export root with one `LOOKUP` per component. That is a round trip
  per component and the obvious place for a cache; correctness first, and the
  comment says so.
- **A path that climbs out of the export is refused, not followed.** `..` in a
  path is either a bug or an attempt, and neither should be served.
- **Two bugs the tests found, both mine.** The write probe named its file after
  the process id, so two mounts of one export in the same process picked the same
  name and the `GUARDED` create failed with `EXIST` for whichever lost — reporting
  a perfectly writable export as read-only. And `READDIRPLUS` attributes are
  optional; the first version fell back to `EntryKind::Other`, which turns a
  folder into a file and stops double-click descending into it.
- **The fixture was wrong twice before it was right, and both times it would have
  tested the wrong thing.** Host bind mounts under Docker Desktop go through
  virtiofs, which gives NFS no stable file handles (writes died mid-way with
  `NFS3ERR_STALE`), omits attributes from `READDIRPLUS`, and does not enforce
  ownership — so `root_squash` silently did nothing and that test passed for the
  wrong reason. Moving the exports onto the container's own filesystem then failed
  outright: overlayfs answers `exportfs` with "does not support NFS export". Only
  Docker named volumes, which are real directories on the VM's ext4, can be
  exported at all. All three failing tests passed the moment the storage was real.

**Next steps**

- `READLINK` and `SETATTR`-as-`set_mtime` are unimplemented because nothing calls
  them yet. Both are a few lines when a caller appears.
- A handle cache would remove the per-component `LOOKUP` on every operation. Worth
  doing when a listing of a deep tree feels slow, not before.
- macOS `nfsd` remains untested; it needs root to export anything.

#### E5-S2 — Identity and ports — **Done**

*2026-09-03.*

**AC**
- [x] AUTH_SYS with configurable uid/gid (default: the current user's), shown in
  the dialog as "Identity".
- [x] `insecure`-less exports produce a clear message naming the option — never an
  attempt to bind below 1024.
- [x] TCP only; UDP not offered.

**Results**

- **The crate binds a privileged source port by default**, walking 300..=1023.
  That is what a kernel client does and exactly what this program must not: it is
  not root, the bind fails with `EACCES`, and E5-S2 is explicit that the answer is
  to explain the export option. It is turned off unconditionally, and the refusal
  that follows is translated into a sentence naming `insecure` and stating that
  this program will not bind a privileged port. The fixture's fourth export exists
  to prove that path, and it was verified independently first: mounting it with
  `noresvport` gets "access denied by server while mounting", with a privileged
  port it succeeds.
- **UDP is not reachable even by mistake.** The crate hard-codes `IPPROTO_TCP` in
  its port resolution and has no datagram RPC path at all, so this is a property
  of the dependency rather than a discipline this code has to keep.
- **The dialog's field is called "Identity", not "Username".** AUTH_SYS sends a
  uid the server chooses to believe; calling it a username would promise an
  authentication step that never happens. It takes `uid` or `uid:gid` and defaults
  to the current user's. Anything unparseable falls back to *this user* rather
  than to root — claiming uid 0 is precisely how `root_squash` maps you to
  `nobody`, and the refusal would then have nothing to do with what was typed.
- The identity is kept in the bookmark but left out of the previewed URI:
  `nfs://501@host/export` is not a form anyone writes, but a bookmark that dropped
  the identity would silently reconnect as somebody else.
- **DNS is resolved here.** The crate parses the host as an IP literal, so a name
  produces "invalid IP address syntax" from somewhere unrelated-looking. IPv4 is
  preferred because one of the crate's connector paths binds an IPv4 socket
  unconditionally, and an IPv6 answer would fail later for reasons that read as
  something else.

#### E5-S3 — Writability — **Done**

*2026-09-03.*

**AC**
- [x] `writable` = `ACCESS` grants `MODIFY|EXTEND` ∧ a create-probe succeeds, and
  the probe is skipped with `false` when the user asked for read-only.
  *(`FSINFO`/`PATHCONF` is not consulted: NFSv3 has no read-only bit there. The
  export's own read-only-ness arrives as `NFS3ERR_ROFS` from the probe, which is
  the same fact observed rather than inferred.)*
- [x] `reason` names the failing term, including the squash case.
- [x] `protocol_label` = `NFSv3`; `security_label` = `sys`.

**Results**

- **Three ways to be refused, and they are not interchangeable.** The user asked
  for read-only; the export is exported `ro`; or this identity is squashed to
  `nobody`. The last is the one worth naming — "permission denied" sends a user
  hunting through their own account instead of the export line, so the reason says
  the export maps root to `nobody` and suggests `no_root_squash`.
- **`ACCESS` is consulted first but is not trusted alone.** It is one round trip
  and usually decisive, but it is advisory: a server may grant the bits and still
  refuse the write. Only the create-probe answers the question the Upload button
  will ask, which is the same reasoning the SFTP and xsync backends use.
- The probe is genuinely skipped when read-only was requested. Creating a file on
  a share the user asked to mount read-only would be a surprising thing for a
  mount to do, and a test asserts the reason says "you asked" rather than anything
  about the server.
- `security_label` is `sys` and nothing grander. AUTH_SYS is an assertion the
  server chooses to believe; E5-S5 is where that label becomes `krb5p`.

#### E5-S4 — Range I/O and transfers — **Partly done**

*2026-09-03.*

**AC**
- [x] `rsize`/`wsize` negotiated from `FSINFO` (`rtmax`/`wtmax`).
- [x] Four reads in flight.
- [x] `COMMIT` on close.
- [~] Cap 1 MiB. **Capped at 512 KiB instead**, deliberately: the codec cannot
  reassemble a reply split across RPC record fragments — it errors *and* leaves
  unread bytes on the socket, which poisons the connection for everything after
  it. Servers send single fragments for replies of ordinary size, so staying well
  under the cap is the cheap way not to find out. Raising it needs the upstream
  fix, not a config change.
- [ ] Throughput ≥ 80% of the kernel client on the same pair. **Not measured.**
  The only server available is a container on this machine, where the number would
  say more about the loopback and the VM's virtiofs than about either client.

**Results**

- **Writes are `UNSTABLE` followed by one `COMMIT`**, which is what makes NFS
  writes worth doing — `FILE_SYNC` would make the server wait on its disk for
  every chunk. The price is the write verifier: if it changes between replies the
  server restarted and everything uncommitted is gone. That is checked and
  reported as a failed transfer rather than silently losing the tail.
- **A short write is re-sent, not assumed.** A server may accept less than was
  offered, and treating the offered length as written would leave a hole.
- Reads are split across the pool and reassembled by offset, stopping at the first
  short chunk. A version that concatenated in completion order would pass every
  test whose file fits in one chunk and corrupt every file that does not, so
  there is a test with a file that does not.
- `capacity` reports `abytes`, not `fbytes` — the root reserve is not space this
  user can write into. The same choice as the xsync and SFTP backends.
- `Caps::atomic_rename` is **true**: NFS `RENAME` is POSIX rename and the server
  does it atomically. That is better than the SFTP backend manages without an
  extension. The no-clobber case is the weak one — v3 has no flag for it, so it is
  a separate `LOOKUP` first and therefore racy, which the code says out loud.

#### E5-S5 — NFSv4 and Kerberos (later)
**AC**
- Spike `libnfs` bindings vs. a minimal v4.0 client for `krb5/krb5i/krb5p` via
  `gssapi`; the dialog's Version/Security controls light up when a backend passes the
  E5-S1 matrix; `security_label` becomes `krb5p` etc. Not a v1 blocker; UI is honest
  until then.

### E6 — SFTP backend

#### E6-S1 — Port Kestrel's SFTP backend — **Done**

*2026-09-03.*

**AC**
- [x] `russh` 0.63 + `russh-sftp` 2.4, exact-pinned (`=0.63.2`, `=2.4.0`); auth:
  password, private key (+passphrase), `ssh-agent`, keyboard-interactive; TOFU host
  keys (E3-S4).
- [x] `ShareBackend` methods map 1:1 to SFTPv3 ops; `open_read(offset)` uses `read` at
  offset with ≥ 8 outstanding 256 KiB requests; `open_write(Resume)` opens
  `WRITE|APPEND`-free positional writes.
- [x] Test against a real OpenSSH server. *(A local `sshd` rather than Docker — see
  Results.)*

**Results**

- **The second real backend, and the first one this project speaks itself.** The
  xsync backend spawns `ssh`; this one is an SSH client. That is worth the extra
  code for one reason: the host key arrives *during the handshake that uses it*, so
  the key being checked is the key of the connection being made. The xsync path
  still scans with `ssh-keyscan` and then connects, which has a window between the
  two. Both reach the same verdict through the same function — `hostkeys::check()`,
  extracted from `verify()` in this story precisely so that they cannot drift.
- **Driving `RawSftpSession` instead of `SftpSession` was the decision the story
  turned on.** The friendly API wraps each open file in an `AsyncRead`, which forces
  a seek-then-read pair per range and serialises every read behind one cursor — the
  first version of this backend carried a `Mutex` to stop two ranged reads landing
  at each other's offsets. The raw session puts the offset in the request, the way
  SFTP actually works. That removed the mutex *and* made three things reachable that
  the first version had written off as impossible:
  - **Pipelined reads.** SFTP is request-response over one channel, so a sequential
    reader spends a round trip per chunk and tops out at `chunk / RTT` no matter how
    fast the link is. Eight 256 KiB requests are now in flight at once, refilled as
    each lands.
  - **`posix-rename@openssh.com`** (E6-S3), via `extended()`.
  - **The server's extension and limits map**, so chunk sizes come from
    `limits@openssh.com` rather than from a guess that a server may reject
    mid-transfer.
- **An earlier note in this backlog said the extension map and extended requests
  were not exposed by `russh-sftp` 2.4 and that `posix-rename` was therefore
  unreachable. That was wrong** — it was a reading of `SftpSession` alone, whose
  `RawSftpSession` field is private. The raw type is public and exports all of it.
- **`readdir` had to be looped.** A server answers with as many names as fit in one
  packet and expects to be asked again; the first version stopped after the first
  reply, which silently truncates any directory bigger than that — it looks like
  missing files, not like a bug. There is now a 500-entry test for it.
- **Keyboard-interactive is a real fallback, not a comment.** An earlier version
  claimed the password credential "also answers keyboard-interactive"; it did not —
  it called `authenticate_password` and nothing else. A server with
  `PasswordAuthentication no` and `KbdInteractiveAuthentication yes` accepts only
  the second, and that is a common default. Every prompt is answered with the one
  password held, which is what `ssh` does when not attached to a terminal — this
  backend cannot ask a person, because E10's rule is that the password reached Rust
  once and never travels back out.
- **Tested against a real OpenSSH server, not Docker and not a fixture.** The AC
  asked for an in-process russh fixture *and* OpenSSH in Docker; the tests spawn the
  machine's own `sshd` on a kernel-assigned port with a generated host key and a
  generated client key, and skip when `sshd`/`sftp-server` are absent. That is the
  same software Docker would have supplied, without requiring a daemon; an
  in-process fixture would have tested the fixture's idea of SFTP rather than
  OpenSSH's. Ten tests: listing, ranged reads, a 2.5 MiB pipelined read checked
  byte-for-byte, chunked writes, resume-at-offset, the mutations, a paged listing,
  the read-only refusal, an untrusted host key, and a rejected password.
- **The port bug worth remembering.** The fixture first derived its port from the
  process id, so every test in the run got the same number — each started an `sshd`
  with its own host key on one port and connected to another test's. It surfaced as
  `HostKeyChanged`, which is exactly what should happen when a key changes under
  you; the trust code was right and the test was wrong.

**Next steps**

- `security_label` is still `"ssh"` rather than the negotiated KEX/cipher — see
  E6-S2, which is where that AC bullet lives and where it remains open.
- Password and keyboard-interactive acceptance are untested end to end: proving
  them needs a real account password, which this fixture cannot supply. The
  *rejection* path is tested, because that is where a loop waiting for prompts that
  never arrive would hang a mount silently.

#### E6-S2 — Writability and capacity — **Done (bar the cipher name)**

*2026-09-03.*

**AC**
- [x] `capacity` via `statvfs@openssh.com`; `writable` = `f_flag` lacks `ST_RDONLY` ∧
  create-probe succeeds (skipped if RO requested); `reason` e.g. "filesystem is
  read-only" or "permission denied on `/srv`".
- [x] `protocol_label` = `SFTP`.
- [ ] `security_label` = KEX/cipher from russh (`aes256-gcm`). **Open**: russh 0.63
  does not expose the negotiated algorithms on `client::Handle`. The label reads
  `ssh`, which is the honest amount to claim — the same thing the xsync-over-SSH
  backend says, and for the same reason. Needs an upstream accessor.

**Results**

- **Three separate terms, and the reason string names whichever one failed.** A
  read-only mount can be the user's own request, the filesystem's `ST_RDONLY`, or
  this user's permissions on this directory — and only the last is answerable by
  actually trying. The create-probe is what answers it, because it asks the question
  the Upload button will ask later. A disabled control with no explanation is the
  thing E9 exists to prevent, so the empty-reason case is asserted in a test.
- **The probe is skipped when read-only was requested**, rather than run and
  discarded: creating a file on a share the user asked to mount read-only would be a
  surprising thing for a mount to do.
- `capacity` reports `blocks_avail`, not `blocks_free` — consistent with the xsync
  backend, and for the same reason: the root-only reserve is not space this session
  can write into, and showing it would overstate what a user can do.

#### E6-S3 — Mutations — **Done**

*2026-09-03.*

**AC**
- [x] `rename(replace)` uses `posix-rename@openssh.com` when advertised; otherwise a
  fallback, with `Caps.atomic_rename = false` so the transfer worker uses the safe
  path.

**Results**

- **`posix-rename@openssh.com` is real**, sent through `extended()` with the same
  two-string payload shape as `hardlink@openssh.com`. Against OpenSSH it is one
  request and the server does it with `rename(2)`, so the destination is never
  absent — `Caps.atomic_rename` is true there, and the mutations test asserts it is,
  because the fallback would otherwise pass the same assertions and hide a
  regression.
- **The fallback is not remove-then-rename.** The AC's wording describes what the
  sibling Kestrel engine does, and it has a window in which the destination does not
  exist at all: a crash or a dropped connection there destroys the file the user was
  replacing. This stands the old file aside under a `.excalibur-replacing` name
  instead, renames onto the now-free destination, and puts the old one back if that
  fails. The destination is absent only for the instant between two renames. The
  test asserts the stash is not left behind on the success path.
- Plain SFTPv3 `rename` already refuses an existing destination, so the
  `replace: false` case needed nothing extra — it is the protocol's own behaviour,
  and the test pins it so a later "improvement" cannot quietly make it clobber.
- Recursive removal walks iteratively rather than recursing: an `async fn` that
  calls itself needs boxing, and a deep tree would then be bounded by the stack
  rather than by the share.

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
- [x] Staged resumable uploads and CAS on write-back. *(Landed 2026-09-03: `xsyncv3.md`
  E4-S6/E4-S7 added `StageOpen/Write/Status/Commit/Abort` and `WriteCas`. A stage
  survives a reconnect and a server restart, so E13's resumable uploads and E12's
  edit-in-place have what they need. The Excalibur adapter does not use them yet — that
  is E12/E13's own work.)*
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
- **Three verbs were refused as `Unsupported` rather than emulated**: `mkdir`, `rename`
  and `remove` had no v3 message. A UI read that and disabled the verb, which is better
  than offering it and reporting a confusing failure. *(Resolved 2026-09-03: `xsyncv3.md`
  E5-S4 landed, all three are real, and `Caps::atomic_rename` is now true. Recursive
  removal walks client-side so it can report progress and stop.)*
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

- **Extract `xsync_client::Fs`** once M2's verbs have settled. This adapter is the
  evidence E9-S1 was waiting for, but `xsyncv3.md` §5 says to expect the method list to
  change after M1 — and it did: E5-S4 added five methods. Extracting before E4-S6/S7
  would only mean revising it again.
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

#### E8-S1 — Listing state and loading *(handoff §Loading)* — **Done**

*2026-09-03.*

**AC**
- [x] `listing: { entries, loading, error }`; navigation cancels the previous request;
  listings < 200 ms show no indicator; past 200 ms a 2px indeterminate accent bar
  under the column header appears and the previous listing stays visible at
  `opacity: .5`.

**Results**

- **"Cancels" is the wrong word for what actually happens, so the code does not use
  it.** There is no `AbortController` reaching the engine — a Tauri command runs to
  completion whatever the window does — so what this really does is *ignore* a reply for
  a path the user has already left. A generation counter enforces it. Naming it
  accurately in `src/state/listing.ts` matters because someone will eventually need to
  know the request is still running on the other side.
- Tested with an out-of-order pair: a slow request for A, then a fast one for B, with A
  answering last. B stays on screen. The same test exists for a *failed* stale request,
  because an error from a superseded navigation painting over a good listing is the
  version of this bug that would actually get shipped.
- **A failed listing keeps the last good one underneath it.** A failed navigation that
  also wiped the view would leave nothing to go back from.
- The 2px track is always in the layout and only the bar inside it appears, so a listing
  crossing the threshold does not shift every row down by two pixels as it arrives.
- Verified in the browser at `?screen=slow`: at 100 ms there is no bar and full opacity;
  at 400 ms the bar is up, the rows are at `0.5`, and all 13 of them are still readable.
- **The breadcrumb updates immediately on navigation, ahead of the listing.** That is
  deliberate: the dimming is what says "these contents are stale", and a path that
  lagged would make Back and Forward feel unresponsive. It does mean the status bar
  briefly shows the new path beside the old item count.

**Next steps**

- `error` is held but not yet rendered — E15-S1's banner is where a connection-level
  failure belongs, and inventing a second place for it now would just have to be undone.
- Once E8-S9's watch arrives, a refresh triggered by an event should *not* raise the
  indicator; only a navigation should. The bar is driven by `slow`, so that will be a
  flag on `load`, not a new mechanism.

#### E8-S2 — Column layout `1f` *(handoff §Screens/1 Column header, Rows; §6 `1f`)* — **Done**

*2026-09-03.*

**AC**
- [x] Header grid `1fr 96px 156px 128px`, gap 16px, padding `0 12px`, bg `#f4f4f5`,
  border-bottom `#dcdcde`, 10.5px/600 `#6a6a70`, sort indicator 8px `#2f6df6` ▲/▼.
  *(The static metrics landed with M0; this story made the indicator follow the
  actual sort and the widths adjustable.)*
- [x] Rows 24px, border-bottom `#f2f2f3`; name cell flex gap 8px, glyph `flex: none`,
  ellipsis truncation; Size mono 11px right `#6a6a70` (`—` `#8a8a90` for folders);
  Modified `31 Aug 2026, 18:24`; Kind human-readable.
- [x] Selected row: bg + border `#2f6df6`, white text, glyph stroke `rgba(255,255,255,.7)`.
- [x] Dotfiles `opacity: .55`, Kind "Hidden", shown only when `showHidden`.
- [x] Virtualised: 100k rows scroll at 60fps (measured with the mock backend).
- [x] Columns click-to-sort and drag-resizable; widths persisted per layout.

**Results**

- **100k rows, 26 in the DOM.** Every row is the same height, so which rows to draw is
  arithmetic rather than measurement — no observers, no per-row bookkeeping. A single
  spacer of the full height carries the scrollbar and the drawn slice is translated into
  it.
- **Measured, not asserted.** 400 scroll steps across 15,000 rows at ~37 rows per step:
  median **1.8 ms** per step, p95 9.8 ms, worst 13.8 ms, and **zero steps over the
  16.7 ms budget**. Honest caveat: this measures the per-step work (scroll → Solid
  re-render → forced layout) synchronously, not compositor frames — `requestAnimationFrame`
  does not run while the preview pane is hidden. It is the cost that decides whether
  60fps is reachable, and it has 9× headroom, but it is not the same as a frame trace.
  `?screen=huge` renders the 100k listing for anyone who wants to check with devtools.
- **A bug worth recording, because the first version looked right.** Dividers were
  positioned by summing column widths in from the right edge. That quietly ignored the
  three 16px gaps and the header's 12px padding, putting every divider ~50px into the
  *next* column, drawn across its label. Now each divider is a child of the cell it
  follows, sitting on that cell's own right edge — it cannot drift, because it is
  anchored to the boundary rather than calculating where the boundary should be.
  Verified numerically: the three dividers centre on 422/574/706, exactly the midpoints
  of the gaps.
- **A first click on Size or Modified sorts descending.** Ascending would mean smallest
  and oldest first, which is almost never what the click meant, and would make every
  user click twice. Name and Kind start ascending.
- **Folders stay first in both directions.** "Folders first" is a preference about
  grouping, not a claim that folders sort before files, so reversing must not sink them
  to the bottom. There is a test for the reversed case specifically.
- **Ties break by name, not by the order the backend returned.** The AC says *stable*,
  and this is stable in the sense that matters — the same listing sorts the same way
  every time — but it deliberately does not preserve backend order on ties, because that
  order is arbitrary and can differ between two listings of an unchanged directory.
  Sorting by Size with every folder at zero would otherwise reshuffle the folders on
  each refresh.
- A drag moves width between exactly the two columns its divider sits between, clamped
  at 48px, and a clamped column cannot silently take width from its neighbour. Verified
  in the browser: dragging the last divider 30px moved Modified 116→86 and Kind 128→158,
  leaving Name and Size untouched.
- Widths persist on **pointer-up, not per move** — a file write per frame otherwise —
  and survive a reload. `src/state/prefs.ts` is a small keyed store on the same
  Tauri-or-browser split the bookmarks use; **E17-S1** is where it grows a real schema.
  The key is `columnWidths.columns`, shaped for `1g`/`1h` so those need no migration.
- Every reader validates what it loads. A persisted file is user-editable and outlives
  upgrades, so `sanitise` falls back **per column** rather than discarding the record.

**Next steps**

- The sort *model* here covers **E8-S6**'s first bullet; the filter half followed
  immediately after. **Done**, below.
- `foldersFirst` is a signal with no UI, default on. **E17** gives it a control.
- `ROW_HEIGHT` is a constant in `Shell.tsx` that must match `--height-row`. Two places
  to keep in step; reading it from the token at mount would remove that, and is worth
  doing when `1g`/`1h` (**E8-S3/S4**) bring their own row heights.
- Keyboard access to the header (tab to a column, Enter to sort) is not wired —
  **E16-S2**'s accessibility pass.

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

#### E8-S6 — Sort and filter — **Done**

*2026-09-03. The sort half landed with E8-S2, which needed it for click-to-sort.*

**AC**
- [x] `sort: { column, ascending }`; folders-first is a preference (default on);
  natural sort for names; stable. *(See E8-S2's results: first click on Size/Modified is
  descending, folders lead in both directions, ties break by name.)*
- [x] Filter is client-side substring (case-insensitive, diacritic-insensitive) over the
  current listing; updates per keystroke without re-listing; counts in the status bar
  reflect the filtered set.

**Results**

- **Diacritic-insensitivity is the half that is easy to get wrong, and NFD alone does not
  do it.** Decomposing splits `é` into `e` plus a combining mark, which strips cleanly —
  but it does nothing for `ø`, `ð`, `ß` or `æ`, which are separate letters rather than a
  base plus a mark. Searching `soren` would miss `Søren`. A short hand-written fold
  covers those; it is not a transliteration table and does not pretend to be.
- It works in both directions: typing `amelie` finds `Amélie.mkv`, and typing `Amélie`
  finds `Amelie.mkv`.
- **A filtered count says it is filtered: `3 of 13 items`.** A bare `3 items` does
  reflect the filtered set, which is all the AC asks — but it reads exactly like a
  directory that contains three files, which is the one thing a user must not conclude
  while a filter is on. The total is counted under the same `showHidden` rule as the
  visible set, so the two numbers describe one population.
- **A bug the browser found: the status bar said "1 selected · 0 B".** The selected count
  came from the raw selection set while the byte total came from the filtered entries, so
  filtering the selected row away left them disagreeing. Both now derive from
  `selectedVisible` — the selection as it is *on screen*. A row hidden by a filter is not
  something the status bar describes or a toolbar verb acts on: "1 selected" beside a
  list with no selected row in it is a claim the user cannot check, and Download would
  act on a file they cannot see. The underlying set is untouched, so clearing the filter
  brings the selection back — verified in the browser, including Quick Look correctly
  disabling and re-enabling.

**Next steps**

- Cmd/Ctrl+F should focus the filter (**E16-S1**); not wired. Escape's ordering landed
  with **E8-S8**.
- `foldersFirst` is a signal with no control, default on. **E17** gives it one.
- The filter matches names only. Matching the Kind column too would be cheap now that
  every entry carries one, but it is not in the AC and would surprise anyone typing an
  extension expecting a name match.

#### E8-S7 — Navigation and history *(handoff §Navigation)* — **Done**

*2026-09-03.*

**AC**
- [x] Double-click folder descends; `↑` ascends; breadcrumb click jumps; back/forward
  keep a per-window `{stack, index}`; sidebar double-click mounts then opens the root.
- [x] Cmd/Ctrl+↓ descends into the selected folder, Cmd/Ctrl+↑ ascends.

**Results**

- **The listing follows the path, not the other way round.** Every way of moving — a
  double-click, a breadcrumb, Back, the keyboard, a fresh mount — sets one signal, and
  an effect loads it. Wiring each of those to its own fetch is how the breadcrumb and
  the contents end up disagreeing.
- The history is one stack and an index, not two stacks. Back and forward become moves
  along a list, and the rule that matters — going somewhere new discards what was ahead
  — is a truncation rather than a push and a clear that have to be kept in step. There
  is a test for exactly that sequence.
- **Going where you already are is not recorded.** Clicking the last breadcrumb, or
  re-opening the folder you are in, would otherwise stack duplicates and make Back look
  broken.
- Paths are share-relative with `""` for the root and no leading slash, matching
  `ShareBackend::list_dir` so nothing is translated on the way to the engine. `""` and
  `"/"` as two spellings of the root would have made every comparison normalise first.
  The root is its own parent, so ascending cannot walk out of the share.
- **A navigation clears the selection.** Carrying it across would leave rows selected
  that are not on screen, and would let a toolbar verb act on a file the user can no
  longer see. Verified in the browser: select a file, descend, selection is empty.
- Cmd/Ctrl+↓ acts only on a *single* selected folder. Descending "into" three folders
  has no meaning and picking one would be a guess. A bare arrow key does not navigate —
  that belongs to E8-S8's selection movement — and there is a check for it.
- The last breadcrumb renders as text, not a button: it is where we already are, and a
  button there offers a click that does nothing.
- Mounting from a sidebar row sends its failure to the bookmark's editor, pre-filled,
  rather than inline. The row is the one place a share is connected without the dialog,
  so there is no inline surface for the error, and the editor is where it can be fixed.
- The fixture backend now synthesises subdirectories. The fixture is one directory deep
  because it comes from one `xcal ls`, so without this there was no way to exercise
  descending, breadcrumbs or Back in a browser at all. It is marked in the code as a
  preview affordance, not a claim about the engine.

**Next steps**

- The share root's own name in the breadcrumb (`media`, `scratch`) is still fixed per
  preview screen. It should come from the active mount, which is **E10**'s job.
- Cmd/Ctrl+[ and ] for back/forward are in **E16-S1**'s shortcut map, which builds the
  bindings and the tooltips from one table — worth doing there rather than adding two
  more ad-hoc key handlers here.
- Mounting a bookmark sets `mounted` on it and clears it on the others, which assumes
  one active mount. The session manager already holds several; reconciling the sidebar
  with `SessionEvent` is part of **E10**'s live status.

#### E8-S8 — Selection model *(handoff §Selection)* — **Done**

*2026-09-03.*

**AC**
- [x] Click selects; shift-click extends; Cmd/Ctrl-click toggles; click-drag rubber-bands
  (rectangle intersects rows); type-to-select jumps to the first match with a 700 ms
  typing buffer; Escape clears filter first, then selection.
- [x] `selection: Set<string>` of paths; verbs and status counts derive from it.

**Results**

- **The selection carries an anchor, and that is the whole story.** Shift-click extends
  from the anchor, and the anchor does *not* follow the far end — so two shift-clicks
  from one starting row grow and shrink the same range instead of walking away down the
  list. That is invisible in a screenshot and obvious in a test, which is why the model
  is a pure module.
- Shift-click *replaces* the range rather than unioning it, because a shift-click means
  "the range is this" and shrinking it by clicking closer to the anchor has to work.
- Paths, not indices. The list re-sorts and re-filters underneath a selection, and an
  index would then point at a different file.
- **The rubber band computes covered rows arithmetically, not by hit-testing.**
  Hit-testing would only ever find the virtualised window, so a band dragged past the
  viewport would stop selecting at the edge of what happens to be in the DOM. The band
  takes content coordinates, so scrolling mid-drag keeps working.
- **It deliberately does not start on the name cell.** That is the part of a row a user
  grabs to drag a file out, which is E13-S5's; taking it now would mean giving it back
  later. Everything else bands — the rest of every row, and the empty space below the
  last one. Verified both ways in the browser.
- A band needs 4px of movement before it starts, so a click with a shaky hand stays a
  click.
- **Type-to-select needed two behaviours, and the first version only had one.** Typing
  `bl` landed on `Blade.Runner.2049.srt` rather than `.mkv`: extending the buffer was
  searching on from the row `b` had just matched, stepping past it. Adding a letter must
  *refine* — it searches from where the burst began — while repeating a letter must
  *cycle*, searching on from the current row. `s`,`s`,`s` walks through the s-names;
  `b`,`l` stays on the first `bl`. Both wrap.
- A buffer of one repeated character matches on a single instance of it, because
  `sss` is what "press s three times" produces and no file is named that.
- Matching reuses the filter's `fold`, so type-to-select is case- and
  diacritic-insensitive on exactly the same terms as the filter. One rule, one
  implementation.
- **Escape clears the filter first, then the selection**, and does *not* do both at once
  — one Escape, one effect. It is also the one key allowed to act while focus is in a
  text field, because that is precisely where the user will be when they want the filter
  gone.
- Type-to-select scrolls its match into view by setting the scroll position rather than
  calling `scrollIntoView`, because the row it jumped to may not be in the DOM.

**Next steps**

- **Arrow keys do not move the selection yet.** They are unbound, and the natural home
  for them is **E16-S1**'s shortcut map, which generates bindings and tooltips from one
  table rather than accumulating ad-hoc handlers here.
- The band selects contiguous rows only, which is what a rectangle over a single-column
  list means. `1h`'s grid layout (**E8-S4**) will need a real 2D rectangle.
- Cmd/Ctrl held while banding adds to the existing selection; shift while banding is not
  handled and currently behaves like a plain band.

#### E8-S9 — Live refresh
**AC**
- Cmd/Ctrl+R refreshes; when the backend offers `watch` (xsync v3) the listing
  refreshes on events, coalesced to at most one relist per 250 ms; an overflow event
  triggers a full relist.

### E9 — Permission gating

#### E9-S1 — One derived value *(handoff §Permissions gating)* — **Done**

*2026-09-03.*

**AC**
- [x] `mount.writable` (from the latest `list_dir`/`mount_info`) is the *only* input to:
  status badge, disabled state of Edit/Upload/rename/delete/new-folder, drop-target
  acceptance, Quick Look footer copy, and the `Remount as RW` recovery button.
- [x] A property-based test enumerates every write affordance and asserts it is disabled
  when `writable = false`.
- [x] No write command is ever *sent* to the engine in that state. *(Was vacuously true
  with no write methods to guard; closed by **E14-S1**, which added them and the
  `writeGuarded` seam that refuses before invoking.)*

**Results**

- **The affordances are enumerated, not implied.** The failure this AC exists to prevent
  arrives by accretion: someone adds a verb and writes `disabled={…}` from whatever
  seemed relevant at the time. So `src/state/permissions.ts` lists every affordance and
  sorts each into read, write or recovery, and **a test asserts every affordance is in
  exactly one group**. A verb added to the type but to no group fails that test, so it
  cannot slip through ungated by being forgotten. That property is what makes the rest of
  the suite worth anything.
- The property test iterates `WRITE_AFFORDANCES` rather than spot-checking. Adding
  `rename` to the list adds three assertions about it for free.
- **No component reads `writable` to gate anything any more.** The toolbar's four verbs
  are one `Verb` component driven by `gate()`. The status *badge* still reads the value
  directly, which is the rule being followed rather than broken: it displays the value,
  it does not gate on it.
- **The tooltip is the server's reason, verbatim.** "Read-only — export mapped
  `ro,root_squash`" tells a user whether it is their export or their credentials;
  "not allowed" does not. Verified on screen in `?screen=1b`.
- A read-only mount's reason **wins over "nothing selected"** when both apply. It is the
  one the user cannot fix by clicking a row, so it is the more useful thing to say.
- `dropTarget` is a write affordance, not a special case. A list that highlights on
  drag-over and then refuses the drop is worse than one that never highlighted. E13-S5
  reads `acceptsDrop` rather than inventing its own condition.
- `remountRw` is gated on the same value **inverted**: offering it on a mount that is
  already read-write invites a reconnect for no reason.

**Next steps**

- The "never sent" half became testable with **E14-S1**'s write verbs, and is **done**:
  `writeGuarded` on the backend seam refuses before invoking, with tests asserting that
  nothing was sent. The AC bullet above is now met.
- `rename`, `delete` and `newFolder` are in the enumeration and gated, but have no
  buttons yet. They are listed *now* precisely so E14 cannot add them ungated.

#### E9-S2 — Disabled styling *(handoff §Screens/2)* — **Done**

*2026-09-03.*

**AC**
- [x] Dark: bg `#2e2f32`, border `#3a3b3f`, text `#63646a`, `cursor: not-allowed`; light
  equivalent derived from tokens; tooltip explains the reason string.

**Results**

- The three dark values and `cursor: not-allowed` landed with M0's token file. **Read
  back from the rendered page rather than from the stylesheet** — computed
  `background-color`, `border-color`, `color` and `cursor` on the disabled Upload button
  in `?screen=1b` are `#2e2f32`, `#3a3b3f`, `#63646a`, `not-allowed`. Comparing the CSS
  source would only prove the CSS says what it says.
- The tooltip was the actual gap, and it closed as a side effect of E9-S1: every gated
  verb now gets `title` from the same `gate()` that disabled it, so a disabled control
  cannot exist without an explanation attached.
- The light equivalents are derived tokens (`--disabled-bg` and friends), so the two
  themes cannot drift apart by someone editing one.

**Next steps**

- The `title` attribute is a native tooltip: it is slow to appear and invisible to touch.
  **E16-S2**'s accessibility pass should give it `aria-describedby` as well, so a screen
  reader gets the reason without hovering.

#### E9-S3 — Remount as RW — **Done (surfaced in the status bar, not the queue)**

*2026-09-03.*

**AC**
- [x] Re-runs `connect` with `mountPreference = 'rw'`; on success re-reads `writable`,
  and if still read-only, the error text updates to the server's reason rather than
  looping.
- [ ] …*in the queue row*. **Not there:** the transfer queue is E13. The button is in the
  status bar instead — see below.

**Results**

- **A refused upgrade returns `Ok`, not `Err`, and that is the whole design.** A share
  that comes back read-only again is an *answer*, not a failure. Returning `Err` is
  precisely how a recovery button becomes a loop: the caller sees a failure, offers the
  same button, and the user presses it forever. `remount_read_write` resolves with the
  mount facts that came back, and the caller reads `writable` off them. There is a test
  asserting the refusal is `Ok`, carries the server's reason, and costs **exactly one**
  further connection.
- **The upgraded preference is stored, so a reconnect keeps it.** Without that, a network
  blip silently reverts the mount to whatever was first typed in the dialog and undoes
  the user's recovery. The test knocks the share over, lets it reconnect, and asserts the
  connector was still asked for `Rw`.
- The window **re-lists** after a successful remount rather than patching the mount facts
  by hand. `writable` travels with the listing, and taking it from anywhere else is how
  the badge and the rows come to disagree.
- **The button is in the status bar, which is beyond the mock**, and that is recorded in
  the new `docs/design-gaps.md` rather than done quietly. The handoff's only `Remount as
  RW` is on a failed transfer row (§Screens/4), so until E13 exists a read-only mount has
  no recovery path anywhere: the status bar states the problem and offers nothing. The
  button shows only when `writable` is false, so a healthy window is unchanged from the
  mock, and removing it later is deleting one element.
- It is **absent, not disabled**, when there is no session to remount — the preview
  screens and the demo share. A button that cannot possibly work should not be drawn.

**Next steps**

- **E13-S2** puts the same action on a failed transfer row, which is where the design
  actually wants it. The engine call is shared; only the surface differs.
- A connection that fails outright currently shows a toast. **E15-S1**'s banner is its
  real home, and that is the second entry in `docs/design-gaps.md`.
- Both design-gap entries want a screenshot attached before sign-off.

### E10 — Status bar *(handoff §Screens/1 Status bar)*

#### E10-S1 — Groups and dividers — **Done**

*2026-09-03.*

**AC**
- [x] 24px, bg `#f7f7f8`, 10px gaps, 1×12px `#dcdcde` dividers. Groups: permission badge
  (mono 9.5px/500 `.04em`, `RW` white-on-`#2f6df6`, `RO` `#e8e8ea`-on-`#4a4b50` in
  dark), plain-language restatement with mono 10.5px path, counts
  `13 items · 1 selected · 48.2 GB` (selected size), capacity `2.4 TB free of 16 TB`,
  right-aligned mono 10px protocol/cipher + 6px health dot (`#2f9e5f` healthy,
  `#c0392b` failed).
- [x] Restatement text: RW → "Read & write on `<path>`"; RO → `reason` from `MountInfo`.
- [x] Capacity refreshes on mount and on reconnect.
- [ ] …and after every transfer completes. **Not possible yet:** transfers are E13. The
  refresh hangs off the mount event stream, so E13 wires the same call rather than a new
  mechanism.

**Results**

- **The window was never listening.** The Tauri side has emitted `mount://changed` since
  E2-S2 and nothing subscribed to it, so the health dot could not turn red, capacity
  never refreshed, and a share that reconnected *read-only* left an `RW` badge on screen.
  The whole point of publishing those events was the case where the mount changes
  underneath the user, and that path had never been connected.
- **Status comes from the event stream; `writable` and capacity come from the listing.**
  A mount that has dropped has no listing to carry its state, which is exactly when the
  dot must go red — so the status is held separately. But on a `ready` event the window
  **re-lists** rather than taking the facts off the event, because `writable` travels
  with the listing and sourcing it from two places is how the badge and the rows come to
  disagree.
- **A CSS-module class collision, and one I introduced myself.** E8-S2 added a
  `.divider` for the draggable column boundary to a file that already had a `.divider`
  for the status bar's 1×12px separator. Same module, same name: the later rule won, and
  the status bar's dividers silently became 9px and `position: absolute` — gone from the
  layout entirely. **Nothing failed. No test caught it, and I had looked at several
  screenshots of that bar since.** It surfaced only because this story read the computed
  width back off the page and compared it to the AC's `1px`. The column one is now
  `.columnDivider`.
- Every literal in the AC is verified from **computed style on the rendered page**, not
  from the stylesheet: bar 24px/`#f7f7f8`/10px gap; badge mono 9.5px/500/`0.38px`
  tracking, `RW` white-on-`#2f6df6`, `RO` `#e8e8ea`-on-`#4a4b50` in dark; dividers
  1×12px `#dcdcde`; path mono 10.5px; protocol mono 10px; dot 6×6, `#2f9e5f` healthy and
  `#c0392b` failed.
- **The chrome now names the mount it is showing.** Host, share, scheme and the
  breadcrumb root came from the `readOnly` preview flag — the leftover E8-S7 flagged.
  They come from the active bookmark now, falling back to the design's two demo screens
  only when nothing is mounted, which is what both the preview and a first run are.

**Next steps**

- **E13** adds the post-transfer capacity refresh, on the same event path.
- A `reconnecting` mount currently only turns the dot red. **E15-S1**'s banner is the
  rest of that state — the message, the dimmed list and the `Reconnect` button.
- The status bar describes the *active* mount only. With several mounted, switching the
  sidebar selection does not yet switch what the window is looking at; that is the other
  half of **E8-S7**'s sidebar work.

### E11 — Quick Look *(handoff §Screens/5)*

#### E11-S1 — Panel — **Done (a floating panel, not a separate window)**

*2026-09-03.*

**AC**
- [x] Always-dark 440px panel; header 30px `#2a2b2e` with filename 11.5px/500, mono 10px
  `#9a9ba0`, `Open in editor`, `✕`; body `#1e1f21` padding 14px; footer 24px with RW/RO
  chip and copy per handoff; right-aligned "Space to close".
- [ ] …as a separate *window*. **Built as a floating panel inside the main window**,
  which is what the handoff itself says ("440px floating panel"). `plan.md` read that as
  a separate OS window; the handoff's own wording does not require one, and a second
  Tauri window costs its own entry point and IPC for no behaviour the mock describes.
  Recorded here rather than done quietly.
- [x] Space toggles; Escape closes; the preview follows the selection.
- [ ] …*arrow keys* move the selection. Arrow keys are still unbound — deferred to
  **E16-S1**'s shortcut map since E8-S8. The preview follows the selection however it
  changes, so this lights up for free when they land.

**Results**

- **The panel is always dark, and its colours are literal rather than tokens.** It
  deliberately does not follow the app's theme — a photo shown against white half the
  time is the thing the handoff is avoiding — so reading theme tokens here would be
  wrong rather than merely unnecessary.
- **It closes itself when the selection stops being exactly one row.** A preview of
  "three files" has no meaning, and leaving the last one on screen would misdescribe
  what is selected.

#### E11-S2 — Text — **Done (no lazy continuation yet)**

*2026-09-03.*

**AC**
- [x] Mono 11px, `line-height: 1.8`, `#c4c4c8`, non-selectable 1-based line numbers
  `#63646a`; reads the first 1 MiB; encoding sniff (UTF-8, UTF-16 BOM, Latin-1
  fallback); binary detection → "unpreviewable" card.
- [ ] …and lazily continues on scroll. **Not built:** a preview is a preview, and the
  first megabyte is already more than the panel shows. The `Range` header that windows
  it is the same one a continuation would use, so this is an addition rather than a
  redesign.

**Results**

- **Encoding is sniffed before decoding, and binary is checked before either.** Decoding
  a binary as Latin-1 always "succeeds" and produces a screen of noise, so the NUL scan
  has to come first. UTF-16 is detected by its BOM because a UTF-16 file decoded as UTF-8
  is not slightly wrong, it is unreadable.
- Latin-1 is the fallback rather than an error, because it never fails and is right often
  enough for a preview.
- Line numbers are `user-select: none`, so copying the preview copies the file's text and
  not a column of numbers interleaved with it.

#### E11-S3 — Images, video, audio — **Done (throughput unmeasured)**

*2026-09-03.*

**AC**
- [x] Images render centred and contained; video/audio use the platform
  `<video>`/`<audio>` fed by a Tauri custom protocol (`excalibur-stream://`) that honours
  HTTP `Range` by calling `read_range`.
- [ ] Seeking a 40 GB MKV starts playback within 2 s on LAN. **Not measured:** it needs a
  40 GB file and a second machine. The mechanism is in place and the range arithmetic is
  tested; the number is not claimed.
- [x] Disabled when `Caps.random_access = false`.

**Results**

- **This is the piece the architecture demanded, and it is why E11 started here.** The
  engine's rule is that *file bytes never cross IPC*: a command's return value is
  serialised through the webview bridge, so previewing a 40 GB film through one would
  mean holding it in memory. The custom protocol streams a range straight from the
  backend to the element that asked for it. Every viewer — text included — reads through
  it; none of them uses a command.
- **`bytes=0-` is capped rather than answered literally.** A player asking for everything
  is how a preview becomes a download; it asks again for the rest. A range *past* the end
  answers `416` rather than being clamped, because clamping would serve bytes the caller
  did not ask for and a player seeking past the end would play them. Both are tested.
- A request with no `Range` gets one megabyte, not the file: that first request is a
  `<video>` discovering the container, and answering it in full defeats the point.
- The `Content-Range` describes what is *in the body*, not what was asked for, so a short
  read at the end of a file is reported honestly.
- The media type comes from the engine's kind table, not from sniffing: sniffing would
  mean reading the file to answer a question the name answers well enough.
- `random_access` gates the media viewers only. A backend that cannot seek can still show
  text and an image, which are read from the start — a scrub bar that cannot scrub is
  worse than no viewer.

#### E11-S4 — Unpreviewable — **Done (Download awaits E13)**

*2026-09-03.*

**AC**
- [x] Shows kind, size and mtime.
- [ ] …and a `Download` button that enqueues a transfer. **The button is there and
  disabled**, with a tooltip saying transfers arrive with E13. Hidden would make the card
  look like a dead end; enabled would be a lie.

### E12 — Edit in place *(handoff §Screens/5, "Edit")*

#### E12-S1 — Round trip — **Done (with three named gaps)**

*2026-09-03.*

**AC**
- [x] `Edit` (toolbar or `Open in editor`) downloads to the platform cache dir, records
  the remote `Attrs`/cookie, opens the OS default editor (`open`/`xdg-open`/`explorer`),
  and watches the file with `notify`.
- [x] Each save writes back through a compare-and-swap; a **toast** records each one.
- [ ] …and a **queue row**. The handoff says "a toast or a queue entry" and this does the
  toast. A write-back is one small `write_all_if_unchanged`, not a streamed transfer, so
  putting it in the queue would mean a row that is finished before it is drawn.
- [x] Remote changed underneath → conflict prompt; **never a silent overwrite**.
- [ ] …with `Keep both` as a third option. The prompt offers Replace / Cancel. `Keep
  both` needs a second destination and a second CAS against *that* name, which is
  E13-S6's naming plus a write — worth doing, not done.
- [x] Disabled on RO mounts; the Quick Look footer reads "Read-only — download a copy to
  edit". Verified on screen.
- [x] Working copies are swept when idle, and never while they hold an unsaved change.
- [ ] …keyed on "the editor's file handle count drops to zero". **Not possible portably**
  — an editor may close the file between saves or never hold it open at all — so the
  sweep is ten minutes of *no modification* instead, which errs towards keeping a file
  somebody may still be editing.

**Results**

- **This is what `xsyncv3.md` E4-S7 was built for, and it is wired end to end.**
  `ShareBackend::write_all_if_unchanged` has a portable default — stat, compare, write —
  and `XsyncBackend` **overrides it** with `StageCommit` + `expect_cookie`. The default is
  honestly documented as *not atomic*: another writer between the stat and the write is
  not detected. The override has no such window, because the cookie is checked on the
  server immediately before the publishing rename. There is a test against a real
  `xs --server` asserting a stale cookie is refused **and the file is left exactly as it
  was**.
- **`Attrs` gained an optional `change_cookie`.** `None` for every protocol except xsync,
  and `unchanged_from` falls back to size-and-mtime there — weaker, because a same-size
  edit inside mtime granularity slips through, but it is what SMB, NFS and SFTP can
  offer, and it is the same comparison xsync's own v2 `Publish` used.
- **The expectation advances on every write-back.** Comparing against the *original*
  would make every save after the first look like somebody else had changed the file —
  which is this editor. There is a test for the second save specifically.
- **A file gone from the remote counts as changed**, not as a file to recreate. Silently
  restoring something somebody deleted is the same class of mistake as overwriting their
  edit.
- **An untouched working copy is not written back.** Editors write files they have not
  changed — a save with no edits, a plugin touching the mtime — and uploading for each
  would put a round trip behind every keystroke-adjacent event.
- **The watch is on the directory, not the file.** An editor that saves by writing a
  temporary and renaming it over the top replaces the inode, and a watch on the file
  itself would follow the old one into the bin. Saves are debounced at 300ms, because one
  save is often four filesystem events.
- The session id is derived from the remote path rather than random, so opening the same
  file twice reuses one working copy instead of orphaning the first edit in a directory
  nobody will look in again.
- **A read-only mount refuses the *checkout*, not the save.** Downloading a file, letting
  the user edit it, and only then saying it cannot be written back would waste their work.

**Next steps**

- `Keep both` on the write-back conflict, reusing E13-S6's naming.
- The sweep runs on demand rather than on a timer; nothing calls it yet. It wants the
  same home as E4-S6's stage sweep — something with a lifecycle, which the app has and
  the engine does not.
- Quitting with an unsaved write-back pending is not detected. The AC asks for the temp
  files to survive that, and they do, but nothing tells the user why they are still there.

### E13 — Transfers *(handoff §Screens/4, §Transfers)*

#### E13-S1 — Queue model and window — **Done (sheet, not a second window)**

*2026-09-03.*

**AC**
- [x] `Transfer { … }`; queue is per-app, persisted, survives navigation and restart
  (unfinished items restored as `queued`).
- [x] 560px panel; header 34px "Transfers" with `Pause all` / `Clear finished`; footer
  24px counts + right-aligned aggregate rate.
- [ ] …as a *separate window* by default, with the sheet a preference. **Built as the
  sheet only**: the preference that would offer the other has nowhere to live until
  **E17**, and a second Tauri window costs an entry point and an IPC bridge for a choice
  nobody can yet make. Recorded in `docs/design-gaps.md`.

**Results**

- **The queue is state, not a side effect.** A transfer survives navigation, a dropped
  mount and a restart, so it is a record the worker acts on rather than a future
  somebody awaits. That single decision is why pause, cancel, retry and "restore as
  queued on launch" are all the same operation on the same field.
- **`update` takes a closure under the lock**, so a worker reporting progress cannot
  resurrect a transfer the user cancelled while the chunk was in flight. There is a test
  for that exact race, because it is the one that would otherwise be found in the wild.
- A restart requeues mid-flight work but **keeps `bytes_done`** — a download resumes from
  its `.part`, and starting from zero would re-transfer what is already on disk. The rate
  is dropped: it belonged to a process that no longer exists.
- **The window can ask for the whole queue** (`transfers`). Progress rides a broadcast
  channel that drops messages for a slow consumer without saying so; the sibling engine
  has no way back to the truth and its queue view can desync permanently.
- The aggregate rate is of what is *moving*. Including paused rows at zero would drag it
  down and misdescribe the link.

#### E13-S2 — Rows — **Done**

*2026-09-03.*

**AC**
- [x] Direction chips per handoff (`↑ UP`, `↓ DOWN`, failed red); filename 11.5px, 500
  while active; right mono 10.5px status strings; 4px progress bar `#e8e8eb`/`#2f6df6`;
  finished rows drop the bar and go `opacity: .6`; destination path mono 9.5px.
- [x] Failed rows: error in `#c0392b`, inline `Retry` and `Remount as RW` — the latter
  only when the error is a permission error on a read-only mount.

**Results**

- **The status strings are a tested function, not string-building in the markup.** The
  design gives three exact shapes and they are pinned:
  `41.2 / 61.7 GB · 112 MB/s · 3m 04s`, `queued`, `done · 84 KB`. The unit appears once,
  on the total — `41.2 GB / 61.7 GB` is what a naive version produces and is not what the
  design shows, so there is a test asserting it does not.
- **What cannot be said is left out rather than filled in.** A row reading `· — · —`
  is noise; a stalled transfer simply loses its rate and ETA from the string.
- `Remount as RW` appears only on a failed *upload* whose error is a permission problem.
  Offering it on a timeout would be a button that reconnects and changes nothing —
  `Transfer::needs_write_access` decides, with tests for the cases that must not offer it.
- Every literal verified from computed style on the rendered page: 4px track,
  `#e8e8eb`/`#2f6df6`, finished rows at `0.6`, and only the two unfinished rows having a
  track at all.

#### E13-S3 — Worker — **Done**

*2026-09-03.*

**AC**
- [x] N concurrent transfers (default 3), pause/resume/cancel, retry with backoff for
  transient errors, throughput and ETA smoothing, progress at ≤ 10 Hz per transfer via
  `transfer://progress`.
- [x] Downloads write `<name>.part`, fsync, atomic rename.
- [x] Uploads use the backend's path; `Caps.atomic_rename` is true for xsync since
  `xsyncv3.md` E5-S4.
- [ ] Concurrency as a *preference*. Fixed at 3 until **E17** has somewhere to put it.

**Results**

- **Modelled on Kestrel's worker with four of its sharp edges designed out**, which is
  the value of having read it rather than guessed:
  - *It never fsyncs transferred data before the rename*, so a download survives a crash
    of the app but not a power loss — the rename can land before the content, leaving a
    correctly named file of zeroes. The AC asked for the fsync; Kestrel's omission is why
    it is worth stating.
  - *It resumes from `<dest>.part` with no identity check, and keeps that file on
    cancel.* A later transfer to the same destination silently appends real bytes onto a
    stranger's and produces a file that passes every length check and is wrong. Ours
    truncates to the length **this queue recorded** and does not leave a partial behind on
    cancel. There is a test that plants exactly that stale partial.
  - *Check-then-act on a `Mutex<State>`* loses the pause/cancel race; ours goes through
    the queue's locked closure.
  - *Broadcast lag desyncs its UI permanently*; ours has `transfers` to re-read.
- **Kestrel computes no ETA at all** — the AC asks for one, so it is new work rather than
  a port. It reports `None` when a transfer stalls: a number growing without bound is
  worse than no number.
- **A bug the rate tests caught on their first run:** the first sample never established a
  baseline, so a resumed transfer reported the whole length of its `.part` file as
  throughput — a gigabyte per second.
- `stop` takes a *status* rather than a boolean, because pause and cancel differ in
  exactly one way that matters: a pause keeps the partial as its resume point, a cancel
  does not get to leave one lying around.
- Retries are transient-only and capped at five. Retrying a permission error four more
  times only delays telling the user.

#### E13-S4 — Download / Upload verbs — **Done (files, not folders)**

*2026-09-03.*

**AC**
- [x] `Download` writes the selection to the configured download folder; `Upload` opens a
  picker targeting the cwd; one queue row per file.
- [ ] …*recursively*, with folder rows aggregating. **Not built:** a folder expands to one
  row per file, and the expansion itself is not written. A single row for a directory can
  only report a percentage that means nothing when one file in it fails, so the row model
  is right; the walk is the missing part.

**Results**

- **The size comes from a fresh `stat`, not the listing the window had.** A progress bar
  built on a number that has since changed is worse than one that admits it does not know.
- The picker is `tauri-plugin-dialog`, granted **`dialog:allow-open` only** — never save
  or ask. The webview's own file input yields `File` objects and the engine needs paths,
  which is the whole reason the plugin is there; the capability file says so.

#### E13-S6 — Conflicts and resume — **Partly done**

*2026-09-03.*

**AC**
- [x] Conflict prompt Replace / Keep both / Skip; `Keep both` appends ` 2`, ` 3`… before
  the extension.
- [x] Downloads resume from the recorded byte count; uploads resume where
  `Caps.resume_write`.
- [ ] "Apply to all remaining" as a *choice*. The prompt currently applies the answer to
  the whole batch and says so. Asking per file was the alternative, and being interrupted
  twenty times mid-copy is worse than one question that is honest about its scope.
- [ ] A reconnect mid-transfer continuing from the last acknowledged byte. The worker
  retries transient failures and the resume point is durable, but the *reconnect* path is
  the session manager's and is not wired to the queue.

**Results**

- **The `Keep both` name is the engine's, not the window's.** The suffix goes before the
  extension — `report 2.pdf`, never `report.pdf 2`, which would be a file the user cannot
  open — numbering skips what is taken, and a dotfile keeps its leading dot. Seven tests,
  including `backup.tar.gz` → `backup.tar 2.gz`, which is not beautiful but is
  predictable and still opens.
- The name is checked against the side the file is **going to**, because that is the only
  side that knows what is free there.

*(E13-S5 is written up with E15-S2 below, since they landed together.)*

#### E14-S1 — New folder, rename, delete — **Done**

*2026-09-03.*

**AC**
- [x] New folder (Cmd/Ctrl+Shift+N) creates `untitled folder` and enters rename; Enter
  renames the selected item inline (RW only); Delete/Backspace deletes.
- [x] Rename/delete confirmations are **"not yet designed"** in the handoff: rename
  inline with no confirmation, delete with a single undoable toast for ≤ 5 items and a
  minimal confirm sheet above that; both flagged in `docs/design-gaps.md`.
- [x] All refused client-side when `!writable` (E9-S1).

**Results**

- **This closes E9-S1's one unmet bullet.** There are write commands now, so the guard
  it asked for is real: `writeGuarded` wraps the backend once, in `App`, and refuses
  `mkdir`/`rename`/`remove` *before invoking*. Gating buttons was never that — a shortcut
  or a later caller reaches the method anyway. The tests assert not merely that the call
  failed but that **nothing was sent**, which is the requirement; a guard that let the
  call through and reported the engine's refusal would pass a naive test and miss it.
  The guard re-reads writability per call, because a mount can go read-only while the
  window is open.
- **"Undoable delete" is a deferred delete, and that is the only honest reading.** The
  rows disappear immediately and the files are removed when the toast expires; Undo
  cancels before anything is sent. A toast offering Undo *after* the files were gone
  would be a button that cannot do what it says. A pending delete is flushed when the
  window navigates away, so a file the user believes they deleted is not quietly spared.
- **A bug the browser found, and it is the interesting one.** Delete on a read-only mount
  *deferred* rather than refusing: the rows vanished and a toast promised a deletion that
  the seam guard would reject eight seconds later. Deferral had outrun the refusal. The
  UI now checks `gate("delete", …)` before it acts, and the seam guard stays behind it as
  the thing that makes the check impossible to skip. Two layers, and the outer one exists
  because the inner one is too late to be *shown*.
- Rename uses `replace: false`, so renaming onto an existing name is refused by the
  server atomically rather than by a check here that could race. The rename field selects
  the stem and not the extension, and commits on blur as well as on Enter — a user who
  clicks away has finished typing.
- New folder selects the new directory and opens it for renaming, because creating a
  folder called `untitled folder` and leaving it that way is not what the shortcut meant.
- `runDelete` asks the listing whether each path is a directory and passes `recursive`
  accordingly, so `remove` picks `unlink` or the recursive walk without the UI deciding.

**Next steps**

- The toolbar has no buttons for these yet; they are keyboard-only. The design's toolbar
  has no New Folder or Delete either, so adding them is a design question rather than an
  oversight — noted rather than done.
- A rename that collides reports the server's `EEXIST` through the failure toast.
  **E15-S1**'s banner is where that belongs.
- Deleting a large tree runs one `remove` per selected path with no progress. **E13**'s
  transfer queue is the right home for anything long enough to watch.

### E15 — Connection health and errors *(handoff §Errors — "not mocked yet")*

#### E15-S1 — Banner — **Done**

*2026-09-03.*

**AC**
- [x] Connection-level failures show a strip under the toolbar `#fdeaea`/`#c0392b` with
  the message and a `Reconnect` button; list dims to `opacity: .5`; health dot flips to
  `#c0392b`.
- [x] Banner clears on successful reconnect; `writable` and capacity re-read.
- [x] Built to the handoff's prose; recorded in `docs/design-gaps.md` for sign-off —
  as a **live preview route** rather than a screenshot, see below.

**Results**

- **The banner has a real source, not a synthetic one.** It is raised by the mount event
  stream E10-S1 connected: a `reconnecting` or `error` event puts it up, and a `ready`
  event takes it down *and* re-lists, so `writable` and capacity are re-read from the
  listing rather than patched. That is the AC's second bullet falling out of E10's
  design rather than needing its own mechanism.
- **Only `Surface::Banner` reaches it.** The engine classifies its own errors, so a
  per-item failure goes to the transfer queue and a validation failure to its field
  without this deciding which is which. That classification has been in `EngineError`
  since M0 and this is the first thing to use it for routing.
- **The health dot follows the banner.** A red strip above a green dot would be the
  window disagreeing with itself, so the dot takes whichever of the two is worse.
- Every literal checked from computed style on the rendered page: `#fdeaea` ground,
  `#c0392b` line and text, list at `0.5`, dot at `#c0392b`.
- It **replaces two placeholders**: E9-S3's failure toast and the design-gaps entry that
  recorded it. That entry is now the banner's own sign-off note.
- `?screen=banner` renders the state from a real dropped-mount event. The AC asked for a
  screenshot; a route that goes through the same path a real failure takes seemed more
  useful than an image that goes stale — and that substitution is flagged in
  `docs/design-gaps.md` rather than made quietly.

**Next steps**

- `Reconnect` calls **E9-S3**'s `remount_read_write`, which asks for read-write. That is
  right for the read-only recovery it was built for and slightly wrong here: a plain
  reconnect should ask for whatever the bookmark asked for. Worth a distinct engine call.
- **E15-S2** (auth failure loop guard) is the other half of this epic: a rejected
  credential should open the connection editor rather than retry.
- A `reconnecting` mount shows the banner but nothing indicates the retry is in flight
  on its own timer. The engine backs off internally; the window only sees the events.

#### E15-S2 — Auth failure loop guard — **Done**

*2026-09-03.*

**AC**
- [x] A rejected credential opens the connection editor pre-filled, never a retry loop
  that locks an account; the keychain entry is offered for update.

**Results**

- **This was a live bug, not a missing feature.** The reconnect loop retried *any*
  failure with backoff, forever — including a rejected credential. A client re-sending a
  wrong password every few seconds locks the account, and it does it while the user is
  looking at a banner that says "reconnecting", so the damage is invisible until they
  cannot log in anywhere.
- The guard is on `transient()`, not on a check for authentication specifically. Anything
  retrying cannot fix stops the loop — a changed host key is the same shape, and so is
  whatever the next non-transient failure turns out to be. The classification already
  existed on `EngineError`; this is the second thing to act on it.
- **A host that is merely down still retries.** There is a test for that alongside the
  one for the guard, because a guard that turned every failure into a dead end would be
  worse than the loop it replaced.
- The mount goes to `Error`, not `Reconnecting`, so the window is not left showing a
  spinner for something that will never resolve. The window opens the editor pre-filled
  from the bookmark; unticking "Save in keychain" there is how a wrong stored credential
  is forgotten, which is E3-S3's `forget_secret` doing the work.

**Next steps**

- The editor opens pre-filled but does not say *why*. A line in the dialog naming the
  rejection would beat the banner behind it carrying that alone.

#### E13-S5 — Drag and drop *(handoff §Transfers)* — **Partly done**

*2026-09-03.*

**AC**
- [x] OS → list: accent 2px inset ring while dragging over the list (only when
  `writable`); drop uploads to cwd, or to the hovered folder row, highlighted.
- [x] Drop onto a sidebar row is a no-op with a "coming later" tooltip.
- [ ] List → OS: drag out initiates downloads to the drop location. **Not built.** It
  needs `tauri-plugin-drag` on macOS and, on Linux, an XDG portal path that downloads to
  a temp *before* the drag can even begin — so the file has to be fetched before the user
  has said where it is going. That is a different shape from every other transfer in the
  app and wants its own thinking; Download does the same job from the toolbar today.

**Results**

- **The drop is gated on `acceptsDrop`, the same permission gate the toolbar uses.**
  Highlighting on drag-over and then refusing the release is worse than never
  highlighting, which is exactly why `dropTarget` was made a write affordance back in
  E9-S1 rather than a special case here.
- Tauri reports the pointer in **physical** pixels and everything it is hit-tested
  against is in CSS pixels. Converted once, in the backend seam, rather than at each call
  site — a factor-of-two error on a Retina display would put the highlight on the wrong
  row and the files in the wrong folder.
- Dropping on a folder row lands the files *in* it. Having them land beside it instead
  would be the surprising reading of that gesture.
- The sidebar refuses explicitly rather than silently: server-to-server is marked future
  in the handoff, and a drop that does nothing with no explanation looks broken.

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
| **M1 First real share (xsync)** — **Done 2026-09-03** | E7-S2, E3-S1 (xsync tab, "via SSH"), E3-S2, E3-S4, E8-S1/S2/S5–S8, E9, E10 | Connect to `ssh host xs --server`, browse, RW/RO from `MountInfo`, capacity from `StatFs`, status bar live. |
| **M2 Read, edit, move** — **Done 2026-09-03** *(bar drag-out and two partials)* | E11, E12, E13, E14, E15 | Quick Look (incl. video over range reads), edit-in-place with CAS, transfer queue with staged resumable uploads, banner — all on xsync. |
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
      screenshot, awaiting design sign-off. *(Started 2026-09-03 with E9-S3's two
      entries; screenshots still to attach.)*
