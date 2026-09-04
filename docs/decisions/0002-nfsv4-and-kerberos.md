# 0002 — NFSv4 and Kerberos

*Decided 2026-09-04. Status: accepted. E5-S5's spike; no code was written.*

## Decision

**Neither route is viable now. NFSv3 with AUTH_SYS remains the whole of the NFS
backend, and the window continues to say so.**

If this is picked up later, the two halves should be picked up *separately* and
in this order:

1. **NFSv4 without Kerberos** — tractable, via a fork of the libnfs bindings.
2. **Kerberos** — expensive, and blocked upstream on macOS today.

The AC bundles them ("NFSv4 and Kerberos"), and the most useful thing this
spike found is that their costs are nothing alike.

## The two routes, as the AC framed them

### Route A — libnfs bindings

**The licence is fine, and this is worth stating plainly because it is the
opposite of [ADR 0001](0001-smb-client.md).** libnfs is split, which is why
GitHub reports it as `NOASSERTION`, and the split is in our favour:

- `lib/` and `include/` — the library actually linked — are
  **LGPL-2.1-or-later** (verified in `COPYING` and in the file headers, not
  from a summary);
- the `*.x` protocol definitions and the generated `libnfs-raw-*` files are
  simplified BSD, and `COPYING` explicitly permits regenerating them under any
  licence;
- **only `examples/` is GPL-3.0-or-later**, and examples are off by default, so
  the shared library carries no GPL.

Dynamic linking is therefore an LGPL question alone, and does not change what
Excalibur may ship. The absorption that forced SMB behind a feature flag does
not apply. *Static* linking would engage LGPL §6's relinking obligation — which
is a further reason the static story below is not the one to reach for.

**Upstream libnfs is healthy and already has exactly the API this story wants.**
Verified in `include/nfsc/libnfs.h` at master:

- `EXTERN void nfs_set_security(struct nfs_context *nfs, enum rpc_sec sec);`
- the URL argument `sec=<krb5|krb5i|krb5p>`
- `EXTERN int nfs_set_version(struct nfs_context *nfs, int version);` with
  `NFS_V4` and `NFS_V4_2`

601 stars, ~270 commits in two years, NFSv4.2 work landing as recently as
2026-08-30. The C library is not the problem.

**The Rust bindings are the problem.** There are exactly two, both by one
author, both last published 2022-09-22, and **neither builds today**:

- `libnfs-sys` 0.2.3 hard-pins `bindgen` 0.60.1, which cannot run against a
  modern libclang. It fails on an anonymous union in `struct accepted_reply`
  that has been unchanged since libnfs 1.9.8 — so this is a bindgen problem,
  not a libnfs-version problem, and no choice of libnfs fixes it. **The fix is
  confirmed to be one line**: vendoring the crate and changing only
  `bindgen 0.60` → `0.72` builds cleanly against a Kerberos-enabled libnfs and
  exposes `nfs_set_version`, `nfs_set_security` and `RPC_SEC_KRB5/I/P`. Git
  master of the crate already made that bump, in 2024, and never published it.
- `libnfs` 0.1.1, the safe wrapper, is **NFSv3-only by construction**: its URL
  parsers are inside a block comment, `nfs_set_version` appears only in a doc
  comment, and the sole entry point is `mount()`, which takes libnfs's v3
  default. Its only security hook is `set_auth`, the old AUTH_UNIX path — there
  is no `set_security`. It is `Rc`-based, so neither `Send` nor `Sync`, and
  sync-only.
- Worth being precise about, because it is the one encouraging fact here:
  **Kerberos is reachable at the raw FFI layer today.** `libnfs-sys` runs
  bindgen with *no allowlist*, so against an installed libnfs ≥ 6.0.0 it
  generates `nfs_set_security` and `rpc_sec_RPC_SEC_KRB5/I/P` for free. The
  missing piece is a safe wrapper, not a binding — which would matter, if the
  `-sys` crate built at all.
- Git master of both is two years stale, still lacks `nfs_set_security`, is
  pinned to libnfs functions removed in 7.x, and fails to compile on macOS
  because `mode_t` is `u16` on Darwin and `u32` on Linux.
- `libnfs` has **zero** reverse dependencies on crates.io.

**And the Kerberos build is broken upstream on macOS.** This is the finding that
decides the second half:

- libnfs's Kerberos support exists **only in the autotools build**. The CMake
  path gates `find_package(GSSAPI)` behind `if(IOS)` and ships no
  `FindGSSAPI.cmake`, so `HAVE_LIBKRB5` can never be defined through CMake on
  any platform.
- No prebuilt package has it. Homebrew's `libnfs` 7.0.1 is built through CMake
  and links `libSystem` only; Debian and Ubuntu ship 5.0.2, which predates
  Kerberos landing in 6.0.0 entirely. Only Fedora builds it with `krb5-devel`.
- **The Kerberos path is never compiled in CI on any platform.** libnfs's CI is
  cmake-only, and cmake can never define `HAVE_LIBKRB5`. Seven commits have ever
  touched `krb5-wrapper.c`, and no test references krb5. The implementation
  itself is real — `gss_get_mic`, `gss_verify_mic`, `gss_wrap`, `gss_unwrap` are
  all genuinely wired through `lib/pdu.c` — but it is untested code that only
  Fedora builds.
- Building it with Kerberos on macOS **fails**: `lib/krb5-wrapper.h` includes
  MIT-style `gssapi/gssapi.h` while `lib/krb5-wrapper.c` includes Apple's
  `GSS/GSS.h`, and the two define incompatible `gss_ctx_id_t` types in one
  translation unit. Past that, the link fails on Heimdal-internal OID symbols
  because configure hardcodes `-lgssapi_krb5` where macOS needs
  `-framework GSS`. Both are real upstream bugs and would be our patch to carry.
- Kerberos and a self-contained static binary are close to mutually exclusive.
  Apple ships no static GSS; MIT krb5 `dlopen`s its plugins by path at runtime,
  so a "static" build silently keeps a filesystem dependency on the host's krb5
  tree and `/etc/krb5.conf`; MIT's own maintainers call static builds "mostly
  unsupported" and no distro ships `libkrb5.a`. The shape that actually works
  is static libnfs plus a *dynamic* system GSS.

**`nfs_set_security()` fails silently on a build without Kerberos, and every
easy build is one.** The function returns `void`, and its entire body sits
inside `#ifdef HAVE_LIBKRB5`:

```c
void nfs_set_security(struct nfs_context *nfs, enum rpc_sec sec) {
#ifdef HAVE_LIBKRB5
        nfs->rpc->wanted_sec = sec;
#endif
}
```

On a Kerberos-less build the symbol still exports, the call links, runs, does
nothing, reports nothing, and **the mount proceeds over AUTH_SYS**. Homebrew's
bottle is exactly such a build — it exports `_nfs_set_security` and contains
zero GSS symbols. So the most likely macOS install path asks for `krb5p`,
is told nothing, and gets no authentication at all.

This project's rule is that `writable` comes from the server and is never
inferred; a security label that silently overstates itself is the same failure
in a worse place. If libnfs is ever adopted, **the URL form is the capability
probe** — `sec=` falls through to "Unknown url argument" and returns -1 —
and `nfs_set_security()` must never be trusted on its own.

**And libnfs establishes GSS for the NFS program only.** `lib/libnfs.c` gates
the handshake on `program == 100003`, so under NFSv3 the MOUNT (100005) and
portmap traffic still goes out as AUTH_UNIX — unlike a Linux kernel client.
NFSv4 has no MOUNT protocol, so the problem disappears there. That is a second,
independent reason Kerberos belongs with v4 rather than bolted onto v3.

Without Kerberos, by contrast, libnfs vendors beautifully: CMake with
`BUILD_SHARED_LIBS=OFF -DENABLE_MULTITHREADING=ON` produces a 528 KB `libnfs.a`
depending on `libSystem` alone. That asymmetry is the whole argument for
splitting the story.

### Route B — a minimal v4.0 client with `gssapi`

**There is no Rust crate that implements RPCSEC_GSS.** Not partially, not behind
a feature flag. The ecosystem is uniformly AUTH_NULL/AUTH_SYS; a handful of
crates define the flavour constant `6` for recognition and implement nothing.

The crate this project depends on is worse than merely lacking it. `nfs3_types`
0.5.2's `auth_flavor` stops at `AUTH_DES = 3`, and its derived codec generates
`_ => Err(InvalidEnumValue)` — so flavour 6 is **neither constructible nor
parseable**. Tested: feeding it a reply whose verifier carries flavour 6 fails
with `Invalid enum value: 6`. The crate cannot read a reply from a
Kerberos-secured server at all. And `nfs3_client`'s apparent hook is not one:
`credential` and `verifier` are set once at construction and replayed verbatim
on every call, where RPCSEC_GSS needs both to change per call — the sequence
number increments, and the verifier is a MIC over *that call's* header.

So this route means writing RFC 2203 from scratch. Reading the RFC, that is:

- nine new XDR types, a seven-state context machine, the
  `RPCSEC_GSS_INIT`/`CONTINUE_INIT` handshake over `NULLPROC`, an authenticated
  `DESTROY`, sequence-window management, and three separate body encodings for
  `none`/`integrity`/`privacy`;
- somewhere between **1700 and 5000 lines excluding tests** — two independent
  readings of the RFC landed at 1700–2500 and 3000–5000, and the spread is
  itself informative: this is not a task anyone can size confidently from the
  spec. The failure modes that matter — silent discard below the window, the
  rule that a retransmit must use a *new* sequence number with the same xid, the
  MIC boundary at the credential/verifier seam, and the integrity checksum
  covering the inner stream but *not* its length prefix — are all invisible
  against a cooperative server;
- an architectural constraint rather than just volume: the whole argument body
  must be serialised into a buffer before it can be MIC'd or wrapped, so a
  zero-copy streaming encoder cannot express it, and the sequence number must be
  plumbed *into* the RPC retry layer rather than sitting under it;
- **two forks**: `nfs3_types`, to add the flavour, and `nfs3_client`, because
  the header MIC must cover "the RPC header up to and including the credential"
  and the crate packs and writes its header internally with no seam for that.

The mechanism layer does exist and resolves cleanly. **`libgssapi` 0.11.0 is
MIT, builds on a stock Mac with no setup** — its build script detects Apple and
links `-framework GSS`, so `brew install krb5` is not needed — and it exposes
every primitive RFC 2203 requires, including `get_mic`/`verify_mic` and
`wrap`/`unwrap`, plus granted-flag inspection. Two things to pin if it is ever
used: set `LIBGSSAPI_IMPL=apple` on macOS, because pkg-config is probed first
and a developer with Homebrew's keg-only krb5 on `PKG_CONFIG_PATH` will
silently link a dylib no end-user Mac has; and leave the `iov` feature off,
since Apple's GSS has no `gss_wrap_iov`.

The alternatives were checked and rejected: `cross-krb5` has **no MIC API at
all** (the whole trait is wrap/unwrap), which rules it out for every service
level including `svc_none`. `sspi` cannot read a ticket cache — and does not
even resolve in this workspace, pinning `curve25519-dalek =5.0.0-rc.1` against
russh's `^5`, which is precisely the failure documented in
[ADR 0001](0001-smb-client.md). `krb5-gss` 0.2.0 is a genuine pure-Rust
near-miss with real MIC and wrap primitives, but its ccache support is an
unimplemented stub, so it cannot act as an already-`kinit`'d user. What does not exist is the layer above them. There is exactly one
RFC 2203 implementation in Rust anywhere — `ddalton/flint`, unpublished, zero
stars, and the *acceptor* half, so it answers `RPCSEC_GSS_INIT` and never sends
one. Useful to read; not a dependency.

One asymmetry worth noting in Route B's favour: `libgssapi`'s build script
already resolves `-framework GSS` on macOS automatically. The exact linking
problem that breaks libnfs's Kerberos build there is solved in the pure-Rust
route.

### Could an existing NFSv4 crate be adopted instead of written?

The AC says "a minimal v4.0 client", which raises the question. Six crates
attempt one; none is adoptable, and the reasons are specific rather than
general unease:

- **`nfs-rs` 0.6.1** is the most complete — real slot table, lease renewal,
  session recovery, pNFS. It also encodes `OpenAttr = 20`, and OPENATTR is 19;
  20 is OPEN_CONFIRM, which does not exist in minor version 1. Its own unit
  test asserts the wrong value, which is proof that the xattr path has never
  run against a real server. All 27 integration tests are `#[ignore]`d behind
  the author's private lab.
- **`nfs` 0.1.0** is the cleanest codebase but uses a **single hardcoded slot**,
  so no request parallelism at all, and has no locking. Its author tagged 0.1.4
  in May; the publish never landed and crates.io still shows 0.1.0.
- **`nfs-crust` 0.1.0**'s repository was archived the same day it was created.
- **`nfs4_client` 0.1.0** never sends CLOSE — it leaks open state for every
  file it creates — and has no handling for `NFS4ERR_DELAY`, `GRACE`,
  `BADSESSION` or `STALE_CLIENTID`, so it dies on the first one.
- **`nfs-v4` 1.2.0** is v4.0 only and says so; it is the engine behind a
  pentest tool.

And the crate this project already depends on will not grow into it:
**`nfs3_client` has no NFSv4 work in any branch or issue.** It is a well-run
NFSv3-only project and shows no sign of becoming otherwise — which is fine,
because NFSv3 is what it is depended on for.

The one genuinely reusable piece is `nfs4` 0.1.0, a types-only crate covering
all 51 v4.1 operations and 102 error codes correctly. If a v4 client is ever
written here, that is the place to start rather than hand-transcribing the XDR.

**And if the pure-Rust route is taken, do not fork `nfs3_client` for it.**
`onc-rpc-client` and `nfs-v4` (both Apache-2.0) are a better host: their wire
types already carry flavour 6 and the GSS `auth_stat` codes, they already
decode SECINFO's `rpcsec_gss_info`, and they expose a **public `RpcTransport`
trait** with a `call_as(cred, …)` method that a third party can implement — so
the RPCSEC_GSS layer could be written *outside* the crate rather than as a
fork of two. That is also the NFSv4 path, where Kerberos is mandatory and the
protocol is designed for it. The trade is giving up a working, well-maintained
NFSv3 backend for a less proven v4 stack, which is why it is a recommendation
for a future story and not a reason to move now.

## Two corrections to the story as written

Reading the NFSv4 RFCs turned up two things that change the advice above, and
both are worth carrying into whatever picks this up.

**Target NFSv4.1, not 4.0 — but only on the pure-Rust route.** The AC says "a
minimal v4.0 client". 4.1 is *materially cheaper to implement securely*, for two
specific reasons below. The catch, found late: **libnfs cannot do 4.1 at all.**
`nfs_set_version()` accepts only v3, v4.0 and v4.2; every *release* is v4.0
only, and the v4.2 support that exists on unreleased master is single-slot
(`ca_maxrequests = 1`), which serialises every COMPOUND. So Route A means v4.0,
and this advice applies to Route B.

- In 4.0, recovering from `NFS4ERR_WRONGSEC` after `PUTFH`, `RESTOREFH`, `LINK`
  or `RENAME` requires re-issuing `SECINFO` against the **parent** directory and
  the failing component's name — so the client must maintain a reverse map from
  every filehandle it might `PUTFH` back to *(parent FH, name)*. And after
  `PUTROOTFH`/`LOOKUPP` there is no way to ask at all, so the client must
  iterate blindly through its own list of triples. 4.1's `SECINFO_NO_NAME`
  removes both.
- In 4.0 the callback is a **separate RPC connection in the reverse direction**,
  and under Kerberos the server authenticates to the client using the
  SETCLIENTID flavour. That means a 4.0 client must implement the GSS
  **acceptor** side — `GSS_Accept_sec_context`, its own handle table, its own
  replay window — which roughly doubles the RPCSEC_GSS surface. 4.1 moves
  callbacks onto the existing session's backchannel and the client is never an
  acceptor.

  *libnfs sidesteps this by refusing to play*: it declines the callback channel
  outright, sending `cb_program = 0` and `r_addr = "0.0.0.0.0.0"` under an
  upstream TODO reading "Just give it garbage for now until we figure out how we
  should solve this." So Route A never needs an acceptor — and never gets
  delegations either. `OP_SECINFO`, `OP_RENEW` and `OP_DELEGRETURN` are likewise
  absent, so the standard way to discover an export's required flavour is not
  available through it.

**NFSv4 mandates Kerberos support, which weakens the clean split above.**
RFC 7530 §3.2.1: "NFSv4 clients and servers MUST support the Kerberos V5
security mechanism", and §3.2 requires RPCSEC_GSS to be *implemented* even
though deploying it is optional. So a v4 client that speaks only AUTH_SYS is
non-conformant to that MUST, even though it will interoperate with any server
configured for AUTH_SYS. The "do v4 first, Kerberos later" ordering is still the
right one on cost — but it should be taken as a deliberate, documented
deviation rather than as a complete NFSv4 client.

**Do not implement RPCSEC_GSS v2 (RFC 5403).** It adds only GSS channel
bindings, and the entire installed base is v1: Linux (client and server),
libtirpc and FreeBSD all hardcode version 1 and their enums stop at v1's
values. A client sending `rgc_version = 2` gets `AUTH_REJECTEDCRED` and falls
back, having spent a round trip. RFC 7861's v3 has essentially no deployment
either.

For sizing: NFSv4.1 with Kerberos over RPCSEC_GSS v1 is roughly **17 new XDR
types and 19 client code paths**, about eleven of which sit below the NFS layer
and would be reusable. The three costs most likely to be underestimated are the
call verifier being a MIC over the encoded RPC header prefix — which constrains
the XDR encoder's architecture rather than adding to it — the 4.0 reverse map
above, and the 4.0 acceptor.

## Why this is a defer rather than a "no"

Nothing here is a dead end, and two of the three obstacles are other people's
bugs rather than design problems:

- The bindgen pin is a one-line fix in a fork.
- The macOS GSSAPI build failure is a small upstream patch.
- Only the missing RPCSEC_GSS layer is genuinely large, and it is only needed if
  Route B is chosen — Route A gets Kerberos from C.

What makes this a defer is that E5-S5 is explicitly not a v1 blocker, and every
route currently starts with adopting and maintaining a fork of something.

## Revisit if

- **A user actually needs NFSv4.** Then do Route A's first half only: fork
  `libnfs-sys` onto current bindgen, write a thin safe layer against libnfs 7.x
  (the existing wrapper's API is pinned to functions 7.x removed, so it is not a
  starting point), and build it statically through CMake with multithreading on.
  No Kerberos, no system package, no licence consequence — and note this is a
  documented deviation from RFC 7530's MUST, not a complete v4 client.
- **A user actually needs `krb5`/`krb5i`/`krb5p`.** Then the macOS header
  collision and the `-framework GSS` link have to be fixed upstream first, or
  carried. Prefer sending them upstream — libnfs is actively maintained and the
  patches are small.
- **`sspi` or another crate ships RPCSEC_GSS.** That would make Route B real,
  and would be worth reassessing against Route A on merit rather than on
  availability.

## What the window says meanwhile

Nothing that promises any of it. E3-S1 deliberately did not build the NFS
Version and Security controls, on the grounds that "a disabled control that will
never enable in this build is a promise". `security_label` reads `sys`, which is
what AUTH_SYS actually is — an assertion the server chooses to believe. That
remains the honest state and needed no change.
