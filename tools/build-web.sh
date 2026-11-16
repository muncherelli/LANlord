#!/usr/bin/env bash
# builds the browser version of lanlord into target/web-dist, which `lanlord webui --static`
# serves on the gateway. it installs nothing: when a tool is missing it says how to get it.
# run it from anywhere: tools/build-web.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
trunk_version="0.22.1"

missing=0
if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
  echo "missing the wasm32 rust target; install it with:"
  echo "  rustup target add wasm32-unknown-unknown"
  missing=1
fi
if ! command -v trunk >/dev/null 2>&1; then
  echo "missing trunk; install it with:"
  echo "  cargo install trunk --version $trunk_version --locked"
  missing=1
elif ! trunk --version | grep -q " $trunk_version"; then
  echo "trunk $(trunk --version | awk '{print $2}') is installed, but this build is pinned to $trunk_version:"
  echo "  cargo install trunk --version $trunk_version --locked"
  missing=1
fi
[ "$missing" -eq 0 ] || exit 1

# trunk fetches the wasm-bindgen and wasm-opt versions pinned in crates/web/Trunk.toml into its
# own cache on first use
cd "$root/crates/web"
trunk build --release
echo "built $root/target/web-dist"
