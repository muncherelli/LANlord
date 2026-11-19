# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- first-run setup for root in the web UI: while no administrator other than root exists, root's password signs in to a full-screen setup that creates the first administrator account and sets root's ssh access, and nothing else; once another administrator exists root is refused as before
- "Root ssh access" setting (allowed, keys only, off) in My Server and in the first-run setup, applied as an sshd drop-in that is checked with `sshd -t` and rolled back if sshd refuses it; sshd is left alone until someone chooses

### Changed

- the sign-in box opens on the connection you used last, "This server" included; the last sign-in is kept in a `last` file beside the recent servers

### Fixed

- the sign-in box no longer fills its fields from the "This server" target, and the last sign-in is written whole and private (mode 0600) with write errors logged

## [0.1.3] - 2026-11-19

### Added

- Admins can set the title and message shown on the web sign-in screen from Settings; the sign-in page reads them from a public endpoint that returns only those two fields (#156)

### Fixed

- `lanlord upgrade` starts enabled `lanlord-*` services, sockets and timers that were down; one that stays down fails the upgrade by name without rolling back, and only the web interface and operations services decide a rollback (#155)
- root (uid 0) on the gateway counts as an admin, so `lanlord apply --module` works when run as root (#155)

## [0.1.2] - 2026-11-18

### Fixed

- `lanlord install` on Debian no longer fails to start `lanlord-firewall` (nft lives in `/usr/sbin` there), and a re-run starts the unit again after a failed run instead of skipping it (#144)

## [0.1.1] - 2026-11-18

### Added

- `lanlord upgrade HOST` upgrades an installed gateway in place without purging anything, checks that the services come back, and rolls the binary and web files back if they do not (#131)
- Secrets stay on the gateway: clients hold no key and never decrypt, secrets are write-only (reads show set or not set), and `lanlord move-secrets HOST` moves an old local key to the gateway once (#127)
- On the gateway, ordinary local accounts sign in without ssh through a local socket, as themselves; the logon box offers "This server" first (#127)
- A root operations service on the gateway runs the web interface's operations for the signed-in account, re-checking its permissions on every request, with no ssh in the web path (#124)
- Live event streams for the browser's Task Manager and apply progress, with automatic reconnection (#125)
- The terminal desktop has the Networks, My Account and Permissions windows (#128)
- The terminal desktop has the update notice, check for updates, Rebuild and Developer menu actions (#129)
- A `devtools` build feature, off in releases, adds a red bug menu with a demo desktop and window-manager debug, in the window, the browser and the terminal (#108, #129)
- A Time Server (NTP) component runs chrony in a container for the networks you tick, and dnsmasq hands out its address as DHCP option 42; `lanlord doctor` warns when a host chronyd or ntpd already listens on udp 123 (#118)
- `lanlord leases`, `lanlord power` (reboot or shutdown, confirmed by typing the host name) and `lanlord facts` show DHCP leases, power off or restart a server, and print system facts (#63)
- Clock changes run on the gateway in plain Rust through a bridge, falling back to the old scripts when it is missing (#113)
- Each account gets a role (Administrators, Operators or Users, or one you define) that says what it may see and change in each area; windows and menu items you may not use are hidden (#115)
- My Account for everyone: edit your profile and change your password, which is changed on every linked account or on none (#115)
- Users and Permissions window for administrators: roles, a grid of areas and levels, and who has which role (#115)
- Applying settings shows a progress dialog with a bar, the task being worked on, the elapsed time and the tasks done so far, with the raw output under Details, in the window and the terminal (#112)
- A Task Manager (Processes, Performance and Networking) opens from the LANlord menu, and a network activity icon in the taskbar tray lights up with traffic and opens it on Networking. Ending a process needs the matching permission (#114)
- The web desktop asks you to log on, and Log Off returns to the logon box. What an account may do depends on its role (#102, #115)
- The web interface logs in with the gateway's accounts, any ordinary account may sign in, and keeps sessions (#101, #124)
- The browser desktop runs every server operation over HTTP (#98)
- Install and set up the web interface (#93)
- `lanlord install` and `lanlord reinstall` set up a gateway in resumable steps (#88)
- Each component shows which LXC features it needs, with checks and fixes (#87)
- A Welcome window opens after logon (#86)
- The real LANlord desktop runs in the browser (#75, #65)
- Networks has a tab per network, with an `ncpa.cpl`-style status for the external one (#77)
- The terminal UI draws the same desktop as the window, with the mouse (#76, #69)
- Only switched-on components show, and a components setup wizard turns them on and off (#73)
- `lanlord doctor` checks the environment in one place (#78)
- Any number of LANlord networks, each with its own dnsmasq (#83)
- Developer tools: cross-compile the server for Linux, preview the web interface locally and deploy to a test server (#82)
- Developer and preview launchers, and dev builds with demo data (#81)
- The web interface is served from the gateway behind Caddy on port 8443 (#66)
- A root helper checks web interface logins with PAM and tells administrators (the `admins` group) from other accounts (#64)
- LXC, nesting and keyctl are detected, and Docker modules stop where they cannot run (#56)
- Install and remove app launchers on Linux, macOS and Windows (#67)
- A Date/Time Properties window opens from the taskbar clock (#55)
- A redrawn logo and app icon in the NT setup style (#54)
- The server moves into the menu, and the update icon is redrawn (#53)
- A developer rebuild-and-relaunch action, and an update notice beside the clock (#52)
- My Server and Networks icons and windows (#57)
- `lanlord purge` removes LANlord from a server, with a dry run (#51)
- Icons are redrawn in the shaded Windows 95 style (#50)
- An NT-style logon box, and a create-administrator screen for root (#48)
- Sign in as root, create admin accounts and ask for the sudo password once (#47)
- Desktop windows get real Windows 95 controls and layouts (#46)
- Edit the domain logon scripts from the command line and the NT4 window (#45)
- A per-user software admin flag in the command line and the window (#44)
- Reset a machine account so a removed machine can rejoin the NT4 domain (#43)
- Run custom logon scripts from netlogon that domain admins can edit (#42)
- NT4 domain: a software share (#40)
- NT4 domain: a machines command (#39)
- The LANlord icon and logo (#38)
- Users, DNS, NT4 domain and Remote Cmds windows (#37)
- The desktop talks to hosts, the console, settings and background tasks (#33)
- NT4 domain: roaming profiles for Windows 9x and 2000, and domain admins (#35)
- DNS module: dnsmasq for local DNS and DHCP without a gateway (#31)
- Web proxy: a status page on the internal address (#30)
- The `lanlord-admins` group, and a root-owned Python virtual environment (#29)
- A desktop with icons, windows and a taskbar (#27)
- Web proxy module: Caddy in Docker for sites and the web interface (#24)
- Manage NT4 logins from the command line and the terminal UI (#23)
- NT4 domain module: Samba in Docker as a domain controller or workgroup server (#22)
- Docker module for LXC, VMs and bare metal (#21)
- Module registry, `apply --module` and a terminal desktop (#19)
- Apply your decrypted settings, starting with the message of the day (#18)
- Settings are encrypted with an age key and stored on the gateway (#17)
- Push the bundled playbook to a gateway and run it as `lanlord` (#16)
- Bootstrap a host with a masked sudo password prompt (#15)
- The terminal shell, with hosts, a command box and a console (#14)
- Bootstrap creates the `lanlord` service account and installs Ansible (#10)

### Changed

- My Server is a fixed-size dialog (minimise and close only) that shows the CPU and memory the container allows (cgroup limits, marked as limited by the container) and the CPU model name; the network interfaces moved to the Networks window. This applies to the window, browser and terminal desktops (#138)
- Linux builds now start on glibc 2.17 and newer (the desktop window also needs the system's display and graphics libraries) (#135)
- Settings are checked on the server when saved and again before Apply: a top-level section LANlord does not know, or (in a component's settings that have no fixed type) text that is just yes, no, on, off, true or false, is refused with an error naming the field; yes/no/on/off in on/off fields are read as true/false (#132)
- The terminal desktop uses one layout system: windows sized to their contents and clamped to the screen, one column of padding, equal-width buttons, a navy selection and wrapped text (#126, #130)
- The terminal desktop restores the terminal when it gets SIGTERM or SIGHUP (#130)
- The administrators group is renamed `admins`, one group for the gateway and the domain (#49)
- Every window opens fitted to its contents, centred (dialogs) or cascaded, and scrolls only when the desktop is too small; the console opens 80 by 24 cells, and the windows with a list, log or text (hosts, settings, changelog, networks, My Server and the rest) can be resized, their lists filling whatever room they are given, while plain forms keep a fixed size. The Changelog window shows plain text without the template's intro (#105)
- The Welcome window shows the version and has a Changelog topic that opens the changelog (#105)
- NT4 domain and network services now run from published `lanlord-nt4-server-components` (Samba built from source) and `lanlord-network-server-components` (dnsmasq) images, tagged with the LANlord version (`X.Y.Z`; the rolling `X.Y`, `X` and `latest` move on releases), published from trunk during development and frozen at release. The playbooks pin the version their build was made for. The old `image` and `build_locally` settings are gone (#107, #120)

### Fixed

- `lanlord doctor` checks container nesting outside the operations service's sandbox (inside it the check always said no), and the web interface, window and terminal desktops show the real signed-in login instead of root for a sudo user (#137)
- A save whose secret path does not fit what is stored (such as `docker[a].token` over a section) is refused instead of wiping the stored secrets (#123)
- The firewall drops NTP (udp 123) on the uplink, and the Time Server health check works on a host without `ss` (#122)
- The taskbar start button shows the logo with no separator after it (#106)
- NT4 domain group mappings that drifted are repaired (#40)
- Settings are read when the plain settings directory cannot be searched (#39)
- A second network works on a NIC nothing else manages, and dnsmasq no longer listens on loopback (#104)
- The browser desktop starts without threads or `std` time (#100)
- The web interface installs on a fresh Arch gateway (#97)
- Ansible is always put on the gateway, whatever is installed locally (#96)
- Only a running Ansible apply makes the server busy, and the purge says so (#95)
- The Docker engine installs in the baseline without network settings (#92)
- Install always reruns bootstrap and creates the state directories as root (#91)
- Ansible runs without the `lanlord` home, and an old `lanlord` account is repaired (#89)
- The terminal UI draws the components wizard and its new nodes (#85)
- The time zone changes first, and is kept in the settings only when LANlord is installed (#80)
- The Wayland app id is set so desktops can match the window to its icon (#60)
- Pixel icons draw as one mesh so they show at 1x scale (#59)
- A container replacement that does not become healthy is rolled back (#36)
- NT4 machines can join a Samba domain (#34)
- The fallback devices tunnel modules create are ignored (#32)
- The NT4 domain logon script task name is quote-free, and the embedded YAML is tested (#41)
- The internal interface's existing address is used by default (#25)
- Bootstrap runs on Arch and creates a regular service account (#12)

### Security

- The server checks every setting and operation argument before anything runs: host names, domains, addresses, MAC addresses, ports, interface names, time zones, login names and paths must be well formed, and an unknown top-level settings section is refused. Every text value reaches the playbooks marked unsafe so they are never run through Jinja templating (#132)
- The release key is downloaded over https only (#94)
- Secrets are kept only on the gateway, under a root-only key that only the root service uses to decrypt; clients never hold a key (#127)
- Web interface operations go through a root service that accepts only the web account and root on its socket, limits request size and time, and re-checks the account's permissions each time (#124)
- Validation patterns now reject values with a trailing newline (#119)

## [0.1.0] - 2026-11-13

### Added

- A `lanlord` command line that runs commands over SSH (#1)
- A native desktop window (#2)
