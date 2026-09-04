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

### Against a real xsync export

The xsync backend is connected. With an `xs` binary built from the sibling
[`xsync`](../xsync) checkout:

```bash
XS=../xsync/target/debug/xs
cargo run --bin xcal -- ls   --xsync-local /path/to/share --xs "$XS"
cargo run --bin xcal -- cat  notes.txt --xsync-local /path/to/share --xs "$XS"
cargo run --bin xcal -- ls   --xsync-ssh nas.local:/srv/media
```

`--xsync-local` runs `xs --server` and speaks protocol v3 over its pipes, which
is the same protocol a remote session uses. Add `--read-only` to see the mount
refuse writes and say why.

The backend's tests run against a real server and are skipped without one:

```bash
EXCALIBUR_XS=../xsync/target/debug/xs cargo test -p excalibur-engine
```

### SMB, and the licence it carries

SMB is **off by default**, and turning it on changes the licence of the binary
you produce. It links `libsmbclient`, which is **GPL-3.0-or-later**, so a build
with the feature is conveyable only under GPLv3 as a whole — Excalibur's own
code included — and is incompatible with the Mac App Store. The default build
links none of it and stays `MIT OR Apache-2.0`.

It also needs libsmbclient present at build and run time:

```bash
brew install samba                     # or: apt install libsmbclient-dev
export PKG_CONFIG_PATH="/opt/homebrew/opt/samba/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo build -p excalibur --features smb
```

See [`docs/decisions/0001-smb-client.md`](docs/decisions/0001-smb-client.md) for
why this is a C library rather than the pure-Rust alternative.

### The other backends' tests

Each runs against a real server and is skipped without one. The SMB and NFS
fixtures need Docker:

```bash
crates/engine/tests/fixtures/smb/up.sh   # Samba on port 445
EXCALIBUR_SMB=127.0.0.1 cargo test -p excalibur-engine --features smb --test smb

crates/engine/tests/fixtures/nfs/up.sh   # NFSv3
EXCALIBUR_NFS=127.0.0.1:12049 cargo test -p excalibur-engine --test nfs
```

SFTP needs nothing: the test spawns the machine's own `sshd` on a spare port and
skips where `sshd`/`sftp-server` are absent.

## Checks

Note `cargo fmt --all` reaches through the `xsync-client` path dependency into
the sibling repo and will rewrite files there. Scope it:

```bash
cargo fmt -p excalibur-engine -p excalibur-cli -p excalibur -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm build      # type-checks with tsc, then builds
pnpm test
```
