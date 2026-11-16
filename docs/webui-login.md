# Web UI login

The LANlord web UI runs on the gateway under an unprivileged account (`lanlord` by default) and
lets in the members of the `admins` group (gid 9998), with their usual gateway password.

An unprivileged process cannot check another user's password through PAM (`pam_unix` needs root
to read the shadow file), so the check is done by a small separate program,
`lanlord-auth-helper`, that runs as root and does nothing else.

## How a login is checked

1. Someone signs in to the web UI as, say, `alice`.
2. The web UI connects to `/run/lanlord-auth/auth.sock`, writes `alice`, a NUL byte, the
   password and another NUL byte, then closes its writing side.
3. systemd starts one `lanlord-auth-helper` process for that connection, as root, with the
   connection as its input and output.
4. The helper:
   - refuses unless the process on the other end runs as the web UI account (`--caller`);
   - refuses a malformed request, or a user name that is not a plain login name;
   - runs PAM as the `lanlord` service (`/etc/pam.d/lanlord`: the distro's normal password and
     account rules), so expired or locked accounts are refused too;
   - when the password is right, checks that `alice` is in the `admins` group.
5. It answers one line: `ok`, `denied` or `error <code>`.

Anything other than `ok` takes at least 1.5 seconds, and an unknown user, a wrong password and a
user outside `admins` all get the same `denied`. The password is never logged, never passed as
an argument or in the environment, and is wiped from the helper's memory after use. The journal
records each attempt as `web ui login for alice: denied`.

## Why a socket and not sudo

Only the web UI account can open the socket (it is `root:<web ui account>`, mode 0660), and the
helper also checks the peer's uid. Nothing gains a sudo rule, the helper is not setuid, and
systemd runs it with a read-only system, no network and no new privileges.

## Installing

The `webui-login` playbook installs it once the binary has been put at
`/opt/lanlord/bin/lanlord-auth-helper`:

- `/usr/local/libexec/lanlord-auth-helper` (root, mode 0755)
- `/etc/pam.d/lanlord`
- `lanlord-auth.socket` and `lanlord-auth@.service`

To let a different account run the web UI, set `lanlord_webui_user`.

To try the helper by hand on a gateway (as root, from a terminal there is no peer to check):

```sh
printf 'alice\0the-password\0' | sudo /usr/local/libexec/lanlord-auth-helper
```
