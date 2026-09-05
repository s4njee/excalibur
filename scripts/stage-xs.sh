#!/usr/bin/env bash
#
# stage-xs.sh — cross-compile `xs` from the sibling xsync checkout and install it
# on a Linux host, so Excalibur's xsync backend can run
# `PATH="$HOME/.local/bin:$PATH" xs --server <root>` over ssh.
#
#   scripts/stage-xs.sh [user@]host [options]
#
#   --target <triple>   force the Rust target instead of detecting it with
#                       `uname -m` over ssh. May carry a glibc suffix
#                       (aarch64-unknown-linux-gnu.2.28).
#   --glibc <version>   glibc version to link against (default 2.31). Ignored
#                       when --target already names one.
#   --xsync <dir>       the xsync checkout (default: ../xsync beside this repo,
#                       or $XSYNC_DIR).
#   --dest <dir>        where to install (default: /usr/local/bin when the
#                       remote login is root, else ~/.local/bin).
#
# It is safe to re-run: the binary lands on a temporary name next to its
# destination, is executed there to prove it runs on that machine, and only then
# replaces the existing one with `mv`. Nothing is ever recursively removed.
#
# Key authentication only — every ssh call uses BatchMode, so the script never
# blocks on a prompt. It fails, loudly, instead.

set -euo pipefail

readonly SCRIPT_NAME=${0##*/}
readonly REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

die() {
  printf '%s: %s\n' "$SCRIPT_NAME" "$1" >&2
  exit 1
}

note() {
  printf '%s: %s\n' "$SCRIPT_NAME" "$1" >&2
}

usage() {
  awk 'NR > 2 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
}

# ---------------------------------------------------------------- arguments --

host=
target=
glibc=2.31
xsync_dir=${XSYNC_DIR:-}
dest_dir=

while [ $# -gt 0 ]; do
  case $1 in
    -h | --help)
      usage
      exit 0
      ;;
    --target)
      [ $# -ge 2 ] || die "--target wants a triple"
      target=$2
      shift 2
      ;;
    --glibc)
      [ $# -ge 2 ] || die "--glibc wants a version, e.g. 2.31"
      glibc=$2
      shift 2
      ;;
    --xsync)
      [ $# -ge 2 ] || die "--xsync wants a directory"
      xsync_dir=$2
      shift 2
      ;;
    --dest)
      [ $# -ge 2 ] || die "--dest wants a directory"
      dest_dir=$2
      shift 2
      ;;
    -*)
      die "unknown option $1 (try --help)"
      ;;
    *)
      [ -z "$host" ] || die "give one host, not two ($host and $1)"
      host=$1
      shift
      ;;
  esac
done

[ -n "$host" ] || {
  usage >&2
  exit 2
}

: "${xsync_dir:=$REPO_ROOT/../xsync}"
[ -d "$xsync_dir" ] || die "no xsync checkout at $xsync_dir (pass --xsync <dir>)"
xsync_dir=$(cd -- "$xsync_dir" && pwd)
[ -f "$xsync_dir/crates/xsync/Cargo.toml" ] || die "$xsync_dir does not look like the xsync checkout"

# ------------------------------------------------------------------- remote --

ssh_opts=(-o BatchMode=yes -o ConnectTimeout=10)

remote() {
  ssh "${ssh_opts[@]}" "$host" "$@"
}

note "probing $host"
probe=$(remote 'printf "%s\n%s\n" "$(uname -m)" "$(id -un)"' 2>&1) ||
  die "cannot reach $host over ssh with key authentication only:
$probe"

remote_arch=$(printf '%s\n' "$probe" | sed -n 1p)
remote_user=$(printf '%s\n' "$probe" | sed -n 2p)

case $remote_arch in
  aarch64 | arm64) detected_target=aarch64-unknown-linux-gnu ;;
  x86_64 | amd64) detected_target=x86_64-unknown-linux-gnu ;;
  *)
    die "$host reports architecture '$remote_arch'; this script only builds
       aarch64-unknown-linux-gnu and x86_64-unknown-linux-gnu. Build xs on the
       host itself, or add the target and pass it with --target."
    ;;
esac

if [ -n "$target" ]; then
  case $target in
    "$detected_target" | "$detected_target".*) ;;
    *) note "warning: --target $target overrides the detected $detected_target" ;;
  esac
else
  target=$detected_target.$glibc
fi

# The cargo target directory never carries the glibc suffix.
cargo_target=${target%%.*}

remote_glibc=$(remote 'getconf GNU_LIBC_VERSION 2>/dev/null || ldd --version 2>&1 | head -1' 2>/dev/null |
  tr ' ' '\n' | grep -E '^[0-9]+\.[0-9]+' | head -1 || true)

if [ -n "$remote_glibc" ] && [ "$cargo_target" != "$target" ]; then
  want=${target#*.}
  older=$(printf '%s\n%s\n' "$want" "$remote_glibc" | sort -V | head -1)
  [ "$older" = "$want" ] ||
    die "$host has glibc $remote_glibc but the build targets $want.
       Re-run with --glibc $remote_glibc (or older)."
fi

if [ -z "$dest_dir" ]; then
  if [ "$remote_user" = root ]; then
    dest_dir=/usr/local/bin
  else
    dest_dir='$HOME/.local/bin'
  fi
fi

note "$host is $remote_arch, glibc ${remote_glibc:-unknown}, login $remote_user -> $dest_dir/xs"

# -------------------------------------------------------------------- build --

command -v cargo >/dev/null || die "cargo is not on PATH"
command -v cargo-zigbuild >/dev/null ||
  die "cargo-zigbuild is not on PATH (cargo install cargo-zigbuild)"
command -v zig >/dev/null ||
  die "zig is not on PATH (brew install zig, or pip install ziglang)"
rustup target list --installed 2>/dev/null | grep -qx "$cargo_target" ||
  die "the $cargo_target target is not installed (rustup target add $cargo_target)"

note "building xs for $target from $xsync_dir"
(
  cd -- "$xsync_dir"
  # --locked: the sibling checkout is not ours to modify, and that includes its
  # Cargo.lock. Only target/ is written.
  cargo zigbuild --locked --release --target "$target" -p xsync --bin xs
) || die "the cross build failed; nothing was sent to $host"

binary=$xsync_dir/target/$cargo_target/release/xs
[ -f "$binary" ] || die "the build reported success but $binary is missing"

note "built $(cd -- "$xsync_dir" && git rev-parse --short=12 HEAD 2>/dev/null || echo 'unknown commit') -> $binary ($(wc -c <"$binary" | tr -d ' ') bytes)"

# ------------------------------------------------------------------ install --

staged="$dest_dir/.xs.staged.$$"

note "sending to $host:$dest_dir/xs"
remote "set -e; mkdir -p \"$dest_dir\"; cat > \"$staged\"; chmod 755 \"$staged\"" <"$binary" ||
  die "could not write $staged on $host"

# Prove the cross-compiled binary runs there *before* replacing what is already
# installed. A glibc or architecture mismatch surfaces here, with the old xs
# still in place.
if ! version=$(remote "\"$staged\" --version" 2>&1); then
  remote "rm -f \"$staged\"" || true
  die "the staged binary does not run on $host:
$version
       Pick an older glibc with --glibc <version> and re-run."
fi

remote "mv -f \"$staged\" \"$dest_dir/xs\"" ||
  die "could not move $staged into place on $host"

# ---------------------------------------------------------------- the proof --

printf '\n'
remote "\"$dest_dir/xs\" --version"

# The backend runs `PATH="$HOME/.local/bin:$PATH" xs --server <root>` over a
# non-interactive ssh, so what matters is that *that* PATH finds it.
if resolved=$(remote 'PATH="$HOME/.local/bin:$PATH" command -v xs' 2>/dev/null); then
  printf '\nxs on the backend'"'"'s PATH: %s\n' "$resolved"
  [ "$resolved" = "$(remote "printf %s \"$dest_dir/xs\"")" ] ||
    note "warning: that is not the copy just staged ($dest_dir/xs) — an older xs shadows it"
else
  note "warning: xs is not on the non-interactive PATH on $host even after installing
       to $dest_dir. Excalibur will report \"xs not found\"; put $dest_dir on PATH
       or re-run with --dest ~/.local/bin."
fi
