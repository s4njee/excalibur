#!/bin/sh
# Build and run the NFSv3 fixture. Idempotent: replaces any existing container.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=${EXCALIBUR_NFS_ROOT:-/tmp/excalibur-nfs}

mkdir -p "$root/rw" "$root/ro" "$root/squash" "$root/secure"
[ -e "$root/rw/notes.txt" ] || echo "hello from nfs" > "$root/rw/notes.txt"
[ -d "$root/rw/Series" ] || mkdir -p "$root/rw/Series"
[ -e "$root/rw/blob.bin" ] || head -c 1048576 /dev/urandom > "$root/rw/blob.bin"
[ -e "$root/ro/fixed.txt" ] || echo "you may not write here" > "$root/ro/fixed.txt"
[ -e "$root/squash/owned.txt" ] || echo "root is squashed here" > "$root/squash/owned.txt"
[ -e "$root/secure/gated.txt" ] || echo "needs a privileged client port" > "$root/secure/gated.txt"

# Docker Desktop presents host bind mounts as owned by uid 0 inside the VM, so
# the mode bits are the only thing an ordinary client's uid can satisfy. The
# writable export is opened up accordingly; without this it would refuse the
# current user, and the test would be measuring the fixture rather than the
# backend. `squash` is deliberately left at 755/root so that a client claiming
# uid 0 is mapped to nobody and refused — which is the case E5-S3 has to tell
# apart from a plain `ro` export.
chmod 777 "$root/rw" "$root/rw/Series"
chmod 666 "$root/rw/notes.txt" "$root/rw/blob.bin"
chmod 755 "$root/squash"

docker build -q -t excalibur-nfs:local "$here" >/dev/null
docker rm -f excalibur-nfs >/dev/null 2>&1 || true
docker run -d --name excalibur-nfs --privileged \
  -p 11111:111 -p 12049:2049 -p 20048:20048 \
  -v "$root/rw":/exports/rw \
  -v "$root/ro":/exports/ro \
  -v "$root/squash":/exports/squash \
  -v "$root/secure":/exports/secure \
  excalibur-nfs:local >/dev/null

printf 'waiting for mountd'
i=0
while [ $i -lt 30 ]; do
  if docker exec excalibur-nfs showmount -e localhost >/dev/null 2>&1; then
    echo " — ready"
    docker exec excalibur-nfs showmount -e localhost
    echo
    echo "export EXCALIBUR_NFS=127.0.0.1:12049"
    exit 0
  fi
  printf .
  i=$((i + 1))
  sleep 1
done
echo " — gave up"
docker logs excalibur-nfs 2>&1 | tail -20
exit 1
