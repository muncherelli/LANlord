#!/bin/sh
# redraws the lanlord logo, app icon and setup panel from tools/make-logo.swift (macos only:
# it needs swiftc and iconutil). run it from the repository root: tools/make-assets.sh
# pass a directory as the first argument to also leave the zoomed small icons there for checking.
# EMBLEM=a|b picks the side the staff leans on (default b, the left); WORDMARK=m1|m2|m3 picks the lettering:
# m1 refined capitals between gold rules, m2 Baskerville with a bold LAN and an italic lord (default),
# m3 engraved Copperplate capitals. for example: WORDMARK=m3 tools/make-assets.sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

swiftc -O -o "$tmp/make-logo" "$root/tools/make-logo.swift"
"$tmp/make-logo" "$root/assets" "$root/crates/ui/assets" ${1:+"$1"}
iconutil -c icns "$root/assets/icon.iconset" -o "$root/assets/icon.icns"
rm -rf "$root/assets/icon.iconset"
echo "wrote assets/ and crates/ui/assets/"
