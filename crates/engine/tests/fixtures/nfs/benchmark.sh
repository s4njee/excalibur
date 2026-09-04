#!/bin/sh
# E5-S4's throughput comparison: this client against the Linux kernel NFS
# client, reading the same file from the same server.
#
# Run `up.sh` first. Takes a couple of minutes.
set -e
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../../.." && pwd)
size_mib=128

if ! docker ps --filter name=excalibur-nfs --format '{{.Names}}' | grep -q excalibur-nfs; then
  echo "the fixture is not running — start it with up.sh" >&2
  exit 1
fi

# Each read uses a file the reading client has not touched, so the number is
# not measuring that client's own page cache. The server's cache is warm for
# both, which is symmetric and is the case that matters anyway.
echo "seeding 4 x ${size_mib} MiB..."
docker exec excalibur-nfs sh -c \
  "for n in a b c d; do [ -f /exports/rw/cold-\$n.bin ] || { dd if=/dev/urandom of=/exports/rw/cold-\$n.bin bs=1M count=${size_mib} 2>/dev/null; chmod 666 /exports/rw/cold-\$n.bin; }; done"

nfsip=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' excalibur-nfs)

echo
echo "== Linux kernel client, direct to the server container =="
echo "   (no port forwarding — a shorter path than this client gets)"
docker run --rm --privileged alpine:3.20 sh -c "
  apk add --no-cache nfs-utils >/dev/null 2>&1
  mkdir -p /mnt/nfs
  mount -t nfs -o nfsvers=3,nolock,rsize=1048576 $nfsip:/exports/rw /mnt/nfs || exit 1
  for n in a b; do dd if=/mnt/nfs/cold-\$n.bin of=/dev/null bs=1M 2>&1 | tail -1; done
  umount /mnt/nfs" 2>/dev/null | tail -2

echo
echo "== this client, through the published port =="
cd "$repo"
for n in c d; do
  EXCALIBUR_NFS=127.0.0.1:12049 EXCALIBUR_NFS_BENCH_FILE="cold-$n.bin" \
    cargo test --release -q -p excalibur-engine --test nfs \
    -- --ignored --nocapture benchmark_read_throughput 2>/dev/null | grep excalibur-nfs-read
done

echo
echo "See docs/benchmarks.md for what these numbers do and do not show."
