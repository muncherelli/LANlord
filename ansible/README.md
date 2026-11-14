# ansible

everything lanlord applies to a gateway lives here. the `playbooks/` folder is
embedded in the `lanlord` binary, copied to `/opt/lanlord/playbooks` on the
gateway by `lanlord apply`, and run there as the `lanlord` user with
`ansible-playbook`.

| file | what it is |
|---|---|
| `playbooks/site.yml` | the entry point: the gateway baseline, then each module |
| `playbooks/network.yml` | shared: finds the host and internal interfaces and the address already on the internal one |
| `playbooks/docker.yml` | the `docker` module |
| `playbooks/nt4-domain.yml` | the `nt4-domain` module (`nt4-domain/` holds the samba image and `smb.conf.j2`) |
| `playbooks/web-proxy.yml` | the `web-proxy` module (`web-proxy/` holds the `Caddyfile.j2`) |

## how modules work

- a module's tasks carry its id as a tag, so `lanlord apply <host> --module nt4-domain`
  runs just that module
- its settings live under the same key in your encrypted settings file, and the
  module does nothing until that section exists
- the comment at the top of each playbook shows the settings it reads

adding a file here means adding it to the list in `crates/core/src/payload.rs`;
a test fails if a file is missing from it.

## the samba image

`playbooks/nt4-domain/image/` is built by CI and published to
`ghcr.io/muncherelli/lanlord-samba-nt4`. the tag is a hash of the three files in that folder,
and the playbook computes the same hash on the gateway, so it always pulls the image that matches
the files it was sent. if the pull fails, or `nt4-domain.build_locally` is true, it builds the same
image on the gateway instead.
