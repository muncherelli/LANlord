#!/usr/bin/env bash
# tries tools/dev-deploy.sh end to end against a folder that stands in for a server: a stub ssh runs
# the remote commands locally, under a fake root, with stub systemctl, curl and friends. nothing
# leaves this machine and no server is involved. run it from anywhere: tools/test-dev-pipeline.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
pass=0
fail=0

ok() { pass=$((pass + 1)); echo "ok   $1"; }
bad() { fail=$((fail + 1)); echo "FAIL $1"; }
check() { # check NAME COMMAND...
  name=$1
  shift
  if "$@" >/dev/null 2>&1; then ok "$name"; else bad "$name"; fi
}
has() { grep -q -- "$1" "$2"; }

# the fake server
srv="$t/srv"
mkdir -p "$srv/opt/lanlord/bin" "$srv/opt/lanlord/web"
printf 'OLD-BINARY' > "$srv/opt/lanlord/bin/lanlord"
printf '<html>old</html>' > "$srv/opt/lanlord/web/index.html"
printf 'gone' > "$srv/opt/lanlord/web/old.js"

# the build output deploy picks up with --no-build
export CARGO_TARGET_DIR="$t/target"
mkdir -p "$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release" "$CARGO_TARGET_DIR/web-dist/sub"
newbin="$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/lanlord"
printf 'NEW-BINARY' > "$newbin"
printf '<html>new</html>' > "$CARGO_TARGET_DIR/web-dist/index.html"
printf 'js' > "$CARGO_TARGET_DIR/web-dist/app.js"
printf 'x' > "$CARGO_TARGET_DIR/web-dist/sub/inner.txt"

# stubs, first in the path
bin="$t/stubs"
mkdir -p "$bin"
cat > "$bin/ssh" <<'SH'
#!/bin/sh
# ssh [-o option] HOST COMMAND...: runs the command here, as the fake server
while [ "$1" = -o ]; do shift 2; done
shift
LANLORD_FAKE_ROOT="$FAKE_ROOT" exec sh -c "$*"
SH
cat > "$bin/id" <<'SH'
#!/bin/sh
case "$1" in -u) echo "${FAKE_UID:-0}" ;; -un) echo tester ;; *) exec /usr/bin/id "$@" ;; esac
SH
cat > "$bin/sudo" <<'SH'
#!/bin/sh
[ "${FAKE_SUDO:-ok}" = ok ] || exit 1
[ "$1" = -n ] && shift
[ "$1" = true ] && exit 0
echo "sudo $*" >> "$FAKE_LOG"
exec "$@"
SH
cat > "$bin/systemctl" <<'SH'
#!/bin/sh
case "$1" in
  cat) echo "ExecStart=/opt/lanlord/bin/lanlord webui --socket /run/lanlord-web/web.sock" ;;
  is-active) echo active ;;
  restart) echo "systemctl $*" >> "$FAKE_LOG" ;;
esac
SH
# the service "answers" unless the installed binary is the bad build
cat > "$bin/curl" <<'SH'
#!/bin/sh
grep -q BAD "$LANLORD_FAKE_ROOT/opt/lanlord/bin/lanlord" && exit 22
exit 0
SH
cat > "$bin/stat" <<'SH'
#!/bin/sh
# stat -c FORMAT FILE
fmt=$2
file=$3
case "$fmt" in
  %u:%g) echo 0:0 ;;
  %a) if [ -d "$file" ]; then echo 755; elif [ "${file##*/}" = lanlord ]; then echo 755; else echo 644; fi ;;
esac
SH
printf '#!/bin/sh\nexit 0\n' > "$bin/chown"
printf '#!/bin/sh\nexit 0\n' > "$bin/sleep"
chmod +x "$bin"/*
export PATH="$bin:$PATH"
export FAKE_ROOT="$srv" FAKE_LOG="$t/log"
: > "$t/log"

deploy() { "$root/tools/dev-deploy.sh" "$@"; }
snap() { (cd "$srv" && find . -type f | sort | while read -r f; do echo "$f $(shasum -a 256 "$f" | cut -c1-12)"; done); }

echo "== dry run changes nothing"
before=$(snap)
out=$(deploy root@gateway.example.lan --no-build --dry-run 2>&1)
[ "$(snap)" = "$before" ] && ok "dry run left the server alone" || bad "dry run left the server alone"
printf '%s\n' "$out" | grep -q "binary /opt/lanlord/bin/lanlord: 10 -> 10 bytes" && ok "dry run shows the binary change" || bad "dry run shows the binary change"
printf '%s\n' "$out" | grep -q "+ ./app.js" && ok "dry run lists a new web file" || bad "dry run lists a new web file"
printf '%s\n' "$out" | grep -q "~ ./index.html" && ok "dry run lists a changed web file" || bad "dry run lists a changed web file"
printf '%s\n' "$out" | grep -q "\- ./old.js" && ok "dry run lists a removed web file" || bad "dry run lists a removed web file"

echo "== a wrong confirmation changes nothing"
if echo "nope" | deploy root@gateway.example.lan --no-build >/dev/null 2>&1; then bad "wrong host refused"; else ok "wrong host refused"; fi
[ "$(snap)" = "$before" ] && ok "server untouched after the refusal" || bad "server untouched after the refusal"

echo "== deploy"
out=$(echo "root@gateway.example.lan" | deploy root@gateway.example.lan --no-build 2>&1) || true
printf '%s\n' "$out" | grep -q "deployed; lanlord-webui is answering" && ok "deploy reports success" || { bad "deploy reports success"; echo "$out"; }
check "new binary installed" has NEW-BINARY "$srv/opt/lanlord/bin/lanlord"
check "previous binary kept" has OLD-BINARY "$srv/opt/lanlord/bin/lanlord.prev"
check "new web build installed" has "new" "$srv/opt/lanlord/web/index.html"
check "web subfolder installed" test -f "$srv/opt/lanlord/web/sub/inner.txt"
check "old web file gone from the live folder" test ! -e "$srv/opt/lanlord/web/old.js"
check "previous web folder kept" test -f "$srv/opt/lanlord/web.prev/old.js"
check "no mac tar litter" test -z "$(find "$srv" -name '._*')"
check "no temp files left" test -z "$(find "$srv" -name '.lanlord.new' -o -name 'web.new')"
check "only the web service was restarted" test "$(grep -c '^systemctl restart lanlord-webui$' "$t/log")" = 1
check "nothing else was restarted" test "$(grep -c systemctl "$t/log")" = 1

echo "== rollback"
out=$(deploy root@gateway.example.lan --rollback --yes 2>&1) || true
printf '%s\n' "$out" | grep -q "rolled back; lanlord-webui is answering" && ok "rollback reports success" || { bad "rollback reports success"; echo "$out"; }
check "old binary back" has OLD-BINARY "$srv/opt/lanlord/bin/lanlord"
check "old web back" test -f "$srv/opt/lanlord/web/old.js"

echo "== a bad build is put back"
printf 'BAD-BINARY' > "$newbin"
good=$(cat "$srv/opt/lanlord/bin/lanlord")
out=$(deploy root@gateway.example.lan --no-build --yes 2>&1) && bad "a failing deploy exits non-zero" || ok "a failing deploy exits non-zero"
printf '%s\n' "$out" | grep -q "putting the previous version back" && ok "failure says it is restoring" || bad "failure says it is restoring"
printf '%s\n' "$out" | grep -q "the previous version is running again" && ok "the previous version answers again" || bad "the previous version answers again"
[ "$(cat "$srv/opt/lanlord/bin/lanlord")" = "$good" ] && ok "the good binary is back in place" || bad "the good binary is back in place"
check "the old web folder is back" test -f "$srv/opt/lanlord/web/old.js"

echo "== not root and no sudo"
printf 'NEW-BINARY' > "$newbin"
before=$(snap)
if FAKE_UID=1000 FAKE_SUDO=no deploy user@gateway.example.lan --no-build --yes >/dev/null 2>&1; then bad "no sudo is refused"; else ok "no sudo is refused"; fi
[ "$(snap)" = "$before" ] && ok "nothing changed without sudo" || bad "nothing changed without sudo"

echo "== not root with sudo uses it"
FAKE_UID=1000 FAKE_SUDO=ok deploy user@gateway.example.lan --no-build --yes >/dev/null 2>&1 && ok "sudo deploy works" || bad "sudo deploy works"
check "sudo was used" has "^sudo -n\|^sudo " "$t/log"

echo "== bad input"
if deploy >/dev/null 2>&1; then bad "no host refused"; else ok "no host refused"; fi
if deploy -oProxyCommand=x >/dev/null 2>&1; then bad "option-like host refused"; else ok "option-like host refused"; fi
if deploy 'a;b' >/dev/null 2>&1; then bad "host with a semicolon refused"; else ok "host with a semicolon refused"; fi

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
