# Ansible

Everything LANlord applies to a gateway lives here. The `playbooks/` folder is embedded in the `lanlord` binary, copied to `/opt/lanlord/playbooks` on the gateway by `lanlord apply`, and run there as the `lanlord` user with `ansible-playbook`.

| File | What it is |
|---|---|
| `playbooks/site.yml` | The entry point: the gateway baseline, then each module. |
| `playbooks/network.yml` | Shared tasks: find the host and internal interfaces and the address already on the internal one. |
| `playbooks/docker.yml` | The Docker module. |
| `playbooks/nt4-domain.yml` | The NT4 Domain module. The `nt4-domain/` folder holds the Samba image and `smb.conf.j2`. |
| `playbooks/web-proxy.yml` | The Web Proxy module. The `web-proxy/` folder holds the `Caddyfile.j2`. |
| `playbooks/dns.yml` | The DNS module. The `dns/` folder holds `dnsmasq.conf.j2` and the dnsmasq image. |
| `playbooks/status/lanlord-status.sh` | The status page generator, run every 30 seconds by a systemd timer that the Web Proxy module installs. |

## How modules work

- A module's tasks carry its id as a tag, so `lanlord apply <host> --module nt4-domain` runs just that module.
- Its settings live under the same key in your encrypted settings file, and the module does nothing until that section exists.
- The comment at the top of each playbook shows the settings it reads.

Adding a file here means adding it to the list in `crates/core/src/payload.rs`. A test fails if a file is missing from it.

## The DNS module

`playbooks/dns.yml` runs dnsmasq in the `lanlord-dns` container on the host network, bound to the internal interface and never to the host one. It answers only for its zone (`retronet` by default) and never forwards a query, so nothing resolves outside the retronet. `lanlord.<zone>`, the gateway's hostname, the NT4 Domain's NetBIOS name and any Web Proxy host ending in `.<zone>` point at the internal address; `records` adds more.

With a `dhcp:` section it also hands out leases, with no router option, the gateway as DNS and WINS server, and the zone as the domain name. The configuration is `/opt/lanlord/dns/dnsmasq.conf` and the leases are in `/opt/lanlord/dns/dnsmasq.leases`. The Docker module's firewall also drops DNS and DHCP traffic on the host interface. The image is built on the gateway from `playbooks/dns/image/`, tagged with a hash of its files.

## The Samba image

`playbooks/nt4-domain/image/` is built by CI and published to `ghcr.io/muncherelli/lanlord-samba-nt4`. The tag is a hash of the three files in that folder, and the playbook computes the same hash on the gateway, so it always pulls the image that matches the files it was sent. If the pull fails, or `nt4-domain.build_locally` is true, it builds the same image on the gateway instead.
