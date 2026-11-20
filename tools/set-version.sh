#!/bin/sh
# usage: tools/set-version.sh X.Y.Z          set the workspace version
#        tools/set-version.sh --check X.Y.Z  fail unless every file already has it
#
# the version lives in the [workspace.package] table of Cargo.toml and in the
# `version =` requirement of every internal (lanlord*) dependency. setting it
# also refreshes Cargo.lock offline, and does nothing else. it takes a plain
# X.Y.Z only, and running it twice changes nothing.
set -eu

die() { echo "set-version: $*" >&2; exit 1; }

mode=set
case "${1:-}" in
  --check) mode=check; shift ;;
  *) ;;
esac
[ "$#" -eq 1 ] || die "usage: set-version.sh X.Y.Z | --check X.Y.Z"
version=$1
# plain X.Y.Z: digits only, no leading zeros, no prerelease or build suffix
case "$version" in
  *[!0-9.]* | .* | *. | *..* | 0[0-9]* | *.0[0-9]*) die "not a plain X.Y.Z version: $version" ;;
  *.*.*.*) die "not a plain X.Y.Z version: $version" ;;
  *.*.*) ;;
  *) die "not a plain X.Y.Z version: $version" ;;
esac

cd "$(dirname "$0")/.."

# prints a manifest with the version set
rewrite() {
  awk -v v="$version" '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && /^version[ \t]*=/ {
      print "version = \"" v "\""; next
    }
    /^lanlord[A-Za-z0-9_-]*[ \t]*=[ \t]*\{/ && match($0, /[{ ,]version[ \t]*=[ \t]*"[^"]*"/) {
      print substr($0, 1, RSTART) "version = \"" v "\"" substr($0, RSTART + RLENGTH); next
    }
    { print }
  ' "$1"
}

grep -q '^version[[:space:]]*=' Cargo.toml || die "no workspace version in Cargo.toml"

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT HUP INT TERM
bad=""
for f in Cargo.toml crates/*/Cargo.toml; do
  [ -f "$f" ] || continue
  rewrite "$f" > "$tmp"
  if ! cmp -s "$f" "$tmp"; then
    case "$mode" in
      check) bad="$bad $f" ;;
      set) cat "$tmp" > "$f" ;;
      *) die "bad mode: $mode" ;;
    esac
  fi
done

case "$mode" in
  check)
    [ -z "$bad" ] || die "not at version $version:$bad"
    echo "set-version: all files are at $version"
    ;;
  set)
    # the lock file records the workspace crates' versions; no network
    if [ -f Cargo.lock ]; then cargo update -w --offline; fi
    ;;
  *) die "bad mode: $mode" ;;
esac
