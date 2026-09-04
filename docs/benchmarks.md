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

## SMB and SFTP

Not measured. E4-S4 asks for a comparison against `smbutil`/`mount_smbfs`,
which has the same problem in a sharper form: mounting SMB on macOS needs root
too, and libsmbclient serialises every call in the process behind one mutex, so
the interesting number is contention rather than throughput.
