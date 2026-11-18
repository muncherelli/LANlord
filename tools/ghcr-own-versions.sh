#!/bin/bash
# prints "<X.Y.Z> <revision>" for every X.Y.Z tag of a ghcr package that lanlord published: its
# image has an org.opencontainers.image.revision label (not "unknown"). the version label is not
# compared: a retagged release keeps the version label of the image it was copied from. an image of
# the old per-image version scheme has no revision, so it is not ours: it counts as not published and
# may be overwritten. a missing package prints nothing; any other registry error fails.
# usage: tools/ghcr-own-versions.sh <package>   (needs gh, docker buildx and jq)
set -euo pipefail
image=$1
if out=$(gh api --paginate "/users/muncherelli/packages/container/$image/versions" -q '.[].metadata.container.tags[]' 2>&1); then
  tags=$out
elif grep -q 'HTTP 404' <<<"$out"; then
  exit 0
else
  echo "$out" >&2; exit 1
fi
for t in $(grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' <<<"$tags" | sort -uV || true); do
  # a tag deleted since the listing is not there to count; any other error (auth, rate limit,
  # registry) fails, so a published tag never looks unpublished
  if ! cfg=$(docker buildx imagetools inspect "ghcr.io/muncherelli/$image:$t" --format '{{json .Image}}' 2>&1); then
    if grep -qiE 'not found|MANIFEST_UNKNOWN' <<<"$cfg"; then continue; fi
    echo "$cfg" >&2; exit 1
  fi
  # a multi-arch index gives one config per platform; the labels are the same in each
  rev=$(jq -r '(if has("config") then . else (to_entries[0].value) end)
    | .config.Labels["org.opencontainers.image.revision"] // empty' <<<"$cfg")
  if [ -n "$rev" ] && [ "$rev" != unknown ]; then printf '%s %s\n' "$t" "$rev"; fi
done
