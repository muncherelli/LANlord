#!/bin/bash
# runs nmbd and smbd in the foreground. the samba accounts live in the mounted
# /var/lib/samba, but the container's unix accounts do not, so they are
# recreated from it on every start.
set -eu
mkdir -p /run/samba /var/log/samba /var/lib/samba/netlogon /var/lib/samba/private

# the groups have fixed ids: samba stores its group mappings by number, and the unix groups are
# recreated on every start, so creation order must never decide who is a domain admin
for entry in domusers:1000 machines:1001 domadmins:1002 domguests:1003 softadmins:1004; do
  getent group "${entry%%:*}" >/dev/null || groupadd -g "${entry##*:}" "${entry%%:*}"
done

# the roaming profiles share (only used when nt4-domain.roaming_profiles is on): members of
# domusers may create their own profile folder in it, nobody else can
install -d -o root -g domusers -m 0770 /var/lib/samba/profiles
install -d -o root -g domusers -m 0750 /var/lib/samba/homes
install -d -m 0700 /var/lib/samba/homes/root

# the software share (only used when nt4-domain.software is on): software admins and domain
# admins write to it, everyone else reads
install -d -o root -g softadmins -m 2775 /var/lib/samba/software

# the classic domain's well-known groups, and administrator as root
if grep -q '^[[:space:]]*domain logons = yes' /etc/samba/smb.conf; then
  # point each well-known group at its unix group, also when an older start mapped it elsewhere
  map_group() {
    if net groupmap list | grep -q "^$2 "; then
      net groupmap modify ntgroup="$2" unixgroup="$3" type=d >/dev/null || echo "could not map $2" >&2
    else
      net groupmap add ${1:+rid=$1} ntgroup="$2" unixgroup="$3" type=d >/dev/null || echo "could not map $2" >&2
    fi
  }
  map_group 512 'Domain Admins' domadmins
  map_group 513 'Domain Users' domusers
  map_group 514 'Domain Guests' domguests
  # the software share's writers, visible to windows as a domain group
  map_group '' 'Software Admins' softadmins
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
