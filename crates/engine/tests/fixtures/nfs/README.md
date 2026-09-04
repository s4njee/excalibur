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

`up.sh` is idempotent — it rebuilds and replaces the container, and the
entrypoint reseeds any export that is empty.

## The exports, and what each one is for

| Export | Options | What it proves |
|---|---|---|
| `/exports/rw` | `rw,no_root_squash,insecure` | The ordinary case: listing, reads, writes, mutations. |
| `/exports/ro` | `ro,no_root_squash,insecure` | `writable` must come back false with a reason naming the export, not the user's request. |
| `/exports/squash` | `rw,root_squash,insecure` | Writes are refused even though the export says `rw`, because uid 0 maps to `nobody`. E5-S3 wants that distinguished from a plain `ro` export. |
| `/exports/secure` | `rw,no_root_squash` (no `insecure`) | The server demands a client source port below 1024. E5-S2 requires a clear message here and explicitly forbids trying to bind a privileged port. |

## Why the exports are not host bind mounts

They live on the container's own filesystem and are seeded by the entrypoint.
Bind-mounting them from the host was the obvious first shape and it was wrong:
Docker Desktop's virtiofs does not give NFS stable file handles, so writes fail
part-way through with `NFS3ERR_STALE`, it does not always return attributes in
`READDIRPLUS`, and it does not enforce ownership — which means `root_squash`
silently does nothing and the squash test passes for the wrong reason.
Exporting that would have been testing virtiofs rather than NFS.

Nothing outside the container needs to read these files: the tests check
through the backend, which is the thing under test.

## Identity, and why the modes are what they are

`/exports/rw` is opened to 777 so an ordinary client uid can write to it.
`/exports/squash` is deliberately left root-owned at 755: a client claiming
uid 0 is mapped to `nobody` there, so the write is refused even though the
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
