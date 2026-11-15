# Ansible

Everything LANlord applies to a gateway lives here. The `playbooks/` folder is embedded in the `lanlord` binary, copied to `/opt/lanlord/playbooks` on the gateway by `lanlord apply`, and run there as the `lanlord` user with `ansible-playbook`.

| File | What it is |
|---|---|
| `playbooks/site.yml` | The entry point: the gateway baseline, then each module. |
| `playbooks/network.yml` | Shared tasks: find the host and internal interfaces and the address already on the internal one. |
| `playbooks/docker.yml` | The Docker module. |
| `playbooks/nt4-domain.yml` | The NT4 Domain module. The `nt4-domain/` folder holds the Samba image and `smb.conf.j2`. |
| `playbooks/web-proxy.yml` | The Web Proxy module. The `web-proxy/` folder holds the `Caddyfile.j2`. |

## How modules work

- A module's tasks carry its id as a tag, so `lanlord apply <host> --module nt4-domain` runs just that module.
- Its settings live under the same key in your encrypted settings file, and the module does nothing until that section exists.
- The comment at the top of each playbook shows the settings it reads.

Adding a file here means adding it to the list in `crates/core/src/payload.rs`. A test fails if a file is missing from it.

## The Samba image

`playbooks/nt4-domain/image/` is built by CI and published to `ghcr.io/muncherelli/lanlord-samba-nt4`. The tag is a hash of the three files in that folder, and the playbook computes the same hash on the gateway, so it always pulls the image that matches the files it was sent. If the pull fails, or `nt4-domain.build_locally` is true, it builds the same image on the gateway instead.
