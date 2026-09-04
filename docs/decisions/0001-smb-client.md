# 0001 — The SMB client library

*Decided 2026-09-03. **Reversed the same day** — see "The reversal" below.
Status: accepted, with `pavao` behind an off-by-default feature.*

## Decision, as it now stands

**Excalibur uses [`pavao`](https://crates.io/crates/pavao) (libsmbclient),
compiled only under the engine's off-by-default `smb` feature.**

The feature is the licence boundary. `cargo build` links no GPL code and the
binary stays `MIT OR Apache-2.0` with no SMB support; `cargo build --features
smb` links libsmbclient and the resulting binary is conveyable only under
**GPL-3.0-or-later**. The owner has accepted that trade for the opt-in build.

The rest of this file is the original reasoning, which reached the opposite
conclusion, followed by what changed. It is kept rather than rewritten because
the argument against `pavao` is still true — it is just no longer decisive.

## The reversal

Two things changed after the original decision.

**The licence stopped being disqualifying.** The owner's call: GPLv3 is
acceptable for a build that opts in. That removes the first and heaviest of the
three objections below. It does not remove the consequence, so the feature
exists to keep it contained and visible.

**The pure-Rust `smb` crate turned out not to be linkable at all.** `russh`
requires `crypto-bigint ^0.7.3`; `smb` pins `sspi =0.18.7`, which pins
`crypto-bigint =0.7.0-rc.8`. Both are in the same 0.7.x compatibility range, so
cargo must unify them and cannot. Every `smb` release back to 0.10.4 requires
`sspi` 0.18.x and every one of those carries the same release candidate, so no
version choice escapes it. `sspi` 0.18.7 pins **27** pre-release RustCrypto
crates while `russh` is built against the stable stack; patching around one of
them reaches the next.

**A cargo feature does not help there, and this was checked rather than
assumed.** Cargo resolves optional dependencies into the lockfile even when the
feature is off, so an optional `smb` fails `cargo metadata` for everyone. It
cannot be the thing behind the feature.

`pavao`'s dependency tree is five crates and collides with nothing, so it can.

So the choice was never between two working options: it was `pavao` or no SMB.

## What was accepted along with it

The other two objections below still stand, and enabling the feature accepts
them:

- **The session's dialect, signing state and cipher cannot be read back** —
  not from `pavao`, not from `pavao-sys`, and not from `libsmbclient.h`. So
  `protocol_label` says `SMB` rather than naming a version this code did not
  observe, and `security_label` says what was *requested* rather than claiming a
  cipher. That is narrower than E4-S2 asked for, and it is the honest width.
- **Every libsmbclient call in the process serialises behind one static mutex.**
  There is no concurrency to be had at any level, so `spawn_blocking` here keeps
  the synchronous C library off the runtime's threads rather than overlapping
  anything.
- Share-level writability is undetectable, so the backend learns it by trying to
  create a file — the same probe the SFTP and NFS backends use, for the same
  reason.
- `Write::flush` is a documented no-op and the close-on-drop error is swallowed
  by the library. Nothing in this codebase can fix that.
- `Caps::atomic_rename` is **false**: Samba unlinks an existing destination
  before renaming onto it, which is the window E6-S3 rejected for SFTP and which
  cannot be avoided here.

## Original decision (superseded)

*What follows is the reasoning as first written, when the conclusion was `smb`.*

## Context

E4-S1 asked for a spike comparing both candidates across a four-target matrix:
Samba 4.x in Docker, macOS's built-in `smbd`, a Synology or TrueNAS, and Windows
11. Only the first was reachable here, and the write-up in `plan.md` says so
plainly. It turned out not to matter, because the decision was not made on the
matrix.

## What decided it

### 1. The licence, which the risk register had wrong

`plan.md`'s risk register described `pavao` as wrapping "libsmbclient (C, LGPL,
mature)". **libsmbclient is GPL-3.0-or-later, not LGPL.** Every file in Samba's
`source3/libsmb/` carries the plain GPL header; the word "Lesser" appears in
none of them. Samba's LGPL-3.0+ parts are only `lib/talloc`, `lib/tdb`,
`lib/tevent` and `lib/ldb`, and there is no linking exception. Debian's
`debian/copyright`, Fedora's `samba.spec`, Homebrew's formula and FFmpeg's
`LICENSE.md` all agree.

This is not a licence *conflict* — MIT and Apache-2.0 are one-way compatible
into GPLv3, so nothing errors. It is absorption, which is worse because it is
silent:

- Excalibur's own source licence would be unaffected. The `.rs` files stay
  `MIT OR Apache-2.0`.
- **The shipped binary would be a different work.** Linking libsmbclient —
  statically or dynamically — makes the executable a combined work conveyable
  only under GPL-3.0-or-later as a whole (GPLv3 §5). Every recipient would get
  GPLv3 rights over all of it, Excalibur's own code included.

"Our code is MIT" would have been true and irrelevant to what we owe users.
GPLv3 §6 and §7 are also what make the Mac App Store incompatible — the VLC
precedent — so an in-process libsmbclient forecloses that channel permanently.
The only arm's-length pattern that holds is running `smbclient` as a separate
process and talking over pipes, which is a real architectural option and costs
the entire API.

The risk register has been corrected.

### 2. `pavao` cannot answer the questions E4-S2 asks

E4-S2's acceptance criteria want `protocol_label` from the negotiated dialect
and `security_label` from the cipher or signing state. Neither is reachable —
**and not because the wrapper is thin. libsmbclient itself has no such API.**
`SmbDialect` and `SmbEncryptionLevel` are policy *inputs*, passed to
`smbc_setOptionProtocols` and `smbc_setOptionSmbEncryptionLevel`. The getter
echoes back what was set, not what was negotiated. Samba's internal
`smbXcli_conn_protocol()` is not exported.

The best available would be to require encryption and report it as configured
intent rather than observed fact. This project's rule about `writable` — that it
comes from the server and is never inferred from what the user asked for — is
the same rule, and it would have to be broken here.

Share writability is likewise undetectable: `SMBC_VFS_FEATURE_RDONLY` is defined
but Samba's `SMBC_fstatvfs_ctx` never sets it, and no xattr route is wrapped.

### 3. A process-wide lock, in a program whose job is concurrency

`pavao` 0.3.1 serialises **every** libsmbclient operation in the process behind
one `static Mutex` — not per client, not per file. Its own documentation says a
blocking request delays every other SMB operation in the process. For a file
browser that lists a directory while rendering a preview and running a transfer,
that is the defining workload, and `spawn_blocking` does not help: the calls
queue on the mutex while occupying blocking-pool threads.

That lock is two days old, added to fix a 873-day-old crash report about using
clients from multiple threads. The fix is correct; the shape it forces is not
one this program can live with.

### 4. Everything else pointed the same way

`pavao` has no positional read or write — `SmbFile` is a shared cursor over
`lseek`+`read`, so E4-S4's ranged I/O would need a lock of its own on top of the
global one. `Write::flush` is a documented no-op. The close error on `Drop` is
discarded. `rename` with an existing destination is delete-then-rename inside
Samba, which has the same window E6-S3 rejected for SFTP, and cannot be asked to
refuse instead. `statvfs` has a units trap where `f_frsize` is a count rather
than a byte size against any non-UNIX-extensions server.

## What `smb` costs

This is not a free win, and the following are accepted knowingly.

- **The session's actual signed/encrypted state is not readable**, only the
  negotiated *capability* — `conn_info().negotiation.encryption_cipher` and
  `.signing_algo`. For SMB 3.0/3.0.2 both are `None`, because there are no
  negotiate contexts and the cipher is implicitly AES-128-CCM. This is a
  narrower version of `pavao`'s problem: the dialect and cipher *are* readable,
  which is what `protocol_label` and `security_label` mostly need, and only
  "is this particular session encrypted right now" is missing. The fix upstream
  is a three-line accessor. See E4-S2 for what the label says meanwhile.
- **Maintenance is the weak axis.** Pre-1.0, one or two maintainers, ~86 stars,
  and about four months since the last commit against a prior two-to-three week
  cadence. This is the real risk, and the mitigation is the one the risk
  register already names: it sits behind `ShareBackend`, so replacing it is a
  file, not a rewrite.
- **`sspi` is pinned at `=0.18.7`** and cannot be bumped without patching. On
  Linux it pulls `async-dnssd`, whose build script hard-fails without
  `libavahi-compat-libdnssd-dev`. Recorded in `docs/supply-chain.md` as a build
  prerequisite.
- One known bug — a `read_block` pre-check that can spuriously deny reads on a
  server returning a restrictive MxAc context — is fixed at head but unreleased.
  Tested against Samba on both a writable and a read-only share; it does not
  trigger, because Samba reports the access the user actually has.

## What `smb` buys

- **MIT.** No effect on what Excalibur may ship or where.
- **Zero `unsafe`**, with `#![forbid(unsafe_code)]` in five of the eight
  `smb-rs` crates. For a parser of untrusted network input that is the posture
  this workspace's own `unsafe_code = "deny"` presumption exists to get.
- **No C library at build or run time**, so packaging is `cargo build`.
- **Positional, concurrent reads.** `File::read_block` takes `&self` with an
  explicit offset, requests are keyed by message id, and credits are genuinely
  multi-credit. E4-S4's pipelining is expressible rather than emulated.
- **Async against tokio**, which is what the engine already is.

## Evidence

Verified against Samba 4.13.7 in Docker with a writable and a read-only share on
one server, before the decision rather than after:

- SMB 3.1.1 negotiated; `encryption_cipher` reads `Aes128Gcm`.
- Listing via `FileIdBothDirectoryInformation`, including the DOS hidden bit.
- Positional read at an offset; capacity via `FileFsFullSizeInformation`
  matching the host's own figures.
- The per-handle access mask reports `file_write_data: false` on the read-only
  share and `true` on the writable one, and a create on the read-only share is
  refused with `STATUS_ACCESS_DENIED` — which is exactly the term E4-S2 needs.

## Revisit if

- `smb-rs` goes unmaintained for a further six months, or a security issue in it
  goes unfixed. The trait boundary is what makes that survivable.
- `libsmb2` (LGPL-2.1) grows a usable Rust binding — the licence there does not
  absorb the binary, so it would be a genuine alternative rather than this one.
- Excalibur decides to ship under GPL-3.0-or-later anyway, at which point
  libsmbclient becomes licensable — though points 2 and 3 above would still
  disqualify it.
