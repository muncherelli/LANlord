# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A Task Manager (Processes, Performance and Networking) opens from the LANlord menu, and a network activity icon in the taskbar tray lights up with traffic and opens it on Networking. Anyone logged in can look; only administrators can end a process (#114)
- The web desktop asks you to log on, and Log Off returns to the logon box. Accounts that are not administrators can read but cannot change the server (#102)
- The web interface logs in with the gateway's accounts and keeps sessions (#101)
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
- A root helper checks web interface logins with PAM and the `admins` group (#64)
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

- The administrators group is renamed `admins`, one group for the gateway and the domain (#49)
- NT4 domain and network services now run from published `lanlord-nt4-server-components` (Samba built from source) and `lanlord-network-server-components` (dnsmasq) images, tagged with the LANlord version (`X.Y.Z`; the rolling `X.Y`, `X` and `latest` move on releases), published from trunk during development and frozen at release. The playbooks pin the version their build was made for. The old `image` and `build_locally` settings are gone.

### Fixed

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

## [0.1.0] - 2026-11-13

### Added

- A `lanlord` command line that runs commands over SSH (#1)
- A native desktop window (#2)
