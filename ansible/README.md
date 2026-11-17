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
| `playbooks/replace-container.yml` | The shared tasks that replace a container and roll back when the new one is unhealthy. |
| `playbooks/status/lanlord-status.sh` | The status page generator, run every 30 seconds by a systemd timer that the Web Proxy module installs. |

## How modules work

- A module's tasks carry its id as a tag, so `lanlord apply <host> --module nt4-domain` runs just that module.
- Its settings live under the same key in your encrypted settings file, and the module does nothing until that section exists.
- The comment at the top of each playbook shows the settings it reads.

## Replacing a container safely

The NT4 Domain, DNS and Web Proxy modules replace their container through the shared `playbooks/replace-container.yml` whenever its image or configuration changes:

1. The old container `<name>` is renamed `<name>-prev` and stopped (not removed). With host networking the two cannot listen on the same ports at once.
2. The new container starts and must stay healthy for 3 checks in a row, 2 seconds apart: running, not restarting, and the module's check passing (`smbcontrol smbd ping` for Samba, the running state for dnsmasq and Caddy).
3. When it does, `<name>-prev` is removed. When it does not, the failed container's last 40 log lines are printed as a debug message, it is removed, `<name>-prev` is renamed back and started, and the run fails with the reason and a note that the previous container was restored.

The first run on a fresh gateway has no previous container, so a failure just fails. A leftover `<name>-prev` from an interrupted run is handled at the start of the next run: it is removed when `<name>` is running, and restored when `<name>` is missing or stopped.

To find a failed replacement's logs, read the "last log lines of the failed ..." message in the apply output. The failed container is removed after that, so the output is the only copy. The previous container's own logs are `docker logs <name>`.

Adding a file here means adding it to the list in `crates/core/src/payload.rs`. A test fails if a file is missing from it.

## Networks and the DNS module

LANlord manages any number of lanlord networks, each on its own interface, next to the server's normal (external) network. They are the `networks:` list in the settings:

```yaml
networks:
  - name: retronet             # a plain label; names the container lanlord-dns-<name>
    interface: eth1            # the interface the network is on
    address: 172.16.120.1/22   # the server's own address there, with its prefix
    mtu: 1450                  # optional
    zone: retronet             # the dns zone; default: the name
    records:                   # extra names in the zone
      - {name: ntsrv1, address: 172.16.120.10}
    dhcp:                      # optional; no dhcp server without it
      range: [172.16.123.1, 172.16.123.254]
      lease: 7d                # 30m, 12h, 7d or infinite; default 7d
      hosts:                   # static leases by mac
        - {name: ntsrv1, mac: "aa:bb:cc:dd:ee:ff", address: 172.16.121.1}
  - name: lab
    interface: eth2
    address: 192.168.50.1/24
```

`lanlord networks HOST list | add | set | remove | lease` edits the list, checks it and applies the docker and dns modules. The checks are the same in the program and in the playbooks: names, interfaces and zones are unique, an interface is not the external network's, addresses are real networks that do not overlap, and a dhcp range or a static lease is inside its network and is not the server's own address.

`playbooks/dns.yml` runs one dnsmasq in Docker for every network: the container `lanlord-dns-<name>` on the host network, with its files in `/opt/lanlord/dns/<name>` (`dnsmasq.conf` and `dnsmasq.leases`). Each one listens only on its network's address and interface (`listen-address`, `bind-dynamic`, `interface=`, and `except-interface=lo`, so two networks never fight over the loopback address), never on another lanlord network's or the host's, and serves dhcp only there (`no-dhcp-interface` for every other interface). It answers only for its zone and never forwards a query, so nothing resolves outside the zone. With a `dhcp:` section it hands out leases with no router option, the server as DNS and WINS server, and the zone as the domain name. `lanlord.<zone>`, the server's hostname, the nt4-domain netbios name (in the zone of the network the domain controller is on) and the web-proxy hosts ending in `.<zone>` all point at the network's address. A network taken out of the list has its container and folder removed on the next apply. The Docker module puts each network's address and mtu on its interface (a systemd-networkd drop-in, plus a minimal `<interface>.network` when nothing else manages the interface, or a NetworkManager connection), and its firewall accepts every lanlord interface first, so the rules that keep netbios, smb, dns and dhcp off the external network can never cut one off.

`nt4-domain.network` names the network the domain controller is on (default: the first). The web-proxy status page is on the first network.

Settings from before there was a list still work: a `dns:` section, `docker.internal_address` and the interface settings are read as one network called `retronet`, and the first `lanlord networks` change writes them back as a `networks:` list (and removes the old keys). On the first apply the single `lanlord-dns` container is replaced by `lanlord-dns-retronet`, and its leases move into `/opt/lanlord/dns/retronet`, so clients keep their leases. `lanlord networks list` shows a network read this way, and says so. The playbooks read both shapes through `playbooks/filter_plugins/lanlord.py` and `playbooks/networks.yml`.

## The server component images

CI publishes two images, one package each, tagged with the LANlord release version:

- `ghcr.io/muncherelli/lanlord-nt4-server-components`: samba built from the signature-checked source tarball as a classic (non-AD) server, with SMB1/NT1 still allowed. Its Dockerfile is in `playbooks/nt4-domain/image/`, next to the samba release key it checks the tarball against.
- `ghcr.io/muncherelli/lanlord-network-server-components`: dnsmasq on Alpine, from `playbooks/dns/image/`.

Tags are versions only. Both images are published under a LANlord version: an `X.Y.Z` tag plus rolling `X.Y`, `X` and `latest` tags that move only when a release is published (development builds publish just their exact `X.Y.Z`, which is what the playbooks pin). The upstream versions are not tags: they are labels (`org.opencontainers.image.version` is the LANlord version; `io.lanlord.samba.version` and `io.lanlord.dnsmasq.version` are the upstream ones, set from the `ARG` in the Dockerfile).

The images hold only upstream software. LANlord's own `entrypoint.sh`, `lanlord-user`, `smb.conf` and `dnsmasq.conf` are mounted into the container when it runs. LANlord passes the image version its build was made for to the playbooks (the `lanlord_image_version` extra var) and they pin that `X.Y.Z` (never `latest`), so a release keeps running the images of its own release and a trunk build the development images. Each apply pulls that tag; if the pull fails it builds the same Dockerfile on the gateway instead.

The version is the one in development: the next patch after the latest `v*` release tag (or the workspace version when that is already ahead), worked out by `tools/image-version.sh`. LANlord's `crates/core/build.rs` runs the same script, and the release workflow sets `LANLORD_IMAGE_VERSION` from the tag, so a build always pins the images CI published for its version. When git or the tags are not available the build falls back to the crate version. The workflow is `.github/workflows/image.yml`, with the images listed in `.github/images.json`:

- A push to trunk that changes image files builds and publishes the development version, overwriting that `X.Y.Z` while `v<X.Y.Z>` is not released.
- Once `v<X.Y.Z>` is released its images are frozen: never rebuilt or overwritten. The next publish goes to the next version.
- A release dispatches the workflow with its tag and makes sure both images have the released `X.Y.Z`. When none changed during the cycle, the newest older version's manifest is retagged (no rebuild); with no older version the image is built from the sources at the tag. A published release is never overwritten. The same run can be started by hand with a `tag` input (like `v0.2.0`). A release run ends by checking that the tag is there and fails when it is not (a waiting run can be replaced by a newer one), so run it again by hand with the same `tag`; an image is skipped for a tag from before its files existed. Without `tag` it publishes the development version. An `image` input picks one image.
- A pull request that touches image files builds them (for amd64 and arm64, without pushing).
- Each architecture is built natively (amd64 and arm64 runners, no emulation) and merged into one multi-arch tag. `X.Y`, `X` and `latest` move only on a release, and only to a version that is the newest in their range. Each image records its commit in `org.opencontainers.image.revision`; a tag counts as published only when the image has that label and a matching version label, and a release reuses or retags an image only when its image files are unchanged since that commit (otherwise it builds from the tag).

## Joining NT4 machines

The `nt4-domain` module in `domain` mode runs a Samba classic NT4-style domain controller, with the compatibility settings NT4 needs (`allow nt4 crypto`, `reject md5 clients = no`, `server schannel = auto`). They weaken the domain on purpose, so keep it on the isolated internal network.

Windows NT 4.0 SP6a could not create its own machine account against Samba 4.22: the step that sets the new account's password failed with `NT_STATUS_NO_USER_SESSION_KEY`, and the client reported "The machine account for this computer either does not exist or is inaccessible". The reliable way is to create the accounts in advance:

1. List the computers in your settings: `machines: [ws1, ws2]`.
2. Apply the module. Each name gets an account whose first password is its own name in lowercase.
3. On the NT4 machine, join the domain without ticking "Create a Computer Account in the Domain". No password prompt appears, because the account already exists.

Set `lanman_auth: true` and keep the Administrator password to 14 characters or fewer if an old client needs the LM hash. Raise `log_level` (for example `2 rpc_srv:10 passdb:10 auth:5`) to debug a join; the Samba helper logs are in `/var/log/samba` inside the container.

Roaming profiles are off by default. Set `roaming_profiles: true` to share a `profiles` folder and tell NT4 to keep each domain user's profile there, so it follows the user between machines.

Every domain logon runs `lanlord.bat` from the `netlogon` share, which LANlord writes on each apply. To run your own commands, create `logon.bat` (for everyone) or `<username>.bat` (for one user, for example `alice.bat`) in `\\LANLORD\netlogon`. Domain admins can edit that share from Windows. Scripts need DOS line endings, which Notepad and `edit` produce. You can also edit them without Windows: the Logon scripts tab of the NT4 window, or `lanlord logon HOST list`, `show NAME` and `set NAME` (the script is read from standard input). LANlord saves them with DOS line endings.

The software share is off by default too. Set `software: true` to add a `software` share that maps to `S:` at logon, for service packs and installers. Domain users read it; domain admins and the Software Admins domain group write to it (the group shows up in User Manager for Domains, but LANlord decides who is in it). To let someone write without making them a domain admin, give their entry in `users` `software_admin: true`. Files you put in the share from the gateway land in `/opt/lanlord/nt4/lib/software`.

## The web UI

The web UI runs only on the gateway. `lanlord webui` listens on the unix socket `/run/lanlord-web/web.sock` (no TCP port) as the unprivileged `lanlord` account, through the `lanlord-webui` systemd service that the Web Proxy module installs. It serves the browser build from `/opt/lanlord/web` (the `web` folder beside the binary's `bin` folder, or `--static DIR`), looking for it on every request, so a new build needs no restart. Until a build is there it shows a placeholder page, and a missing file such as a `.wasm` or `.js` is always a 404, never a page. Dot files and anything a link leads to outside the build are never served. Caddy answers on port 8443 of the normal network address only, never the retronet, and proxies to the socket.

The socket and its own folder, `/run/lanlord-web`, belong to the `lanlord-web` group (gid 9990), which gives access to nothing else. It is deliberately not `/run/lanlord`, where an apply keeps the decrypted settings. The proxy reaches the socket in one of two ways:

- this module's Caddy container runs as root and gets only `/run/lanlord-web` mounted, read-only (the folder survives service restarts, so the mount stays valid). This is the default, `proxy_user: root`.
- a Caddy installed from a package runs as its own account. Set `proxy_user: caddy` (or whatever account it runs as) and the module adds that existing account to `lanlord-web` (it never creates one); restart that Caddy afterwards so it picks up the group.

Every web UI response carries a content security policy (the app's page lists the hashes of its own inline loader script; anything else gets the plain policy), `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`, from the app and again from Caddy, which also drops its `Server` header and adds `Strict-Transport-Security` with a real certificate (not with `internal`). The service runs with a private network, no capabilities and a system call filter.

```yaml
web-proxy:
  webui:
    tls: internal               # required: letsencrypt-dns, cloudflare-origin, or internal (test gateways only)
    host: gateway.example.lan   # default: the normal network address
    port: 8443                  # the default
    proxy_user: root            # the default; caddy for a packaged caddy
```

The web UI is HTTPS only, so a `webui` section must name a certificate method in `tls`, or the apply stops and says so. Leave the section out to have no web UI.

- `cloudflare-origin` uses a Cloudflare origin certificate, for traffic that comes through Cloudflare. Put the certificate at `/opt/lanlord/caddy/config/webui-origin.pem` and its key at `/opt/lanlord/caddy/config/webui-origin.key`.
- `letsencrypt-dns` (Let's Encrypt through a Cloudflare DNS challenge) is planned; it needs a Caddy build with the Cloudflare DNS module, so the module refuses it for now.
- `internal` uses Caddy's own certificate authority. Browsers do not trust it, so it is for test gateways only.

The service waits for the `lanlord` binary at `/opt/lanlord/bin/lanlord`. Its name never goes into the retronet DNS zone.
