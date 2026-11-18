#!/bin/sh
# prints the version the component images are published under and the playbooks pin.
# - LANLORD_IMAGE_VERSION wins when set (the release workflow sets it from the tag).
# - on a checkout of exactly v<workspace version>, that is a release: the workspace version.
# - otherwise it is the version in development: the next patch after the latest v* tag, or the
#   workspace version when that is already ahead of the latest tag. without git or tags it is the
#   workspace version.
# used by .github/workflows/image.yml and crates/core/build.rs, so the images CI publishes and the
# images a build pins always agree.
set -eu
if [ -n "${LANLORD_IMAGE_VERSION:-}" ]; then
  printf '%s\n' "$LANLORD_IMAGE_VERSION"
  exit 0
fi
root=$(cd "$(dirname "$0")/.." && pwd)
cargo=$(sed -n 's/^version = "\([0-9]*\.[0-9]*\.[0-9]*\)"$/\1/p' "$root/Cargo.toml" | head -n1)
[ -n "$cargo" ] || { echo "no workspace version in Cargo.toml" >&2; exit 1; }
latest=$(git -C "$root" tag --list 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null | sed -n 's/^v\([0-9]*\.[0-9]*\.[0-9]*\)$/\1/p' | sort -t. -k1,1n -k2,2n -k3,3n | tail -n1 || true)
if [ -z "$latest" ]; then
  printf '%s\n' "$cargo"
  exit 0
fi
if [ "$(git -C "$root" tag --points-at HEAD 2>/dev/null | grep -cx "v$cargo" || true)" != 0 ]; then
  printf '%s\n' "$cargo"
  exit 0
fi
newest=$(printf '%s\n%s\n' "$latest" "$cargo" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n1)
if [ "$newest" = "$cargo" ] && [ "$cargo" != "$latest" ]; then
  printf '%s\n' "$cargo"
else
  printf '%s.%s.%s\n' "${newest%%.*}" "$(echo "${newest#*.}" | cut -d. -f1)" "$(( ${newest##*.} + 1 ))"
fi
