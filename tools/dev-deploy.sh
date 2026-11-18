#!/usr/bin/env bash
# puts a development build of the lanlord server and its browser build onto a test server and
# restarts the web service there. it only touches /opt/lanlord/bin/lanlord, /opt/lanlord/web and the
# lanlord-webui service, and it keeps the previous versions beside them so a bad build can be undone.
# it does not apply new service units or playbooks or restart the other services (the operations
# service): for a full upgrade of an installed server run `lanlord upgrade HOST` instead.
# nothing about a server is stored here: the host is always an argument. used by tools/dev.sh deploy.
#
#   tools/dev-deploy.sh HOST [--no-build] [--yes] [--dry-run] [--target x86_64|aarch64]
#   tools/dev-deploy.sh HOST --rollback [--yes]
#
# HOST is user@host. you sign in with your own ssh credentials; a user other than root needs sudo
# without a password prompt on the server (`sudo -n`), and a root-only server is fine.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

usage() { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; }

host=""
build=1
yes=0
dry=0
rollback=0
arch=x86_64
while [ $# -gt 0 ]; do
  case "$1" in
    --no-build) build=0 ;;
    --yes) yes=1 ;;
    --dry-run) dry=1 ;;
    --rollback) rollback=1 ;;
    --target)
      shift
      [ $# -gt 0 ] || { echo "--target needs x86_64 or aarch64" >&2; exit 2; }
      arch=$1
      ;;
    -h | --help) usage; exit 0 ;;
    -*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      [ -z "$host" ] || { echo "only one host, not $host and $1" >&2; exit 2; }
      host=$1
      ;;
  esac
  shift
done
[ -n "$host" ] || { echo "deploy needs a host, like user@gateway.example.lan" >&2; usage >&2; exit 2; }
# the host goes to ssh and into prompts: letters, digits and . _ @ : - only, and not an option
case "$host" in
  -* | *[!A-Za-z0-9._@:-]*) echo "that does not look like a host: $host" >&2; exit 2 ;;
esac
case "$arch" in x86_64 | aarch64) ;; *) echo "--target must be x86_64 or aarch64" >&2; exit 2 ;; esac

target=${CARGO_TARGET_DIR:-$root/target}
bin="$target/$arch-unknown-linux-gnu/release/lanlord"
web="$target/web-dist"
bin_dir=/opt/lanlord/bin
web_dir=/opt/lanlord/web
unit=lanlord-webui

# prefix every path with $LANLORD_FAKE_ROOT on the server; it is empty there, and set only by
# tools/test-dev-pipeline.sh to try this script against a folder
rootvar='R=${LANLORD_FAKE_ROOT:-}'

sshc() { ssh -o ConnectTimeout=15 "$host" "$@"; }

confirm() { # confirm WHAT: type the host to go on
  [ "$yes" = 1 ] && return 0
  printf 'type the host (%s) to %s: ' "$host" "$1"
  read -r typed
  [ "$typed" = "$host" ] || { echo "that is not the host; nothing was changed" >&2; exit 1; }
}

# what the server has now, read-only: who we are, sudo, the installed files, the service
probe() {
  sshc "sh -s" <<PROBE
$rootvar
echo "uid=\$(id -u)"
if [ "\$(id -u)" = 0 ]; then echo sudo=root; elif sudo -n true 2>/dev/null; then echo sudo=ok; else echo sudo=none; fi
b="\$R$bin_dir/lanlord"
if [ -f "\$b" ]; then
  echo "bin_size=\$(wc -c < "\$b" | tr -d ' ')"
  echo "bin_sha=\$(sha256sum "\$b" | cut -d' ' -f1)"
else
  echo bin_size=none
fi
w="\$R$web_dir"
if [ -d "\$w" ]; then
  (cd "\$w" && find . -type f | sort | while read -r f; do echo "webfile=\$(sha256sum "\$f" | cut -d' ' -f1) \$f"; done)
fi
if [ -f "\$R$bin_dir/lanlord.prev" ]; then echo prev_bin=yes; else echo prev_bin=no; fi
if [ -d "\$R$web_dir.prev" ]; then echo prev_web=yes; else echo prev_web=no; fi
sock=\$(systemctl cat $unit 2>/dev/null | sed -n 's/^ExecStart=.*--socket[= ]\\([^ ]*\\).*/\\1/p' | head -1)
echo "socket=\${sock:-/run/lanlord-web/web.sock}"
echo "active=\$(systemctl is-active $unit 2>&1 | head -1)"
PROBE
}

# the part that runs on the server as root: wait for the service and its socket to answer
remote_check='check() {
  i=0
  while [ "$i" -lt 10 ]; do
    if [ "$(systemctl is-active '$unit' 2>/dev/null)" = active ] \
      && curl -fsS -m 3 --unix-socket "$SOCK" -o /dev/null http://localhost/ 2>/dev/null; then return 0; fi
    i=$((i + 1))
    sleep 1
  done
  return 1
}'

echo "reading what $host has now..."
facts=$(probe)
get() { printf '%s\n' "$facts" | sed -n "s/^$1=//p" | head -1; }
case "$(get sudo)" in
  root) sudo_cmd="" ;;
  ok) sudo_cmd="sudo -n"; echo "signed in as $(get uid | sed 's/^0$/root/'); using sudo without a password" ;;
  *)
    echo "this account is not root and sudo needs a password here, so nothing can be changed." >&2
    echo "sign in as root (root@...), or allow this account passwordless sudo on the server." >&2
    exit 1
    ;;
esac
sock=$(get socket)
# the installed unit gives the socket relative to its working directory, which is /
case "$sock" in /* | "") ;; *) sock="/$sock" ;; esac
case "$sock" in /*[!A-Za-z0-9._/-]* | "") echo "the service reports an odd socket path: $sock" >&2; exit 1 ;; esac

if [ "$rollback" = 1 ]; then
  [ "$(get prev_bin)" = yes ] || [ "$(get prev_web)" = yes ] || { echo "nothing to roll back to: there is no previous version on $host" >&2; exit 1; }
  echo "rolling back restores: binary $(get prev_bin), web $(get prev_web)"
  [ "$dry" = 0 ] || { echo "dry run: nothing was changed"; exit 0; }
  confirm "roll back"
  sshc "$sudo_cmd sh -s" <<ROLLBACK
set -u
$rootvar
SOCK=$sock
B="\$R$bin_dir"; W="\$R$web_dir"
$remote_check
if [ -f "\$B/lanlord.prev" ]; then mv -f "\$B/lanlord" "\$B/lanlord.bad"; mv -f "\$B/lanlord.prev" "\$B/lanlord"; fi
if [ -d "\$W.prev" ]; then rm -rf "\$W.bad"; mv "\$W" "\$W.bad"; mv "\$W.prev" "\$W"; fi
systemctl restart $unit
if check; then echo "rolled back; $unit is answering"; else echo "rolled back, but $unit is not answering"; exit 1; fi
ROLLBACK
  exit 0
fi

if [ "$build" = 1 ]; then
  echo "building the server binary for $arch linux..."
  "$root/tools/dev-linux-build.sh" --target "$arch"
  echo "building the browser version..."
  "$root/tools/build-web.sh" --dev
fi
[ -f "$bin" ] || { echo "no binary at $bin; run tools/dev.sh linux-build first (or drop --no-build)" >&2; exit 1; }
[ -f "$web/index.html" ] || { echo "no browser build in $web; run tools/dev.sh web first (or drop --no-build)" >&2; exit 1; }

# a server built without the webui feature crash-loops on the server ("this build has no web server")
if grep -aqF 'this build has no web server' "$bin"; then
  echo "$bin was built without the webui feature, so the service would crash-loop; nothing was changed." >&2
  echo "build it with tools/dev.sh linux-build (it enables webui), or drop --no-build." >&2
  exit 1
fi

new_sha=$(shasum -a 256 "$bin" | cut -d' ' -f1)
new_size=$(wc -c < "$bin" | tr -d ' ')
echo
echo "what will change on $host:"
if [ "$(get bin_size)" = none ]; then
  echo "  binary $bin_dir/lanlord: new, $new_size bytes"
elif [ "$(get bin_sha)" = "$new_sha" ]; then
  echo "  binary $bin_dir/lanlord: unchanged ($new_size bytes)"
else
  echo "  binary $bin_dir/lanlord: $(get bin_size) -> $new_size bytes, sha256 $(get bin_sha | cut -c1-12) -> ${new_sha:0:12}"
fi
local_list=$(cd "$web" && find . -type f | sort | while read -r f; do echo "$(shasum -a 256 "$f" | cut -d' ' -f1) $f"; done)
remote_list=$(printf '%s\n' "$facts" | sed -n 's/^webfile=//p')
added=$(comm -13 <(printf '%s\n' "$remote_list" | awk '{print $2}' | sort) <(printf '%s\n' "$local_list" | awk '{print $2}' | sort))
removed=$(comm -23 <(printf '%s\n' "$remote_list" | awk '{print $2}' | sort) <(printf '%s\n' "$local_list" | awk '{print $2}' | sort))
changed=$(comm -13 <(printf '%s\n' "$remote_list" | sort) <(printf '%s\n' "$local_list" | sort) | awk '{print $2}' | sort | comm -12 - <(printf '%s\n' "$remote_list" | awk '{print $2}' | sort))
count() { if [ -n "$1" ]; then printf '%s\n' "$1" | wc -l | tr -d ' '; else echo 0; fi; }
echo "  web files in $web_dir: $(count "$added") new, $(count "$changed") changed, $(count "$removed") removed, $(($(count "$local_list") - $(count "$added") - $(count "$changed"))) unchanged"
for f in $added; do echo "    + $f"; done | head -20 || true
for f in $changed; do echo "    ~ $f"; done | head -20 || true
for f in $removed; do echo "    - $f"; done | head -20 || true
echo "  the previous binary and web folder are kept as lanlord.prev and web.prev; only $unit is restarted"
echo "  new units, playbooks and the other services are not touched; use \`lanlord upgrade $host\` for those"
if [ "$dry" = 1 ]; then echo "dry run: nothing was changed"; exit 0; fi
confirm "deploy"

echo "uploading..."
# macos tar adds ._ files unless told not to
sshc "$sudo_cmd sh -c '$rootvar; cat > \"\$R$bin_dir/.lanlord.new\"'" < "$bin"
COPYFILE_DISABLE=1 tar -C "$web" -cf - . | sshc "$sudo_cmd sh -c '$rootvar; rm -rf \"\$R$web_dir.new\"; mkdir -p \"\$R$web_dir.new\" && tar -C \"\$R$web_dir.new\" -xf -'"
got=$(sshc "$sudo_cmd sh -c '$rootvar; sha256sum \"\$R$bin_dir/.lanlord.new\"'" | cut -d' ' -f1)
if [ "$got" != "$new_sha" ]; then
  sshc "$sudo_cmd sh -c '$rootvar; rm -rf \"\$R$bin_dir/.lanlord.new\" \"\$R$web_dir.new\"'" || true
  echo "the uploaded binary does not match (sha256 $got); nothing was installed" >&2
  exit 1
fi

echo "installing..."
sshc "$sudo_cmd sh -s" <<INSTALL
set -u
$rootvar
SOCK=$sock
B="\$R$bin_dir"; W="\$R$web_dir"
$remote_check
restore() {
  if [ -f "\$B/lanlord.prev" ]; then mv -f "\$B/lanlord.prev" "\$B/lanlord"; fi
  if [ -d "\$W.prev" ]; then rm -rf "\$W"; mv "\$W.prev" "\$W"; fi
}
# keep the owner and modes the server has now
own=\$(stat -c '%u:%g' "\$B/lanlord" 2>/dev/null || echo 0:0)
mode=\$(stat -c %a "\$B/lanlord" 2>/dev/null || echo 755)
chown "\$own" "\$B/.lanlord.new"; chmod "\$mode" "\$B/.lanlord.new"
wown=0:0; wdir=755; wfile=644
if [ -d "\$W" ]; then
  wown=\$(stat -c '%u:%g' "\$W"); wdir=\$(stat -c %a "\$W"); wfile=\$(stat -c %a "\$W/index.html" 2>/dev/null || echo 644)
fi
chown -R "\$wown" "\$W.new"
find "\$W.new" -type d -exec chmod "\$wdir" {} +
find "\$W.new" -type f -exec chmod "\$wfile" {} +
# swap them in, keeping the previous ones
if [ -f "\$B/lanlord" ]; then mv -f "\$B/lanlord" "\$B/lanlord.prev"; fi
mv -f "\$B/.lanlord.new" "\$B/lanlord"
if [ -d "\$W" ]; then rm -rf "\$W.prev"; mv "\$W" "\$W.prev"; fi
mv "\$W.new" "\$W"
systemctl restart $unit
if check; then echo "deployed; $unit is answering"; exit 0; fi
echo "$unit did not come back; putting the previous version back"
restore
systemctl restart $unit
if check; then echo "the previous version is running again"; else echo "RESTORE FAILED: $unit is not answering; look at it on the server"; fi
exit 1
INSTALL
