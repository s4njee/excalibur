# The SMB test fixture

A Samba server with a writable and a read-only share. Run it by hand, then:

```sh
crates/engine/tests/fixtures/smb/up.sh
export EXCALIBUR_SMB=127.0.0.1
cargo test -p excalibur-engine --features smb --test smb
```

The tests are compiled only under the `smb` feature, which is the same feature
that makes the binary GPL-3.0-or-later — see `crates/engine/src/backend/smb.rs`
and `docs/decisions/0001-smb-client.md`.

## The reconnect tests kill the server

Two of them (E4-S6) `docker kill` the container to prove the mount notices and
comes back. Every other test in the file would fail while it is down, and
`cargo test` runs them in parallel, so they are opt-in and serial:

```sh
EXCALIBUR_SMB=127.0.0.1 EXCALIBUR_SMB_DISRUPTIVE=1 \
  cargo test -p excalibur-engine --features smb --test smb -- --test-threads=1
```

A `Drop` guard restarts the container and waits for it, so a failed assertion
cannot leave it down and break every later run.

## Port 445, and why there is no choice

`libsmbclient` offers no way to set the port, so the server must be on the
standard one. macOS leaves 445 free unless File Sharing is turned on; if the
container cannot bind it, that is why.

## Why a named volume rather than a bind mount

Docker Desktop's virtiofs cannot support the operations Samba needs for delete
and rename: `smbclient` itself answers `NT_STATUS_NOT_SUPPORTED` for both on a
bind-mounted share, so every mutation test fails against a server that is
working correctly. The NFS fixture hit the same wall — there it showed up as
`NFS3ERR_STALE` and as `root_squash` silently doing nothing.

Nothing outside the container needs to read these files: the tests check
through the backend, which is the thing under test.

| Share | Options | What it proves |
|---|---|---|
| `public` | writable | Listing, ranged reads, writes, resume-at-offset, mutations. |
| `readonly` | `readonly = yes` | `writable` comes back false with a reason. There is no share-level flag readable through libsmbclient, so the backend learns this by trying to create a file. |
