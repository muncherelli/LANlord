#!/usr/bin/env bash
# cross-compiles the lanlord server binary (no gui, with the web server) for a linux machine, from
# macos or linux, using zig as the linker through cargo-zigbuild. it installs nothing: when a tool is
# missing it says how to get it. used by tools/dev.sh linux-build and tools/dev.sh deploy.
#
#   tools/dev-linux-build.sh [--target x86_64|aarch64] [--debug]
#
# this builds only the server binary. the web ui login helper (lanlord-auth-helper) links libpam,
# which zig cannot provide without a sysroot of the target, so it is not built here: build it on a
# linux machine of the same architecture with libpam0g-dev (or pam-devel) installed,
# `cargo build --release -p lanlord-auth-helper`, and pass it to `lanlord install --auth-helper`.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

arch=x86_64
profile=--release
dir=release
while [ $# -gt 0 ]; do
  case "$1" in
    --target)
      shift
      [ $# -gt 0 ] || { echo "--target needs x86_64 or aarch64" >&2; exit 2; }
      arch=$1
      ;;
    --debug) profile=""; dir=debug ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
case "$arch" in
  x86_64 | aarch64) ;;
  *) echo "--target must be x86_64 or aarch64, not $arch" >&2; exit 2 ;;
esac
triple="$arch-unknown-linux-gnu"
# an old glibc floor, so the binary runs on any current distribution
glibc="2.28"

missing=0
if ! command -v zig >/dev/null 2>&1; then
  echo "missing zig; install it with:"
  echo "  brew install zig"
  missing=1
fi
if ! command -v cargo-zigbuild >/dev/null 2>&1; then
  echo "missing cargo-zigbuild; install it with:"
  echo "  cargo install cargo-zigbuild --locked"
  missing=1
fi
if ! rustup target list --installed 2>/dev/null | grep -qx "$triple"; then
  echo "missing the $triple rust target; install it with:"
  echo "  rustup target add $triple"
  missing=1
fi
[ "$missing" -eq 0 ] || exit 1

# the server needs no window: leaving the gui feature out also leaves out its graphics libraries
# shellcheck disable=SC2086
cargo zigbuild -q $profile -p lanlord --no-default-features --features webui --target "$triple.$glibc"

target=${CARGO_TARGET_DIR:-$root/target}
bin="$target/$triple/$dir/lanlord"
[ -f "$bin" ] || { echo "the build finished but $bin is missing" >&2; exit 1; }
echo "built $bin"
echo "  $(wc -c < "$bin" | tr -d ' ') bytes, sha256 $(shasum -a 256 "$bin" | cut -d' ' -f1)"
echo "note: the login helper is not built here (it links libpam); see the header of this script"
