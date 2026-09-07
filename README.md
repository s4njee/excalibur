# Excalibur

A desktop share browser for **SMB, NFS, SFTP and xsync**. Rust + Tauri v2 shell,
SolidJS frontend, and every byte of protocol and filesystem I/O in Rust.

Single-pane, Cyberduck-style: one directory of one share at a time, bookmarks in
a permanently visible sidebar, and the mount's effective permission first in the
status bar, gating every write verb.

> **Status: M4.** xsync, SFTP and NFS are connected, and SMB is behind an
> off-by-default feature; Quick Look, edit-in-place and the transfer queue run
> against a real server. The macOS build is local and unsigned — see
> [Run it against your own server](#run-it-against-your-own-server) and
> [`plan.md`](plan.md).

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

- <http://localhost:7188/?screen=1a> — light, read-write
- <http://localhost:7188/?screen=1b> — dark, read-only

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

# The reconnect tests kill the server, so they are opt-in and serial:
EXCALIBUR_SMB=127.0.0.1 EXCALIBUR_SMB_DISRUPTIVE=1 \
  cargo test -p excalibur-engine --features smb --test smb -- --test-threads=1

crates/engine/tests/fixtures/nfs/up.sh   # NFSv3
EXCALIBUR_NFS=127.0.0.1:12049 cargo test -p excalibur-engine --test nfs
```

SFTP needs nothing: the test spawns the machine's own `sshd` on a spare port and
skips where `sshd`/`sftp-server` are absent.

## Run it against your own server

Four steps: build the app, put `xs` on the server, connect, and recognise the two
ways it fails.

### 1. Build the app

```bash
pnpm install
pnpm tauri build                 # .app and .dmg
pnpm tauri build --bundles app   # just the .app, and quicker
```

```text
target/release/bundle/macos/Excalibur.app
target/release/bundle/dmg/Excalibur_0.1.0_aarch64.dmg
```

**Nothing here is signed.** With no `APPLE_SIGNING_IDENTITY` set, Tauri does not
run `codesign` at all: the `.app` carries only the *linker's* ad-hoc signature on
the executable, and the bundle has no `_CodeSignature` directory, so
`codesign --verify` and `spctl --assess` both refuse it with "code has no
resources but signature indicates they must be present".

That is harmless for the copy you just built — macOS only consults Gatekeeper for
files carrying `com.apple.quarantine`, and a locally built app has none. It is
*not* harmless for the `.dmg`, because anything that arrives over a browser, AirDrop
or a message is quarantined. On the receiving machine:

```bash
xattr -dr com.apple.quarantine /Applications/Excalibur.app
```

Right-click → Open is the usual advice and it does **not** work here: the
assessment fails on a malformed signature rather than an unknown developer, so
macOS offers no override. Removing the quarantine flag is the whole workaround
until the app is signed with a Developer ID and notarised (E18-S1's deferred
half).

If you would rather have a bundle that is at least internally consistent, ask
Tauri for a real ad-hoc signature:

```bash
PATH=/usr/bin:$PATH APPLE_SIGNING_IDENTITY=- pnpm tauri build
```

Tauri then runs `codesign --options runtime`, so the bundle gets a sealed
`_CodeSignature` **and the hardened runtime**, and `codesign --verify` passes.
`spctl` still says `rejected` — ad-hoc is not a Developer ID, and no amount of
local signing changes that — and notarisation is skipped with a warning naming
the `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` (or `APPLE_API_KEY`) variables it
would need.

The `PATH` prefix is not decoration. The signing path runs `xattr -cr` on the
bundle, and Homebrew installs an `xattr` that has no `-r`; if that one is first
on your `PATH` the bundle step dies with `failed to run xattr` after the whole
release build has finished. The unsigned build never calls `xattr`, which is why
it is the default here.

### 2. Put `xs` on the server

The xsync backend has no daemon yet: it reaches a server by running
`PATH="$HOME/.local/bin:$PATH" xs --server <export>` over `ssh`. So the server
needs an `xs` binary, built from the sibling [`xsync`](../xsync) checkout.

```bash
scripts/stage-xs.sh root@myserver
```

That cross-compiles `xs` for the host's architecture — detected with `uname -m`
over ssh — and installs it to `/usr/local/bin/xs` when the login is root, or
`~/.local/bin/xs` otherwise. It needs `cargo-zigbuild`, `zig` and the matching
rustup target on this Mac, and key-based ssh to the host; it prompts for nothing
and says which of those is missing rather than hanging. It is safe to re-run: the
binary is staged under a temporary name, executed on the host to prove it runs
there, and only then moved over the existing one.

```text
--target <triple>   force the target instead of detecting it
--glibc <version>   glibc to link against (default 2.31)
--dest <dir>        install somewhere else
--xsync <dir>       a checkout other than ../xsync
```

The glibc default is deliberately old: a binary linked against 2.31 runs on
anything newer, and the script refuses to install one that would need a glibc
newer than the host's. The last thing it prints is `command -v xs` as a
non-interactive ssh sees it — which is the PATH that actually matters — and it
warns when an older `xs` elsewhere on that PATH shadows the one just installed.

Building on the server itself works just as well, from an xsync checkout there:

```bash
cargo build --release -p xsync --bin xs
install -D target/release/xs ~/.local/bin/xs
```

### 3. Connect

**New Connection → xsync**:

| Field | What to type |
|---|---|
| Server | the host, `myserver` or `10.0.0.4` |
| Export | the absolute path on the server that becomes the mount root, `/srv/media` |
| Port | blank — ssh's 22 |
| Username | the ssh login |
| Mount as | Read & write or Read only |

There is no password field, and that is not an omission: the session is an
`ssh -o BatchMode=yes`, so the only credentials that can work are a key or an
agent. "Mount as" is a *preference*; the mount's real writability comes back from
the server and is what the status bar shows and what gates every write verb.

`Test` connects for real and reports the mount facts. It is worth pressing.

**The host-key prompt.** The first connection to a host shows the key's algorithm
and fingerprint and asks whether to trust it. Compare it against the server:

```bash
ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub    # on the server
```

Accepting writes the key to Excalibur's own `known_hosts` in its app data
directory — `~/.ssh/known_hosts` is neither read nor written, so trusting a host
here does not change what your other ssh tools trust, and vice versa. If the key
later *changes*, you get a red banner with no "accept anyway" in it; that is
deliberate, and the way through is to remove the old key from that file after you
have established why it changed.

The CLI walks the same path without the app, which is the quickest way to tell a
server problem from a UI one:

```bash
cargo run --bin xcal -- ls --xsync-ssh myserver:/srv/media --user root
cargo run --bin xcal -- ls --xsync-ssh myserver:/srv/media --user root --trust
```

### 4. When it does not connect

Two things go wrong, and they want different fixes.

**ssh refused the login.** Nothing Excalibur can retry helps; the transport never
came up. Check the same thing it checks:

```bash
ssh -o BatchMode=yes myserver true
```

`BatchMode` is why: a GUI has no terminal for a password or passphrase prompt, so
key or agent authentication is the only kind that can succeed. If that command
fails, add your public key to the server's `authorized_keys`, or `ssh-add` the
key. A *changed* host key looks different — a banner rather than a login failure —
and is described above.

**ssh connected but could not find `xs`.** The login shell ran and reported no
such command. Re-run `scripts/stage-xs.sh`, or check by hand with the PATH the
backend actually uses:

```bash
ssh myserver 'PATH="$HOME/.local/bin:$PATH" command -v xs'
```

A non-interactive ssh does not read the same startup files as a login shell, so
an `xs` that works when you ssh in and type it can still be invisible here. Both
`~/.local/bin/xs` and `/usr/local/bin/xs` are found.

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
