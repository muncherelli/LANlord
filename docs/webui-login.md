# Web UI login

The LANlord web UI runs on the gateway under an unprivileged account (`lanlord` by default) and
lets in any ordinary gateway account, with its usual gateway password. Being in the `admins` group
(gid 9998) does not decide who may sign in; it decides what the account may do, because the
operations the web UI forwards are checked against the account's roles and only the permitted ones
run (see the web UI section of `ansible/README.md`).

An unprivileged process cannot check another user's password through PAM (`pam_unix` needs root
to read the shadow file), so the check is done by a small separate program,
`lanlord-auth-helper`, that runs as root and does nothing else.

## How a login is checked

1. Someone signs in to the web UI as, say, `alice`, from a browser at 192.0.2.7.
2. The web UI connects to `/run/lanlord-auth/auth.sock` and writes `alice`, a NUL byte, the
   password, a NUL byte and, optionally, the browser's address followed by a NUL byte. Then it
   closes its writing side.
3. systemd starts one `lanlord-auth-helper` process for that connection, as root, with the
   connection as its input and output.
4. The helper:
   - refuses unless the process on the other end runs as the web UI account (`--caller`);
   - gives each read and write on the connection five seconds at most;
   - refuses a malformed request, a user name that is not a plain login name, or an address
     that is not an IP address;
   - runs PAM as the `lanlord` service (`/etc/pam.d/lanlord`: the distro's normal password and
     account rules), so expired or locked accounts are refused too, and hands PAM the browser's
     address as the remote host;
   - when the password is right, checks whether `alice` is in the `admins` group, and answers
     `ok` for an administrator or `ok user` for any other ordinary account.
5. It answers one line: `ok`, `ok user`, `ok setup`, `denied` or `error <code>`.

## The first-run exception for root

Root never signs in to the web UI as an ordinary session. The one exception is a gateway that has
no administrator yet: while the `admins` group has no member other than root, the helper accepts
root's password (checked through PAM as strictly as anyone's) and answers `ok setup`. The web UI
then gives root a setup session that can do one thing, the first-run setup: create the first
administrator account and choose root's ssh access. Every other operation, stream and page is
refused for that session by the web UI, and the operations service refuses everything but the setup
for root as well. When the setup is done the session ends and the person signs in as the new
account.

- Only the name `root` is left out of the count of administrators, so another account with uid 0
  neither gets the setup nor counts; a passwd database that cannot be read is an error, never an
  empty group.
- "Root ssh off" is refused in the setup when sshd takes no ssh passwords: the new account has no
  key yet, so no one could sign in over ssh.
- The group is read at every login, never cached. Once any other account is in `admins`, root is
  refused exactly as every other system account is: the same `denied`, after the same time.
- The setup checks again, on the gateway, that no other administrator exists, so a second try
  (or a race between two) is refused.
- The new account's password travels once, from the browser to the web UI, and from there to the
  operations service on its socket. It is not logged and is never an argument or in the
  environment of any command.
- Other accounts below uid 1000 (daemon, nobody and the rest) are still denied.

## What it never gives away

- An unknown user, a wrong password, an expired account and a system or service account all get the
  same `denied`, after the same time: every answer but `ok` and `ok user` takes two seconds plus a random
  quarter of a second at most. That is the only delay; libpam's own failure delay, which only
  follows a wrong password, is switched off so it cannot set that case apart.
- The password is never logged, never passed as an argument or in the environment, and only
  ever sits in the helper's own buffers, which are wiped after use: the connection is read
  without any library buffering. The process cannot be traced or dumped by other users, and
  systemd turns core dumps off.
- The journal records who asked and how it went, for example
  `web ui login for alice from 192.0.2.7: ok, not in group admins (gid 9998)` or `refused uid 1000: only lanlord may ask`. A name typed for an account that does
  not exist is logged as `an unknown user`, since it may be a password typed into the wrong
  field.

## What the web UI must do

The helper slows down guessing, but it is the web UI's job to stop it: it should queue login
attempts and limit them per client address (a few per minute), and pass that address on. At most
four helpers run at once.

## Why a socket and not sudo

Only the web UI account can open the socket (it is `root:<web ui account>`, mode 0660), and the
helper also checks the peer's uid. Nothing gains a sudo rule and the helper is not setuid.
systemd runs it with a read-only system, no network, a narrow set of capabilities and system
calls, and no way to gain privileges.

## Installing

The `webui-login` playbook installs it once the binary has been put at
`/opt/lanlord/bin/lanlord-auth-helper` (owned by root, writable only by root, or the playbook
stops):

- `/usr/local/libexec/lanlord-auth-helper` (root, mode 0700)
- `/etc/pam.d/lanlord`, built on the distro's `common-auth` or `system-auth`
- `lanlord-auth.socket` and `lanlord-auth@.service`

To let a different account run the web UI, set `lanlord_webui_user`.

To try the helper by hand on a gateway (as root, from a terminal there is no peer to check),
without the password landing in your shell history:

```sh
read -rs -p 'password: ' pw; echo
printf 'alice\0%s\0' "$pw" | sudo /usr/local/libexec/lanlord-auth-helper
unset pw
```
