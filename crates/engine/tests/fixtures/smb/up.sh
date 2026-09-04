#!/bin/sh
# Build and run the SMB fixture. Idempotent: replaces any existing container.
#
# Port 445 is not negotiable: libsmbclient offers no way to set the port, so
# the server has to be on the standard one. macOS leaves it free unless File
# Sharing is on.
set -e

docker rm -f excalibur-samba >/dev/null 2>&1 || true

# A named volume, not a host bind mount. Docker Desktop's virtiofs cannot
# support the delete and rename operations Samba needs: `smbclient` itself
# answers NT_STATUS_NOT_SUPPORTED for both on a bind-mounted share, so a
# fixture built that way tests virtiofs rather than SMB. The NFS fixture hit
# the same wall for the same reason.
for name in public readonly; do
  docker volume rm "excalibur-smb-$name" >/dev/null 2>&1 || true
  docker volume create "excalibur-smb-$name" >/dev/null
done

docker run -d --name excalibur-samba -p 445:445 \
  -v excalibur-smb-public:/share/public \
  -v excalibur-smb-readonly:/share/readonly \
  dperson/samba:latest \
  -u "excalibur;excalibur1" \
  -s "public;/share/public;yes;no;no;excalibur;excalibur;excalibur" \
  -s "readonly;/share/readonly;yes;yes;no;excalibur" \
  -p >/dev/null

printf 'waiting for smbd'
i=0
while [ $i -lt 30 ]; do
  if docker exec excalibur-samba smbclient -L //localhost -U excalibur%excalibur1 >/dev/null 2>&1; then
    break
  fi
  printf .
  i=$((i + 1))
  sleep 1
done

# Seed inside the container so the files live on the volume.
docker exec excalibur-samba sh -c '
  printf "hello from samba\n" > /share/public/notes.txt
  mkdir -p /share/public/Series
  dd if=/dev/urandom of=/share/public/blob.bin bs=1024 count=1024 2>/dev/null
  printf "you may not write here\n" > /share/readonly/fixed.txt
  chmod -R 777 /share/public
  chmod -R 555 /share/readonly
' >/dev/null

echo " — ready"
docker exec excalibur-samba smbclient -L //localhost -U excalibur%excalibur1 2>/dev/null | head -6
echo
echo "export EXCALIBUR_SMB=127.0.0.1"
