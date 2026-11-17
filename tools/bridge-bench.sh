#!/bin/sh
# compare what one bridge round trip costs against today's clock read script, over ssh.
#
#   tools/bridge-bench.sh <host> [runs]
#
# <host> is an ssh destination whose lanlord is installed (the bridge is
# /opt/lanlord/bin/lanlord). it times, over one warm ControlMaster connection:
#   bridge   ssh <host> /opt/lanlord/bin/lanlord bridge hello   (greeting only)
#   script  ssh <host> sh -s < read-script                      (what the gui sends today)
# with hyperfine when it is installed, otherwise a plain loop timed by the shell.
# run it yourself against a gateway you own; nothing runs it automatically.
set -eu

host=${1:?usage: bridge-bench.sh <host> [runs]}
runs=${2:-20}
bin=/opt/lanlord/bin/lanlord
dir=$(mktemp -d)
ssh_cmd="ssh -o BatchMode=yes -o ControlMaster=auto -o ControlPath=$dir/ctl -o ControlPersist=60"

cleanup() {
  $ssh_cmd -O exit "$host" >/dev/null 2>&1 || true
  rm -f "$dir/read.sh"
  rmdir "$dir"
}
trap cleanup EXIT

# the read script, as crates/core/src/clock.rs sends it
cat > "$dir/read.sh" <<'SCRIPT'
timedatectl show 2>/dev/null
echo "Container=$(systemd-detect-virt --container 2>/dev/null || true)"
echo "Epoch=$(date -u +%s)"
echo "Offset=$(date +%z)"
echo "Zone=$(readlink -f /etc/localtime 2>/dev/null | sed 's#.*/zoneinfo/##')"
if systemctl is-active --quiet systemd-timesyncd 2>/dev/null; then
  echo "Sync=timesyncd"
elif systemctl is-active --quiet chronyd 2>/dev/null || systemctl is-active --quiet chrony 2>/dev/null; then
  echo "Sync=chrony"
else
  echo "Sync=none"
fi
echo "Servers=$(sed -n 's/^NTP=//p' /etc/systemd/timesyncd.conf.d/lanlord.conf 2>/dev/null | head -n 1)"
timedatectl timesync-status 2>/dev/null | sed 's/^ *//; s/^/Status=/' | head -n 8
SCRIPT

# open the shared connection once so neither side pays for the handshake
$ssh_cmd "$host" true

bridge_cmd="$ssh_cmd $host $bin bridge hello"
script_cmd="$ssh_cmd $host sh -s < $dir/read.sh"

if command -v hyperfine >/dev/null 2>&1; then
  hyperfine --warmup 3 --runs "$runs" \
    --command-name bridge "$bridge_cmd" \
    --command-name script "$script_cmd"
else
  echo "hyperfine not found; timing $runs runs of each (whole seconds)"
  for name in bridge script; do
    eval "cmd=\$${name}_cmd"
    start=$(date +%s)
    i=0
    while [ "$i" -lt "$runs" ]; do
      eval "$cmd" > /dev/null
      i=$((i + 1))
    done
    echo "$name: $(($(date +%s) - start))s for $runs runs"
  done
fi
