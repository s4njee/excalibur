#!/bin/sh
set -e
# Seed the exports on the container's own filesystem. They are deliberately
# NOT host bind mounts: Docker Desktop's virtiofs does not give NFS stable file
# handles (writes fail with NFS3ERR_STALE) and does not enforce ownership, so
# `root_squash` silently does nothing. Exporting it would test virtiofs rather
# than NFS. Nothing outside needs to read these files — the tests check through
# the backend.
seed() {
  [ -e /exports/rw/notes.txt ] || printf 'hello from nfs\n' > /exports/rw/notes.txt
  [ -d /exports/rw/Series ] || mkdir -p /exports/rw/Series
  [ -e /exports/rw/blob.bin ] || dd if=/dev/urandom of=/exports/rw/blob.bin bs=1024 count=1024 2>/dev/null
  [ -e /exports/ro/fixed.txt ] || printf 'you may not write here\n' > /exports/ro/fixed.txt
  [ -e /exports/squash/owned.txt ] || printf 'root is squashed here\n' > /exports/squash/owned.txt
  [ -e /exports/secure/gated.txt ] || printf 'needs a privileged client port\n' > /exports/secure/gated.txt
  # The writable export must admit an ordinary uid; the squash export must not,
  # so that a client claiming uid 0 is mapped to nobody and refused.
  chmod 777 /exports/rw /exports/rw/Series
  chmod 666 /exports/rw/notes.txt /exports/rw/blob.bin
  chmod 755 /exports/ro /exports/squash /exports/secure
}
seed

rpcbind -w
rpc.statd --no-notify
exportfs -rav
# v3 only. The backend speaks v3, and leaving v4 registered would
# make a passing test ambiguous about which version answered.
rpc.nfsd --no-udp --port 2049 -N 4 8
rpc.mountd --debug all --no-udp --port 20048 --no-nfs-version 4
echo "--- exports ---"; exportfs -v
echo "READY"
# Keep the container alive and surface mountd/nfsd logs.
tail -f /dev/null
