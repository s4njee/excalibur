#!/bin/sh
set -e
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
