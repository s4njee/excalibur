# Benchmarks

Each section says how to reproduce it.

## Listing round trips over xsync (XTP-E9-S3)

Two panes and sync browsing list twice as often as one pane does, so the story
asked for the listing that took two round trips to take one. `ReadDirPath`
(type 98) answers what `Open` + `ReadDir` + `Close` answers, and the server
advertises `PATH_LISTING`.

Reproduce with:

```text
EXCALIBUR_XS=../xsync/target/debug/xs \
  cargo test -p excalibur-engine --test bench_listing -- --ignored --nocapture
```

*2026-09-09. macOS on Apple silicon, `xs --server` over a local pipe, a
directory of 200 files, each listing asking for the same attribute mask.*

| Path | Frames per listing | Round trips |
|---|---|---|
| `Open` + `ReadDir` + `Close` (before) | 6 — `Open`, `Opened`, `ReadDir`, `DirPage`, `Close`, `Done` | **3** |
| `ReadDirPath` (now) | 2 — `ReadDirPath`, `DirPage` | **1** |

**Wall-clock, over a local pipe: 1.09–1.41 ms per listing either way.** The two
are inside each other's noise there, and that is the honest reading rather than
a disappointing one: a round trip over a pipe on the same machine costs
microseconds, so the time is dominated by the syscalls and the serialisation
that both paths do identically. What changed is the number of times a listing
has to wait for the other end, which is the cost that is paid in latency the
moment the other end is another machine — three waits per listing became one,
and sync browsing doubles how often that happens.

The frame counts come from the message log's own tap (XTP-E5), taken from a
single listing: the log is a bounded ring and two hundred listings overflow it,
which would report a number capped by the ring rather than by the exchange.

## NFSv3 read throughput (E5-S4)

E5-S4 asks for read throughput at **≥ 80% of the kernel client on the same
pair**. This is the measurement, and the caveat about "the same pair" is the
most important part of it.

*2026-09-04. macOS on Apple silicon, Docker Desktop, Linux `nfsd` in a
container, 128 MiB sequential read, `rsize=1048576`. Each read uses a file that
client has not read before, so no number is measuring the reading client's own
page cache; the server's cache is warm for both.*

| Client | Path | Throughput |
|---|---|---|
| Linux kernel NFS client | direct to the server container | 1.3, 1.5 GB/s |
| **This client** | **via the published port** | **950, 1153 MiB/s** (≈1.10 GB/s) |
| Linux kernel NFS client | via the published port (`host.docker.internal`) | 442, 491, 618 MB/s |
| Linux kernel NFS client | via the published port, `nconnect=4` | 471, 491 MB/s |

**Result: ≈79% of the kernel client's fastest path**, while traversing a port
forward that path does not. Read against the kernel on a comparable hop count
it is well ahead; read against the kernel at its best it is a shade under the
bar. Both readings are in the table because picking one would be a choice about
what to claim rather than what was measured.

### Why there is no clean number

The two clients cannot be put on the same network path on this machine, and
that — not noise — is what limits the comparison:

- **This client** runs on the macOS host and reaches the server through Docker
  Desktop's published port.
- **The kernel client** cannot run there: mounting NFS on macOS needs root, and
  this benchmark will not ask for it. So it runs in a container, where it can
  either reach the server *directly* (a shorter path than this client gets) or
  hairpin out to `host.docker.internal` and back in (a longer one).

The direct row is therefore the strictest available baseline and the one quoted
above. The hairpinned rows are included because they are what an
"apples-to-apples port" comparison would naively produce — and at ~2.2× they
would flatter this client considerably, which is why they are not the headline.

`nconnect=4` tells us something useful: giving the kernel client four
connections changes nothing on that path, so its ceiling there is the hairpin
rather than connection count. That also means this client's lead over the
hairpinned rows is not simply "four connections against one".

### What these numbers do not show

- **Nothing about a real network.** Client and server share one VM; this is
  memory bandwidth over loopback. On a link with real latency, NFS is
  round-trip bound and the four-deep pipelining matters far more than it does
  here — but that is an expectation, not a measurement.
- Run-to-run variance is large (this client ranged 850–1234 MiB/s across the
  session, the kernel client 442–618 MB/s on one path). Single runs are not
  meaningful; the table quotes consecutive samples.
- Writes are not measured. Reads are pipelined and writes are not — they are
  sequential by design, because a pipelined write that failed halfway would
  leave holes — so the write figure would be a different story and is not
  claimed here.

## xsync download throughput (E13-S7)

E13-S7 asks for a 1 GiB download in a **release** build at **≥ 70% of the raw
`ssh cat` figure measured the same day**. It reaches ~100%, and the bar turned
out to be the wrong worry: the link is what limits both.

*2026-09-05. macOS on Apple silicon over gigabit Ethernet to `orion.local`, a
Raspberry Pi 5 running `xs --server` over SSH. Export root `/srv/strife`, file
`excalibur-bench/big-1g.bin` (1 GiB, sha256 `571759409deca6cf4404…`). Release
build. Every figure is one 1 GiB transfer; the timer starts after the mount, so
it measures the transfer and not the SSH handshake.*

```
cargo run --release --bin xcal -- get excalibur-bench/big-1g.bin /tmp/big.bin \
    --xsync-ssh orion.local:/srv/strife --user root --depth 4
```

### Reads in flight

| Reads in flight | 1 GiB in | Throughput |
|---|---|---|
| 1 *(what the worker did before this story)* | 38.9 s | 27.6 MB/s |
| 2 | 24.7 s | 43.5 MB/s |
| **4 (the default)** | **16.3 s** | **66.1 MB/s** |
| 6 | 17.5 s | 61.5 MB/s |
| 8 | 17.1 s | 62.7 MB/s |
| 16 | 17.9 s | 59.9 MB/s |

**Depth 1 is the honest before-number: 27.6 MB/s.** (The 22 MB/s recorded from the
GUI earlier the same day was a debug build, which is why it is lower still.) Four
reads in flight is 2.4× that, and everything past four is flat or slightly
worse — so the default is 4 rather than 8. Past the plateau, depth only buys a
megabyte of buffer per read in flight and a longer tail on a cancel.

### Against raw `ssh cat`

Interleaved, alternating one run of each, because the link moved by ±10% over
the session and two figures taken an hour apart would be comparing times of day:

| Run | `ssh root@orion.local cat … \| dd of=/dev/null bs=1m` | `xcal get --depth 4` |
|---|---|---|
| 1 | 58.5 MB/s | 63.6 MB/s |
| 2 | 62.1 MB/s | 61.8 MB/s |
| 3 | 64.1 MB/s | 61.9 MB/s |
| mean | **61.6 MB/s** | **62.4 MB/s** |

**≈101% of raw `ssh cat`**, against a bar of 70%. Three earlier `ssh cat` runs
the same morning read 99.2, 65.2 and 61.9 MB/s; the 99.2 is the only sample
anywhere near that, and treating a single unrepeatable outlier as the baseline
would be choosing a number rather than measuring one. The interleaved table is
the comparison that means something.

What this says is that the transfer is no longer round-trip bound — it is
bound by the same thing `ssh` is, which on a Pi 5 is the SSH cipher and the
server's own read path. And this is with every chunk BLAKE3-verified end to
end, which `ssh cat` does not do.

### What these numbers do not show

- **Nothing about many small files.** One 1 GiB file is the best case for
  pipelining: the depth is inside a file, so a thousand 1 MB files still pay
  one open each, serially, three transfers at a time.
- The server's page cache is warm for both sides of the comparison by the
  second run. Neither figure is a cold-disk figure.
- Nothing about SFTP, NFS or SMB. They keep the wrapper around `read_range`,
  so their transfers are exactly as fast — or as slow — as they were.

## Local↔xsync transfers through xsync's engine (E22-S1)

E22-S1 asks for a 1 GiB file, a `congress`-shaped tree of small files, and a
tree with a symlink and an empty directory, measured against orion and freya.
**orion.local does not resolve any more** — `xcal reachable orion.local 22` says
unreachable, `ping` cannot resolve the name, and a Raspberry Pi 5 is advertising
on the network as `gentoo-rpi5.local` whose SSH host key has *changed* since it
was last recorded. Overriding that is the user's decision and not a benchmark's,
so it was left alone. What is measured instead is freya for the network figure,
read-only, and a local `xs --server` for everything that needs a push.

*2026-09-05. macOS 26.6.2 on an Apple M1 Max, 64 GiB. Release build. Fixtures:
a 1 GiB file of `/dev/urandom` bytes (sha256 `bf78be5575e3e744c672…`) so zstd
cannot shrink it and the figures are real payload; a `congress` shape of 10,000
× 8 KB random files in 20 directories (78 MB); and a tree holding one file, a
symlink, a **dangling** symlink and an empty directory. The timer starts after
the mount, so it measures the transfer and not the SSH handshake.*

### Over a local `xs --server`

No network at all: this is the engine, the disk and the page cache, and it is
the only place a **push** could be measured with orion gone. Read it as an upper
bound on the route, not as a link figure.

```
xcal put <source> <remote path> --xsync-local <export> --xs <xs> --xsync-engine
```

| Workload | Time | Throughput |
|---|---|---|
| 1 GiB single file | 4.68, 4.48, 4.64 s | 229, 239, **232 MB/s** |
| `congress`: 10,000 × 8 KB (78 MB) | 1.67, 1.77, 1.53 s | 49.2, 46.3, **53.7 MB/s** |
| `congress` again, destination identical | 0.10 s | nothing moved — 10,000 files unchanged |
| symlink + dangling symlink + empty directory | 0.02 s | — |

The small-file number is the one worth reading: **≈5,900 files a second**,
including a BLAKE3 digest computed and checked for every one of them. The
re-run is the other one — a second pass over an unchanged tree of ten thousand
files costs a tenth of a second and moves nothing, which is what makes "resume
by re-running" cheap.

The shapes tree arrives intact: `link.txt -> real.txt` and
`dangling -> ../../nowhere` are both recreated **as symlinks**, not followed,
and the empty directory is created. A dangling link is the interesting one,
because a route that resolved links would have to fail on it.

### Small files, against the chunk loop

The same 10,000-file tree pulled back, interleaved, one run of each: `xcal get`
with `--xsync-engine`, and without it, which is E13-S7's worker moving one file
at a time through `read_range`.

| Run | worker (`--depth 4`) | xsync engine |
|---|---|---|
| 1 | 119.90 s *(0.7 MB/s)* | 4.04 s *(20.3 MB/s)* |
| 2 | 150.72 s *(0.5 MB/s)* | 3.14 s *(26.1 MB/s)* |
| 3 | 118.52 s *(0.7 MB/s)* | 2.58 s *(31.8 MB/s)* |
| mean | **129.7 s** | **3.25 s** |

Both landed all 10,000 files. **The xsync route is ≈40× faster on this shape**,
and the gap is not throughput, it is per-file cost: the worker pays one open,
one stat, one `.part`, one fsync and one rename each, ten thousand times, three
files at a time. xsync sends the same tree as batched frames and pays none of
it. This is the clearest argument the story has, and it is why E22-S2 wants
every backend behind the same planner rather than only this pair.

### Over SSH, against the same tree

The only real-network figure available: a 1.25 GiB tree of two files pulled from
`freya.local:/mnt/raid6`, read-only, interleaved one run of each because the
link moved between runs.

```
xcal get "movies/<tree>" /tmp/dest --xsync-ssh freya.local:/mnt/raid6 --xsync-engine
```

| Run | worker (`--depth 4`) | xsync engine |
|---|---|---|
| 1 | 37.6 MB/s | 30.2 MB/s |
| 2 | 33.6 MB/s | 31.2 MB/s |
| 3 | 40.3 MB/s | 26.3 MB/s |
| mean | **37.2 MB/s** | **29.2 MB/s** |

Both copies were digest-identical to each other. **On one large file over this
link the xsync route is about 20% slower than the chunk loop**, and that is
reported rather than explained away: E13-S7 tuned reads-in-flight against
exactly this shape and got the depth right, while the v1/v2 route is
single-stream by construction — `streams` is encoded and dropped, because
multi-stream push spawns its own `ssh` processes and is not available to a
caller that owns the session. The trade is a route that is 40× faster on ten
thousand small files and 20% slower on one big one.

### What these numbers do not show

- **Nothing comparable to E13-S7's 66 MB/s.** That was gigabit Ethernet to a
  Pi 5. `en0` here reports `media: autoselect` with no negotiated speed and no
  `<full-duplex>`, which on this machine means Wi-Fi. The worker column above is
  the honest same-day baseline; the E13-S7 figure is not.
- **No push over a network at all.** orion is gone and freya is pull-only by
  instruction, so every push figure is over a pipe on one machine.
- **The local figures are page-cache figures** in both directions, on a machine
  with 64 GiB. A 4 GiB push completed in under four seconds on a second run,
  which is memory bandwidth rather than a transfer.
- **Resume after a cancel was not timed.** It could not be caught: the local
  route moves a gigabyte in under five seconds, so a kill is a race. What *is*
  verified, by `crates/engine/tests/xsync_transfer.rs`, is the part that
  matters — a cancel kills the child and leaves nothing under the destination's
  final name, only the `.xsync.tmp.<hex>` stage the next run resumes from.

## SMB and SFTP

Not measured. E4-S4 asks for a comparison against `smbutil`/`mount_smbfs`,
which has the same problem in a sharper form: mounting SMB on macOS needs root
too, and libsmbclient serialises every call in the process behind one mutex, so
the interesting number is contention rather than throughput.
