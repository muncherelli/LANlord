#!/usr/bin/env bash
# build an empty-bodied libpam.so.0 to link the login helper against, so the release builds need
# neither the libpam headers nor a target libpam: the helper declares the few pam calls it makes
# itself (crates/auth-helper/src/pam.rs), and the real libpam.so.0 of the gateway answers them at
# run time. needs zig on the path.
# usage: tools/pam-stub.sh <x86_64|aarch64> <output dir>
set -euo pipefail
arch=${1:?usage: pam-stub.sh <x86_64|aarch64> <output dir>}
out=${2:?usage: pam-stub.sh <x86_64|aarch64> <output dir>}
mkdir -p "$out"
src=$(mktemp -d)
trap 'rm -rf "$src"' EXIT
for f in pam_start pam_set_item pam_authenticate pam_acct_mgmt pam_end; do
  echo "int $f(void) { return 0; }"
done > "$src/stub.c"
zig cc -target "$arch-linux-gnu.2.17" -shared -nostdlib -Wl,-soname,libpam.so.0 \
  -o "$out/libpam.so.0" "$src/stub.c"
ln -sf libpam.so.0 "$out/libpam.so"
