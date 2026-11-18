#!/usr/bin/env bash
# easy ways to run development and preview builds of lanlord from a checkout.
# run it from anywhere: tools/dev.sh help
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

usage() {
  cat <<'EOF'
usage: tools/dev.sh <command> [options]

  gui [options]        build and open the desktop window from this checkout
  tui [options]        build and open the terminal ui from this checkout
  web [--serve] [--port N]
                       build the browser version with the developer tools (a red bug in the corner:
                       demo desktop, window outlines; ?demo=1&wmdebug=1&open=all); --serve also runs it here (default port 8080)
  linux-build [--target x86_64|aarch64] [--debug]
                       cross-compile the server binary for linux (needs zig and cargo-zigbuild)
  deploy HOST [options]
                       put the server binary and browser build on a test server
  app                  build and install the macos app (see tools/build-app.sh)
  launchers            create the dev and preview launchers (see tools/dev-launchers.sh)

options for gui and tui:
  --demo               canned data and a fake update, so no real server is touched (gui)
  --clean              start with an empty config in a throwaway folder
  --open LIST          open these windows at start, like dns,my-server (gui)
  --release            build optimised instead of debug

deploy options (HOST is user@host, always given, never stored):
  --dry-run            read the server and show what would change, change nothing
  --yes                skip typing the host to confirm
  --no-build           use the binary and browser build already built
  --rollback           put the previous version back
  --target ARCH        x86_64 (default) or aarch64

examples:
  tools/dev.sh gui --demo --open networks
  tools/dev.sh tui --clean
  tools/dev.sh launchers
  tools/dev.sh web --serve
  tools/dev.sh deploy root@gateway.example.lan --dry-run
EOF
}

# the lanlord binary for this checkout, built if needed; $1 is "" or --release
build() {
  target=${CARGO_TARGET_DIR:-$PWD/target}
  if [ "${1:-}" = "--release" ]; then
    cargo build -q -p lanlord --release --features devtools
    bin="$target/release/lanlord"
  else
    cargo build -q -p lanlord --features devtools
    bin="$target/debug/lanlord"
  fi
}

# parse the shared options into $profile, and the environment they imply
parse() {
  profile=""
  open_list=""
  demo=0
  clean=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --demo) demo=1 ;;
      --clean) clean=1 ;;
      --release) profile="--release" ;;
      --open)
        shift
        [ $# -gt 0 ] || { echo "--open needs a list of windows" >&2; exit 2; }
        open_list=$1
        ;;
      *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
  done
}

run_env() {
  env_args=()
  if [ "$demo" = 1 ]; then
    env_args+=(LANLORD_FAKE_SERVER=1 LANLORD_FAKE_CLOCK=1 LANLORD_FAKE_UPDATE=1 LANLORD_SCREEN=desktop)
  fi
  [ -z "$open_list" ] || env_args+=("LANLORD_OPEN=$open_list")
  if [ "$clean" = 1 ]; then
    scratch=$(mktemp -d)
    echo "using an empty config in $scratch"
    env_args+=("XDG_CONFIG_HOME=$scratch")
  fi
  # the developer menu's rebuild-and-relaunch looks for the checkout here
  env_args+=("LANLORD_DEV=$PWD")
}

cmd=${1:-help}
[ $# -eq 0 ] || shift

case "$cmd" in
  gui)
    parse "$@"
    build "$profile"
    run_env
    exec env "${env_args[@]}" "$bin" gui
    ;;
  tui)
    parse "$@"
    build "$profile"
    run_env
    exec env "${env_args[@]}" "$bin"
    ;;
  web)
    serve=0
    port=8080
    while [ $# -gt 0 ]; do
      case "$1" in
        --serve) serve=1 ;;
        --port)
          shift
          [ $# -gt 0 ] || { echo "--port needs a number" >&2; exit 2; }
          port=$1
          ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
      esac
      shift
    done
    case "$port" in '' | *[!0-9]*) echo "the port must be a number" >&2; exit 2 ;; *) ;; esac
    "$root/tools/build-web.sh" --dev
    if [ "$serve" = 1 ]; then
      # the real web server from this checkout, on this machine only, so the page comes with the
      # same headers as on a gateway
      cargo build -q -p lanlord --no-default-features --features webui
      target=${CARGO_TARGET_DIR:-$root/target}
      echo "serving the browser build at http://127.0.0.1:$port/ (ctrl-c to stop)"
      echo "this build is a stand-in page: it shows no server data and talks to no server."
      if [ "$(uname -s)" = Darwin ]; then (sleep 2; open "http://127.0.0.1:$port/") & fi
      exec "$target/debug/lanlord" webui --listen "127.0.0.1:$port" --static "$target/web-dist"
    fi
    ;;
  linux-build)
    exec "$root/tools/dev-linux-build.sh" "$@"
    ;;
  deploy)
    exec "$root/tools/dev-deploy.sh" "$@"
    ;;
  launchers)
    exec "$root/tools/dev-launchers.sh" install "$@"
    ;;
  app)
    exec "$root/tools/build-app.sh"
    ;;
  help | -h | --help) usage ;;
  *) echo "unknown command: $cmd" >&2; usage >&2; exit 2 ;;
esac
