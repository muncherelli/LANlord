#!/bin/bash
# runs nmbd and smbd in the foreground. the samba accounts live in the mounted
# /var/lib/samba, but the container's unix accounts do not, so they are
# recreated from it on every start.
set -eu
mkdir -p /run/samba /var/log/samba /var/lib/samba/netlogon /var/lib/samba/private

# the roaming profiles share (only used when nt4-domain.roaming_profiles is on): members of
# domusers may create their own profile folder in it, nobody else can
getent group domusers >/dev/null || groupadd domusers
install -d -o root -g domusers -m 0770 /var/lib/samba/profiles
install -d -o root -g domusers -m 0750 /var/lib/samba/homes
install -d -m 0700 /var/lib/samba/homes/root

# the software share (only used when nt4-domain.software is on): software admins and domain
# admins write to it, everyone else reads
getent group softadmins >/dev/null || groupadd softadmins
install -d -o root -g softadmins -m 2775 /var/lib/samba/software

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
  # the software share's writers, visible to windows as a domain group
  net groupmap list | grep -q 'Software Admins' ||
    net groupmap add ntgroup='Software Admins' unixgroup=softadmins type=d
  # what a domain admin may do: join computers, back up and restore, shut down, manage users
  domain=$(testparm -s --parameter-name=workgroup 2>/dev/null)
  for right in SeMachineAccountPrivilege SeAddUsersPrivilege SeTakeOwnershipPrivilege \
    SeBackupPrivilege SeRestorePrivilege SeRemoteShutdownPrivilege SePrintOperatorPrivilege \
    SeDiskOperatorPrivilege; do
    net sam rights grant "${domain}\\Domain Admins" "$right" >/dev/null 2>&1 || true
  done
fi
printf '!root = Administrator\n' > /etc/samba/usermap

# unix accounts for every samba account, and machine accounts for joined computers
pdbedit -L 2>/dev/null | cut -d: -f1 | while read -r name; do
  [[ -n $name && $name != root ]] && ! id "$name" >/dev/null 2>&1 || continue
  case "$name" in
    *'$') useradd --badname -M -g machines -d /dev/null -s /usr/sbin/nologin "$name" ;;
    *) useradd -M -g domusers -s /usr/sbin/nologin "$name" ;;
  esac
done

# each user's home folder, and domain admin membership remembered from earlier runs
# (if blocks, because a failed test as the last command would end the script under set -e)
pdbedit -L 2>/dev/null | cut -d: -f1 | while read -r name; do
  if [[ -n $name && $name != root && $name != *'$' ]] && id "$name" >/dev/null 2>&1; then
    install -d -o "$name" -g domusers -m 0700 "/var/lib/samba/homes/$name"
  fi
done
if [[ -f /var/lib/samba/private/domadmins ]]; then
  while read -r admin; do
    if [[ -n $admin ]] && id "$admin" >/dev/null 2>&1; then
      usermod -aG domadmins "$admin"
    fi
  done < /var/lib/samba/private/domadmins
fi
lanlord-user sync

nmbd --foreground --no-process-group --debug-stdout &
smbd --foreground --no-process-group --debug-stdout &
# if either one stops, stop the container so docker restarts it
wait -n
