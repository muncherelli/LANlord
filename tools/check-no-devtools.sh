#!/usr/bin/env bash
# fails when a release or image build could carry the developer tools (the red bug, the demo
# desktop, window-manager outlines). run by CI; run it from anywhere: tools/check-no-devtools.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
bad=0

# 1. the release and image workflows and the release scripts never ask for the feature or `--dev`
for f in .github/workflows/release.yml .github/workflows/image.yml .github/workflows/release-plz.yml \
  tools/build-app.sh Dockerfile; do
  [ -f "$f" ] || continue
  if grep -nE 'devtools|build-web\.sh +.*--dev' "$f"; then
    echo "$f asks for the developer tools" >&2
    bad=1
  fi
done

# 2. no crate turns the feature on by default, so a plain build never has it
for pkg in lanlord lanlord-ui lanlord-web; do
  # a failing `cargo tree` is a failed check, not a pass
  tree=$(cargo tree -p "$pkg" -e features -i "$pkg" --prefix none --locked) || {
    echo "cargo tree failed for $pkg" >&2
    exit 1
  }
  if grep -q 'feature "devtools"' <<<"$tree"; then
    echo "$pkg has the devtools feature on in a default build" >&2
    bad=1
  fi
done

[ "$bad" = 0 ] && echo "ok: no developer tools in release builds"
exit "$bad"
