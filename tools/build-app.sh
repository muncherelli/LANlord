#!/bin/sh
# builds the lanlord desktop app and installs it as ~/Applications/LANlord.app (macos).
# run it from anywhere after pulling: tools/build-app.sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
app="${1:-$HOME/Applications/LANlord.app}"

cd "$root"
cargo build --release -p lanlord
# the new app is put together beside the installed one and swapped in at the end, so stopping
# the build part way (cancelling an update) never leaves the installed app broken
tmp="$app.tmp.$$"
trap 'rm -rf "$tmp"' EXIT
rm -rf "$tmp"
mkdir -p "$tmp/Contents/MacOS" "$tmp/Contents/Resources"
cp target/release/lanlord "$tmp/Contents/MacOS/lanlord-bin"
cp assets/icon.icns "$tmp/Contents/Resources/icon.icns"
cat > "$tmp/Contents/MacOS/LANlord" <<'LAUNCH'
#!/bin/sh
# opens the lanlord desktop window; the real program sits next to this script
exec "$(dirname "$0")/lanlord-bin" gui
LAUNCH
chmod +x "$tmp/Contents/MacOS/LANlord"
cat > "$tmp/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>LANlord</string>
  <key>CFBundleDisplayName</key><string>LANlord</string>
  <key>CFBundleIdentifier</key><string>com.muncherelli.lanlord</string>
  <key>CFBundleExecutable</key><string>LANlord</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
rm -rf "$app.old.$$"
if [ -e "$app" ]; then mv "$app" "$app.old.$$"; fi
mv "$tmp" "$app"
rm -rf "$app.old.$$"
touch "$app"
echo "installed $app"
