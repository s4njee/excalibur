# The NFS test fixture

An NFSv3 server with the four export shapes E5 has to tell apart. Built and run
by hand rather than by the test, because it needs `--privileged` and a Docker
build — too slow and too intrusive to do per test run. The tests skip unless
`EXCALIBUR_NFS` is set, the same way the xsync tests skip without `EXCALIBUR_XS`.

## Running it

```sh
crates/engine/tests/fixtures/nfs/up.sh
export EXCALIBUR_NFS=127.0.0.1:12049
cargo test -p excalibur-engine --test nfs
```

`up.sh` is idempotent — it rebuilds and replaces the container.

## The exports, and what each one is for

| Export | Options | What it proves |
|---|---|---|
| `/exports/rw` | `rw,no_root_squash,insecure` | The ordinary case: listing, reads, writes, mutations. |
| `/exports/ro` | `ro,no_root_squash,insecure` | `writable` must come back false with a reason naming the export, not the user's request. |
| `/exports/squash` | `rw,root_squash,insecure` | Writes are refused even though the export says `rw`, because uid 0 maps to `nobody`. E5-S3 wants that distinguished from a plain `ro` export. |
| `/exports/secure` | `rw,no_root_squash` (no `insecure`) | The server demands a client source port below 1024. E5-S2 requires a clear message here and explicitly forbids trying to bind a privileged port. |

## Identity, and why the modes are what they are

Docker Desktop shows host bind mounts as owned by uid 0 inside its Linux VM, so
an ordinary client's uid can only ever satisfy the *other* permission bits.
`up.sh` therefore opens `/exports/rw` to 777 — otherwise it would refuse the
current user and the test would be measuring the fixture, not the backend.

`/exports/squash` is left root-owned at 755 on purpose. A client that claims
uid 0 there is mapped to `nobody`, so the write is refused even though the
export says `rw`. That is the case E5-S3 must report differently from a plain
`ro` export, and claiming uid 0 is how a test reaches it — which is also why
E5-S2 makes the AUTH_SYS uid configurable rather than fixed.

`fsid=` is set per export because they are bind mounts of separate directories
rather than one filesystem, and `exportfs` otherwise refuses them.

NFSv4 is not disabled in the kernel's registration even with `-N 4`, but mountd
serves v3 only and the backend speaks v3 explicitly, so a passing test is not
ambiguous about which version answered.

UDP is off (`--no-udp`) and only TCP ports are published, which is why the host's
own `showmount` cannot query it — that tool defaults to UDP. E5-S2 requires TCP
only, so this matches what the backend will do.
