#!/usr/bin/env bash
# generates the development launchers for this checkout: dev builds (target/debug) of the gui and
# the tui, a combined preview build (origin/trunk plus every open pull request) with its own gui
# and tui launchers, and the updater that rebuilds the preview. nothing about a machine is stored
# in the repository: paths are worked out here, when you run it.
#
#   tools/dev-launchers.sh install [--force]   create the launchers (existing ones are kept unless --force)
#   tools/dev-launchers.sh remove --yes        delete the launchers and the preview build (asks for --yes)
#   tools/dev-launchers.sh update-preview      rebuild the preview now (the same as the Preview Update launcher)
#   tools/dev-launchers.sh list                show what is installed
#
# macos gets app bundles in ~/Applications; linux gets desktop entries in ~/.local/share/applications.
# LANLORD_LAUNCHER_DIR and LANLORD_BIN_DIR override where they go (used by the tests).
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
preview_home="${LANLORD_PREVIEW_HOME:-$HOME/.local/share/lanlord-preview}"
bin_dir="${LANLORD_BIN_DIR:-$HOME/.local/bin}"
os=$(uname -s)
if [ "$os" = Darwin ]; then
  launcher_dir="${LANLORD_LAUNCHER_DIR:-$HOME/Applications}"
else
  launcher_dir="${LANLORD_LAUNCHER_DIR:-$HOME/.local/share/applications}"
fi
updater="$bin_dir/lanlord-preview-update"
force=0

# owner/name of the github repository, from the origin remote
slug() {
  git -C "$repo" remote get-url origin | sed -E 's#^(git@[^:]+:|https?://[^/]+/)##; s#\.git$##'
}

# one launcher: a name, a kind (gui|tui|update) and which build it runs (dev|preview)
install_launcher() {
  name=$1 kind=$2 build=$3
  if [ "$build" = dev ]; then
    bin="$repo/target/debug/lanlord"
    missing="No dev build yet. In the repo run: cargo build -p lanlord"
  else
    bin="$preview_home/bin/lanlord-preview"
    missing="No preview yet. Run the LANlord Preview Update launcher first."
  fi
  if [ "$os" = Darwin ]; then
    app="$launcher_dir/$name.app"
    if [ -e "$app" ] && [ "$force" = 0 ]; then echo "kept     $name (exists)"; return; fi
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    [ ! -f "$repo/assets/icon.icns" ] || cp "$repo/assets/icon.icns" "$app/Contents/Resources/icon.icns"
    id=$(printf 'lan.lord.%s' "$(printf '%s' "$name" | tr 'A-Z ' 'a-z-' | tr -cd 'a-z0-9.-')")
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleDisplayName</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>$id</string>
  <key>CFBundleExecutable</key><string>launch</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
    case "$kind" in
      gui)
        cat > "$app/Contents/MacOS/launch" <<SH
#!/bin/sh
BIN="$bin"
[ -x "\$BIN" ] || { osascript -e 'display dialog "$missing" buttons {"OK"} default button "OK"' >/dev/null; exit 1; }
exec "\$BIN" gui
SH
        ;;
      tui)
        cat > "$app/Contents/MacOS/launch" <<SH
#!/bin/sh
BIN="$bin"
[ -x "\$BIN" ] || { osascript -e 'display dialog "$missing" buttons {"OK"} default button "OK"' >/dev/null; exit 1; }
osascript -e 'tell application "Terminal"' -e 'activate' -e "do script \"exec '\$BIN'\"" -e 'end tell' >/dev/null
SH
        ;;
      update)
        cat > "$app/Contents/MacOS/launch" <<SH
#!/bin/sh
osascript -e 'tell application "Terminal"' -e 'activate' -e 'do script "$updater; echo; echo --- whats in this preview ---; cat $preview_home/INCLUDED.txt; echo; read -n 1 -s -r -p \\"press any key to close\\""' -e 'end tell' >/dev/null
SH
        ;;
      *) echo "unknown launcher kind $kind" >&2; return 1 ;;
    esac
    chmod +x "$app/Contents/MacOS/launch"
  else
    entry="$launcher_dir/$(printf '%s' "$name" | tr 'A-Z ()' 'a-z-' | tr -cd 'a-z0-9.-').desktop"
    if [ -e "$entry" ] && [ "$force" = 0 ]; then echo "kept     $name (exists)"; return; fi
    mkdir -p "$launcher_dir"
    case "$kind" in
      gui) exec_line="$bin gui"; terminal=false ;;
      tui) exec_line="$bin"; terminal=true ;;
      update) exec_line="$updater"; terminal=true ;;
      *) echo "unknown launcher kind $kind" >&2; return 1 ;;
    esac
    cat > "$entry" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$name
Exec=$exec_line
Terminal=$terminal
Icon=$repo/assets/icon-512.png
Categories=Development;
DESKTOP
  fi
  echo "created  $name"
}

install_updater() {
  if [ -e "$updater" ] && [ "$force" = 0 ]; then echo "kept     lanlord-preview-update (exists)"; return; fi
  mkdir -p "$bin_dir"
  cat > "$updater" <<SH
#!/usr/bin/env bash
# builds a LANlord preview: origin/trunk plus every open pull request (bar release-plz), merged in a
# scratch worktree. extra branch names given as arguments are merged too. the result is
# $preview_home/bin/lanlord-preview; a running preview keeps working. generated by tools/dev-launchers.sh.
set -euo pipefail
export PATH="/opt/homebrew/bin:/usr/local/bin:\$HOME/.cargo/bin:\$PATH"
repo="\${LANLORD_REPO:-$repo}"
slug="\${LANLORD_SLUG:-$(slug)}"
home="$preview_home"
src="\$home/src"
mkdir -p "\$home/bin"

git -C "\$repo" fetch -q --prune origin
if [ ! -d "\$src" ]; then
  git -C "\$repo" worktree add -q --detach "\$src" origin/trunk
fi
cd "\$src"
git merge --abort 2>/dev/null || true
git checkout -q --detach origin/trunk
git reset -q --hard origin/trunk
git clean -qfd

prs=()
while IFS= read -r line; do [ -n "\$line" ] && prs+=("\$line"); done < <(gh -R "\$slug" pr list --state open --json number,headRefName,isCrossRepository \\
  --jq '.[] | select((.isCrossRepository|not) and (.headRefName|startswith("release-plz")|not)) | "\\(.number)\\t\\(.headRefName)"')
for b in "\$@"; do prs+=("local	\$b"); done

included=() skipped=()
for line in "\${prs[@]}"; do
  num=\${line%%\$'\\t'*} ref=\${line#*\$'\\t'}
  [ "\$num" = local ] && rev="\$ref" || rev="origin/\$ref"
  if git -c commit.gpgsign=false merge -q --no-edit "\$rev" >/dev/null 2>&1; then
    included+=("#\$num \$ref")
  elif [ "\$(git diff --name-only --diff-filter=U)" = "Cargo.lock" ]; then
    # only the lockfile clashed: keep ours, cargo re-resolves it on build
    git checkout -q --ours Cargo.lock && git add Cargo.lock
    git -c commit.gpgsign=false commit -q --no-edit
    included+=("#\$num \$ref (lockfile re-resolved)")
  else
    git merge --abort 2>/dev/null || true
    skipped+=("#\$num \$ref (conflicts)")
  fi
done

echo "building preview..."
if ! CARGO_TARGET_DIR="\$home/target" cargo build -q -p lanlord; then
  echo "PREVIEW BUILD FAILED: the merged tree does not compile; the old preview binary is unchanged" | tee "\$home/INCLUDED.txt" >&2
  exit 1
fi
cp "\$home/target/debug/lanlord" "\$home/bin/.lanlord-preview.new"
mv -f "\$home/bin/.lanlord-preview.new" "\$home/bin/lanlord-preview"

{
  echo "built \$(date '+%F %T') on trunk \$(git rev-parse --short origin/trunk)"
  for i in "\${included[@]}"; do echo "  included \$i"; done
  for s in "\${skipped[@]}"; do echo "  SKIPPED  \$s"; done
} | tee "\$home/INCLUDED.txt"
SH
  chmod +x "$updater"
  echo "created  lanlord-preview-update"
}

names=("LANlord GUI (dev)" "LANlord TUI (dev)" "LANlord Preview GUI" "LANlord Preview TUI" "LANlord Preview Update")

cmd=${1:-help}
[ $# -eq 0 ] || shift
[ "${1:-}" != --force ] || force=1
yes=0
[ "${1:-}" != --yes ] || yes=1

case "$cmd" in
  install)
    install_updater
    install_launcher "LANlord GUI (dev)" gui dev
    install_launcher "LANlord TUI (dev)" tui dev
    install_launcher "LANlord Preview GUI" gui preview
    install_launcher "LANlord Preview TUI" tui preview
    install_launcher "LANlord Preview Update" update preview
    ;;
  remove)
    [ "$yes" = 1 ] || { echo "this deletes the launchers, the updater and the preview build in $preview_home; run it again with --yes" >&2; exit 2; }
    for n in "${names[@]}"; do
      if [ "$os" = Darwin ]; then rm -rf "$launcher_dir/$n.app"
      else rm -f "$launcher_dir/$(printf '%s' "$n" | tr 'A-Z ()' 'a-z-' | tr -cd 'a-z0-9.-').desktop"; fi
    done
    rm -f "$updater"
    if [ -d "$preview_home/src" ]; then git -C "$repo" worktree remove -f -f "$preview_home/src" 2>/dev/null || true; git -C "$repo" worktree prune; fi
    rm -rf "$preview_home"
    echo "removed the launchers and the preview build"
    ;;
  update-preview) exec "$updater" "$@" ;;
  list)
    for n in "${names[@]}"; do
      if [ "$os" = Darwin ]; then p="$launcher_dir/$n.app"; else p="$launcher_dir/$(printf '%s' "$n" | tr 'A-Z ()' 'a-z-' | tr -cd 'a-z0-9.-').desktop"; fi
      if [ -e "$p" ]; then echo "installed $n"; else echo "missing   $n"; fi
    done
    [ -x "$updater" ] && echo "installed lanlord-preview-update" || echo "missing   lanlord-preview-update"
    ;;
  *) sed -n '2,12p' "$0"; exit 2 ;;
esac
