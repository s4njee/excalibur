#!/bin/sh
# Build and run the NFSv3 fixture. Idempotent: replaces any existing container.
set -e
here=$(cd "$(dirname "$0")" && pwd)
docker build -q -t excalibur-nfs:local "$here" >/dev/null
docker rm -f excalibur-nfs >/dev/null 2>&1 || true

# Named volumes, not the container's own filesystem and not host bind mounts.
# A volume is a directory on the VM's real ext4, which is the only one of the
# three that can be NFS-exported at all: overlayfs is refused outright
# ("does not support NFS export") and virtiofs has no stable file handles.
# Recreated each run so a previous run's files cannot change a result.
for name in rw ro squash secure; do
  docker volume rm "excalibur-nfs-$name" >/dev/null 2>&1 || true
  docker volume create "excalibur-nfs-$name" >/dev/null
done

docker run -d --name excalibur-nfs --privileged \
  -p 11111:111 -p 12049:2049 -p 20048:20048 \
  -v excalibur-nfs-rw:/exports/rw \
  -v excalibur-nfs-ro:/exports/ro \
  -v excalibur-nfs-squash:/exports/squash \
  -v excalibur-nfs-secure:/exports/secure \
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
