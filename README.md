# Excalibur

A desktop share browser for **SMB, NFS, SFTP and xsync**. Rust + Tauri v2 shell,
SolidJS frontend, and every byte of protocol and filesystem I/O in Rust.

Single-pane, Cyberduck-style: one directory of one share at a time, bookmarks in
a permanently visible sidebar, and the mount's effective permission first in the
status bar, gating every write verb.

> **Status: M0.** The shell renders the design's two browser screens against an
> in-memory mock. No protocol backend is connected yet. See [`plan.md`](plan.md).

## Layout

```text
crates/engine/   excalibur-engine — backends, transfers, previews. No Tauri.
crates/cli/      xcal — drives the engine headlessly, for bring-up and bugs.
src-tauri/       The shell: commands, DTOs, the event bridge. Nothing else.
src/             SolidJS frontend.
design_handoff_share_browser/   The specification this implements.
```

Two boundaries hold this apart, and both are load-bearing:

- **File bytes never cross IPC.** Tauri commands carry paths and metadata; Rust
  does the reading and writing.
- **The engine has no Tauri dependency.** Every backend is testable headlessly,
  which is why `xcal` exists.

## Running it

```bash
pnpm install
pnpm dev          # the frontend alone, in a browser
pnpm tauri dev    # the desktop app
```

The frontend serves the design's two screens from one component tree, so they
can be compared against the mock without a server:

- <http://localhost:5173/?screen=1a> — light, read-write
- <http://localhost:5173/?screen=1b> — dark, read-only

`?screen=1b` is the one worth looking at: it is the same tree with
`mount.writable` false, and it disables Edit and Upload while leaving Quick Look
and Download alone. That single derived value is what the design stakes
everything on.

```bash
cargo run --bin xcal -- ls          # the mock share, as the engine sees it
cargo run --bin xcal -- ls --read-only --json
```

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm build      # type-checks with tsc, then builds
pnpm test
```
