# LANlord

> all your LAN are belong to you

## supported platforms

the gateway is the machine lanlord manages; the client is where you run `lanlord`.

| role | os | arch | status |
|---|---|---|---|
| gateway | arch linux (rolling) | x86_64 | first target |
| gateway | debian (stable) | x86_64 | supported |
| client | macos | arm64, x86_64 | supported |
| client | arch linux, manjaro, debian | x86_64, arm64 | supported |
| client | windows 10+ | x86_64, arm64 | planned; needs the openssh client |

other distros and architectures come after arch and debian. arch is a rolling release, so there is no fixed version to pin.

## where ansible runs

- **on the gateway (default):** `lanlord bootstrap` installs ansible under `/opt/lanlord` and the client only needs ssh, so it works on any client, windows included.
- **locally (optional):** if ansible is installed on your machine, lanlord can run it here and reach the gateway over your own ssh, so your agent and keychain are used directly. this needs a client that can run ansible (macos, linux, or windows through wsl).
