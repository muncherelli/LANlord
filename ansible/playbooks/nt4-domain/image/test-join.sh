#!/bin/sh
# regression test for the session key patch: runs inside the build stage of the image
# (docker build --target build), starts smbd as a classic domain controller and acts like
# windows nt 4.0 joining it: a bare ntlmv1 smb1 logon (no spnego), then samr calls that create
# a computer account and set its password. without the patch the password set fails with
# NT_STATUS_NO_USER_SESSION_KEY.
set -eu
root=/out/usr
export PATH="$root/bin:$root/sbin:$PATH" LD_LIBRARY_PATH="$root/lib/samba:$root/lib"
work=$(mktemp -d)
pw='Passw0rd-example'
mkdir -p "$work/lib" "$work/priv" "$work/log" "$work/run" "$work/cache" "$work/lock"
cat > "$work/smb.conf" <<EOF
[global]
   workgroup = TESTDOM
   netbios name = TESTDC
   interfaces = 127.0.0.1
   bind interfaces only = yes
   server min protocol = NT1
   ntlm auth = ntlmv1-permitted
   lanman auth = no
   security = user
   encrypt passwords = yes
   map to guest = never
   passdb backend = tdbsam
   idmap config * : backend = tdb
   idmap config * : range = 3000-7999
   server role = classic primary domain controller
   domain logons = yes
   server schannel = auto
   allow nt4 crypto = yes
   reject md5 clients = no
   add machine script = /bin/adduser -D -H -h /dev/null -s /sbin/nologin %u
   private dir = $work/priv
   lock dir = $work/lock
   state directory = $work/lib
   cache directory = $work/cache
   pid directory = $work/run
   log file = $work/log/%m.log
   disable netbios = yes
   smb ports = 445
EOF
export SMB_CONF_PATH="$work/smb.conf"
# the domain admin: root maps to the samba account root
(printf '%s\n%s\n' "$pw" "$pw") | smbpasswd -c "$work/smb.conf" -s -a root >/dev/null
net -s "$work/smb.conf" sam rights grant 'root' SeMachineAccountPrivilege >/dev/null 2>&1 || true
# the compiled-in log and state folders
mkdir -p /var/log/samba /run/samba /var/lib/samba /var/cache/samba
smbd -s "$work/smb.conf" --daemon --no-process-group -l "$work/log"
trap 'kill "$(cat "$work/run/smbd.pid")" 2>/dev/null || true' EXIT
sleep 3
cat "$work"/log/log.smbd 2>/dev/null | tail -5
# what nt 4.0 speaks: smb1, no spnego, ntlmv1 only
rpc() {
  rpcclient -s "$work/smb.conf" -U "root%$pw" \
    --option='client min protocol=NT1' --option='client max protocol=NT1' \
    --option='client use spnego=no' --option='client ntlmv2 auth=no' \
    --option='client lanman auth=no' --option='client plaintext auth=no' \
    127.0.0.1 -c "$1"
}
rpc 'createdomuser ws1$'
# level 24 carries the new password encrypted with the smb session key, like nt4 does
out=$(rpc "setuserinfo2 ws1\$ 24 $pw" 2>&1) || { echo "FAIL: $out"; exit 1; }
case "$out" in
  *NT_STATUS*) echo "FAIL: $out"; exit 1 ;;
esac
echo "ok: machine account created and its password set over a bare ntlmv1 session"
