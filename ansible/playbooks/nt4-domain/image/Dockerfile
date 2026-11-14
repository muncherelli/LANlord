# samba for retro windows clients; the version is whatever debian trixie ships
FROM debian:trixie-slim
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends samba smbclient \
 && rm -rf /var/lib/apt/lists/*
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
COPY lanlord-user /usr/local/bin/lanlord-user
RUN chmod 0755 /usr/local/bin/entrypoint.sh /usr/local/bin/lanlord-user
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
