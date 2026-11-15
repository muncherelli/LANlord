#!/bin/sh
# builds the lanlord desktop app and installs it as ~/Applications/LANlord.app (macos).
# run it from anywhere after pulling: tools/build-app.sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
app="${1:-$HOME/Applications/LANlord.app}"

cd "$root"
cargo build --release -p lanlord
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/lanlord "$app/Contents/MacOS/lanlord-bin"
cp assets/icon.icns "$app/Contents/Resources/icon.icns"
cat > "$app/Contents/MacOS/LANlord" <<'LAUNCH'
#!/bin/sh
# opens the lanlord desktop window; the real program sits next to this script
exec "$(dirname "$0")/lanlord-bin" gui
LAUNCH
chmod +x "$app/Contents/MacOS/LANlord"
cat > "$app/Contents/Info.plist" <<'PLIST'
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
touch "$app"
echo "installed $app"
