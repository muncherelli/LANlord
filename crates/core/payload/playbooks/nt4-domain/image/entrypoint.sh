#!/bin/bash
# runs nmbd and smbd in the foreground. the samba accounts live in the mounted
# /var/lib/samba, but the container's unix accounts do not, so they are
# recreated from it on every start.
set -eu
mkdir -p /run/samba /var/log/samba /var/lib/samba/netlogon /var/lib/samba/private

for group in machines domadmins domusers domguests; do
  getent group "$group" >/dev/null || groupadd "$group"
done

# the classic domain's well-known groups, and administrator as root
if grep -q '^[[:space:]]*domain logons = yes' /etc/samba/smb.conf; then
  net groupmap list | grep -q 'Domain Admins' ||
    net groupmap add rid=512 ntgroup='Domain Admins' unixgroup=domadmins type=d
  net groupmap list | grep -q 'Domain Users' ||
    net groupmap add rid=513 ntgroup='Domain Users' unixgroup=domusers type=d
  net groupmap list | grep -q 'Domain Guests' ||
    net groupmap add rid=514 ntgroup='Domain Guests' unixgroup=domguests type=d
fi
printf '!root = Administrator\n' > /etc/samba/usermap

# unix accounts for every samba account, and machine accounts for joined computers
pdbedit -L 2>/dev/null | cut -d: -f1 | while read -r name; do
  [ -n "$name" ] && [ "$name" != root ] && ! id "$name" >/dev/null 2>&1 || continue
  case "$name" in
    *'$') useradd --badname -M -g machines -d /dev/null -s /usr/sbin/nologin "$name" ;;
    *) useradd -M -g domusers -s /usr/sbin/nologin "$name" ;;
  esac
done

nmbd --foreground --no-process-group --debug-stdout &
smbd --foreground --no-process-group --debug-stdout &
# if either one stops, stop the container so docker restarts it
wait -n
