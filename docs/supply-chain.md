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
  **No module does today** — this section is where the first one gets recorded,
  with the same detail a new dependency gets.
- `cargo audit` and `cargo deny check` run on every push and pull request and
  fail on any finding not explicitly accepted, with a reason, in `deny.toml`.
- Everything comes from crates.io. Unknown registries and git sources are denied
  outright, so no dependency can arrive from a URL nobody reviewed.
- No workspace crate is publishable (`publish = false`): this is an application,
  not a set of libraries.

## Audit, 2026-09-03

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
| `tempfile` | engine (dev) | Scratch exports for the xsync and SFTP integration tests. |

### On `sha2`, `base64` and `dirs`

All three were already in the tree — `sha2` and `base64` through several
dependencies, `dirs` through `keyring` — so naming them directly added no
packages and no licences. They are here rather than hand-rolled because a
fingerprint that does not match `ssh-keygen -lf` byte for byte is worse than
useless: the whole point is that a user compares it against what the server
operator told them. There is a test pinning ours to a real key's published
fingerprint.

### On `keyring`

`keyring` v4 is the crate the Rust ecosystem has settled on for this, and it is
the same one Kestrel uses. Its platform stores are separate crates
(`apple-native-keyring-store` here) over a small `keyring-core`, so the tree it
adds is narrow and the platform code is the vendor's, not ours. It introduced no
advisory and no new licence to the allow list.
