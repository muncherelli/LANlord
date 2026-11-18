#!/bin/bash
# checks tools/ghcr-own-versions.sh against stub gh and docker commands: an image counts as ours by
# its revision label alone (a retagged release keeps the version label of its source), an old-scheme
# image without one does not, and a tag that disappears between the listing and the inspect is skipped.
set -euo pipefail
command -v jq >/dev/null || { echo "skip: jq is not installed"; exit 0; }
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/gh" <<'S'
#!/bin/sh
if [ -n "${LISTING_WITH_FAILURE:-}" ]; then printf '%s\n' 0.1.0 0.1.3; else printf '%s\n' 0.1.0 0.1.1 0.1.2 1.0.0 latest 1; fi
S
cat > "$tmp/docker" <<'S'
#!/bin/sh
# the last argument before --format is the image reference
for a in "$@"; do case "$a" in ghcr.io/*) ref=$a ;; esac; done
case "$ref" in
  *:0.1.0) echo 'WARNING: a buildx warning' >&2; echo '{"linux/amd64":{"config":{"Labels":{"org.opencontainers.image.version":"0.1.0","org.opencontainers.image.revision":"aaa"}}}}' ;;
  *:0.1.1) echo '{"config":{"Labels":{"org.opencontainers.image.version":"0.1.0","org.opencontainers.image.revision":"aaa"}}}' ;;
  *:0.1.2) echo 'ERROR: ghcr.io/example-image:0.1.2: not found' >&2; exit 1 ;;
  *:0.1.3) echo 'ERROR: 503 service unavailable' >&2; exit 1 ;;
  *:1.0.0) echo '{"linux/amd64":{"config":{"Labels":{"org.opencontainers.image.version":"1.0.0"}}}}' ;;
esac
S
chmod +x "$tmp/gh" "$tmp/docker"
got=$(PATH="$tmp:$PATH" "$here/ghcr-own-versions.sh" example-image)
want=$(printf '0.1.0 aaa\n0.1.1 aaa')
if [ "$got" = "$want" ]; then echo "ok   own versions"; else echo "FAIL own versions: $got"; exit 1; fi
# a registry error that is not "not found" must fail, not hide the tag
if LISTING_WITH_FAILURE=1 PATH="$tmp:$PATH" "$here/ghcr-own-versions.sh" example-image >/dev/null 2>&1; then
  echo "FAIL a registry error was swallowed"; exit 1
else
  echo "ok   registry errors fail"
fi
