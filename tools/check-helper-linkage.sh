#!/usr/bin/env bash
# fail unless the login helper links the system pam (libpam.so.0) and carries no rpath or runpath,
# so it never loads a pam from anywhere but the gateway's own library path.
# usage: tools/check-helper-linkage.sh <lanlord-auth-helper>
set -euo pipefail
f=${1:?usage: check-helper-linkage.sh <lanlord-auth-helper>}
dynamic=$(objdump -p "$f")
if ! printf '%s\n' "$dynamic" | grep -Eq '^ *NEEDED +libpam\.so\.0$'; then
  echo "error: $f does not need libpam.so.0" >&2
  exit 1
fi
if printf '%s\n' "$dynamic" | grep -Eq '^ *(RPATH|RUNPATH) '; then
  echo "error: $f has an rpath or runpath" >&2
  exit 1
fi
echo "$f links libpam.so.0 with no rpath"
