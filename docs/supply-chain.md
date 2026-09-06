# Supply Chain

Policy lives in [`deny.toml`](../deny.toml) and is enforced in CI. This file
records the audit result so a change in the dependency tree shows up as a diff
rather than as a surprise at release time. The sibling `xsync` repository keeps
the same discipline, and this file follows its format.

## Guarantees

- `Cargo.lock` is committed and CI builds `--locked`, so CI resolves exactly the
  versions recorded here.
- The workspace denies `unsafe_code` (`[workspace.lints.rust]` in `Cargo.toml`),
  and `src-tauri` additionally `#![forbid(unsafe_code)]`s. `deny` is the
  presumption rather than an absolute: a module may allow it for itself where no
  adequate safe crate exists, under the rules in plan.md's "Decisions taken".
  **Exactly one module does**, since 2026-09-06 — see [The one `unsafe`
  module](#the-one-unsafe-module) below.
- `cargo audit` and `cargo deny check` run on every push and pull request and
  fail on any finding not explicitly accepted, with a reason, in `deny.toml`.
- Everything comes from crates.io. Unknown registries and git sources are denied
  outright, so no dependency can arrive from a URL nobody reviewed.
- No workspace crate is publishable (`publish = false`): this is an application,
  not a set of libraries.

## Audit, 2026-09-03

*(Updated 2026-09-05 for E21-S1: 652 packages. `trash` adds seven of them and
`fs4` none — it was already in the tree. All four gates still pass and the
licence allow list is unchanged; see the `trash` note below.)*

642 packages in `Cargo.lock`. `cargo deny check` passes all four gates:
advisories, bans, licences, sources. One vulnerability is accepted with a
written argument rather than fixed; it is the first, and it is described below.

### Accepted advisories

**RUSTSEC-2023-0071** — the Marvin timing side-channel in `rsa`, reached
through `russh`'s default features so that SFTP can use RSA keys. It is the one
accepted entry that is a *vulnerability* rather than an unmaintained notice, and
the only fix available here is `default-features = false` on russh, which
removes RSA key support outright. That trades a narrow timing attack for a large
number of users unable to connect at all. An SSH client signs with its private
key rather than decrypting, so exploiting it needs a malicious server able to
trigger and precisely time many thousands of signatures — a server the user
chose to keep connecting to. Real, and narrow. The full argument and the
revisit condition are in `deny.toml` beside the entry.

Then sixteen `unmaintained` notices, listed individually in `deny.toml` so that
a genuinely new one still fails the build. None is a vulnerability and none is
reachable from a dependency chosen here:

- Ten GTK3 binding crates (`gtk`, `gdk`, `atk` and their `-sys` companions),
  reached through Tauri's Linux backend. Tauri v2 still targets GTK3; moving off
  it is upstream's decision, not ours. On macOS these do not build at all.
- Six build-time-only crates (`proc-macro-error`, five `unic-*`), used by
  `tauri-macros` and `tauri-codegen`. They run during compilation and ship no
  code in the binary.

`cargo audit` additionally reports an unsoundness notice for `glib`
(RUSTSEC-2024-0429), same Linux-only Tauri origin, same reasoning.

### The `smb` feature changes the binary's licence

`pavao` links **libsmbclient**, which is **GPL-3.0-or-later** — not LGPL. Samba's
LGPL-3.0+ parts are only `lib/talloc`, `lib/tdb`, `lib/tevent` and `lib/ldb`;
`source3/libsmb/` carries the plain GPL and there is no linking exception.

This is not a licence *conflict*: MIT and Apache-2.0 are one-way compatible into
GPLv3, so nothing errors. It is absorption. Excalibur's own source licence is
unaffected — the `.rs` files stay `MIT OR Apache-2.0` — but a binary built with
`--features smb` is a combined work conveyable only under GPL-3.0-or-later as a
whole, and every recipient gets GPLv3 rights over all of it. GPLv3 §6 and §7 are
also what make it incompatible with the Mac App Store.

The default build does not link it and is unaffected. This is why the feature
exists rather than the dependency simply being present, and why `cargo deny`'s
licence gate is run for both configurations.

`pavao` and `pavao-sys` themselves are MIT; the obligation comes from the C
library they bind, not from the wrapper.

### Licences

Every licence in the tree is permissive and compatible with the declared
`MIT OR Apache-2.0`, with one exception worth naming: five crates are
**MPL-2.0** — `cssparser`, `cssparser-macros`, `selectors`, `dtoa-short` (all via
Tauri's CSS stack) and `option-ext` (via `dirs`). MPL-2.0 is file-level
copyleft: linking them unmodified, as we do, carries no obligation beyond
keeping their own notices. Modifying one of those files would, so don't vendor
them.

| Count | Licence expression |
|------:|---|
| 257 | `MIT OR Apache-2.0` |
| 112 | `MIT` |
| 53 | `Apache-2.0 OR MIT` |
| 20 | `MIT/Apache-2.0` |
| 18 | `Unicode-3.0` |
| 17 | `Zlib OR Apache-2.0 OR MIT` |
| 11 | `Unlicense OR MIT` |
| 5 | `MPL-2.0` |
| 5 | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| 3 | `Apache-2.0/MIT` |
| 25 | (remainder: BSD-2/3-Clause, ISC, Zlib, CC0-1.0, 0BSD and dual forms) |

## Direct dependencies, and why

Only crates named in a workspace `Cargo.toml` are listed. Anything else arrived
as a transitive dependency of these.

| Crate | Used by | Why |
|---|---|---|
| `tauri`, `tauri-build`, `tauri-plugin-store` | `src-tauri` | The shell: window, IPC, and persisted bookmarks. |
| `xsync-client` | engine | The xsync v3 backend. Path dependency on the sibling repo until its `v3` branch is pushed (E7-S0). |
| `xsync-core` | engine | E22-S1. `run_client_push` / `run_client_pull` — the transfer engine itself, driven over an `ssh` session Excalibur spawns and whose host-key policy Excalibur keeps. **Already in the tree**: `xsync-client` depends on it, so naming it directly promotes a transitive dependency and adds no package and no licence. See the note below for what a path dependency on an uncommitted tree costs. |
| `tokio` | engine | Async runtime the backends and session manager are written against. |
| `async-trait` | engine, cli | `ShareBackend`/`Connector` need async methods on an object-safe trait. The CLI took it too for `xcal get`, which implements the worker's `BackendSource` (E13-S7); nothing new entered the tree. |
| `serde`, `serde_json` | both | DTOs across IPC, and the CLI's fixture output. |
| `thiserror` | engine | The engine's error enum. |
| `keyring` | engine | E3-S3. The OS credential store: Keychain on macOS, Secret Service on Linux, Credential Manager on Windows. Chosen over writing three platform bindings because the workspace denies `unsafe_code` and all three need FFI. |
| `zeroize` | engine | E3-S3. Wipes a password's heap allocation on drop, so a credential does not outlive its use in a core file. |
| `sha2` | engine | E3-S4. SHA-256 for SSH host key fingerprints. |
| `base64` | engine | E3-S4. Decodes `known_hosts` key material and encodes the fingerprint the way OpenSSH prints it. |
| `dirs` | engine | E3-S4. Locates the per-platform data directory the `known_hosts` file lives in. |
| `russh` | engine | E6-S1. The SSH transport for the SFTP backend, exact-pinned. Speaking SSH in-process rather than spawning `ssh` is what lets the host key be checked during the handshake that uses it, closing the scan-then-connect window the xsync backend still has. |
| `russh-sftp` | engine | E6-S1. SFTP v3 over that transport, exact-pinned. The backend drives its `RawSftpSession`, not the friendlier `SftpSession`: the raw one carries the offset in each request, which is what makes pipelined reads and `posix-rename@openssh.com` reachable. |
| `nfs3_client` | engine | E5-S1. The NFSv3 wire codec. `Unlicense`, zero `unsafe`, and exactly two direct dependencies — a genuinely small tree for a network protocol. It is a codec rather than a client: no DNS, no connection pool, no retry, no path layer, so the session layer above it is ours. |
| `pavao` *(optional)* | engine | E4. libsmbclient bindings, behind the off-by-default `smb` feature. **Enabling it makes the binary GPL-3.0-or-later** — see the note below. |
| `notify` | `src-tauri` | E12-S1. Watches an edit-in-place working copy so a save in the user's own editor reaches the share. The alternative is polling every open file. |
| `blake3` | engine | E12-S1. The whole-file digest a staged xsync commit verifies. Already in the tree via `xsync-client`. |
| `fs4` | engine | E21-S1. `statvfs` for the local backend's capacity. Already in the tree via `xsync-core`, and thin: it is a `FsStats` struct over `rustix::fs::statvfs`, with no `libc` and no code of its own on the platform paths. |
| `trash` | engine *(target-gated)* | E21-S1. Moves a deleted file or folder to the platform's Trash. See the note below. |
| `objc2`, `objc2-app-kit`, `objc2-foundation` | engine *(macOS only)* | E24-S2. Apple's drag and drop, for the one FFI module. Vetted bindings rather than hand-written `extern` blocks — the middle step of the preference order in "Decisions taken". All three were **already in the lock file** via Tauri, wry and `trash`, at the same versions, so this adds no package and no licence to the audit. See the note below. |
| `tempfile` | engine (dev) | Scratch exports for the xsync and SFTP integration tests. |

### On `xsync-core`, and one red gate

`xsync-core` is `MIT OR Apache-2.0`, like `xsync-client`, and its twenty
dependencies — `blake3`, `zstd`, `redb`, `ignore`, `rustix`, `filetime`,
`globset` and the rest — were **every one of them already resolved in
`Cargo.lock`** before this story, because `xsync-client` depends on the same
crate. The package count does not move.

Two things about it are worth writing down rather than leaving to be
rediscovered.

**The tree it points at is not committed.** `xsync-client`'s row says "until its
`v3` branch is pushed", which understates it: `xsync-core` currently depends on
`xsync-endpoint`, a crate that is *untracked* in the xsync repository. So a
build of Excalibur today depends on files that exist on one machine and in no
history. That is a property of the sibling-repo arrangement E7-S0 exists to end,
not something E22-S1 introduced, but E22-S1 is the story that made the
dependency direct and it should be said plainly here.

**`cargo deny check` fails on `bans`, and did before this change.**

```
error[wildcard]: found 1 wildcard dependency for crate 'xsync-core'.
allow-wildcard-paths is enabled, but does not apply to public crates as
crates.io disallows path dependencies.
  ┌─ ../xsync/crates/xsync-core/Cargo.toml:48:18
48 │ xsync-endpoint = { path = "../xsync-endpoint" }
```

`allow-wildcard-paths` excuses a versionless path dependency only on a crate
that is not publishable. `xsync-core` and `xsync-endpoint` both lack
`publish = false`, so cargo-deny treats `xsync-core` as public and refuses the
line. It reaches Excalibur through `xsync-client`, and removing the direct
`xsync-core` dependency does not clear it — that was checked, not assumed.

It is deliberately **not** papered over here. The fix is one word in the sibling
repo — `version = "0.1.0"` beside that path, which is exactly what
`xsync-client` already writes for its own dependency on `xsync-core` — or
`publish = false` on the two crates. The alternative, a `skip` in this repo's
`deny.toml`, would switch off the check whose stated purpose is "to catch a
wildcard on a *published* crate", which is precisely the case it is now
catching. So the gate stays red and honest until xsync takes the one-word fix.

### The one `unsafe` module

`crates/engine/src/drag/macos.rs` (E24-S2, 2026-09-06) is the first and so far
only module in this tree to carry `#![allow(unsafe_code)]`. It is recorded here
with the same detail a dependency gets, because that is what the rule in
plan.md's "Decisions taken" asks for.

**Why it exists.** A drag out of the file list has to put a *private pasteboard
type* on an `NSDraggingSession` — `dev.excalibur.selection`, naming the mount and
the paths — so that a second Excalibur window can tell an Excalibur drag from a
Finder one. Tauri's own drag events carry OS file paths and nothing else, and a
webview cannot start a drag another webview will receive. The preference order
was walked in full: there is no safe crate for this (`tauri-plugin-drag` drags
*files* and cannot write a private type), and `objc2-app-kit` is the vetted-FFI
step, so what is written by hand is the message sends that crate cannot express
for a custom `NSView` subclass.

**What the `unsafe` actually is.** Five blocks, each with a `// SAFETY:` note:

| Where | What |
|---|---|
| `define_class!` for `DropSurface` | Declaring an `NSView` subclass and its `NSDraggingDestination` methods. The `#[unsafe(super(..))]` and `#[unsafe(method(..))]` markers are the macro's, asserting the superclass has no subclassing requirements and that each selector's signature matches AppKit's. |
| `define_class!` for `DragSource` | The same, for an `NSObject` implementing `NSDraggingSource`. |
| `msg_send![super(this), initWithFrame:]` / `init` | Calling the superclass's designated initialiser on a freshly allocated instance. |
| `setDraggingFrame:contents:` | `contents` is typed `Option<&AnyObject>`; an `NSImage` is the documented case. |
| `view_at` | Turning the `*mut c_void` `tauri::Window::ns_view()` returns into `&NSView`. The pointer is the window's content view, alive for as long as the window, and the borrow never outlives the call it is passed to. |

**What contains it.** `crates/engine/src/drag/mod.rs` is a safe wrapper — three
functions and four plain data types — and is all the rest of the tree can see.
The payload's encoding, the phase and modifier vocabulary, and the refusal on
non-macOS targets are testable without a window; the FFI itself has tests that
build the pasteboard item and read the payload back, and that assert the drop
target registers for our type and no other. What is *not* covered by a test is a
real drag session: an `NSDraggingInfo` cannot be constructed without one. That
half is a manual check, recorded in the story's notes.

**Threading.** AppKit's dragging protocols are `MainThreadOnly`. Every function
in the wrapper returns an error off the main thread rather than doing something
undefined, so the shell has to say `run_on_main_thread` out loud.

### On `sha2`, `base64` and `dirs`

All three were already in the tree — `sha2` and `base64` through several
dependencies, `dirs` through `keyring` — so naming them directly added no
packages and no licences. They are here rather than hand-rolled because a
fingerprint that does not match `ssh-keygen -lf` byte for byte is worse than
useless: the whole point is that a user compares it against what the server
operator told them. There is a test pinning ours to a real key's published
fingerprint.

### On `trash`

`remove` on a local share moves things to the Trash rather than unlinking them.
That is a correctness requirement, not a nicety: deleting a file on your own Mac
through this window must not be more permanent than deleting it in Finder, and
there is no way to be less permanent afterwards.

Doing it by hand is not the small job it looks like. macOS wants
`NSFileManager -trashItemAtURL:`, which is Objective-C and therefore FFI; the
freedesktop specification wants a `.trashinfo` sidecar, the right trash
directory for the *volume* the file is on, and a de-duplicating rename; Windows
wants `IFileOperation`. `trash` v5 is MIT, is the crate the ecosystem uses for
this, and on macOS reaches AppKit through `objc2`/`objc2-foundation` — vetted
bindings — rather than through hand-written `extern` blocks, which is what keeps
this side of `unsafe_code = "deny"`. It added seven packages to the lock —
`trash` itself, `urlencoding` (the freedesktop sidecar's encoding) and the five
`windows` 0.56 crates — all permissive, and none new to the allow list. The
`objc2` crates it uses on macOS were already in the tree via Tauri.

It is declared under a `[target.'cfg(...)']` table rather than plainly, because
it has no platform module for iOS or Android and would not compile there. On a
target without one the backend deletes for real and says `permanent delete` in
the mount's options instead of `trash`, so the difference is visible in the
status bar rather than only in the source.

A failure to trash is reported, never downgraded to a real delete. The volumes
where trashing fails — a read-only image, some network mounts — are exactly the
ones where silently making a delete permanent would be worst.

### On `keyring`

`keyring` v4 is the crate the Rust ecosystem has settled on for this, and it is
the same one Kestrel uses. Its platform stores are separate crates
(`apple-native-keyring-store` here) over a small `keyring-core`, so the tree it
adds is narrow and the platform code is the vendor's, not ours. It introduced no
advisory and no new licence to the allow list.
