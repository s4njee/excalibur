# Benchmarks

Reproduce with `crates/engine/tests/fixtures/nfs/benchmark.sh`, after `up.sh`.

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

## SMB and SFTP

Not measured. E4-S4 asks for a comparison against `smbutil`/`mount_smbfs`,
which has the same problem in a sharper form: mounting SMB on macOS needs root
too, and libsmbclient serialises every call in the process behind one mutex, so
the interesting number is contention rather than throughput.
