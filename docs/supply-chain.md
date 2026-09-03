# Supply Chain

Policy lives in [`deny.toml`](../deny.toml) and is enforced in CI. This file
records the audit result so a change in the dependency tree shows up as a diff
rather than as a surprise at release time. The sibling `xsync` repository keeps
the same discipline, and this file follows its format.

## Guarantees

- `Cargo.lock` is committed and CI builds `--locked`, so CI resolves exactly the
  versions recorded here.
- The workspace denies `unsafe_code` (`[workspace.lints.rust]` in `Cargo.toml`),
  and `src-tauri` additionally `#![forbid(unsafe_code)]`s.
- `cargo audit` and `cargo deny check` run on every push and pull request and
  fail on any finding not explicitly accepted, with a reason, in `deny.toml`.
- Everything comes from crates.io. Unknown registries and git sources are denied
  outright, so no dependency can arrive from a URL nobody reviewed.
- No workspace crate is publishable (`publish = false`): this is an application,
  not a set of libraries.

## Audit, 2026-09-03

528 packages with `--all-features`. `cargo audit` reports **no vulnerabilities**.
`cargo deny check` passes all four gates: advisories, bans, licences, sources.

### Accepted advisories

Sixteen `unmaintained` notices, listed individually in `deny.toml` so that a
genuinely new one still fails the build. None is a vulnerability and none is
reachable from a dependency chosen here:

- Ten GTK3 binding crates (`gtk`, `gdk`, `atk` and their `-sys` companions),
  reached through Tauri's Linux backend. Tauri v2 still targets GTK3; moving off
  it is upstream's decision, not ours. On macOS these do not build at all.
- Six build-time-only crates (`proc-macro-error`, five `unic-*`), used by
  `tauri-macros` and `tauri-codegen`. They run during compilation and ship no
  code in the binary.

`cargo audit` additionally reports an unsoundness notice for `glib`
(RUSTSEC-2024-0429), same Linux-only Tauri origin, same reasoning.

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
| `async-trait` | engine | `ShareBackend`/`Connector` need async methods on an object-safe trait. |
| `serde`, `serde_json` | both | DTOs across IPC, and the CLI's fixture output. |
| `thiserror` | engine | The engine's error enum. |
| `keyring` | engine | E3-S3. The OS credential store: Keychain on macOS, Secret Service on Linux, Credential Manager on Windows. Chosen over writing three platform bindings because the workspace denies `unsafe_code` and all three need FFI. |
| `zeroize` | engine | E3-S3. Wipes a password's heap allocation on drop, so a credential does not outlive its use in a core file. |
| `tempfile` | engine (dev) | Scratch exports for the xsync integration tests. |

### On `keyring`

`keyring` v4 is the crate the Rust ecosystem has settled on for this, and it is
the same one Kestrel uses. Its platform stores are separate crates
(`apple-native-keyring-store` here) over a small `keyring-core`, so the tree it
adds is narrow and the platform code is the vendor's, not ours. It introduced no
advisory and no new licence to the allow list.
