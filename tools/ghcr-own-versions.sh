#!/bin/bash
# prints "<X.Y.Z> <revision>" for every X.Y.Z tag of a ghcr package that lanlord published: its
# image has org.opencontainers.image.version equal to the tag and an org.opencontainers.image.revision.
# a tag without them (an image of the old per-image version scheme) is not ours, so it counts as
# not published and may be overwritten. a missing package prints nothing; any other registry error fails.
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
  cfg=$(docker buildx imagetools inspect "ghcr.io/muncherelli/$image:$t" --format '{{json .Image}}')
  # a multi-arch index gives one config per platform; the labels are the same in each
  rev=$(jq -r --arg v "$t" '(if has("config") then . else (to_entries[0].value) end)
    | .config.Labels as $l | select($l["org.opencontainers.image.version"] == $v)
    | $l["org.opencontainers.image.revision"] // empty' <<<"$cfg")
  if [ -n "$rev" ] && [ "$rev" != unknown ]; then printf '%s %s\n' "$t" "$rev"; fi
done
