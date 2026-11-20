<p align="center"><img src="assets/icon-512.png" alt="LANlord" width="160"></p>

# LANlord

[![Release](https://img.shields.io/github/v/release/muncherelli/LANlord?label=release)](https://github.com/muncherelli/LANlord/releases)
[![Quality Gate](https://sonarcloud.io/api/project_badges/measure?project=muncherelli_LANlord&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=muncherelli_LANlord)
[![Coverage](https://sonarcloud.io/api/project_badges/measure?project=muncherelli_LANlord&metric=coverage)](https://sonarcloud.io/component_measures?id=muncherelli_LANlord&metric=coverage)
[![Code Smells](https://sonarcloud.io/api/project_badges/measure?project=muncherelli_LANlord&metric=code_smells)](https://sonarcloud.io/project/issues?id=muncherelli_LANlord&resolved=false&types=CODE_SMELL)

> all your LAN are belong to you

## What it is

LANlord manages a retro-computing gateway: a Linux machine that sits between your normal network and an isolated "retronet" of vintage machines. You describe the gateway in one encrypted settings file, and LANlord applies it with Ansible. It ships as a single `lanlord` binary with a command line, a terminal UI, a native desktop window and, later, a web interface.

LANlord is early software. The pieces below work today, but expect rough edges.

## What works today

| Module | What it does |
|---|---|
| Gateway | A baseline: the apply-time record and the message of the day. |
| Docker | Installs Docker and drops NetBIOS and SMB on the host network. Works on LXC containers (they need nesting and keyctl. Proxmox: `pct set <CTID> --features nesting=1,keyctl=1`; Incus/LXD: `security.nesting=true`), VMs and bare metal. |
| NT4 Domain | Samba in a container, as an NT4-style domain controller or a plain workgroup server, bound to the internal network. The desktop window edits its settings and logins (passwords are stored encrypted, apart from the plain settings). |
| Web Proxy | Caddy in a container, proxying sites on the internal network, with plain HTTP or Caddy's own certificate authority. It also serves a plain HTML retronet status page (machines, services, sites) at `http://lanlord.retronet/` and on the internal address, readable by old browsers. |
| DNS | dnsmasq in a container: local-only DNS for the retronet and optional DHCP without a gateway option, for Windows 98, 2000, NT4 and old Linux clients. The desktop window edits the zone, the DHCP range, static leases and extra records, and lists the current leases. |
| Time Server (NTP) | chrony in a container: serves the time to the networks you tick in the Components window, from public pools or servers you list, without ever setting the server's own clock. dnsmasq hands its address out as DHCP option 42 on the networks both serve. |
| Remote Cmds | Runs a command on the gateway over SSH from the desktop window, with the output in the window and the console. Running commands on NT4 machines is still being researched. |

The desktop window's LANlord menu switches between saved servers, and reboots or shuts down the selected server after asking. The taskbar always names the server that actions target.

Internet access is off on every network until you turn it on. `lanlord networks HOST set NAME --internet on|off` switches it for a whole network. For single computers on a network that is otherwise off, `lanlord networks HOST internet NAME allow MAC [--for 1h | --until 'YYYY-MM-DD HH:MM'] [--name LABEL]` lets one MAC address out, for good or for a while, `deny MAC` takes it back and `list` shows who is allowed. A time-limited grant stops working by itself when it ends, with no apply, and a timer then removes it from the settings. Traffic leaves through the gateway's own network, is translated (NAT) there, and the same firewall keeps the networks from reaching each other. Where Docker's FORWARD policy would block it, the Docker module sets `ip-forward-no-drop` in `daemon.json`. dnsmasq hands the gateway out as the router, to the whole network or only to the allowed computers. Note that any access on a network, even for one computer, makes the DNS on that network forward other names to the gateway's resolvers for every computer on it, because dnsmasq cannot choose per computer. Computers that are not allowed out can therefore still resolve names, and could tunnel data over DNS. Open flows of a computer that is denied, whose grant ended, or whose network is switched off are cut at the next apply or prune when `conntrack` is installed, and grants come back after a reboot only while they have not ended. The Networks window shows the state read-only.

Ssh to the server from the LANlord networks is off on every network until you turn it on, enforced by the gateway's firewall (the host network is not touched, and the sshd ports come from `sshd -T`). Tick "Allow ssh to this server" on a network's tab in the Networks window, or run `lanlord networks HOST ssh NAME on|off`. For single computers on a network that is otherwise off, `lanlord networks HOST ssh NAME allow MAC [--for 1h | --until 'YYYY-MM-DD HH:MM'] [--name LABEL]` lets one MAC address in, for good or for a while, `deny MAC` takes it back and `list` shows who is allowed. A session that is already open is left alone when the rules change, and ends when the person closes it. Gateways that upgrade get the block; My Server tells you so. A change that would cut off the connection it is made over (ssh off for your own network, or denying your own computer) is refused by the CLI without `--allow-lockout` and asked about in the Networks window; the upgrade leaves the firewall unapplied then and prints the commands that keep you in. Limits: a mac grant is only as strong as the mac, which any computer on the same layer 2 network can imitate; `ether saddr` does not match on tun or wireguard interfaces, so those are not covered by per-computer grants; and a session that is already open stays open after its grant ends or the computer is denied.

Planned: Let's Encrypt certificates through Cloudflare, a Cloudflare tunnel, running commands on NT4 machines, links for remote retro machines, and the web interface with PAM login.

## Quick start

```sh
lanlord install user@gateway                # Set up the gateway, Docker, the web UI and a minimal settings file (an existing one is kept).
lanlord networks user@gateway add --help    # Add a network to those settings, or edit them in the web UI.
lanlord config push user@gateway examples/settings.yml   # Or send a whole settings file; the gateway seals the secrets with its own key.
lanlord apply user@gateway                  # Apply everything, or add --module nt4-domain for one module.
lanlord                                     # The terminal UI.
lanlord gui                                 # The desktop window.
```

On linux, download the `lanlord-vX.Y.Z-linux-ARCH.tar.gz` release asset (ARCH is `x86_64` or `arm64`) for the client. The `-headless` build has no desktop window, so `lanlord gui` does not run with it. `lanlord install` downloads `lanlord-vX.Y.Z-linux-ARCH-headless.tar.gz` and `lanlord-vX.Y.Z-web.tar.gz` onto the gateway by itself.

Secrets never leave the gateway: it makes its own key on first start (root only, in `/var/lib/lanlord-operations`), and a client can set a secret but never read it back. A settings folder sealed by an older version to a key in `~/.config/lanlord/identity` moves over once with `lanlord move-secrets user@gateway`. On the gateway itself, the terminal UI and the desktop window offer "This server" in the logon box: no ssh, signed in as the account you are using.

The user you connect as needs `sudo` on the gateway, or can be `root`, which works but prints a warning (see Security notes). LANlord uses your own `ssh` client, so your `~/.ssh/config`, jump hosts and SSH agent (including 1Password) all work.

## Date, time and time zone

Double-click the clock in the taskbar to open Date/Time Properties for the server you are signed in to (the taskbar clock shows the server's time, not your computer's). It has the familiar three tabs: Date & Time, Time Zone and Internet Time. From the command line, `lanlord clock HOST show`, `set-timezone ZONE`, `set-time 'YYYY-MM-DD HH:MM:SS'`, `ntp on|off [SERVERS...]` and `update` do the same. The time zone and the time servers are also kept in the `gateway` settings (`timezone`, `ntp`, `ntp_enabled`), so a later apply does not undo them; a time set by hand is a one-off. A server that is an LXC container takes its clock from the Proxmox host, so there only the time zone can be changed.

## Checking a server

`lanlord doctor HOST` (also `lanlord check HOST`) says what kind of machine a server is (bare metal, a virtual machine, a privileged or unprivileged LXC container) and what it can do for LANlord: nesting and keyctl for Docker in an LXC container, `/dev/net/tun`, nftables, ip forwarding, which kinds of network link the kernel allows (bridges, VLANs, veth, macvlan, ipvlan, VXLAN, GRE, WireGuard), Docker's firewall backend and FORWARD policy, the network manager, and VDE. It marks each line OK, WARN, FAIL or unknown and gives the exact fix, for example `pct set <CTID> --features nesting=1,keyctl=1`; `--json` prints the report as data. `lanlord apply` asks the same probe before the modules that use Docker (docker, nt4-domain, web-proxy and dns; a plain apply checks the ones your settings configure) and stops only on a definite FAIL, never on something it could not tell or a probe that failed. `ansible/playbooks/docker.yml` runs the same probe script. LANlord cannot change a container's features from inside it; the fix is made on the Proxmox host, followed by a restart of the container.

## Starting over

`lanlord purge HOST` removes LANlord from a server: its containers and images, `/opt/lanlord`, the firewall rules, the `lanlord` service account, the `admins` group and the sudo rules. It first lists everything it would remove and the data you would lose, then asks you to type the host name. The Docker engine, `/etc/motd` and the accounts of people stay unless you pass `--remove-admins` (root login only). Note that `settings.age` is the only copy of the stored secrets.

## Running a development build

`tools/dev.sh` opens a build from a checkout without installing anything:

- `tools/dev.sh gui` and `tools/dev.sh tui` build and open the desktop window or the terminal ui. Add `--demo` for canned data and a fake update (no server is touched), `--clean` for an empty config in a throwaway folder, `--open dns,networks` to start with windows open, and `--release` for an optimised build.
- `tools/dev.sh app` builds and installs the macOS app.
- `cargo gui` and `cargo tui` are shorter ways to run the same builds.

For the web UI and a test server:

- `tools/dev.sh web --serve` builds the browser version and serves it on this machine only (`http://127.0.0.1:8080/`, or `--port N`), with the same web server and headers a gateway uses. The browser build is still a stand-in page: it shows no server data and talks to no server. It needs `trunk` and the wasm target, which `tools/build-web.sh` tells you how to install.
- `tools/dev.sh linux-build` cross-compiles the server binary (no window, with the web server) for a Linux machine, `--target x86_64` (default) or `aarch64`. It needs zig and cargo-zigbuild, and installs nothing itself: `brew install zig`, `cargo install cargo-zigbuild --locked` and `rustup target add x86_64-unknown-linux-gnu`.
- `tools/dev.sh deploy user@gateway.example.lan` builds both, shows what would change on the server (the binary, the web files), asks you to type the host, then installs them over SSH and restarts only the `lanlord-webui` service. It touches only `/opt/lanlord/bin/lanlord` and `/opt/lanlord/web`, keeps the previous versions as `lanlord.prev` and `web.prev`, and puts them back by itself if the service does not answer within 10 seconds. `--dry-run` changes nothing, `--no-build` reuses the last build, and `deploy HOST --rollback` restores the previous version. The host is always an argument and is never stored. Sign in as root, or as a user with passwordless sudo.
- `lanlord install user@gateway.example.lan` also installs and sets up the web UI (the `lanlord-webui` service, caddy on https port 8443 with its own certificate, and the PAM login for the `admins` group). It downloads this version's release files on the gateway and checks their sha256 (integrity only: the sums come from the same release, and there is no signature check yet); for a development build pass the local files instead: `lanlord install HOST --server-binary target/x86_64-unknown-linux-gnu/release/lanlord --auth-helper ./lanlord-auth-helper --web-dist target/web-dist` (after `tools/dev.sh linux-build` and `tools/dev.sh web`; the login helper links libpam, so build it on a linux machine with `cargo build --release -p lanlord-auth-helper` and point `--auth-helper` at that). `lanlord reinstall` takes the same flags, and `lanlord purge` removes all of it.
- `tools/test-dev-pipeline.sh` tries the deploy, rollback and failure paths against a folder that stands in for a server, with no network.

`tools/dev-launchers.sh install` creates launchers for them (app bundles in `~/Applications` on macOS, desktop entries on Linux): the GUI and TUI from your checkout's debug build, and a preview build that merges `origin/trunk` with every open pull request (the ones that conflict are skipped and listed), with a Preview Update launcher that rebuilds it. It keeps launchers that already exist unless you pass `--force`, and `tools/dev-launchers.sh remove --yes` takes them away. A debug build also has a Developer entry in the LANlord menu that rebuilds and relaunches it.

## Supported platforms

The gateway is the machine LANlord manages. The client is where you run `lanlord`.

| Role | OS | Architecture | Status |
|---|---|---|---|
| Gateway | Arch Linux (rolling) | x86_64, arm64 | First target |
| Gateway | Debian (stable) | x86_64, arm64 | Supported |
| Client | macOS | arm64, x86_64 | Supported |
| Client | Arch Linux, Manjaro, Debian | x86_64, arm64 | Supported |
| Client | Windows 10 and later | x86_64, arm64 | Planned; needs the OpenSSH client |

Other distributions and architectures will follow. Arch is a rolling release, so there is no fixed version to pin.

## Where Ansible runs

- **Today:** on the gateway. `lanlord bootstrap` installs `ansible-core` under `/opt/lanlord`, so the client only needs SSH and works on any operating system.
- **Planned:** locally by default when Ansible is installed on your machine, reaching the gateway over your own SSH so your agent and keychain are used directly. Running on the gateway stays available. Windows clients would need WSL for the local mode.

## Security notes

- Secrets (values named `password`, `admin_password`, `token`, `api_token` or `secret`) are encrypted on your machine with [age](https://age-encryption.org); the gateway stores only ciphertext for them. The desktop window keeps the rest of your settings as readable YAML on the gateway (`/opt/lanlord/config/settings.yml`, readable by `admins`) and never shows a secret's value. Settings pushed with `lanlord config push` are still one encrypted file until you save from the desktop window. During an apply, the merged plaintext exists on the gateway only briefly, in a root-only directory on a memory-backed filesystem.
- The `lanlord` account on the gateway can run commands as root, because Ansible needs that. Treat access to it like root access.
- Membership of the `admins` group on the gateway is a root-level grant, because LANlord applies configuration as root. The group exists to say who may administer LANlord, and `lanlord bootstrap` adds the ssh user to it. Manage it with `lanlord admin <host> list`, `add <user>` and `remove <user>`. The planned next step is a root-owned socket daemon that accepts only members of this group, so that administering LANlord no longer needs a general sudo rule.
- Root's ssh access is a setting: allowed, keys only or off (`PermitRootLogin yes`, `prohibit-password`, `no`), set in My Server or in the web UI's first-run setup. LANlord writes `/etc/ssh/sshd_config.d/10-lanlord-root.conf`, checks it with `sshd -t` and puts the old state back if sshd refuses it. The first-run setup requires a choice and refuses "off" when sshd takes no ssh passwords. A server whose sshd does not read `sshd_config.d` says so instead. "Off" also stops LANlord clients that connect as root over ssh. On a gateway with no administrator yet, root's password signs in to the web UI for the first-run setup only (see `docs/webui-login.md`).
- You can sign in as `root`, which is how a new container boots, and LANlord runs its commands directly. It tells you that this is fine but recommends a privileged account, and `lanlord admin <host> create <name>` makes one (asks for its password, `--ssh-key FILE|-` authorizes a public key, `--passwordless-sudo` is an opt-in per account). Bootstrap installs `/etc/sudoers.d/admins` so members of `admins` may use sudo, and sudo remembers their password for 15 minutes across LANlord's separate ssh sessions: you are asked once, LANlord stores the password nowhere, and the timer lives in sudo on the gateway. A root login over ssh may always run LANlord's commands (`networks`, `apply`, `config push` and the rest); the operations service treats root as an administrator and notes in the system log that the operation ran as root.
- At the end of `lanlord install`, a server with no administrator other than root gets one: at a terminal the installer asks (as a non-root user: make that account the administrator or create a new one; as root: create one, or skip and sign in to the web UI as root). `--admin USER` adopts an account that can already become root with sudo (refused when it cannot), `--create-admin NAME` creates one (the password is asked for twice, or read from one line of standard input with `--admin-password-stdin`; `--admin-ssh-key FILE` authorizes a key), and `--no-admin` skips it. With no terminal and no flag it is skipped, and the next steps point at the web UI's first-run setup.
- The Python virtual environment in `/opt/lanlord/venv` is owned by root, not by `lanlord`. Ansible runs from it as root, so a writable copy would let the `lanlord` account run its own code as root.
- The Samba container speaks SMB1 and NTLMv1 so that old Windows can join. That is why it listens only on the internal network, and why a firewall rule drops those protocols on the host network.

## Development

A pull request that changes what people see or do adds a line under `## [Unreleased]` in [`CHANGELOG.md`](CHANGELOG.md), in Added, Changed or Fixed, ending with its PR number like `(#123)`. Nothing checks this; reviewers ask for it. Commit titles follow Conventional Commits, but the changelog is hand-written, and the release pull request turns `## [Unreleased]` into the version's section.

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

A change to a file LANlord stores adds a migration step and a fixture test; see [`docs/stored-formats.md`](docs/stored-formats.md).

Shell code is linted by `tools/lint-shell.sh` (shellcheck, actionlint, and a check that every `case` has a `*)` branch); to run it before every push (it lints the working tree, not the commits being pushed), use `git config core.hooksPath .githooks`.

Linux builds of the desktop window need the usual GUI libraries (for example `libxkbcommon-dev`, `libwayland-dev` and `libgl1-mesa-dev` on Debian). Build with `--no-default-features` for a binary without the window. The gateway's web server (`lanlord webui`) is behind the `webui` feature: `cargo build -p lanlord --no-default-features --features webui`. The Ansible files live in [`ansible/`](ansible/README.md).
