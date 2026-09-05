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
| **M4.5** | E14-S2 — Context menu | **Done** 2026-09-05 |
| **M4.5** | E14-S3 — Move to… | **Done** 2026-09-05 |
| **M4.5** | E14-S4 — Properties | **Done** 2026-09-05 (POSIX mode still parked) |
| **M2** | E15-S1 — Banner | **Done** 2026-09-03 |
| **M2** | E15-S2 — Auth failure loop guard | **Done** 2026-09-03 |
| **M3** | E6-S1 — SFTP backend | **Done** 2026-09-03 |
| **M3** | E6-S2 — Writability and capacity | **Done** (bar `security_label`) |
| **M3** | E6-S3 — Mutations | **Done** 2026-09-03 |
| **M3** | E4-S1 — SMB client decision | **Done** 2026-09-03 (`smb`; see ADR 0001) |
| **M3** | E4-S2/S3/S4/S5 — SMB backend | **Done**, behind the off-by-default `smb` feature (GPLv3 when on) |
| **M3** | E4-S6 — SMB reconnect | **Done** 2026-09-04 (transfer resume defers to E13-S6) |
| **M3** | E5-S1 — NFSv3 client | **Done** 2026-09-03 |
| **M3** | E5-S2 — Identity and ports | **Done** 2026-09-03 |
| **M3** | E5-S3 — Writability | **Done** 2026-09-03 |
| **M3** | E5-S4 — Range I/O and transfers | **Done** 2026-09-04 (1 MiB cap; throughput measured, see benchmarks.md) |
| **M3** | E5-S5 — NFSv4 and Kerberos | **Spike done** 2026-09-04; implementation deferred (ADR 0002) |
| **M4** | E17-S1 — Stored state | **Done** 2026-09-04 |
| **M4** | E16-S1 — Shortcut map | **Done** 2026-09-04 (row-layout keys bound with E8-S4) |
| **M4** | E16-S2 — Accessibility | **Done** 2026-09-04 (one contrast exception) |
| **M4** | E8-S3 — Layout `1g` name-led | **Done** 2026-09-04 |
| **M4** | E8-S4 — Layout `1h` mono grid | **Done** 2026-09-04 (no View menu to add to) |
| **M4** | E8-S9 — Live refresh | **Partly done** (coalescer built; `watch` has no producer) |
| **M4.5** | E8-S10 — The `..` row | **Done** 2026-09-05 (asked for directly; beyond the mocks) |
| **M4** | E1-S4 — Responsive column dropping | **Done** 2026-09-04 |
| **M4** | E1-S5 — Transitions | **Done** 2026-09-04 |
| **M4.5** | E1-S6 — First run and empty state | **Done** 2026-09-05 (the demo mock is gone from the shell) |
| **M4.5** | E3-S5 — Dialog fields in the real webview; Quick Look button | **Done** 2026-09-05 |
| **M4.5** | E7-S4 — SSH failures reported, not swallowed | **Done** 2026-09-05 (`ssh-keyscan` still swallows the same distinction) |
| **M4.5** | E7-S5 — Capacity on xsync | **Done** 2026-09-05 (refreshed around a listing, 30 s) |
| **M4.5** | E13-S4 — Folder download (the recursive walk) | **Done** 2026-09-05 (aggregating folder rows refused, not deferred) |
| **M4.5** | E13-S7 — Streaming reads for transfers | **Done** 2026-09-05 (27.6 → 66.1 MB/s release; ≈101% of raw `ssh cat`) |
| **M4.5** | E18-S1 — macOS build (local, unsigned) | **Partly done** 2026-09-05 (icon set and `.app`/`.dmg`; signing deferred) |
| **M4.5** | E18-S4 — Staging `xs` on a server | **Done** 2026-09-05 (verified on `root@orion.local`) |
| **M4.5** | E18-S5 — macOS local network permission | **Done** 2026-09-05 (found in the bundled app) |
| **M4.5** | E21-S1 — Local filesystem backend | Not started (asked for directly 2026-09-06) |
| **M4.5** | E21-S2 — "Local" in the dialog and the window | Not started (asked for directly 2026-09-06) |
| M4+ | E19, E7-S1/S3, signing and notarisation | Not started |

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
- [x] `system` following `prefers-color-scheme`. *(E17-S1: it is the default
  choice, followed live rather than read once at launch.)*

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
- [x] Collapsible; the collapsed flag persists; it never auto-hides *(handoff
  §Responsive)*. *(E17-S1. Hidden outright rather than narrowed to a rail — the
  design has no rail. The toggle's placement is a design gap.)*
- Dark theme uses the dark sidebar tokens (`#26272a`, active `#1f3358`/`#4f86ff`,
  subtitle `#8fa8d8`).

#### E1-S4 — Responsive column dropping — **Done**

*2026-09-04.*

**AC**
- [x] Below ~900px list width the Kind column is removed, then Modified; header
  and rows stay aligned; no horizontal scroll.

**Results**

- **The list's width, not the window's.** Collapsing the sidebar hands the list
  ~200px back, and the columns should take it; measuring the window would keep
  Kind hidden on a wide list inside a narrow-ish window.
- **Header and rows share one `template()` call** with the same arguments. That
  is the whole of "stay aligned" — a column that vanished from one and not the
  other would put every cell one place out.
- **A divider for a dropped column is withheld too**, otherwise the user can
  drag a boundary that resizes something they cannot see.
- **The second threshold is chosen, and the first attempt at it was wrong.** The
  handoff gives 900 for Kind and says "then Modified" without a number. 700
  looked right until the browser showed Modified disappearing at the handoff's
  own "minimum useful window" of ~820px — because that is a *window* width and
  this is a *list* width, and the sidebar takes about 200px of it. It is 560
  now, which keeps a date at that size and still leaves Name 276px.
- Verified at three widths: all four columns at a 1194px list, Kind gone at
  894px, both gone at 654px — with header and row templates equal and no
  horizontal scroll at any of them.

#### E1-S5 — Transitions — **Done**

*2026-09-04.*

**AC**
- [x] 120ms ease on hover/disabled colour changes.
- [x] 140ms opacity + 4px rise on Quick Look open/close.
- [x] **No** layout animation in the list, verified by a test asserting no
  `transition` on row geometry properties.

**Results**

- **The close animation needed the panel to outlive the signal.** Solid unmounts
  on the frame `quickLook` flips, so an exit animation had nothing to play on —
  only the opening would ever have been seen, and the close would have been a
  disappearance. The panel now stays mounted for the length of its animation.
  The unmount is on a timer rather than `animationend`, because a panel that
  does not animate — reduced motion — would never fire that event and would
  never unmount.
- **The rise composes with the centring transform** rather than replacing it:
  both keyframes carry the `-50%`, and only the 4px moves. Overwriting
  `transform` would have dropped the panel to the top of the window mid-animation.
- **The no-layout-animation test was checked against a violation, not just run.**
  Injecting `transition: height` on a row makes it fail with the selector and
  property named. A rule about what must *not* be added is worth nothing if it
  passes by matching nothing.
- It also holds two smaller lines: transitions in the shell may only name
  colour-ish properties, and none may hardcode a duration — a literal `120ms`
  would drift the moment the token changed.
- Four controls with hover or disabled states had no transition at all
  (`.bannerAction`, `.primary`, `.sidebarFooterButton`, `.statusAction`) and the
  column divider's hover changed its line's colour instantly. Each now
  transitions exactly the properties its own states change.
- `prefers-reduced-motion` turns the panel's animation off. Someone who asked
  the system not to animate should not be animated at.

#### E1-S6 — First run and empty state — **Done**

*Found 2026-09-05 by launching the bundled app with nothing saved; fixed the
same day.*

The desktop app opened on the design's fixture: a fake `smb://nas.local/media`
with fake films, an `RW` badge and `2.4 TB free of 16 TB`, and nothing mounted.
That is right for the browser preview and wrong for a first run — a user who has
just installed the app is looking at a share that does not exist, with verbs
that act on it (Download queued a row for a file that is not there). The Tauri
shell's `AppState::backend()` falls back to a demo mock when no session is
active, and the window's chrome invents `nas.local` for it.

Also found: the Transfers sheet is anchored over the toolbar's right half, so
`Quick Look`, `Edit` and `Download` cannot be clicked while it is open — the
click lands on the sheet. Selecting a file and pressing Download with the sheet
up does nothing, silently.

**AC**
- [x] In the desktop app, with no mount active, the list area shows an empty
  state ("No share connected" + a `New connection…` button that opens the
  dialog) and the status bar, breadcrumb and window title name nothing. No verb
  is enabled. The fixture is served **only** in the browser preview
  (`!inTauri()`), never by the Tauri shell — the `demo` field is gone from
  `AppState`, from the transfer worker's `BackendSource` and from the stream
  protocol handler, and `AppState::backend()` now returns the session manager's
  `Disconnected` error.
- [x] The browser preview (`pnpm dev`, `?screen=…`) is unchanged. `?screen=empty`
  is added, which is the only way to look at the new state in a browser.
- [x] The Transfers sheet no longer covers the toolbar: it opens below the
  toolbar row. Recorded in `docs/design-gaps.md`.
- [x] Tests: the empty-state render; a test that `defaultBackend()` in Tauri does
  not fall back to the fixture.

**Results**

- **The bug was not a missing empty state; it was a convincing fake one.** The
  window had no way to tell the mock from a real share, and neither did the
  user: rows, an `RW` badge, `2.4 TB free of 16 TB`, and verbs that acted on it.
  So the fix is mostly a subtraction — the mock is gone from all three places it
  was wired in, not only from `AppState::backend()`. It also sat behind the
  transfer worker (an upload with nothing mounted would have been "moved" into
  a mock) and behind the streaming protocol (a Quick Look of a file that does
  not exist). Fixing only the command path would have left both.
- **The listing is the gate for the window's chrome, not the bookmark.** The
  first version read the title and breadcrumb from `activeBookmark()`, which is
  right until you relaunch with a bookmark still flagged `mounted` from the last
  session — then the title bar names a share the shell is not holding, which is
  the same bug in a second costume. The test seeds exactly that bookmark.
- **"No listing" is not the same as "not connected".** There is one frame
  before the first request settles where the listing is also null, and an empty
  state that flashed before every window would be worse than none. The state is
  `listing() === null && error() !== null` — the engine has actually answered.
  A failed navigation on a live mount leaves the previous listing on screen, so
  it does not land here either.
- **`listingState.error()` had never been read by anything.** A listing failure
  was silently swallowed, and its message was `String(problem)` on a serialised
  Tauri error — "[object Object]". It goes through `engineFailure` now, which is
  the module that exists for exactly that.
- Every verb in the empty window is disabled through the *same* `gate` the
  mounted window uses, with `NO_SHARE_REASON` as the context's reason. A
  separate "if disconnected, disable everything" branch would be a second rule
  that could disagree with the first, and E9-S1's whole point is that there is
  one.
- **The sheet's overlap was measured, not computed.** It was anchored to the
  title bar at 34px; the verbs sit at y 43.5–67.5, and the sheet's 560px covered
  the right half of the toolbar. It now opens at
  `calc(var(--height-titlebar) + var(--height-toolbar))` = 78px, verified in the
  preview by hit-testing each verb's centre with `elementFromPoint` rather than
  by reading the stylesheet.

- **Relaunch, checked by the lead in the bundled app:** a bookmark read back from the
  store with `mounted: true` put itself under MOUNTED and titled the window after a share
  no session was holding. Sessions are not persisted, so every stored `mounted` flag is
  stale at launch; they are cleared on load (`withMounted(saved, new Set())`) and the
  row appears under BOOKMARKS, where a double-click mounts it.

**Next steps**

- A bookmark keeps `mounted: true` across a relaunch even though no session
  survives one, so the sidebar's `MOUNTED` section can show a row for a share
  that is not mounted. The chrome no longer believes it, but the sidebar still
  draws it. It wants either a "clear `mounted` on load" pass or a reconcile
  against `mount://changed` — E17-S1's store is where that belongs.
- The empty state does not offer the saved bookmarks directly, though it names
  them. A "pick one of these" first run is the obvious next question for a
  designer, and is flagged as such in `docs/design-gaps.md`.
- The window title is the web title bar only; the OS window is still called
  "Excalibur" from `tauri.conf.json`. Nothing reads wrong today, but a real
  first run has no share name to put anywhere.

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

- **Two probe defects found 2026-09-05 with the bundled app, fixed the same day.** The
  sidebar dimmed `orion.local` while `nc` reached it in 39 ms: `orion.local` resolves to
  an IPv6 link-local address *before* its IPv4 one, and `TcpStream::connect` on a name
  tries addresses in order, so the link-local attempt spent the whole 2 s budget. The
  probe now connects to every address at once and takes the first answer (27 ms on
  orion; `xcal reachable HOST PORT` runs the same probe from a shell). And because
  `reachable` is persisted with the row, the launch path only re-probed bookmarks still
  marked reachable — one failed probe dimmed a bookmark for good. The app now probes
  every bookmark on launch, whatever the store says.

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

#### E3-S5 — Dialog fields in the real webview; the Quick Look button — **Done**

*Found 2026-09-05 by driving the bundled app; fixed the same day.*

Two things the browser preview could not show:

1. **WKWebView autocapitalised the username.** Typing `root` produced `Root`,
   the connection failed, and the failure read as a network error (see E7-S4).
   The same applies to Server, Export, Port, Domain and the password field —
   none of them is prose. Safari/WKWebView also autocorrects and spell-checks
   text inputs by default.
2. **The toolbar's `Quick Look` button does nothing.** `Shell.tsx` renders the
   `Verb` with no `onClick`; only the Space shortcut opens the panel. The button
   is enabled, has a tooltip, and is inert.

**AC**
- [x] Every field in the New Connection dialog carries `autocapitalize="none"`,
  `autocorrect="off"`, `spellcheck={false}` (and `autocomplete` set sensibly —
  `off` for the password, `username` for the username). The Filter field too.
- [x] The toolbar `Quick Look` verb toggles the panel exactly as Space does,
  including the one-selected-row rule.
- [x] A test that the dialog's inputs carry the attributes, and one that the
  verb's click reaches the same handler as the shortcut.

**Results**

- **The four attributes travel as one value** (`PLAIN_FIELD`, spread onto each
  input) rather than being repeated six times. The bug this story fixes is a
  field that is missing one of them, and a set that has to be retyped per field
  is a set that will be. `autocomplete` is `off` everywhere except Username,
  which is `username`: a password manager may reasonably hold the account name
  for a host, while the secret belongs in the OS keychain through the "Save in
  keychain" box.
- **The test enumerates whatever the dialog renders, per protocol tab**, instead
  of naming the six fields. The dialog swaps its field set rather than disabling
  what does not apply, so Password and Domain exist only on some tabs — and the
  field that will actually break this is the one added later, which no named
  case would cover.
- **The Quick Look test asserts the two entry points are one verb, not that
  each works.** Open with the button, close with Space, open with Space, close
  with the button. Two handlers each holding their own copy of the
  one-selected-row rule would pass "the button opens the panel" and fail this.
  The rule itself is now `toggleQuickLook()`, which both call.
- The Filter field got the same attributes for the same reason: a filter that
  quietly became `Arrival` after the user typed `arrival` matches nothing, and
  the failure looks like the filter being broken.
- Verified in the rendered DOM rather than the source — all seven inputs
  (six dialog fields across the SMB and xsync tabs, plus Filter) read back
  `autocapitalize="none"`, `autocorrect="off"`, `spellcheck="false"` and an
  `autocomplete` in the browser preview. jsdom does not implement the
  `spellcheck` IDL property, so the test reads the attribute, which is what
  WebKit reads too.

**Next steps**

- The same treatment is owed to any field added later — the rename input in the
  list, and the Preferences sheet's download-folder box, are both plain text in
  a WKWebView today. Neither was in this story's AC.
- `autocapitalize`/`autocorrect` are WebKit's; nothing here is verified against
  a Linux WebKitGTK build, where the defaults differ and the attributes are
  harmless either way.

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

**Reversed 2026-09-03: the decision is now `pavao`, behind a feature.**

The owner accepted GPLv3 for an opt-in build, which removed the licence
objection — and the pure-Rust `smb` crate then turned out not to be linkable at
all, for the reasons below. A cargo feature does not rescue it either: cargo
resolves optional dependencies into the lockfile even when the feature is off,
so an optional `smb` breaks `cargo metadata` for everyone. That was checked, not
assumed. `pavao` collides with nothing, so the choice was never between two
working options — it was `pavao` or no SMB. See
[ADR 0001](docs/decisions/0001-smb-client.md) and E4-S2 below.

**The blocker that ruled out the pure-Rust `smb` crate**

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

- Worth filing upstream regardless: a stable `sspi` release pinned to 27 release
  candidates breaks any consumer with a normal crypto stack. If that is fixed, `smb`
  becomes a real alternative to `pavao` and a better one — MIT, no C dependency, and
  a readable negotiated cipher.

#### E4-S2/S3/S4/S5 — Connect, listing, range I/O, mutations — **Done, behind a feature**

*2026-09-03.*

**AC**
- [x] SMB 3.1.1 preferred, 3.0.2/3.0/2.1 negotiable; **SMB1 never** — the dialect
  bound is set to 2.0.2–3.1.1, and that bound is the only guarantee available
  because the negotiated dialect cannot be read back.
- [ ] `protocol_label` = `SMB3`/`SMB2`; `security_label` = the cipher.
  **Not possible.** libsmbclient exposes no getter for the negotiated dialect,
  signing state or cipher — the setters are policy inputs and the getters echo
  them. So `protocol_label` is `SMB` and `security_label` says
  `smb2+ · encryption requested`, which is what was asked for rather than a
  cipher this code never observed. Same shape as E6-S2's open bullet.
- [~] `writable` = `MaximalAccess` ∧ share not read-only ∧ the user did not ask
  for RO. **`MaximalAccess` is not exposed either**, and the `ST_RDONLY` bit
  `statvfs` could carry is never set by Samba's implementation. So the only term
  that can be evaluated is the create-probe, which is the one that answers the
  question the Upload button will ask.
- [x] Listing with the DOS hidden bit; kind detection from name and attributes.
- [x] Range reads and positional writes; resume lands at its offset.
- [x] `mkdir`, `rename(replace)`, `remove` (file and recursive directory),
  capacity.

**Results**

- **The DOS hidden bit is honoured, and so is the leading dot.** A hidden file on
  SMB is one the server flagged; the Unix convention does not travel. Both are
  checked, because a share written from a Unix client will have dotfiles the
  server never flagged.
- **The capacity units trap is real and easy to miss.** Against any server
  without UNIX extensions — every Windows box and most NAS — Samba packs
  `f_frsize` with sectors-per-allocation-unit and `f_bsize` with
  bytes-per-sector, so the byte size is the product of all three rather than the
  POSIX `blocks * frsize`. Getting it wrong reports a 4 TB share as a few
  gigabytes, so a test asserts the total is at least a gigabyte.
- **`Caps::atomic_rename` is false and the tests assert it.** Samba unlinks an
  existing destination before renaming onto it — the same window E6-S3 rejected
  for SFTP — and libsmbclient offers no no-clobber flag, so the `replace: false`
  refusal is this backend's own stat-first check and is therefore racy.
- **The dialog's Domain field was being collected and thrown away.** It reached
  the draft and never reached the spec, so an SMB share on a domain could not
  have been reached at all. `ConnectionSpec` now carries it.
- **The window asks the engine which protocols exist rather than listing them.**
  SMB is behind a cargo feature but the frontend is one bundle, so a static list
  would be wrong for one of the two builds. `supported_protocols` is computed
  with `cfg!(feature = "smb")`.
- **The fixture was wrong in the same way the NFS one was.** On a Docker Desktop
  bind mount, Samba answers `NT_STATUS_NOT_SUPPORTED` for both delete and rename
  — verified with `smbclient` itself, not just through this backend — so every
  mutation test failed against a server that was working correctly. A named
  volume fixed it. That is now twice that virtiofs has produced a plausible
  wrong answer; any future protocol fixture should start on a volume.

**Next steps**

- **E4-S6 (reconnect) is not started.** It shares E13-S6's resume work.
- Kerberos is reachable (`use_kerberos`, `use_ccache`) but unwired: there is no
  UI for it and the system ccache is the only source libsmbclient will take.
- Throughput is unmeasured, for the same reason as NFS: the only server
  available is a container on this machine.

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

#### E4-S6 — Reconnect — **Done**

*2026-09-04.*

**AC**
- [x] On `STATUS_NETWORK_NAME_DELETED`/socket drop, the session manager
  reconnects and re-tree-connects.
- [x] `writable` is re-read from the new connection.
- [ ] Open transfers resume at offset. **Deferred with E13-S6**, which owns
  resume-across-reconnect for every backend; nothing here is SMB-specific.

**Results**

- **Almost none of this was new code, which is the point.** The reconnect loop
  from E2-S2 is generic: it notices a mount through `ping`, tears the backend
  down, rebuilds it through the connector — which for SMB *is* a fresh tree
  connect, since there is no session to resume — and republishes whatever the
  new connection reports. The work was making SMB's failures legible to it.
- **`STATUS_NETWORK_NAME_DELETED` arrives as `ENETRESET`**, which `std` does not
  categorise, so it was reading as a plain I/O error. That still reconnects —
  `transient()` covers `Io` too — but it would have shown on a transfer row
  instead of the banner, which is wrong: the mount is what broke. A dead tree
  also surfaces as `BadFileDescriptor` from a listing while a `stat` on the same
  mount reports `ECONNREFUSED`; all three now map to `Disconnected`. The errnos
  come from `libc` rather than literals, because the numbers differ between
  macOS and Linux.
- **The test found a real bug, and it is the one worth recording.** Reconnecting
  onto a server that was still coming back up succeeded — and then the
  *write-probe* failed with `ENETRESET`, so `writable` came back **false**. The
  mount reported itself Ready and read-only, and would have stayed read-only
  until the next drop. Every write affordance in the window is gated on that one
  value, so the user's share silently loses its Upload button after a blip.
- **The fix is a distinction the probe was not making**: a server that refuses
  the write has told us something; a connection that broke has told us nothing.
  Reporting the second as the first is the bug. `probe_mount` now returns `Err`
  on a disconnect rather than a read-only mount, which sends the session manager
  back round its retry loop — the only thing that can actually resolve it.
- **The same bug was in the NFS and SFTP backends**, written earlier the same
  day with the same shape, and both are fixed the same way. Not strictly E4-S6,
  but it is one defect in three places rather than three defects, and shipping
  two of them knowingly would have been worse.
- **The two reconnect tests are opt-in** (`EXCALIBUR_SMB_DISRUPTIVE=1`, and
  `--test-threads=1`). They kill the server, and every other test in the file
  would fail while it is down. A `Drop` guard restarts it and waits, so an
  assertion failure cannot leave the container down and poison later runs.

**Next steps**

- Resume-at-offset across a reconnect is E13-S6's, for all four backends at once.
- The generic loop rebuilds the whole backend on any `ping` failure. For SMB
  that is right — there is no session to resume — but xsync will eventually want
  a cheaper path, which is its own `Resume` story.

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
- [x] Cap 1 MiB. *(Was 512 KiB; see below.)*
- [~] Throughput ≥ 80% of the kernel client on the same pair. **Measured at
  ≈79% of the kernel client's fastest path**, while paying a port forward that
  path does not — and against the kernel on a comparable hop count it is ~2.2×
  ahead. Recorded in [`docs/benchmarks.md`](docs/benchmarks.md) with both
  readings, because choosing one would be a claim rather than a measurement.

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

**The 512 KiB cap, resolved 2026-09-04**

- The cap was set low out of caution, not evidence: the codec cannot reassemble a
  reply split across RPC record fragments, and a fragmented reply errors *and*
  leaves the body unread, poisoning the connection. Rather than raise the number
  and hope, the question was measured. **Linux `nfsd` advertises exactly 1 MiB as
  `rtmax` and answers a 1 MiB READ in a single fragment**, so the caution was
  costing half the read size for nothing.
- **A server that does fragment is now handled rather than guessed at.** The read
  size starts at what the server advertises and *halves* when a reply comes back
  fragmented, down to a 32 KiB floor, after which the error is reported. The
  connection is discarded on the way — the unread body makes it unusable — and
  the pool dials a fresh one, so the read continues from the same offset at the
  smaller size. Picking a size low enough that no server could ever fragment is
  unknowable in advance and slow for every server that would not have.
- The cap is now 1 MiB, which is E5-S4's number, and a test asserts the
  negotiated size against a real server rather than trusting the constant.
- `Caps::atomic_rename` is **true**: NFS `RENAME` is POSIX rename and the server
  does it atomically. That is better than the SFTP backend manages without an
  extension. The no-clobber case is the weak one — v3 has no flag for it, so it is
  a separate `LOOKUP` first and therefore racy, which the code says out loud.

#### E5-S5 — NFSv4 and Kerberos — **Spike done; implementation deferred**

*2026-09-04. No code was written; the deliverable is a decision.*

**AC**
- [x] Spike `libnfs` bindings vs. a minimal v4.0 client for `krb5/krb5i/krb5p`
  via `gssapi`. Recorded in
  [ADR 0002](docs/decisions/0002-nfsv4-and-kerberos.md).
- [n/a] The dialog's Version/Security controls light up when a backend passes
  the E5-S1 matrix. No backend passes, so nothing lights up.
- [n/a] `security_label` becomes `krb5p` etc. It stays `sys`, which is what
  AUTH_SYS is.
- [x] UI is honest until then. **Already true and unchanged** — E3-S1
  deliberately did not build those controls, on the grounds that "a disabled
  control that will never enable in this build is a promise". This story
  confirms that was the right call rather than changing anything.

**Results**

- **The decision is to defer, and to split the story if it is ever picked up.**
  The AC bundles "NFSv4 and Kerberos"; the most useful thing the spike found is
  that their costs are nothing alike. NFSv4 without Kerberos is tractable.
  Kerberos is expensive and currently blocked upstream on macOS.
- **The licence is the opposite of the SMB story, and that is worth carrying
  forward.** libnfs's `lib/` and `include/` are LGPL-2.1-or-later, the generated
  protocol files are BSD, and only `examples/` — off by default — is GPL. So
  dynamic linking raises no absorption question at all. The thing that forced
  SMB behind a feature flag simply does not apply here. *Static* linking would
  engage LGPL §6, which is one more reason not to reach for the static build.
- **Upstream libnfs already has exactly the API this story wants** — verified in
  its header, not inferred: `nfs_set_security(enum rpc_sec)`, the URL argument
  `sec=<krb5|krb5i|krb5p>`, and `nfs_set_version` with `NFS_V4`/`NFS_V4_2`. The
  C library is healthy and actively developed. **The Rust bindings are the dead
  link**: both crates were last published in 2022, `libnfs-sys` pins a bindgen
  that cannot run against a modern libclang, and the safe wrapper is NFSv3-only
  by construction with its URL parsers sitting inside a block comment.
- **`nfs_set_security()` degrades silently, and every easy build triggers it.**
  The setter returns `void` and its body is inside `#ifdef HAVE_LIBKRB5`, so on
  a Kerberos-less build — which is Homebrew's bottle and every apt package — it
  links, runs, reports nothing, and the mount proceeds over AUTH_SYS. Asking for
  `krb5p` and silently getting no authentication is the same class of failure as
  a `writable` that is inferred rather than observed. The URL form fails loudly
  and is the capability probe to use instead.
- **libnfs's Kerberos build is broken on macOS today.** Its `krb5-wrapper` pulls
  MIT-style `gssapi/gssapi.h` and Apple's `GSS/GSS.h` into one translation unit,
  which define incompatible `gss_ctx_id_t` types; past that the link fails
  because configure hardcodes `-lgssapi_krb5` where macOS needs
  `-framework GSS`. Both are small upstream bugs and should be sent upstream
  rather than carried.
- **Kerberos also only covers the NFS program in libnfs.** The handshake is
  gated on `program == 100003`, so under NFSv3 the MOUNT and portmap traffic
  still goes out as AUTH_SYS. NFSv4 has no MOUNT, so this disappears there —
  a second, independent reason Kerberos belongs with v4 rather than bolted onto
  the v3 backend that exists.
- **Nothing in Rust implements RPCSEC_GSS.** Not partially, not behind a
  feature. The crate this backend depends on is worse than merely lacking it:
  `nfs3_types`' `auth_flavor` stops at `AUTH_DES = 3` and its codec rejects
  unknown tags, so flavour 6 is neither constructible **nor parseable** — it
  cannot read a reply from a Kerberos-secured server at all. Writing the layer
  means somewhere between 1700 and 5000 lines plus tests — two independent
  readings of the RFC disagreed by roughly twofold, which is itself a signal —
  and the failure modes that matter are invisible against a cooperative server. If it is ever done, the
  host should be `onc-rpc-client`/`nfs-v4`, whose public `RpcTransport` trait
  lets it be written *outside* the crate, rather than forking `nfs3_client`,
  whose credential is fixed at construction.
- **No existing NFSv4 crate is adoptable either**, for specific reasons rather
  than general unease — the most complete one encodes `OPENATTR` as 20 when it
  is 19, with a unit test that blesses the wrong value. ADR 0002 lists the rest.

**Next steps**

- Do nothing until a user actually needs v4 or Kerberos. This is not a v1
  blocker and every route currently begins by adopting a fork.
- If **NFSv4** is needed: fork `libnfs-sys` onto current bindgen and write a thin
  safe layer against libnfs 7.x, built statically through CMake. No Kerberos, no
  system package, no licence consequence — though note RFC 7530 says a v4 client
  MUST *support* Kerberos, so an AUTH_SYS-only v4 backend is a documented
  deviation rather than a complete client. On the *pure-Rust*
  route, target **4.1 rather than the AC's 4.0** — `SECINFO_NO_NAME` removes
  4.0's filehandle-to-parent reverse map and its blind `PUTROOTFH` iteration,
  and the session backchannel removes 4.0's requirement that the client also be
  a GSS *acceptor* — with `nfs4` 0.1.0 as a types-only starting point. Via
  libnfs the choice does not exist: it offers v3, v4.0 and (unreleased,
  single-slot) v4.2, and no 4.1 at all.
- If **Kerberos** is needed: fix libnfs's macOS header collision and GSS link
  upstream first.
- Revisit the pure-Rust route if any crate ships RPCSEC_GSS. `libgssapi` already
  resolves `-framework GSS` on macOS, so that route's platform story is the
  better one — it is only the missing protocol layer that makes it expensive.

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

#### E7-S4 — SSH failures reported, not swallowed — **Done**

*Found 2026-09-05 against `root@orion.local`; fixed the same day.*

`XsyncBackend::connect_ssh` inherits `ssh`'s stderr. In the desktop app that
goes nowhere, so `Permission denied (publickey)` reached the user as
`connection lost: connection closed before the request was answered` — a
network error for what was an authentication failure (a capitalised username,
E3-S5). Worse, `Disconnected` is `transient()`, so had this happened to an
established mount the session manager would have retried a refused login
forever — the exact loop E15-S2 exists to prevent.

**AC**
- [x] `ssh`'s stderr is captured (piped, drained on a task, bounded) and, when the
  session fails before the first reply, its last lines are the error. A
  `Permission denied` / `Host key verification failed` / `Could not resolve
  hostname` from `ssh` maps to `AuthenticationFailed` / `HostKeyChanged` /
  `NotFound`-class errors respectively, and only genuine transport drops map to
  `Disconnected`. *(The `NotFound` class became its own variant,
  `EngineError::HostNotFound` — see Results.)*
- [x] `xs: command not found` on the remote maps to a message that says so and names
  the fix ("install xs on the server or put it on PATH"; see E18-S4).
- [x] After the session is up, `ssh`'s stderr lines are logged (`tracing`/`eprintln`
  is fine) rather than dropped, so a later disconnect has a reason.
- [x] Tests using `rsh` (the existing test hook) with a fake `ssh` script that
  prints a given stderr and exits.

**Results**

- **The user reads what `ssh` said.** `xcal mount --xsync-ssh
  orion.local:/srv/strife --user nosuchuser` ended in `connection lost:
  connection closed before the request was answered`; it now ends in
  `authentication failed: nosuchuser@orion.local: Permission denied
  (publickey).` The classification matters more than the wording:
  `AuthenticationFailed` is not `transient()`, so an established mount that
  fails this way stops at E15-S2's guard instead of re-offering a rejected
  login until the account locks.
- **Matching OpenSSH's English is unlovely and is the only signal there is.**
  `ssh` exits 255 for a refused key, an unreachable host and a rejected
  password alike, so the exit status separates only the *remote command's*
  failures — 127 from the login shell being the one that says `xs` was not
  found. Both the status and the wording are consulted there, because the
  phrasing differs by shell (`bash: xs: command not found`, `sh: xs: not
  found`).
- **Anything unrecognised stays `Disconnected` — carrying the captured text.**
  A phrase this build has never heard of is still a dropped session, and
  putting `ssh`'s words in the message means the next unfamiliar failure is
  readable rather than invisible. That was the actual bug: not the mapping, the
  silence.
- **A host that does not resolve needed a variant of its own.** Reported
  through `NotFound` it would have read "no such file or directory" for a
  mistyped hostname, so `EngineError::HostNotFound` says "no such host"
  instead. It is deliberately *not* transient, for the same reason a rejected
  credential is not: the common cause is a name that will never resolve, and
  retrying one every few seconds hides it behind a banner that says
  "reconnecting".
- **The drain has to be a task, and the bound belongs on the buffer.** Reading
  the pipe only after a failure would let it fill, and a full pipe blocks `ssh`
  itself — hanging the connection instead of reporting it. It keeps the last 20
  lines, each clamped to 512 bytes, and ends at EOF, which is when `ssh` exits:
  it cannot outlive the process, and a chatty remote cannot grow it. Nothing is
  stripped, because there is nothing secret in it — `BatchMode=yes` means `ssh`
  never asks for a password and it never echoes one.
- **The failure and the reason arrive on different descriptors.** The client
  sees a closed pipe, `ssh` writes why; which lands first is a race, so the
  error waits (bounded, 500 ms) for the drain to end before it decides. Without
  that wait the reason would arrive a moment after the error that needed it.
- Lines are logged as they arrive rather than only kept, which is what gives a
  disconnect *after* the session is up a reason at all — the mount that drops
  an hour in has no failing call to attach an explanation to.
- The tests drive a fake `ssh` through the existing `rsh` hook: it prints one
  of OpenSSH's real messages and exits with one of its real codes. One of them
  asserts the opposite property on purpose — a refused *connection* stays
  `transient()` — because a mapping that made every failure fatal would be
  worse than the loop it replaced, which is the same balance E15-S2 struck.

**Next steps**

- **`ssh-keyscan` sits upstream of all this and still swallows the same
  distinction.** `hostkeys::verify` runs before `connect_ssh`, and a host that
  does not resolve fails *there* as `Disconnected("… nodename nor servname
  provided …")` — transient, so a typo'd hostname in a bookmark reconnects
  forever — and because the scan comes first, a mistyped host never reaches the
  `HostNotFound` mapping written here at all (that one catches the narrower
  case where the scan resolved the name and `ssh` then could not). It is this
  story's bug one layer up, and the classifier written here is where the answer
  lives; it was left alone because `hostkeys` belongs to E3-S4 and the fix
  wants to be a shared helper rather than a second copy of the phrase list.
- `connect_local` still inherits `xs`'s own stderr. That is right for a
  terminal and wrong for the window, and it will want the same treatment when a
  local export is something the app offers rather than a test fixture.
- The dialog shows the message; it still does not use the *class*. An
  `AuthenticationFailed` from a bookmark should open the connection editor with
  the rejection named, which is E15-S2's own outstanding next step and now has
  an error worth naming.
- *(Closed the same day by the lead:)* `hostkeys::Keyscan::scan` now maps a name that
  does not resolve to `HostNotFound` — the same class `connect_ssh` uses — with a test
  against a `.invalid` name, so a typo'd bookmark is shown once and never reconnected.

#### E7-S5 — Capacity on xsync — **Done**

*Found 2026-09-05: the status bar showed `5 items` and no capacity for a mount
whose `StatFs` answers. Fixed the same day.*

`XsyncBackend::mount_info()` returns `capacity: None` even though
`capacity()` works (`xcal df` prints `357503451136 free of 491590631424`). The
window takes capacity from the listing's mount facts (E10-S1), so nothing ever
shows it.

**AC**
- [x] `MountInfo.capacity` is populated at mount from `StatFs`, and refreshed on
  each `list_dir` (one extra round trip per listing is acceptable; if it is not,
  refresh at most every 30 s and say so). *(Refreshed around a listing, at most
  every 30 s — said out loud below.)*
- [x] The status bar shows `333 GB free of 458 GB` (or the app's formatting of the
  numbers above) for the orion mount.
- [x] A test against `xs --server` (gated on `EXCALIBUR_XS`) that `list_dir`'s
  mount facts carry a capacity.

**Results**

- **The number was never missing; it was never anywhere the window looks.**
  `capacity()` worked all along, and nothing calls it: the status bar reads the
  mount facts that rode along with the listing it already has (E10-S1). So the
  fix is a cached `StatFs` answer behind `mount_info()`, which is synchronous
  and therefore cannot make a round trip of its own.
- **It refreshes at most every 30 s rather than once per listing**, which is
  the alternative the AC allowed and asked to have stated: `list_dir` is also
  the walk inside a recursive delete, where a `StatFs` per directory would
  double the traffic of a delete to keep a figure nobody is reading. Thirty
  seconds is well inside the time a user spends in one directory.
- **A failed refresh keeps the previous answer.** A listing that worked must
  not fail because the number beside it could not be updated — and a stale
  capacity is a much smaller lie than a directory that would not open.
- Seeded at mount, so the *first* listing carries a figure rather than the
  second one. `capacity()` itself stays a live round trip and updates the cache
  on the way past, so `xcal df` still asks the server.
- Against orion the listing's mount facts now carry `{"free": 356355829760,
  "total": 491590631424}`, which `formatCapacity` renders as **`356 GB free of
  492 GB`** — the same bytes as the story's `333 GB free of 458 GB`, in the
  decimal units `formatSize` uses everywhere else in the app.

**Next steps**

- **SFTP, NFS and SMB take their capacity once at connect and never update it**,
  so a mount left open all day still shows the space it had when it was
  mounted. The shape built here — a cached answer with a TTL, refreshed where a
  listing already costs a round trip — transfers directly, but it belongs with
  each backend's own story rather than being smuggled in behind this one.
- Nothing invalidates the cache after a transfer completes, so a large upload
  is reflected on the next listing after the TTL rather than immediately. The
  transfer worker knows exactly when that happened and could say so.

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
- `foldersFirst` is a signal with no UI, default on. **Done** — E17-S1's sheet
  gives it a checkbox, and the value persists.
- `ROW_HEIGHT` is a constant in `Shell.tsx` that must match `--height-row`. Two places
  to keep in step; reading it from the token at mount would remove that, and is worth
  doing when `1g`/`1h` (**E8-S3/S4**) bring their own row heights.
- Keyboard access to the header (tab to a column, Enter to sort) is not wired —
  **E16-S2**'s accessibility pass.

#### E8-S3 — Layout `1g` name-led — **Done**

*2026-09-04.*

**AC**
- [x] `display: flex; gap: 9px; padding: 6px 10px`; glyphs 16×13 / 13×16; name
  12.5px; second line mono 10px `48.2 GB · mkv · 31 Aug 18:24`; soft selection
  with `#4a6ba8` metadata; names never truncate; no column resize.

**Results**

- **The soft selection needed dark-theme values, and the screenshot is how that
  was found.** `#e8effd` and `#4a6ba8` are the handoff's, and they are
  light-theme colours; defining them only in `:root` left the dark theme
  inheriting a pale blue fill under near-white text, and the selected row's
  **name vanished entirely** while its metadata line stayed. The name's colour
  is now a token chosen against the fill rather than inherited from the list.
- One `gridcell`, not four. The metadata line describes the name rather than
  filling a column anyone can sort or align to, so the row is one cell and
  `aria-colcount` is 1 — the a11y tree says what the layout actually is instead
  of pretending every variant is the same grid.
- `extensionOf` reads the terse token the second line wants (`mkv`) rather than
  the Kind column's prose ("Matroska Video"), and treats a leading dot as a
  hidden file rather than an extension — `.DS_Store` has none.

#### E8-S4 — Layout `1h` mono grid — **Done**

*2026-09-04.*

**AC**
- [x] Header mono 10px uppercase `.05em`, 22px rows, all mono 11px; leading `d`
  in `#4f86ff` / dim `-`; sizes `48.2G`/`2.0K`; dates `MM-DD HH:MM`.
- [x] `rowLayout` persisted (E17-S1) and switchable.
- [~] "via View menu and a shortcut". **The shortcut and a preferences picker**;
  there is no menu bar in this application to add a View menu to, and adding
  one for three items would be a larger liberty than this. Recorded in
  [`docs/design-gaps.md`](docs/design-gaps.md).

**Results**

- **⌘1/⌘2/⌘3 are bound now, and were deliberately not bound in E16-S1** — until
  `1g` and `1h` existed, two of the three would have done nothing visible,
  which is the promise E3-S1 refused to make. They come from the same table
  that labels the toolbar, so the preferences picker reads "Columns (⌘1)"
  without anyone writing that string twice.
- **The mono cells set their own colour, so they had to hand it back when
  selected.** `.rowSelected` colours the row, but an explicit `color` on the
  size and date cells overrode it and left them muted grey on the accent fill —
  the least readable thing on screen, on the one row the user had picked.
- `formatSizeTerse` caps at five characters (`48.2G`, `999K`, `1.0T`), because
  the layout aligns by character rather than by pixel and a six-character size
  breaks the column. There is a test over the range rather than a few examples.

**Next steps**

- The row-layout tokens `--soft-selection-*` and `--mono-*` are additions to the
  palette, not values the handoff specified for both themes. The dark ones are
  reasoned rather than drawn.

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
- `foldersFirst` is a signal with no control, default on. **Done** in E17-S1.
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

#### E8-S9 — Live refresh — **Partly done**

*2026-09-04.*

**AC**
- [x] Cmd/Ctrl+R refreshes.
- [x] Coalesced to at most one relist per 250 ms; an overflow triggers a full
  relist. Built and tested as `state/refresh.ts`.
- [ ] Refreshes on `watch` events. **Deferred**: no backend emits any.
  `Caps.watch` reads xsync's `NOTIFY` support bit, which no server sets — the
  same reason E7-S2 left it. Building the subscription with nothing to publish
  to it would be dead code.

**Results**

- **The coalescer is useful now rather than only when `watch` lands.** Several
  things already ask for a relist at once — a mutation finished, a transfer
  landed, a reconnect republished the mount — and deleting twenty files gave
  twenty reasons to relist and one result worth showing. Six call sites now go
  through it.
- **Leading *and* trailing.** A leading-only throttle drops the last change of a
  burst and leaves the list wrong until something else happens to refresh it; a
  trailing-only one delays a single change by a quarter second for nothing. A
  test pins each half.
- **⌘R stays uncoalesced.** The user asked for it now, and a quarter-second
  pause would read as the key not having worked. Navigation is likewise
  excluded: that is a *different* directory, and delaying it would leave the
  previous one's rows on screen under the new path.
- Overflow cancels a pending relist rather than queueing behind it — the point
  of overflow is that the pending one's result is already known to be stale.
- The clock is injected, so the tests drive time rather than wait for it.

#### E8-S10 — The `..` row — **Done**

*2026-09-05. Asked for directly: "in the files view there should be a '..' entry to
go to the parent folder". The handoff draws no such row — recorded in
`docs/design-gaps.md`.*

**AC**
- [x] A `..` row at the top of the list in every folder below the share root, and
  **not** at the root, where there is nowhere above to go.
- [x] Double-click ascends, the same gesture that opens any other folder row.
- [x] It is not an entry: it has no path, is not selectable, is not counted by the
  status bar, and cannot be sorted, filtered, downloaded, renamed or deleted.
- [x] It survives the virtual window: indices, the rubber band, type-to-select's
  scroll and `aria-rowindex` all still name the right row.
- [x] Drawn in all three row layouts at that layout's row height.

**Results**

- **It is drawn beside the listing, not inserted into it.** The alternative — a
  synthetic `Entry` in the array the window already has — would have had to be
  excluded from the selection, the sort, the filter, the status bar's count, the
  rubber band, Download, Delete and the drag-and-drop hit test, one at a time, and
  the first exclusion anyone forgot would be a real operation aimed at a row that
  is not a file. Instead the row has no `data-path`, which is the attribute every
  one of those paths keys on, so none of them can see it without being told to.
- **The cost is one row of offset**, and it is expressed once: `leadRows()` is
  0 or 1, and the spacer's height, the drawn window's transform, the reveal
  scroll, the band's origin and `aria-rowindex` each add it. A test breaks the
  `aria-rowindex` term deliberately to prove the offset is actually asserted —
  an off-by-one there aims a rubber band or a screen reader one row wrong, which
  is exactly the kind of bug that survives a screenshot review.
- **Its absence is what says "you are at the root".** `FileList` takes an
  optional `onAscend` and draws the row only when it has one; `App` passes
  `nav.canAscend(path()) ? ascend : undefined` — the same predicate the toolbar's
  `↑` reads. There is no second piece of state that could come to disagree.
- **Double-click, not single.** Single-clicking it does nothing, which is what
  clicking past the last row already does. A single click that ascended would
  make the second click of an ordinary double-click land on whichever row of the
  *parent* listing happened to be under the pointer, and select it.
- **No hover or selected state.** No row in this list has one, and giving the
  only non-file row the only pointer response would misdescribe it.
- Verified from computed style on the rendered page, not from the stylesheet: the
  row is exactly one row tall, its name cell starts at the same x as every other
  row's, the spacer is one row taller than the entries, and it scrolls with the
  content rather than pinning to the viewport.

**Next steps**

- Keyboard ascent is still the toolbar's `↑` and its shortcut; the row itself is
  a pointer affordance and is not in the tab order. Making it focusable would put
  a tab stop in the middle of the rowgroup, and arrow-key navigation through the
  list is the selection's, which this row is deliberately outside of.
- Dropping a dragged file **on** the `..` row uploads into the current folder,
  not the parent — the row has no `data-path`, so the hit test reads it as list
  background. That is the safe reading of an ambiguous gesture, but it is a
  guess; the alternative wants a real decision.

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

- **Regression found and fixed 2026-09-05, while running the suite against the current
  `xs`.** The third save of one file failed with `staged file does not match its digest`.
  A refused compare-and-swap left its stage open on the server (the client's `Stage`
  has no `Drop` that aborts), and `StageOpen` with an empty resume token adopts any
  stage for that destination — so the next save *resumed* the refused one, wrote a
  shorter edit over longer stale bytes, and failed its own digest. The write path now
  aborts a refused or failed stage by its token, and opens a whole-file replacement on
  a fresh stage (aborting any it finds with bytes in it). The test asserts no
  `.xsync.tmp.*` survives a refusal. Nothing in xsync changed; the assumption that a
  dropped handle was an aborted stage was Excalibur's.

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
- [x] Concurrency as a *preference*. *(E17-S1. Raising applies at once;
  lowering applies as running transfers finish, because a permit already held
  cannot be recalled without stopping work nobody asked to stop.)*

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

#### E13-S4 — Download / Upload verbs — **Done**

*2026-09-03; the recursive walk 2026-09-05.*

**AC**
- [x] `Download` writes the selection to the configured download folder; `Upload` opens a
  picker targeting the cwd; one queue row per file.
- [x] …*recursively*. A folder now expands to one row per file at
  `<dest>/<folder>/<relative path>`, with the directories — including the empty ones —
  created before any row runs.
- [ ] …with folder rows **aggregating**. Refused rather than unbuilt: a single row for a
  directory can only report a percentage that means nothing once one file in it fails,
  and the row that says `31 of 402 files` is a *view* over rows the queue already has.
  It belongs with grouping in the transfer sheet, not with the walk.

**Results**

- **The size comes from a fresh `stat`, not the listing the window had.** A progress bar
  built on a number that has since changed is worse than one that admits it does not know.
- The picker is `tauri-plugin-dialog`, granted **`dialog:allow-open` only** — never save
  or ask. The webview's own file input yields `File` objects and the engine needs paths,
  which is the whole reason the plugin is there; the capability file says so.

*The walk, 2026-09-05.*

- **The walk is in the engine, not in the command.** `transfer::plan::plan_downloads`
  takes a `&dyn ShareBackend` and a destination and returns files, directories and
  problems; `enqueue_downloads` turns that into rows. That is what let it be tested
  against a rigged backend — an unreadable subdirectory and a listing that loops are
  three lines to arrange in a test and an afternoon to arrange against a real server.
- **Nothing is skipped silently, and that is the whole story.** A subdirectory that
  cannot be listed becomes a **failed row** carrying the server's own words, not one
  fewer file. A copy that quietly omits what it could not read is the worst outcome a
  file transfer has, because it looks exactly like a copy that worked.
- **A name from a listing is data, not a path.** Every component is checked before it
  becomes part of a local path: `..`, `.`, an empty name, a separator or a NUL is
  refused and reported. A server answering a listing with `..` is describing a write
  outside the folder the user picked, and there is a test that asserts nothing escapes.
  This is the class of bug that makes a file browser a security problem rather than a
  buggy one.
- **Empty directories are created explicitly.** The worker creates a file's parent on its
  way past, which is exactly what an empty directory has nothing to trigger — so the plan
  names every directory and the command creates them first, parents before children.
- **A `stat` that fails still becomes a row.** Dropping it would be the silent skip; the
  row carries the real error from the worker, which is where the user is already looking.
- A directory symlink is **not** followed as a directory. It is copied as the bytes it
  points at, like any other read. Following it would let a link to `/` walk a server's
  whole filesystem into somebody's Downloads folder.
- The walk is iterative and keeps a visited set, so a listing that names a directory
  already walked is reported as a loop rather than followed forever. There is a backstop
  at 100k directories for the endless tree the visited set cannot see.
- Verified against the real share on `orion.local`: `xcal get excalibur-bench/tree` and
  the server's own `find` produce the same tree, empty directories included.

**Next steps**

- **Conflicts are not checked for the expanded rows.** `check_conflicts` still runs over
  the paths the window selected, so a folder download that would overwrite an existing
  local tree is not prompted about. The prompt's "apply to all remaining" scope (E13-S6)
  and this are the same conversation and should be had once.
- **A folder is walked before anything moves.** For a share with a hundred thousand files
  that is a visible pause with no feedback. Streaming the rows into the queue as they are
  found is the fix, and it needs the sheet to cope with a queue that is still growing.
- Uploads still take a list of files. The picker can select a directory; the walk for the
  local side is the mirror of this one and is not written.

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

#### E13-S7 — Streaming reads for transfers — **Done**

*2026-09-05.*

*Measured 2026-09-05: a 1 GiB download from `root@orion.local` (Raspberry Pi 5,
gigabit) took 48.5 s in the app (≈22 MB/s) against 12.6 s for
`ssh orion cat file > /dev/null` (≈85 MB/s). Debug build; the release number is
recorded below.*

The worker downloads through `ShareBackend::read_range`, one 1 MiB chunk at a
time, and the xsync implementation opens, reads and closes the file on every
call — three round trips per chunk, serially. SFTP's `read_range` pipelines
inside one call but still opens per call. The seam is right for previews and
media seeking; it is the wrong shape for a transfer.

**AC**
- [x] `ShareBackend` gains an `open_read(path) -> Box<dyn ReadHandle>` (name and
  shape are the implementer's, but: one open per transfer, `read_at(offset,
  length)` callable concurrently, an explicit close). A default implementation
  wraps `read_range` so SFTP, NFS and SMB keep working unchanged; xsync
  implements it over one `OpenFile` with `read_verified`.
- [x] The worker keeps N reads in flight per download (N configurable, default 4–8,
  chunk 1 MiB or the mount's `max_read` if smaller) and writes them in order;
  pause/cancel/resume semantics and the `.part` contract are unchanged, with
  the existing tests still passing.
- [x] A 1 GiB download from orion in a **release** build reaches ≥ 70% of the raw
  `ssh cat` figure measured the same day, recorded in `docs/benchmarks.md`
  with the command used. **≈101%**, not 70.
- [x] Quick Look's `excalibur-stream://` handler is left on `read_range` (a seek is
  a range, not a stream) — say so in a comment rather than changing it.

**Results**

- **The seam was the whole bug.** `read_range` opens, reads and closes on every call:
  three serial round trips to move one megabyte, and the link idle between each of them.
  Nothing about the chunk size fixes that, which is why the story is a new method rather
  than a bigger constant. One 1 GiB file went from **27.6 MB/s to 66.1 MB/s** in the same
  release build — and the 66 is now **≈101% of raw `ssh cat` on the same link,
  interleaved run for run**, with every chunk BLAKE3-verified end to end, which `ssh cat`
  does not do. The transfer is no longer round-trip bound; it is bound by what `ssh`
  itself is bound by on a Pi 5.
- **The 85 MB/s baseline in the story header did not reproduce.** Six `ssh cat` runs the
  same morning read 99.2, 65.2, 61.9, 58.5, 62.1 and 64.1 MB/s. Taking the fastest as
  *the* baseline would have been choosing a number rather than measuring one, so the
  comparison in `docs/benchmarks.md` alternates one run of each and quotes the means.
  Had the 85 stood, the bar would still have been cleared — but only just, and by luck.
- **The default is 4, not 8.** The sweep flattens hard: 27.6 / 43.5 / 66.1 / 61.5 / 62.7 /
  59.9 MB/s at depths 1, 2, 4, 6, 8, 16. Past the plateau depth buys nothing and costs a
  megabyte of buffer per read in flight per running transfer, plus a longer tail on a
  cancel — which finishes the window it has already asked for rather than abandoning it.
- **The default implementation lives in `backend/mod.rs` and wraps `read_range` honestly.**
  It is not a fast path: every call still opens and closes underneath. What it buys is one
  shape at the call site, so the worker has a single code path and SFTP, NFS and SMB
  behave exactly as they did. The trait method returns `Option`, and the free function
  `backend::read_handle` does the wrapping — because a default body holding `&self`
  cannot hand back a `'static` handle, and `'static` is what lets the worker spawn the
  reads.
- **Reads are scheduled ahead and written strictly in order.** The window is a queue and
  its front is awaited before anything behind it is looked at. Writing chunks as they
  arrived would need seeks and would leave a `.part` with holes — which is exactly the
  file the resume rule cannot tell from a good one. The test uses a payload whose every
  byte depends on its offset, because a file of identical bytes cannot tell a correct
  copy from a shuffled one.
- **A short read is the end of the file, and that is a contract now.** Every backend in
  the tree already fills a read that is not at the end — SMB and NFS loop internally to
  make it so — so the trait says it out loud and the worker stops there.
- **One existing test had to change, and it was measuring the wrong thing.** The
  cancel-mid-flight test relied on eight 1 MiB chunks taking longer than 80 ms *because
  they were serial*. They are not any more, so the mock's latency went from 40 ms to
  200 ms and the comment now says why. The property it asserts is untouched.
- **A download without a size now asks for one.** Scheduling ahead needs a total, so a row
  enqueued with `bytes_total == 0` stats the file and tells the queue — which also gives
  it a real progress bar instead of an indeterminate one.
- `xcal get REMOTE LOCAL` drives the queue, the worker and the streaming path headlessly,
  so the benchmark measures the app rather than something shaped like it. It takes
  `--depth`, which is how the sweep was run, and it walks a directory through E13-S4's
  planner.

**Next steps**

- **Many small files are untouched by this.** The depth is *inside* a file; a thousand
  1 MB files still pay one open each, serially, three transfers at a time. Depth across
  transfers is the concurrency preference, and 3 was chosen for a serial reader.
- **Uploads are still sequential**, by design — a pipelined write that failed halfway
  would leave holes — but nothing has measured what that costs on this link.
- **SFTP, NFS and SMB keep the wrapper.** SFTP pipelines *inside* one `read_range` and
  still opens per call, so it has the most to gain from implementing `open_read`; that is
  a story of its own, with its own measurement.
- The read depth is not a preference. It is a `Worker` setter with no UI, like
  concurrency was before E17-S1, and it should join it there rather than get its own
  control.

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

#### E14-S2 — Context menu — **Done**

*2026-09-05.*
*Asked for directly 2026-09-05: "a context menu that has common operations such as
edit if its a text file, rename, move to, delete, and properties that work for a
selection of files/folders as well". The handoff draws no context menu — this and
the two stories below go in `docs/design-gaps.md`.*

**AC**
- [x] Right-click on a row opens a menu at the pointer. Right-clicking a row that is
  **not** in the current selection selects it first; right-clicking one that **is**
  keeps the selection, so the menu acts on all of it. Right-clicking the list
  background acts on the folder being shown.
- [x] Items, in this order, with separators: `Quick Look`, `Edit`, `Download`, —,
  `Rename`, `Move to…`, `Delete`, —, `Properties`.
- [ ] …and `Properties` on the background menu, for the folder being shown.
  **Not built:** Properties describes entries, and the folder on screen is not
  in its own listing — the window knows its name and nothing else about it, and
  the seam has no `stat` reaching the frontend. Inventing an entry for it would
  mean showing a size and a date that were never asked for. The background menu
  is `New Folder` and `Upload…`.
- [x] **`Edit` appears only for a single entry the engine calls editable** — the kind
  table's category, never the extension or the label — and `Rename` only for a
  single entry. Everything else works over a selection of any size.
- [x] **Every item is gated through `gate()`**, the same call the toolbar's verbs
  make. A refused item is disabled and carries the server's reason as its
  tooltip. `move` and `properties` join the `Affordance` union — `move` as a
  write, `properties` as a read — so the test that each affordance is in exactly
  one group covers them.
- [x] Escape closes it, a click anywhere else closes it, scrolling the list closes it,
  and it never opens off the edge of the window.
- [x] `role="menu"` with `role="menuitem"` children; ↑/↓ move, Enter/Space activate,
  Escape closes. Shift+F10 and the Menu key open it on the selection. *(Focus
  does not "return to the list": no row is focusable — the list is driven by a
  window-level key handler and a selection, not by DOM focus. Closing the menu
  hands the keyboard straight back to it.)*
- [x] Tests: the item list is built by a pure function from (entry under the pointer,
  selection, mount) and is tested as one — including that a read-only mount
  disables exactly the write items and no others.

**Results**

- **The menu is a third way to reach verbs that already had two, so it asks the
  same question they do.** Every item goes through `gate()`; nothing in the menu
  decides for itself whether a verb is allowed. The test that matters is a
  *partition* rather than a list — on a read-only mount, exactly the write
  affordances come back disabled and exactly the read ones do not — because the
  failure this design fears is not a wrong item today, it is a verb added later
  and sorted into the wrong half.
- **What the menu acts on is settled before it opens, and it is the selection.**
  Right-clicking a row outside the selection selects it first; right-clicking one
  inside it changes nothing. So `menuFor` never learns there was a pointer: it
  takes the selection and the mount, which is why the interesting cases are a
  pure-function test rather than a browser one.
- **The labels carry the count** — `Delete 3 items`, `Move 3 items to…` — because
  the one question a user has when they right-click with several rows highlighted
  is whether the verb means all of them or the one under the cursor. Verbs that
  cannot mean all of them say `Select one item` instead of silently doing
  nothing, which is what Enter-to-rename did before this.
- **A found bug, and the reason `editable` now exists.** `Edit` was offered for
  any single selection and `editFile` would happily have handed a 48 GB video to
  `$EDITOR`. Editability is now the engine's category — the same table
  `previewFor` reads, so "shows as text" and "edits as text" cannot drift — and
  it is in the permission `Context`, so the toolbar's Edit button is gated by the
  same value rather than by a second rule of its own.
- **With several selected the menu deliberately has no opinion about type.**
  Telling someone who selected three text files that "only text files can be
  edited" is both unhelpful and untrue; the reason is the count, and the count is
  what they can act on. A test pins that exact wording, because the first version
  got it wrong.
- **A real stacking bug, found by clicking rather than by reading.** The sheets
  the menu opens rendered *behind* the file list, buttons and all, showing
  through only in the gaps between columns. `dialog.module.css`'s `.scrim` had no
  `z-index`, and the list's rows sit inside a `transform`, which puts them in the
  same painting layer — so which won came down to tree order. The New Connection
  dialog happened to be mounted after the list and looked fine; anything mounted
  before it did not. `overlays.test.ts` now reads every component stylesheet and
  fails on a full-window overlay with no `z-index`; removing the fix makes it
  fail by name.
- Escape, a click anywhere else, a scroll, a resize and a window blur all close
  it. The scroll listener is in the capture phase, because the list scrolls in
  its own container and that event does not bubble — a menu still pointing at a
  row that has scrolled away is pointing at a different file.

**Next steps**

- **The toolbar still offers Edit and Quick Look for a multiple selection**,
  where they do nothing. The type half of that is fixed; the count half is the
  same `ONE_ONLY` rule the menu applies, and moving it into `gate()` would mean
  `Context` learning how many things are selected rather than merely whether any
  are. Worth doing, not done.
- No submenus, no icons, no "Open With". The design has no opinion on any of
  them, and this menu is already past the mocks.
- The Menu key and Shift+F10 open the menu beside the first selected row by
  reading that row's rectangle from the DOM — the one place the window measures
  its own list from outside the list. If a second caller ever needs it, it wants
  to be a callback out of `FileList` instead.

#### E14-S3 — Move to… — **Done**

*2026-09-05.*
**AC**
- [x] A destination picker over the same share: it lists **only directories**, starts
  in the folder the items are in, has a breadcrumb and a `..` row, and its primary
  button is `Move here`.
- [x] Refuses, with the reason on screen and the button disabled: the folder the items
  are already in (nothing to do), a folder being moved (into itself), and anything
  inside a folder being moved (into its own descendant).
- [x] Moves with `rename(from, to, replace: false)` per item, so a name that is taken
  is the **server's** refusal at the moment of the move rather than a check here
  that could race.
- [x] A partial failure is reported as one: how many moved, which one failed, and why.
  The listing is re-read afterwards either way.
- [x] Gated on `writable`, and refused at the seam by `writeGuarded` as every other
  write is.
- [x] The rules — which destinations are illegal and why — are a pure module with its
  own tests, not conditions inside the component.

**Results**

- **The rules are a module because the interesting case is three clicks deep.**
  `src/state/move.ts` is two functions: `illegalDestination` returns the sentence
  to put on screen or `null`, and `destinationOf` says where one item lands. A
  component test can only ask whether the button was disabled; it cannot ask
  *why*, and the case that actually damages a share — a folder moved inside
  itself — is reachable only by building a tree and walking down it.
- **The containment check is by segment, not by prefix, and a test proves it.**
  `"docs-old".startsWith("docs")` is true, so the obvious spelling refuses a
  perfectly ordinary move while claiming the folder is inside itself. Swapping
  the body for `startsWith` was tried on purpose: exactly one test fails, the one
  that names the trap.
- **Entries are checked whatever their `kind`.** Only a directory can really
  contain the destination, so the loop could have skipped files — but a rule that
  first decides what counts as a directory is a rule with a second way to be
  wrong.
- **The picker walks; it does not select.** The destination is always the folder
  the breadcrumb ends on — the one `Move here` names — so a row is a place to go
  and one click goes there. A selected row *and* a current folder would be two
  destinations on screen at once with the button naming only one of them. The
  list keeps a fixed height, because a sheet that resizes as you walk the tree
  moves `Move here` out from under the pointer between clicks.
- **A folder being moved is still listed, and refused by name** — `You cannot
  move “docs” into itself.` Hiding it would leave a hole where the user can see a
  folder ought to be. The refusal sits beside the button it disabled, with
  `aria-describedby` joining them: a disabled control and its reason a paragraph
  apart are two facts the user is left to connect.
- **The move is one `rename` per item with `replace: false`**, so a name already
  taken over there is the *server's* refusal at the moment of the move rather
  than a check here that a second writer could invalidate. It stops at the first
  failure instead of trying the rest: the failures that matter are properties of
  the destination, so the remaining items would fail identically, and a dozen
  identical errors are worse than one and a count of what did move.
- **The picker's rows carry `data-destination`, never `data-path`.** The sheet
  renders inside the window, and everything that hit-tests the listing — the
  selection, the drop target, the status bar's count, the `..` row's own tests —
  keys on `data-path`. A picker row with one would have joined the listing.
- Escape is taken in the capture phase and stopped there, so dismissing the sheet
  does not also clear the filter behind it.

**Next steps**

- **No keyboard navigation inside the picker.** Rows are buttons, so Tab and
  Enter work, but ↑/↓ do not walk the list and there is no type-to-select. In a
  folder of two hundred directories that is a lot of tabbing; it belongs with
  E16-S1's key table rather than as a second, private one.
- **No New Folder from inside the picker.** Moving into a folder that does not
  exist yet means cancelling, creating it, and starting again.
- **It lists folders the user may not be able to write to.** Per-directory
  permission is not in a listing, so a destination the server will refuse looks
  identical to one it will take. That is the same bargain the story makes for
  name collisions, and it is honest until `Attrs` carries an access hint.
- A mount that goes read-only *while* the picker is open still offers `Move
  here`; the refusal then comes from `writeGuarded` at the seam rather than from
  the button. True of every dialog in the window today.

#### E14-S4 — Properties — **Done (bar POSIX mode)**

*2026-09-05.*
*This takes "Get Info / POSIX mode inspector" out of E20's parking lot. It was
asked for directly, which is the sign-off that story was waiting for; the shape
is still ours to invent, so it is written up in `docs/design-gaps.md`.*

**AC**
- [x] One entry: name, kind label, size in bytes *and* in the list's own units,
  modified time in full, the share-relative path, and the symlink target where
  there is one.
- [x] Several: how many files and how many folders, the total size of the files, and
  the folder they are in. A folder's own contents are **not** walked to size it —
  that is a transfer's worth of work, and a number that appears after a pause is
  worse than one that is not offered.
- [x] Read-only: it opens on a read-only mount and offers no write.
- [x] The summary is a pure function over the entries, tested as one.

**Results**

- **The sheet asks the engine nothing.** Every field it shows arrived with the
  listing, so opening Properties on two hundred selected files costs one pure
  function and no round trips. That is not an optimisation — it is what makes the
  AC's "do not walk a folder" rule cheap to keep, because there was never a call
  site to be tempted by.
- **A folder's size is an absence, and the sheet says so in words.** The list
  shows `—` in the Size column and this shows the same `—`, followed by a
  sentence explaining that the contents were not measured. The failure avoided is
  not a crash, it is a plausible number: `0 B` on a folder reads as *empty*, and
  a user deciding whether to delete something would be reading a claim the app
  never checked.
- **The surprise was in the input, not the output.** Several backends report a
  non-zero `size` for a directory — the inode's own size, 4096 on most Linux
  exports — so summing `entry.size` gives a total that is wrong by an arbitrary
  amount and looks entirely believable. `summarise` skips directories rather than
  trusting the zero, and its test uses a folder of `size: 4096` precisely because
  the naive version passes every test written against the fixture, where folders
  happen to be zero.
- **A symlink counts as a file**, not because it is one, but because the list
  already treats it that way — it shows a size for every row that is not a
  directory. A third count would be a taxonomy the design nowhere draws.
- **`newestModified` skips entries with no mtime rather than coercing them to
  `0`.** The coercion is invisible in a "newest" answer and catastrophic in an
  "oldest" one, and it is how a file comes to claim it was modified in 1970.
- Read-only fell out rather than being built: the sheet has no `writable` prop
  because it contains no control but `Done`. Its test enumerates the buttons and
  asserts there are no inputs at all — aimed squarely at the plausible future
  edit of adding a rename field here, which would be a write verb with no gate in
  front of it.

**Next steps**

- **POSIX mode is still not shown**, which is the half of E20's "Get Info /
  POSIX mode inspector" that stays parked. `Entry.mode` is in the DTO, but a mode
  is only meaningful beside an owner and a group and the engine sends neither —
  `xsyncv3` deferred owner *names* rather than deciding anything about them.
  `0644` beside no owner is an inspector that cannot answer the question anyone
  opens it to ask.
- **No live refresh.** The sheet renders the entries it was handed; if the folder
  is re-listed underneath it, the numbers are the ones from the moment it opened.
- **Folder sizing has a natural home if it is ever wanted** — an explicit
  *Calculate* button that queues the walk in **E13**'s transfer queue, where a
  long job is already something you can watch and cancel.

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

#### E16-S1 — Shortcut map — **Done**

*2026-09-04.*

**AC**
- [x] Space Quick Look; Enter rename; Cmd/Ctrl+↓/↑ descend/ascend; Cmd/Ctrl+R
  refresh; Cmd/Ctrl+F filter; Escape filter→selection; Cmd/Ctrl+D download;
  Cmd/Ctrl+U upload; Cmd/Ctrl+E edit; Cmd/Ctrl+[ / ] back/forward;
  Cmd/Ctrl+Shift+N new folder; Cmd/Ctrl+, preferences; Cmd/Ctrl+Shift+T
  transfers.
- [x] Cmd/Ctrl+1/2/3 row layouts. *(Bound with E8-S4, once `1g` and `1h`
  existed. Until they did, two of the three would silently have done nothing —
  and it was indeed a three-line change when there was something to switch
  to.)*
- [x] Every toolbar verb's tooltip shows its shortcut; the map is one table
  that generates both the bindings and the tooltips.

**Results**

- **`state/shortcuts.ts` is the table, read twice** — once by the key handler to
  decide what a keystroke means, once by the toolbar to label itself. Two lists
  would drift, and a tooltip promising ⌘D while the handler listens for
  something else is worse than no tooltip, because the user believes it. The
  gear's tooltip had already been hardcoded as `Preferences (⌘,)` in E17-S1;
  that was exactly the drift, and it now comes from the table.
- **Modifiers match exactly rather than at least.** ⌘⇧N is New Folder and ⌘N
  must not fall through to it — the old if-chain tested `metaKey || ctrlKey`
  and then the letter, so it would have. A test asserts the near-misses, and
  the browser confirmed ⌘N creates nothing.
- **The table says what a key means; the caller says whether it is allowed.**
  Every verb still goes through the same permission gate the buttons use, so a
  shortcut on a read-only mount produces the same refusal rather than silently
  doing nothing — which would be indistinguishable from a broken binding.
- **Shift changes the reported key**, so `n` and `N` both have to match; the
  table lowercases before comparing, and a test pins it.
- Escape stays layered — panel, then filter, then selection — because clearing
  two states at once would make one key do two things. That is one `case` now
  rather than three scattered branches.
- Tooltips defer to the refusal reason when a button is disabled: teaching a
  shortcut for something the user cannot do would be worse than saying nothing.

#### E16-S2 — Accessibility — **Done**

*2026-09-04.*

**AC**
- [x] Focus rings visible (2px accent, offset) on every control.
- [x] List is an ARIA grid with row/cell roles.
- [x] Status bar is a live region.
- [~] Contrast of every token pair used for text meets 4.5:1, checked by a test
  over the token file, with the documented exceptions listed. **One exception,
  and it is a real one** — see below.

**Results**

- **The contrast test found three genuine failures, not hypotheticals.**
  `--text-placeholder` sat at 2.60:1 (light) and 2.86:1 (dark); `--glyph-file`
  at 2.26:1, below even the 3:1 floor WCAG sets for non-text. All three are
  fixed, and the file glyph stays lighter than the folder glyph so they remain
  distinguishable.
- **`--text-tertiary` is the documented exception, because the arithmetic does
  not allow anything else.** It fails on every light surface (2.96–3.43:1). The
  light value that would clear 4.5:1 is `#6c6c72` against secondary's
  `#6a6a70` — indistinguishable; in dark it would have to become *lighter* than
  secondary and invert the hierarchy. The design's three grey tiers and this
  threshold cannot both hold on these surfaces, which is a decision for a
  designer rather than for the code. It is still held to 3:1 by the test so it
  cannot quietly get worse. Recorded in
  [`docs/design-gaps.md`](docs/design-gaps.md).
- **The test reads the stylesheet rather than a copy of it.** Asserting against
  duplicated hex values would keep passing after someone edited the real ones,
  which is the only failure mode that matters. It parses both theme blocks,
  with dark inheriting what it does not redefine — the same way the browser
  resolves them.
- **The grid now contains its own header.** It did not: the column header was a
  sibling of the scrolling body, so no column name could be associated with a
  cell. The grid wraps both with `display: contents`, which gives the correct
  tree without moving anything on screen — verified, since a fixed header over
  a scrolling body is exactly the layout that would have broken.
- Focus rings are one global `:focus-visible` rule rather than one per
  component, because a control that forgets its ring is invisible to a keyboard
  and a rule per component is a rule someone forgets. Measured in the browser at
  `2px solid rgb(47, 109, 246)`, offset 2px.

**Next steps**

- Arrow-key row selection is still absent; the list is navigable by type-ahead
  and the mouse. It belongs with E8's row work rather than here.

### E17 — Preferences and persistence

#### E17-S1 — Stored state — **Done**

*2026-09-04.*

**AC**
- [x] `theme`, `rowLayout`, `sidebarCollapsed`, `showHidden`, `foldersFirst`,
  `downloadFolder`, `transferConcurrency`, `conflictDefault`, column widths —
  persisted and restored on launch.
- [x] Preferences UI is "not yet designed": a minimal native-feeling sheet with
  these fields only, flagged in
  [`docs/design-gaps.md`](docs/design-gaps.md).

**Results**

- **`prefs.ts` stayed the storage seam; `settings.ts` is the schema.** The split
  is deliberate: the store is shared with bookmarks and knows only keys and
  JSON, while the schema knows what a preference *is* — its type, its default,
  and what to do when the stored value is neither.
- **Validation is per field, not per record.** A preferences file is
  user-editable and outlives upgrades, so one bad value costs that value alone.
  Discarding the record would silently reset a user's entire setup because they
  mistyped one line. The column widths already worked this way; this applies it
  to everything, and a test pins it by corrupting three fields and asserting the
  fourth survives. `loadSettings` also survives a store that *throws* — a launch
  is not the moment to fail over an unreadable preference.
- **Concurrency is clamped rather than rejected.** Someone who edited the file
  to 40 meant "more", and the ceiling is the nearest honest answer; 0 would mean
  nothing ever runs, which is a hung queue rather than a setting.
- **The choice and the painted theme are different values.** They diverge
  exactly when the choice is `system`, and conflating them loses the ability to
  tell "the user picked light" from "the OS is light today". `system` follows
  `prefers-color-scheme` live, so a Mac flipping at sunset takes the window
  with it.
- **Lowering the transfer limit could not be done with the semaphore alone.**
  `forget_permits` only removes permits that are *free*, so a shrink while
  transfers are running is partial — and the rest would silently return when
  those transfers finished, undoing it. The worker now tracks what it still owes
  and pays it down at acquisition time. Recalling a permit already held would
  mean stopping work the user did not ask to stop, so the new limit takes effect
  as transfers finish. Three tests cover it, including that lowering mid-flight
  still runs every queued transfer — the failure mode of that design is a
  transfer that never starts.
- Only *changed* keys are written. A fresh profile has an empty store, so the
  defaults are the defaults rather than a snapshot of them.
- Verified in the browser: theme switched light, hidden files disappeared and
  the count fell 13 → 12, the sidebar collapsed, and all three survived a
  reload.

**Next steps**

- The sheet is the plainest thing that works, and the sidebar toggle's placement
  is a guess. Both are in `docs/design-gaps.md` awaiting sign-off.
- `rowLayout` persists but has no control until E8-S3/S4 give it something to
  choose between.

### E18 — Packaging and release

#### E18-S1 — macOS — **Partly done (a local build; everything signed is deferred)**

*2026-09-05.*

**AC**
- [x] App icon set. `pnpm tauri icon src-tauri/icons/icon.png` writes a twelve-member
  `icon.icns` (16 through 1024, `@1x` and `@2x`), `bundle.icon` names it, and the
  bundler copies it to `Contents/Resources/icon.icns`, points `CFBundleIconFile` at
  it, and reuses it as the DMG's volume icon.
- [x] A `.dmg`. `pnpm tauri build` produces
  `target/release/bundle/dmg/Excalibur_0.1.0_aarch64.dmg` (4.6 MB, the app plus an
  `/Applications` symlink) alongside a `Excalibur.app` that launches and stays up.
- [ ] **Universal.** *Deferred:* the build is `aarch64` only. A universal binary is
  `--target universal-apple-darwin` plus the `x86_64-apple-darwin` std, and nothing
  in the tree is architecture-specific — but there is no Intel Mac here, and a slice
  nobody has ever run is worse than one architecture honestly labelled.
- [ ] **Hardened runtime, and notarised.** *Deferred:* both need a Developer ID.
  The runtime flag itself is already reachable — `APPLE_SIGNING_IDENTITY=-` makes
  Tauri sign with `--options runtime` — but hardened runtime is a promise about who
  signed, and ad-hoc names nobody. Notarisation additionally needs a paid
  certificate and App Store Connect credentials, which a build cannot invent.
- [ ] **Auto-update via the Tauri updater with a signed manifest.** *Deferred:* the
  updater plugin wants a keypair, an endpoint serving `latest.json`, and a signed
  artifact to point at. That is E18-S3's release channel, and building the client
  half first would be a client for nothing.

**Results**

- **With no `APPLE_SIGNING_IDENTITY`, Tauri does not run `codesign` at all** — it
  does not ad-hoc sign the bundle, as one might assume from the fact that the built
  app is nonetheless *reported* as ad-hoc. What `codesign -dv` shows is
  `flags=0x20002(adhoc,linker-signed)`, `Info.plist=not bound`,
  `Sealed Resources=none`: that is the **linker's** signature, which every arm64
  Mach-O gets whether anyone asked or not. There is no `_CodeSignature` directory in
  the bundle.
- **So the bundle is not merely unsigned, it is inconsistently signed**, and
  Gatekeeper says so rather than saying "unidentified developer":
  `spctl --assess --type execute` and `codesign --verify --strict` both fail with
  *code has no resources but signature indicates they must be present*, and
  `syspolicy_check distribution` calls that a fatal notary error. This matters for
  the README's one-line workaround: **right-click → Open does not help**, because
  the assessment fails on a malformed signature rather than an unknown developer, so
  macOS offers no override. `xattr -dr com.apple.quarantine` is the workaround, and
  it is only needed for a copy that *arrived* somewhere — a locally built app carries
  no quarantine flag and Gatekeeper is never consulted for it.
- **`APPLE_SIGNING_IDENTITY=-` is a strictly better local artifact and is still not
  distributable.** It gets a sealed `_CodeSignature`, `flags=0x10002(adhoc,runtime)`
  — the hardened runtime, for free — and `codesign --verify` passes. `spctl` still
  answers `rejected`. Documented in the README as the opt-in it is, rather than made
  the default, because of the next point.
- **The signing path is booby-trapped by Homebrew.** Tauri runs `xattr -cr` on the
  bundle before signing; Homebrew's `xattr` does not implement `-r`, and when it is
  first on `PATH` the bundle step dies with `failed to run xattr` *after* a full
  release build. `PATH=/usr/bin:$PATH` in front of the command is the fix. Making
  ad-hoc signing the default would have handed that failure to everyone building
  here, for a signature that Gatekeeper rejects anyway.
- **The bundler generates its own `.icns` when handed only PNGs, and it is a poor
  one.** The pre-existing build's `Excalibur.icns` had six members —
  `il32/l8mk`, `it32/t8mk`, `ic13`, `ic09` — three of them the 1990s 32/128px
  formats, with no 256, no 1024, and nothing for the 16px sizes Finder uses in list
  view. Supplying a real `icon.icns` in `bundle.icon` replaces that wholesale.
- `pnpm tauri icon` also emits Windows Store logos and full Android and iOS asset
  catalogues. They are deleted: this ships macOS and Linux, and unused generated
  binaries in the tree are noise that will be stale before anyone needs them. The
  `.ico` is kept, since the decisions table says Windows stays buildable.
- CI gains a `package-macos` job that runs `pnpm tauri build --bundles app` and
  asserts the `.icns` reached `Contents/Resources`. It needs no secrets and signs
  nothing, deliberately: the bundle configuration is the one thing no other job
  exercises, and a `bundle.icon` that quietly stops naming the `.icns` produces a
  generic Dock icon that no test would ever notice.

**Next steps**

- **The artwork is still a flat blue square** — a 512px `#2f6bf1` fill with square
  corners, which is not what a macOS icon looks like. The icon *set* is correct now;
  the mark is E0-S3's, and this story deliberately did not invent one.
- Signing, notarisation, the universal build and the updater all wait on a Developer
  ID. Until then the `.dmg` is distributable only by hand, with the `xattr` line
  attached to it.
- E18-S2 (Linux `.AppImage`/`.deb`) is the other half of M5 and is untouched.

#### E18-S4 — Staging `xs` on a server — **Done**

*2026-09-05.*

The xsync backend needs `xs` on the remote's `PATH` (`connect_ssh` runs
`PATH="$HOME/.local/bin:$PATH" xs --server <root>`). The user's servers are
Linux (aarch64 and x86_64); the dev machine is a Mac with `cargo-zigbuild` and
the `aarch64-unknown-linux-gnu` / `x86_64-unknown-linux-gnu` targets installed.

**AC**
- [x] `scripts/stage-xs.sh [user@]host [--target <triple>]` cross-compiles `xs`
  from the sibling `../xsync` checkout (release, the host's triple detected
  over ssh with `uname -m` when not given) and installs it to
  `~/.local/bin/xs` on the host (or `/usr/local/bin` when the login is root),
  then prints `xs --version` from the remote as proof.
- [x] README gains a "Run it against your own server" section: build the app,
  stage `xs`, open New Connection → xsync, what to type in each field, what the
  host-key prompt means, and the two failure messages E7-S4 produces and what
  to do about each. *(The section describes the two failure **classes** —
  ssh refused the login, and ssh ran but could not find `xs` — with the command
  that distinguishes them. It does not quote E7-S4's strings, which were being
  written at the same time as this.)*
- [x] Verified against `root@orion.local` (Gentoo, aarch64, glibc 2.43).

**Results**

- The whole run, unedited, first against the default destination:

  ```text
  $ scripts/stage-xs.sh root@orion.local
  stage-xs.sh: probing root@orion.local
  stage-xs.sh: root@orion.local is aarch64, glibc 2.43, login root -> /usr/local/bin/xs
  stage-xs.sh: building xs for aarch64-unknown-linux-gnu.2.31 from /Users/sanjee/projects/xsync
     Compiling xsync v0.1.0 (/Users/sanjee/projects/xsync/crates/xsync)
      Finished `release` profile [optimized] target(s) in 1m 04s
  stage-xs.sh: built 6874799a35cd -> …/target/aarch64-unknown-linux-gnu/release/xs (6377560 bytes)
  stage-xs.sh: sending to root@orion.local:/usr/local/bin/xs

  xs 0.1.0 (6874799a35cd 2026-09-05) aarch64-unknown-linux-gnu
  commit:    6874799a35cd
  built:     2026-09-05
  target:    aarch64-unknown-linux-gnu
  protocol:  wire v2, browse v2
  features:  blake3, zstd, unix-permissions, non-utf8-paths

  xs on the backend's PATH: /root/.local/bin/xs
  stage-xs.sh: warning: that is not the copy just staged (/usr/local/bin/xs) — an older xs shadows it
  ```

- **That last warning is the reason the check exists, and it fired on the first
  real run.** `/usr/local/bin` is the obvious place to install for root, and it is
  the wrong one: the backend prepends `$HOME/.local/bin` to `PATH`, orion already
  had an `xs` there from an older commit, and a staging script that stopped at
  "installed successfully" would have left the app running a binary that was
  replaced. Re-running with `--dest '$HOME/.local/bin'` staged over the shadowing
  copy and printed the same version block with no warning.
- **The staged binary is executed on the host before it replaces anything.** It
  arrives as `.xs.staged.$$` next to its destination, is run there with `--version`,
  and only then `mv`s into place; a glibc or architecture mismatch fails with the
  previous `xs` untouched. Nothing is ever removed recursively — the single `rm -f`
  in the script targets the staged file it just wrote, on the failure path.
- **glibc is pinned to 2.31 by default, not left to the toolchain.** zig links
  against whatever it defaults to, which is newer than several of the machines this
  is aimed at, and the failure mode is a `GLIBC_2.xx not found` at exec time on the
  server rather than anything the build says. 2.31 is old enough for any host worth
  supporting; the script reads the host's own version with `getconf
  GNU_LIBC_VERSION` and refuses to install a binary that would need a newer one.
- **`--locked` on the cross build, because the xsync checkout is not ours.** Only
  `target/` is written; the sibling repo's `Cargo.lock` is left exactly as it was.
- An architecture that is neither aarch64 nor x86_64 stops the script before it
  builds anything, and says to build on the host instead. Every ssh call carries
  `BatchMode=yes`, so a host that wants a password fails in ten seconds rather than
  hanging on a prompt nobody will see.
- The round trip from this Mac, headless, after staging:

  ```text
  $ cargo run -q --bin xcal -- ls --xsync-ssh orion.local:/srv/strife --user root
  ssh[orion.local]: [xsync server] process started: pid=416148, root=/srv/strife, read_only=false, attr_cache_ms=1000, dir_cache_ms=30000
  ssh[orion.local]: [xsync server] waiting for client handshake
  ssh[orion.local]: [xsync server] received handshake: frame_id=1, role=Session, capabilities=0xcc
  ssh[orion.local]: [xsync server] v3 features: client=0x0, server=0x3900, common=0x0
  nvme                                   4096 Folder
  app                                    4096 Folder
  ext                                    4096 Folder
  postgres                               4096 Folder
  cache                                  4096 Folder
  excalibur-bench                        4096 Folder

  RW · 6 items · read & write
  ```

  The `ssh[orion.local]:` lines are E7-S4's stderr drain, which landed while this
  story was being written; before it, those server lines went nowhere.

**Next steps**

- The script installs; it does not *uninstall*, and it does not notice that the
  staged commit differs from the one the desktop app was built against. A version
  skew check belongs with E7-S1 (reduced mode), which is where protocol mismatch is
  already being thought about.
- x86_64 is implemented but untested — there is no x86_64 Linux host here. The only
  thing that differs is the triple, but that is not the same as having run it.
- Nothing stages `xs` for the user automatically from the app. It should stay that
  way: a share browser that copies binaries onto servers is a different, larger
  promise than the one on the tin.

#### E18-S5 — The macOS local network permission — **Done**

*2026-09-05. Found by running the bundled app rather than the dev build.*

Both saved bookmarks showed **unreachable** in the sidebar while `nc` reached
one of the hosts in 25 ms and the engine's own probe, run from a terminal,
agreed with `nc`. The same code in the same build disagreed with itself
depending on how it was launched.

macOS 15 and later gate *every* connection to a host on the local network
behind a per-app permission, and an app whose `Info.plist` does not say why it
wants one is refused — with no prompt and no error, the connection simply never
completes. A binary run from a terminal inherits the terminal's grant, which is
why every test up to this point had passed.

**AC**
- [x] `src-tauri/Info.plist` declares `NSLocalNetworkUsageDescription`, and
  Tauri merges it into the bundle. Verified with `PlistBuddy` against the built
  `Excalibur.app`, not against the source file.
- [x] The bundled app shows a reachable LAN host as reachable.

**Results**

- **The sidebar's dot was the symptom, not the bug.** The reachability probe is
  the only thing that connects to a LAN address *in process* on the xsync path —
  the mount itself runs `ssh` as a child process, which has its own permission
  and was never blocked. That is exactly why this survived a full session of
  end-to-end testing against a real server: the feature that mattered worked,
  and the thing that broke was a grey dot nobody had reason to distrust.
- **SFTP, NFS and SMB were the real casualties, and are untested here.** All
  three open their sockets in the app's own process. On any macOS 15+ machine
  this build would have failed to connect to a share on the user's own network,
  and reported it as a connection failure rather than as a permission the user
  could grant. The xsync backend's use of a child `ssh` is what hid it.
- **The declaration was enough; no prompt appeared.** The permission was granted
  on the strength of the usage string. The dot for the live host went normal on
  the next launch and the share mounted, listed and reported `10.2 TB free of
  19.6 TB`.
- The story that would have caught this earlier is E19-S3 (end to end): every
  check up to now ran either in a browser or from a terminal, and neither is
  the environment the app ships in.

**Next steps**

- **Linux has the same shape of problem waiting** in Flatpak and Snap
  confinement, where network access is a manifest permission. E18-S2 packages
  a `.deb` and an AppImage, which are unconfined, so it does not bite yet.
- A user who *denies* the permission gets the same silent failure this fixed.
  The window cannot tell that case apart from a host being down — the probe just
  times out — so a share that will not connect on macOS should probably say
  "check Local Network in System Settings" somewhere. Not built; it needs a way
  to distinguish the two, and macOS does not offer one directly.

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

### E21 — Local filesystem *(asked for directly 2026-09-06: "browsing the local mac filesystem")*

The handoff draws four network protocols and nothing else. A directory on this machine
goes in behind the same `ShareBackend` seam rather than as a special mode of the window:
the listing, the verbs, the transfer worker and the writability rule are all already
written against that trait, and a local backend is the smallest possible implementation
of it. It is also what makes a local↔share transfer a *copy between two backends* later
(E13) rather than a third code path. Recorded in `docs/design-gaps.md`, since the
dialog gains a tab the mock does not draw.

#### E21-S1 — Local filesystem backend
**AC**
- `crates/engine/src/backend/local.rs`: `LocalBackend::open(root: &Path, read_only:
  bool) -> Result<LocalBackend>` implementing every `ShareBackend` method over the
  standard library: `list_dir`, `stat`, `read_range`, `open_read` (one `File`, positional
  reads, concurrent — the E13-S7 handle), `write_range`, `mkdir`, `rename`,
  `remove`, `capacity`, `ping`, `mount_info`, `caps` (`random_access`,
  `resume_write`, `atomic_rename` true; `symlinks` true; `watch` false).
- **Confinement:** share-relative paths are resolved under `root` and anything that
  would escape it — `..`, an absolute path, a NUL — is refused as `NotFound`, never
  served. A symlink *inside* the root that points outside is listed as a symlink with
  its target and may be followed on open: this is the user's own machine, and Finder
  follows them; the confinement is about the path *model*, not a privilege boundary.
- **Writability is probed, not assumed:** `writable` is true only when a temp file can
  be created and removed in `root` (removed on every exit path); `read_only` mounts
  refuse every write at the backend with `ReadOnly` and a reason, exactly as the
  network backends do. The reason names the cause (`mounted read-only` vs `no write
  permission on <root>`).
- **Kinds and hiddenness use the shared rules:** `classify_entry` from `kind.rs`; an
  entry is `hidden` when its name starts with `.` **or**, on macOS, when the
  `UF_HIDDEN` flag is set (`~/Library` is flagged, not dotted) — read through
  `std::os::macos::fs::MetadataExt`, no unsafe.
- **`remove` moves to the Trash** on platforms that have one (the `trash` crate),
  and falls back to a real delete only where there is none, saying which in the mount's
  `options`. Deleting a file on your own Mac must not be more permanent than Finder
  makes it. The dependency is recorded in `docs/supply-chain.md` and passes `cargo deny`.
- `capacity` from `statvfs` via a safe crate already in the tree if there is one, else
  `fs4` (record it). `security_label` is `local`; `protocol_label` is `local`.
- `xcal --local PATH` drives it headlessly.
- Tests in `crates/engine/tests/local.rs` against a `tempdir`: listing, stat, range
  read, the read handle's concurrent reads, write, mkdir/rename/remove (asserting the
  file left the root — and, on macOS, that it is in the Trash rather than gone),
  capacity, the read-only refusal, the write-probe on an unwritable root, every
  confinement case, hidden-by-flag on macOS, and a symlink pointing outside the root.

#### E21-S2 — "Local" in the dialog and the window
**AC**
- The New Connection dialog gains a **Local** tab: one field, *Folder*, with a
  `Choose…` button that opens the OS folder picker (`tauri-plugin-dialog` `open`
  with `directory: true` — already granted `dialog:allow-open`), plus the Mount-as
  radio. No host, port, username or password; the URI preview reads `file:///…`.
  The field set is decided in `state/connection.ts` beside the other protocols'.
- `ProtocolConnector` routes `Protocol::Local` to `LocalBackend::open`; `test_connection`
  reports the folder's real writability; `supported_protocols` lists `local`.
- A local bookmark is never *probed*: reachability is not a question a folder answers,
  so the row is never dimmed and no TCP connect is attempted for it.
- Chrome: the title reads `<folder name> — This Mac` (or the machine's hostname on
  Linux), the breadcrumb root is the chosen folder, the status bar shows `local`
  where a network protocol would show `xsync v3 · ssh`. The bookmark id derives from
  the path, not a host.
- Download from a local share to the download folder is an ordinary transfer; upload
  into it is an ordinary upload. Neither is special-cased.
- Tests: the field set, the URI, validation (a relative path is refused, a missing
  `Folder` is refused), the id, that the probe is skipped, and the chrome naming.

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
