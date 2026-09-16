# Excalibur — a map for people and agents

Where things are and which file to open. Written to be read *before* grepping:
most questions here are answered by one file, and the point of this document is
to say which.

Planning lives elsewhere — `backlog.md` is unfinished work, `integration.md` is
cross-project work, `twopane.md` is the two-pane mode, `docs/design-gaps.md`
records every deliberate departure from the design handoff, and `legacy/` is
frozen history you should read but never edit.

---

## The shape of it

The native port lives in `crates/gpui`: its views call the Rust engine through
the GPUI model and service layers directly. The Tauri/SolidJS path below remains
the parity reference during the migration.

```
                    ┌─────────────────────────────────────────┐
   SolidJS webview  │ src/                                    │
                    │   App.tsx        the window's state      │
                    │   components/    what it draws           │
                    │   app/           state extracted from App│
                    │   state/         pure logic, no DOM      │
                    └───────────────┬─────────────────────────┘
                                    │  invoke() by command name
                                    │  listen() by event name
                    ┌───────────────▼─────────────────────────┐
   Tauri shell      │ src-tauri/src/                          │
                    │   commands/  one module per area         │
                    │   stream, dragging, menu, events, state  │
                    └───────────────┬─────────────────────────┘
                                    │  Rust calls
                    ┌───────────────▼─────────────────────────┐
   Engine           │ crates/engine/   Tauri-free              │
                    │   backend/   one module per protocol     │
                    │   transfer/  the queue and the worker    │
                    └───────────────┬─────────────────────────┘
                                    │  path deps
                    ┌───────────────▼─────────────────────────┐
   xsync            │ ../../xsync/crates/{client,core,…}       │
                    └─────────────────────────────────────────┘
```

**The engine is Tauri-free on purpose.** `crates/engine` has no `tauri`
dependency, so its tests run without a webview and `crates/cli` (`xcal`) drives
the same code from a terminal. Anything that needs a window handle
belongs in the active shell (`crates/gpui` or `src-tauri`), not here.

The GPUI port keeps the same engine boundary but removes the webview boundary:
`crates/gpui` calls the engine through a Tokio-backed native service facade.
`browser.rs` owns pure sort/filter/selection/history rules, `model.rs` protects
async listings with generation tokens, and `connector.rs` carries the existing
credential and host-key policy without depending on Tauri. During migration,
`src-tauri` and `src/` remain the behavioural reference for surfaces not yet
ported.

**The path dependencies only resolve when the siblings sit next to each other.**
`crates/engine/Cargo.toml` points at `../../../xsync/crates/*`. A standalone
checkout of this repo cannot build; it needs the `xcalibur/` superproject.

---

## Three seams worth knowing before you change anything

Almost every bug of consequence in this codebase has been at one of these.

### 1. `ShareBackend` — `crates/engine/src/backend/mod.rs`

One trait, six implementations: `local`, `xsync`, `smb`, `nfs`, `sftp`, `mock`.
Everything above it — the listing, the verbs, the transfer worker, the
writability rule — is written against the trait, never against a protocol. A new
protocol is a new module here and nothing else.

`list_dir` returns `Listing { entries, mount }`: the mount facts travel *with*
the listing so the status bar cannot drift from what is on screen.

### 2. `Backend` — `src/state/backend/`

The frontend's single door to Rust. Three implementations chosen at runtime by
`defaultBackend()`:

| File | What it is |
|---|---|
| `types.ts` | the interface, and the values that cross it |
| `tauri.ts` | the real one — `invoke` by command name, `listen` by event name |
| `fixture.ts` | the in-browser preview and every component test |
| `guard.ts` | `writeGuarded` — refuses writes on a read-only mount *before* IPC |

**Never call `invoke` from a component.** Going around this seam is how a write
reaches a read-only share, and how a test stops being able to run headless.

`tauri.events.test.ts` beside it is load-bearing: it reads `tauri.ts` as text and
asserts that window-addressed events are subscribed with `listenHere`, not
`listen`. One drag once started two uploads because that rule was broken
invisibly.

### 3. Command and event names are strings

`invoke("list_dir")` and `listen("transfer://progress")` are matched by string
across the language boundary. Renaming either side compiles fine and breaks the
app at runtime. The names are registered in `src-tauri/src/lib.rs`'s
`generate_handler!` list.

---

## Where to look, by question

| Question | File |
|---|---|
| How is a directory listed? | `crates/engine/src/backend/<protocol>.rs`, `list_dir` |
| Why is a verb disabled? | `src/state/permissions.ts` — one `gate`, every affordance |
| What does a keyboard shortcut do? | `src/state/shortcuts.ts`, then `App.tsx`'s keydown switch |
| How are columns sized and dropped? | `src/state/columns.ts` |
| How is a listing sorted or filtered? | `src/state/sort.ts`, `src/state/filter.ts` |
| How does a transfer actually run? | `crates/engine/src/transfer/worker.rs` |
| Which route does a transfer take? | `transfer/xsync_engine.rs` (xsync), `transfer/endpoint.rs` (everything else), `transfer/http.rs` (URLs) |
| How does the queue survive a restart? | `transfer/queue.rs` — JSON-persisted |
| Where do downloaded files go? | `src-tauri` `download_folder`, then `plan::local_name` for the name |
| How does drag-and-drop decide a target? | `src/state/dnd.ts` for the rules, `App.tsx` for the wiring |
| What does the preview stream through? | `excalibur-stream://` in `src-tauri`, `src/state/preview.ts` in the window — **file bytes never cross IPC** |
| How are credentials stored? | `crates/engine/src/secrets/` — OS keychain, never app state or disk |
| How is an SSH host key trusted? | `crates/engine/src/hostkeys/` |

---

## `src-tauri/src/` — the shell

The only crate that knows about windows. Everything here is either a command the
webview can call, or plumbing that needs an `AppHandle`.

| File | What lives there |
|---|---|
| `lib.rs` | `run()`, the builder, and the `generate_handler!` list — **the registry of every command name** |
| `commands/mounts.rs` | connect, browse, unmount, writability |
| `commands/fs.rs` | mkdir, rename, remove, conflict checks |
| `commands/transfers.rs` | the queue and named queues — the biggest, 26 commands |
| `commands/secrets.rs` | keychain and SSH host keys |
| `commands/edits.rs` | check out, watch, write back |
| `commands/windows.rs` | ⌘N, titles, and starting a native drag |
| `events.rs` | **every event name, as a constant** — no literal is spelled by hand elsewhere |
| `stream.rs` | the `excalibur-stream://` handler and its range requests |
| `dragging.rs` | file promises and the staging directory behind a drag to Finder |
| `menu.rs` | the macOS menu, and the AppKit handoff for the Window list |
| `state.rs` | `AppState`, and how a window finds its mount |
| `connector.rs` | the per-protocol `connect` match — where a new backend gets wired in |

Adding a command means: write it in the right `commands/` module, and add its
name to `generate_handler!` in `lib.rs`. Forgetting the second is a runtime
error in the webview and nothing at compile time.

---

## `src/state/` — pure logic, and the reason it exists

Every module here is a pure function library with a sibling `.test.ts`. No DOM,
no `invoke`, no signals. That is why there are ~600 frontend tests that run in
milliseconds: the hard parts are testable without rendering anything.

If you are adding logic and it does not need the DOM, it goes here and it gets a
test. `paneSize.ts` is a recent example of the shape.

`types.ts` is the shared vocabulary — `Entry`, `Transfer`, `MountInfo`,
`SortColumn`. Rust's `serde(rename_all = "camelCase")` structs must agree with it
field for field; there is no generated binding, so the two are kept in step by
hand and by the tests that would fail if they drifted.

## `src/app/` — state lifted out of `App.tsx`

`App.tsx` is the window: it owns the signals and hands them down. It is still the
largest file here, and the modules in `src/app/` are the pieces that have been
pulled out of it — each a factory taking an explicit `deps` object, so what it
touches is written down rather than implied by closure.

`queues.ts` (the app-wide transfer queue) and `addDownload.ts` (the Add Download
dialog) are the two done so far. More are worth doing; see the note at the end.

`pane.ts` is the other shape in here: not a piece lifted out of the window but a
thing the window *has more than one of* — a place in a share, its history, its
selection, and its `tree.ts` (which folders the pane has listed and opened).
Anything a second pane would need its own copy of belongs there; anything the
toolbar can only ask once — sort, hidden files, the column widths — stays a
window setting.

## `src/components/` — one file per component

Flat, and named for what it draws. Two stylesheets are deliberately shared,
because their classes cross-reference each other:

- `shell.module.css` — the window's chrome: `TitleBar`, `Toolbar`, `Sidebar`,
  `FileList`, `StatusBar`, `Banner`, `EmptyState`, and `App.tsx` itself for the
  body, the list column and the scrims it draws directly.
- `dialog.module.css` — every sheet: `ConnectionDialog`, `Preferences`,
  `Properties`, `MoveTo`, `GoToFolder`, `AddDownloadDialog`. It is also where a
  dialog's `max-height` and scrolling live, which matters because the window
  itself does not scroll.

Everything else has its own stylesheet next to it.

Two structural tests police the styles and are worth knowing before you add CSS:
`overlays.test.ts` requires a `z-index` on anything pinned to all four edges, and
`transitions.test.ts` allows only colour-ish transitions in the shell, using the
duration tokens rather than raw milliseconds.

---

## Conventions you will meet

- **Story ids in comments.** `E13-S2`, `XDL-E2-S1`, `XTP-E1-S1` — they point into
  `legacy/plan.md`, `legacy/xdl.md` and `twopane.md`. Keep them when you move code.
- **Comments carry the reason, not the mechanism.** Many explain a bug that is
  invisible in review. Moving code means moving its comment with it; deleting one
  because it looks redundant is how the bug comes back.
- **British spelling** in prose and comments.
- **Anything past the design handoff gets a `docs/design-gaps.md` entry.** That is
  a definition-of-done item, not a nicety.

## The gates

CI runs all of these; run them before claiming anything works.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets   # CI adds -D warnings
cargo test --workspace
pnpm build                               # tsc --noEmit, then vite
pnpm test                                # vitest
```

**`pnpm test` does not typecheck.** `vitest` runs the tests; `tsc` runs only in
`pnpm build`. A change can leave the suite green and the build broken, so run
both.

---

## Known rough edges

Honest notes, so you do not mistake them for intent.

- **`App.tsx` is still ~2600 lines.** The clusters worth lifting next are
  drag-and-drop (~350 lines, but it reaches for a dozen things), favourites,
  and the mount/bookmark handlers. Each wants the `deps`-object shape the two
  modules in `src/app/` already use.
- **`FileList.tsx` is ~670 lines** — one component, three row layouts, a
  virtualised grid, inline rename and drag. Large but cohesive.
- **`Entry.addedMs` is birth time, not Finder's Date Added.** They agree for a
  file created where it sits and differ for one moved in. True parity needs
  `getattrlist`, and this workspace denies `unsafe_code`.
- **No live refresh from the server.** `state/refresh.ts` coalesces relists, but
  nothing produces `watch` events: xsync's Notify is Phase 4, types 110–115,
  unimplemented. See `backlog.md` E8-S9.
