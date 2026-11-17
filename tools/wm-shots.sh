#!/usr/bin/env bash
# screenshots every window on its own, for looking at the window manager. devtools builds only:
# it builds the dev web bundle (tools/build-web.sh --dev), serves target/web-dist on a local port
# and, for each window, runs headless chromium on `?demo=1&open=<window>` (demo mode needs no
# gateway), at 1280x720 and 800x500, with and without `wmdebug=1`. the pictures land in
# target/wm-shots/<window>-<w>x<h>[-wmdebug].png. it also shoots the logon screen (`login`, no demo)
# and the debug menu open over the demo desktop (`menu`), for the red bug's two homes.
#   tools/wm-shots.sh              all windows
#   tools/wm-shots.sh welcome hosts  only these ids
#   SKIP_BUILD=1 tools/wm-shots.sh   reuse the bundle already in target/web-dist
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/target/wm-shots"
port=${PORT:-8765}
windows=("$@")
[ ${#windows[@]} -gt 0 ] || windows=(welcome changelog hosts my-server networks settings console components datetime gateway docker nt4-domain web-proxy remote-commands dns)
[ -n "${SKIP_BUILD:-}" ] || flock ~/.cache/lanlord-cargo.lock "$root/tools/build-web.sh" --dev
mkdir -p "$out"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$root/target/web-dist" >/dev/null 2>&1 &
server=$!
trap 'kill $server 2>/dev/null || true' EXIT
sleep 1
for size in 1280x720 800x500; do
  for debug in "" "&wmdebug=1"; do
    for w in "${windows[@]}"; do
      name="$w-$size${debug:+-wmdebug}"
      chromium --headless=new --use-angle=swiftshader --enable-unsafe-swiftshader \
        --window-size="${size/x/,}" --virtual-time-budget=15000 \
        --screenshot="$out/$name.png" "http://127.0.0.1:$port/?demo=1&open=$w$debug" >/dev/null 2>&1
      echo "$out/$name.png"
    done
    [ -n "$debug" ] || {
      shoot() { # name url-query
        chromium --headless=new --use-angle=swiftshader --enable-unsafe-swiftshader \
          --window-size="${size/x/,}" --virtual-time-budget=15000 \
          --screenshot="$out/$1-$size.png" "http://127.0.0.1:$port/?$2" >/dev/null 2>&1
        echo "$out/$1-$size.png"
      }
      shoot login "x=1"
      shoot menu "demo=1&menu=1"
    }
  done
done
