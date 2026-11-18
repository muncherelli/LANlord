#!/usr/bin/env bash
# fail when a binary needs a GLIBC_ symbol version newer than the one it was built to run on.
# usage: tools/check-glibc.sh <max version, like 2.17> <binary>...
set -euo pipefail
max=${1:?usage: check-glibc.sh <max version> <binary>...}
shift
bad=0
for f in "$@"; do
  need=$(objdump -T "$f" | grep -o 'GLIBC_[0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1 || true)
  echo "$f needs glibc ${need:-nothing} (limit $max)"
  if [ -n "$need" ] && [ "$(printf '%s\n%s\n' "$need" "$max" | sort -V | tail -n 1)" != "$max" ]; then
    echo "error: $f needs glibc $need, newer than $max" >&2
    bad=1
  fi
done
exit "$bad"
