# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- IIS 4 presets `smtpsvc-start`, `smtpsvc-stop` and `smtpsvc-restart`
- the NT4 server container can run a command on a joined NT4 machine over SMB1; nothing is installed on the machine. A command that outlives its timeout has its `cmd.exe` ended, but programs it started may keep running (NT4 has no way to end them); output files they hold open are removed by the next run
- a Computers tab in the NT4 Server window (and in the text interface) lists the domain machines and runs a command or a general preset (`ver`, `ipconfig`, `net-start`) on the one you pick, after a confirmation for anything that changes it; the output, exit code, duration and a timed-out state show under the list. It needs manage rights on Remote commands
- Operations to list domain machines and run commands or presets on them
- `lanlord machines HOST` lists the domain machines and `lanlord machines HOST run NAME [--timeout N] -- COMMAND` runs a command on one; `lanlord run NAME --via HOST -- COMMAND` does the same and returns the exit code. The commands run through `lanlord-exec` as the `lanlord-exec` account, whose password is the secret `nt4-domain.exec_password`
- `lanlord hosting HOST NAME PRESET [ARGS...] [--timeout N] [--yes]` runs an IIS 4 preset on a domain machine and returns its exit code; `lanlord hosting --list` shows the presets and which ones change the machine. A preset that changes the machine asks first, unless `--yes` is given. `Operation::MachinePreset` now takes a `timeout_secs` (120 when left out). Starting and stopping the IIS services still goes through `cmd` (`net start|stop`). The general presets (`ver`, `ipconfig`, `net-start`) run with `lanlord machines HOST preset NAME PRESET [ARGS...]`
- Every `lanlord-exec` run is appended to `/var/lib/samba/private/exec.log` in the NT4 container (caller, machine, address, command or its hash, result, duration; never the password), and its output now comes in chunks of at most 1 MiB. A free-form command with a plain `exit N` is refused, as it ends the shell on the machine and the run would show as timed out
- the `lanlord-exec` domain admin account is created on a domain controller, with a generated 24-character password sealed as `nt4-domain.exec_password`; it has no logon script and is not listed as a user. `lanlord secrets HOST rotate nt4-domain.exec_password` makes and sets a new one. The name `lanlord-exec` is reserved: settings that list a user of that name are refused, and an account of that name that already exists on the server is taken over on apply (its password is set and it is made a Domain Admin)
- the Hosting component (requires the NT4 domain, no network attachment): a Hosting window in the desktop and the text interface lists the domain machines and runs the IIS 4 presets on the one you pick, and has a new `Hosting` permission area. It has an `iis` part (on by default) and a `frontpage` part (off by default, nothing uses it yet). The IIS presets (including `iis-dump`) need the component with its `iis` part turned on; with it off the Hosting window hides them and the server refuses them with a message naming the component, while `machines run` and the Computers tab are unaffected. The built-in Administrators and Operators roles can manage Hosting (so operators can use the IIS presets through it) and Users cannot see it; custom roles are not changed, so give them the Hosting area to let them use the presets, and View on Domain as well, because the Hosting window lists the machines through it
- `lanlord machines HOST put NAME SCRIPT [--file PATH]... [--timeout N] [ARGS...]` puts a script from the shipped whitelist (`lldump.vbs`) and up to four data files (1 MiB each) on a domain machine and runs it; the machine sees them as `%LL_SCRIPT%` and `%LL_DATA1%` and so on. They are staged under generated `ll-<hex>` names in `<drive>:\lanlord`, checked like the other run files, and deleted after the run. Paths and names outside the whitelist are refused. New `Operation::MachinePut`, with the same permission as running a command
- `lldump.vbs` dumps the IIS metabase (W3SVC, MSFTPSVC and SMTPSVC with their sites, virtual directories, bindings, comments, paths, access flags, script maps, filters and filter load order) as `key=value` lines; a property with `Pass` in its name is never read and prints as `<redacted>`. Run it with the `iis-dump` preset (`lanlord hosting HOST NAME iis-dump`, or the IIS presets box in the Hosting window)

### Changed

- the IIS 4 presets: `services-status` also lists SMTP; `iisadmin-start` also starts the web, FTP and SMTP services that `iisadmin-stop` takes down and ends with the service listing, so it exits with the code of `findstr` and a failed `net start` is masked; `startweb` and `stopweb` print the site's `ServerState` afterwards (2 started, 4 stopped, 1 starting, 3 stopping) and exit with the code of that read, not of the script

### Security

- NT4 commands now run from `<drive>:\lanlord` (the system drive's root), a folder only administrators and SYSTEM can write, instead of `%SystemRoot%\Temp` (which is writable by Everyone), so a local user can't swap the helper or the `.bat` before the service starts and get SYSTEM, or read other runs' output. Every run checks that the drive root can't be changed by others (nobody else may delete children, change permissions or take ownership there), sets the owner and permissions of the folder, reads them back (again, with the uploaded files, right before the service is created) and refuses to run unless they are right; an existing `lanlord` folder owned by someone else is refused, not taken over. The image now keeps `smbcacls` for this

### Fixed

- the IIS service status now shows the SMTP service on IIS 4
- the NT4 command cleanup no longer tries to delete a folder that happens to be named like a leftover file from an earlier run
- the read-only IIS 4 presets that list the metabase (`adsutil-enum-w3svc`, `adsutil-enum-site`, `adsutil-enum-ftp-site`) no longer print secrets such as `AnonymousUserPass` or `WAMUserPass`: the machine drops every property line whose name holds `pass` (any case, through `findstr`, so key headers such as `[/W3SVC/1/ROOT/Passport]` stay) before the output leaves it, and the output of any machine run hides the value of a property whose name holds `Pass` (any case and type, with the lines a list value goes on in), live and in the result; the raw output is not kept in the debug log either. `AnonymousPasswordSync` is no longer listed by these presets. The exit code of these three presets is `findstr`'s (0 when lines are left), not `cscript`'s
- `lanlord-exec` deletes the `ll-*` files an earlier run left in `<drive>:\lanlord` (a run that timed out leaves its output behind when the program still held it open) at the start of the next run, after the folder is verified
- `lanlord-exec` errors say "no output" when the failed tool printed nothing, instead of ending in a bare colon
- running a command on a machine, listing the domain's machines and resetting a machine account now use the container of the machine's domain (`lanlord-nt4-<name>`), not always `lanlord-nt4`, so they work on a gateway with more than one domain controller; a machine name that exists in two domains is refused with an error naming both, and a controller whose container cannot be listed is skipped with a warning
- the Computers command box closes when you switch hosts from another tab, instead of staying open and refusing OK
- the Computers tab no longer runs a command on the wrong server after switching hosts: an open command box only runs on the host it was opened on, and the pick and the last output are cleared as soon as another host is shown
- the NT4 command helper (`lanlord-svc.exe`) no longer crashes with an illegal instruction on NT4: it is built without SSE and MMX, which NT4 does not enable, and the image build fails if the helper contains such an instruction
- running commands on NT4 machines works: `lanlord-exec` starts commands through a small NT4 service helper (`lanlord-svc.exe`, built into the NT4 server image) instead of `cmd.exe`, which NT4 never let start as a service. A command that outlives its timeout, or a run that is cut short, now has the service stopped before it is deleted, which ends the helper's `cmd.exe`; programs it started may keep running
- `lanlord-exec` against NT4 SP6a: the rpc calls no longer require SMB signing on IPC$ (NT4 computes a bad key after an NTLMv2 login), a start that ends in `NT_STATUS_IO_TIMEOUT` is watched for the exit code like any other start timeout, and a busy service manager gives a clear "try again in a minute" error
- Windows NT 4.0 machines now run the logon script at sign-in, so the `S:` software drive is mapped (Samba refused to let NT start a script that had no execute bit), and installers on `S:` can be started from the share. Existing gateways get this on their next apply
- `lanlord-exec` logins to NT4 machines work again: NT4 has no SMB extended security, so smbclient and net now run with `client use spnego=no` (NTLMv2 stays on). A failed copy, create or start now reports the last real error lines instead of samba's deprecation warnings, and audit lines are written now (the image's BusyBox `flock` has no `-w`, so none was ever written) and before the cleanup, so a run that is cut short during cleanup is still logged
- running a command on an NT4 machine works when `%SystemRoot%\Temp` does not exist yet (a default NT4 install), and leftovers that earlier versions put in `%SystemRoot%` are cleaned up

## [0.1.5] - 2026-11-21

### Added

- a native apply engine for the NT4 domain module, chosen per module with `engine: {nt4-domain: native}` in the settings (`ansible` stays the default, `shadow` lets Ansible apply and then logs what the engine would still change). It reads `nt4-servers` and `domains` and loops per server: each server has its own container (`lanlord-nt4-<name>`; the server named `nt4-domain` keeps `lanlord-nt4` and `/opt/lanlord/nt4`) and data folder (`/opt/lanlord/nt4/<name>`), honours `wins`, and a server dropped from the list loses its container and keeps its data. It aims at the same end state as the playbook without starting Ansible (`tools/engine-parity.sh` compares the two on a scratch gateway), rolls back what it changed when a step fails, and hands identity changes and image builds back to Ansible. NT4 passwords are set only when they change in LANlord, tracked by an HMAC fingerprint under `/var/lib/lanlord-operations/engine/`, so a password changed from Windows is left alone afterwards, and a personal Domain Admin keeps the right to join machines. On the first native apply nothing is recorded yet, so every password in the settings is set once, a password changed from Windows included; to keep those, run `sudo lanlord engine record --module nt4-domain` first, which marks the password of every existing account as already set and keeps it as it is, even if it differs from the settings, without setting any. If the fingerprint key is lost and made again, the apply warns and does the same one-time reset. `Operation::PlanApply` reports what an apply would change, and `sudo lanlord operate` now forwards apply progress events
- the DHCP server is an entry of its own in Add or Remove Components, next to the DNS server (the two share one dnsmasq per network). DNS on with DHCP off writes no dhcp-range, DHCP on with DNS off sets `port=0`, and with both off the container is removed. Settings from before the split come up with both ticked
- My Server has an SSH tab: ssh sign-in with a password can be turned off and on (an sshd drop-in, `11-lanlord-auth.conf`, checked with `sshd -t` and `sshd -T` and rolled back when sshd would not take it), the administrators' ssh public keys can be listed, added and removed (keys LANlord adds are marked `# added by lanlord`, and the mark goes with the key). turning passwords off, or removing an account's last key while they are off, is refused unless you confirm, while an administrator, the signed-in account, or root (when it may sign in with a password) has no key, or when sshd reads keys from files other than the default ones, so nobody is locked out by accident
- the settings and the apply can hold more than one NT4 Server: a `nt4-servers` list (name, network, NetBIOS name, role, domain or workgroup, placement) and a `domains` map, one Samba container and one data folder per server. the settings checks refuse duplicate server names, a NetBIOS name used twice on a network, two domain controllers for one domain on a network, two WINS servers on a network, two servers on one address of a network (they share the host network, so a second server needs an address of its own), a domain controller without its domain and NetBIOS names over 15 characters. there is no screen for it yet: the NT4 Server window, the text interface and the web API still edit the first server

### Changed

- upgrade note: the settings format is now 6. `lanlord upgrade` (and the first save) turns the `nt4-domain:` section into one `domains` entry and one `nt4-servers` entry, and keeps a `.bak` copy of the old file. the migrated server keeps its container (`lanlord-nt4`), its data folder (`/opt/lanlord/nt4`) and its NetBIOS name, so nothing changes on the network and the domain keeps its SID and accounts. logins (`users`, `admin_password`) stay in `nt4-domain:`. a LANlord older than this one refuses the new file. servers added later run as `lanlord-nt4-<name>` with data in `/opt/lanlord/nt4/<name>`; removing a server removes its container but keeps its data folder
- upgrade note: the DNS and DHCP entries of Add/Remove Components now take effect on the DNS/DHCP play. An install with DNS or DHCP switched off there, or with an old `subcomponents` list that left one out, stops that service on the next apply. With both off the containers go but the lease folders under /opt/lanlord/dns stay, so turning DHCP back on keeps the leases
- the NT4 Domain module is now called NT4 Server on the desktop, in its window title, in Add/Remove Components, in the text interface and in the docs, since it can be a plain file server as well as a domain controller; module ids, settings keys and container names are unchanged
- root's ssh access moved out of the Advanced tab to the new SSH tab of My Server
- the NT4 server image is built with winbind and keeps `rpcclient` and `wbinfo`, so local and built-in groups can be made from User Manager for Domains
- before an apply changes the NT4 mode, domain name or NetBIOS name, the server's Samba state is copied to `/opt/lanlord/nt4/snapshots` (the five newest are kept)
- upgrade note: ssh to the server from the computers on your LANlord networks is now off. the firewall drops the sshd ports (read from `sshd -T`, not a fixed 22) on every LANlord interface unless the network is switched on with "Allow ssh to this server" (Networks window, or `lanlord networks HOST ssh NAME on`), or the computer is allowed on its own by mac, for good or until a time (`lanlord networks HOST ssh NAME allow MAC [--for 1h]`). the host network is untouched, and a session that is already open is not cut by the change. My Server tells you so; turn it on before you upgrade if you manage the server over ssh from a LANlord network
- builds made between releases now say vX.Y.Z-development, with the number of the next release (v0.1.5-development after v0.1.4), in the title bar, About, Welcome, `lanlord --version` and the web UI, instead of the last release number; `lanlord install`, `lanlord reinstall` and `lanlord upgrade` from such a build stop and ask for `--server-binary`, `--web-dist` and `--auth-helper` (there is no release to download for it), and its update check tells you when that release comes out
- tabbed windows (My Server, NT4 Server, DNS, Networks, Date/Time, Task Manager) keep one size on every tab: the page frame is as big as the largest page, so switching tabs no longer resizes or moves the window, and a tabbed dialog stays where it opened; the sizes are measured again when what a page shows changes, also while it is hidden
- the lists on tabbed pages (DNS leases and records, NT4 logins, logon scripts) stretch to the bottom of the window when it grows
- the Add/Remove Components wizard keeps one size on every page, so its buttons stay where they are
- the Compatibility page of My Server has a details box of a fixed height (longer text scrolls in it) and a fix box that is greyed, not removed, when there is no fix, so picking a check never changes the window
- windows keep NT4's 11 pixel margin around their contents
- a window you open, or bring back from the taskbar, the Start menu or a second double-click on its icon, comes to the front and its title bar turns navy
- closing or minimizing the active window activates the one below it; clicking the empty desktop leaves every title bar gray
- new windows open down and to the right of the last one in steps of 22 pixels, starting again at the corner of the desktop once a step would pass a quarter of its width or height; a window you reopen takes the next step, and one already open keeps its place

### Fixed

- the Details button of the Applying Settings window opens and closes the details in the web UI again, and in the text interface the button now reads `[ ► Details ]` (it was `[+] Details`) and stays open or closed from one apply to the next: its open state is kept with the dialog, not in the page, so a click always lands. the details show the output of the apply, or every task with its status when the apply printed none. `?apply=1` on a development build of the web UI plays a finished apply into the dialog
- the percentage on the progress bar of the Applying Settings window is white over the filled blocks and black over the rest (it was black on navy, unreadable), as in the text interface
- the Components networks page no longer shows a bare Address box: each joined network of every component that joins a network, except NT4 Server, offers "Gateway address (the gateway's address there)" as the default, or "Its own address" with the box. NT4 Server has no box there, because its network and address come from its `nt4-servers` entry in the Settings window (the gateway's address when blank), and the page says so; the NT4 Server description now says the same
- address fields (the DHCP range in DNS, and the address, DHCP range and ssh mac address in the Networks window) no longer call an address wrong while you are still typing it: `1` or `10.0.` stay quiet while that field has the keyboard, and the error shows as soon as the keyboard moves to another field or the value can never be right; the field also keeps the keyboard when the error line appears; Next and Save stay off until it is right; error lines in the Networks window no longer show the settings path (`networks[1].address:`): they sit under their own field, or start with the field's label, the static lease or the record they are about
- the per-user logon script (`<username>.bat` in netlogon) now runs on Windows 95 and 98 too: every user gets a small generated script in `netlogon\users` that sets the user name, which Windows 9x does not, and runs `lanlord.bat`; with roaming profiles on, Windows 9x also maps its home folder to `H:`
- changes made in User Manager for Domains stick: creating, renaming and deleting users, creating and deleting groups, group membership (Domain Admins included) and primary groups are applied and remembered across applies and container restarts; the Software Admins group is no longer rebuilt from the domain admins on every apply (it is filled from them once, then it is yours), settings now decide only new users, a password you set and `state: absent`, so `admin: true` and `software_admin: true` apply when an account is created and no longer revert a change made from Windows
- NT 4.0 machines can join the domain with an administrator's name and password, creating their own computer account (the Samba in the nt4 image is patched to keep the session key of NT4's logon)
- `lanlord upgrade` checks first whether the stored secrets are sealed to an old client key, and settles it (or stops with the exact `lanlord move-secrets` command) before changing anything, instead of failing at the last step
- resizing a window by its edge or corner no longer flickers: the frame is drawn where its contents are and follows the pointer in the same frame, and dragging the left or top edge past the window's least size holds the opposite edge still instead of sliding the window
- title bars reach the right edge of their window again (Date/Time Properties had a gap)
- the Networks window no longer clips the Received column of its Activity table, and it keeps one size when a network is added or removed (its tabs are as tall as a LANlord network's)
- the web UI draws at the screen's full frame rate again, so dragging windows is smooth (it was capped at 30 frames a second in 0.1.4)
- the DNS panel works per network: a Network drop-down picks the network whose zone, DHCP range, static leases and records are shown and saved (the other networks stay as they are), addresses are checked against that network's subnet, and the networks are listed instead of the retronet being called unknown on a gateway with two networks; settings with only the older `dns:` section still show and save as the one network, and the gateway is only asked for its network when a network has no address of its own
- the Add/Remove Components wizard lists the networks added in the Networks window (also on the web demo desktop), re-reads them when it opens, when a network is saved and when it goes to its Networks page, and shows only the message, with no empty boxes, when the server has none
- a dialog that cannot be closed with Escape (the purge box, the update box) no longer has a close button that does nothing
- dialogs opened from a window have a close button and a title bar that spans the box, and the DHCP range boxes of the network form fit it

## [0.1.4] - 2026-11-19

### Added

- closing ssh to the server cannot lock you out by accident: the gateway says where your ssh connection comes from (`$SSH_CONNECTION`, the route, and the mac of the next hop), and a change that would end it (`ssh NAME off`, `ssh NAME deny MAC`, the Networks window's switch and Remove) is refused until you pass `--allow-lockout` or confirm in the window, naming the network and the commands that keep you in; an upgrade skips applying the firewall in that case and prints them, and tells you to run `lanlord apply` when it cannot tell; a connection that cannot be placed (no interface found, or a network stored without one) is treated the same way and not let through, and the commands put the whole network first when you are behind a router, since that computer's mac is the router's; changes made in the web UI are not checked
- the Networks window lists the computers allowed to ssh to the server on their own under each LANlord network, with Add... (mac address, a name, and always, for a while or until a time) and Remove: the same change as `lanlord networks HOST ssh NAME allow MAC` and `deny MAC`
- `lanlord install` stores a minimal settings file on the server when it has none (never over an existing one), so `lanlord networks add` and `lanlord apply` work right after it; it ends with a next-steps block (the web UI address, and what to do when root has no password), hides raw package manager and ansible output unless `--verbose` (shown when a step fails), and the web proxy summary no longer prints an empty site list; `config push` and `apply` end with a success line; `examples/settings.yml` is a sample settings file
- `lanlord install` offers to set up the first administrator when the server has none besides root: it adopts an account that can already use sudo or creates a new one (name, password asked twice, optional ssh key) through the existing admin-creation operation; `--admin USER`, `--create-admin NAME` (with `--admin-password-stdin`, `--admin-ssh-key FILE`) and `--no-admin` do it without prompts, and with no terminal and no flag it is skipped and the next steps point at the web first-run setup
- a root login over ssh may always run CLI operations (`networks`, `apply`, `config push`, ...); the run is noted in the system log as root
- internet access, off by default: per network (`lanlord networks HOST set NAME --internet on`) or per computer by MAC address, for good or for a while (`lanlord networks HOST internet NAME allow MAC --for 1h`); ended grants stop on their own, NAT and forwarding live in the lanlord firewall table, and dnsmasq hands the gateway out as the router; any access on a network turns on upstream DNS for every computer on it, so computers that are not allowed out can still resolve names and could tunnel over DNS
- every stored file now carries a format version (`settings.yml`, `settings.age`, and the client's `hosts`, `last`, `prefs` and launcher manifest); a file with no version is version 1, and older formats are brought up to date in memory when they are read
- `lanlord upgrade` migrates the stored files once on the gateway, keeping a copy of each old file (`<file>.v<N>.<timestamp>.bak`) and putting them back if the migration fails and the upgrade is rolled back
- a file written by a newer LANlord is refused with "written by LANlord X.Y (format N); upgrade LANlord to use it" and is never overwritten; docs/stored-formats.md says how to change a format and how to go back to an older LANlord
- the settings format is now 4: networks may carry `internet` and `internet_allow`, and an older LANlord refuses a file in this format instead of dropping the grants when it saves
- the Networks window adds, changes and removes LANlord networks (New network..., the Properties of a network, and a click on an unused card in the interface list), like `lanlord networks add`, `set` and `remove`; the operations service has a `change_networks` operation that needs manage on Network
- first-run setup for root in the web UI: while no administrator other than root exists, root's password signs in to a full-screen setup that creates the first administrator account and sets root's ssh access (one of the three levels, nothing preselected), and nothing else; once another administrator exists root is refused as before
- "Root ssh access" setting (allowed, keys only, off) in My Server and in the first-run setup, applied as an sshd drop-in that is checked with `sshd -t` and rolled back if sshd refuses it; the first-run setup refuses "off" when sshd takes no ssh passwords, since the new account has no key, and only an administrator changes the setting later

- an optional ssh public key for the new account in the first-run setup (checked for format, installed as `~/.ssh/authorized_keys` with modes 700/600 through the operations layer), and My Account lists, adds and removes your own authorized keys; "off" is allowed in the setup when a key was given
- the sign-in box says "root can't sign in to the web once an administrator exists; sign in as your administrator account" when root's password was right but root is closed; a wrong password keeps the generic message

### Changed

- the desktop stops redrawing when nothing changes: no timer on the sign-in screen, one frame a minute for the taskbar clock, no blinking text cursor, icons uploaded once as textures, the look set once, and the browser build capped at 30 frames a second without dithering
- the sign-in box opens on the connection you used last, "This server" included; the last sign-in is kept in a `last` file beside the recent servers
- the LANlord menu no longer has "Quit LANlord"; the terminal interface still quits with Ctrl+Q, which its menu notes on the last row, and the sign-in box still shuts down

### Fixed

- first-run setup: the root ssh label is no longer clipped, the page no longer shifts when a choice is picked (every choice is described at all times), and "keys only" explains that the administrator needs an ssh key to use ssh
- status bars wrap a long message instead of cutting it off, so a refused and rolled back root ssh change is shown whole
- the web menu no longer starts with "Server: local", which only made sense where a server can be chosen
- a window's status bar stays on its bottom edge, in every window and below contents that scroll, instead of floating up to the end of its contents or scrolling away
- every check on My Server's Compatibility tab says what it is for, along with the components that use it
- the lease times of a network connection's details show the gateway's UTC offset, or UTC when its clock cannot be read
- the sign-in box no longer fills its fields from the "This server" target, and the last sign-in is written whole and private (mode 0600) with write errors logged
- turning internet access on no longer fails on a server where docker has no `/etc/docker` yet

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
