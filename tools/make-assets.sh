#!/bin/sh
# redraws the lanlord app icon, social avatar, setup panel and the installer wallpaper from tools/make-logo.swift
# (macos only: it needs swiftc and iconutil). run it from the repository root: tools/make-assets.sh
# pass a directory as the first argument to also leave previews there: the icon at every size on light and dark
# gray, and both wallpaper patterns.
# EMBLEM=a|b picks the side the staff leans on (default b, the left); BACKGROUND=a|b picks the wallpaper
# (a classic damask, b fleur-de-lis grid with scrollwork, the default). for example: BACKGROUND=a tools/make-assets.sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

swiftc -O -o "$tmp/make-logo" "$root/tools/make-logo.swift"
"$tmp/make-logo" "$root/assets" "$root/crates/ui/assets" ${1:+"$1"}
iconutil -c icns "$root/assets/icon.iconset" -o "$root/assets/icon.icns"
rm -rf "$root/assets/icon.iconset"
echo "wrote assets/ and crates/ui/assets/"
