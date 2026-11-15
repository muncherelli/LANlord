# dnsmasq for the retro network; the version is whatever debian trixie ships
FROM debian:trixie-slim
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends dnsmasq-base \
 && rm -rf /var/lib/apt/lists/*
ENTRYPOINT ["dnsmasq", "--keep-in-foreground", "--conf-file=/etc/dnsmasq.conf", "--log-facility=-"]
