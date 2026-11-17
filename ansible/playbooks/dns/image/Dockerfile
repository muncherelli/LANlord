# dnsmasq for the retro network, on a pinned alpine. only upstream software is in the image;
# lanlord's dnsmasq.conf is mounted when the container runs.
#   docker build --build-arg DNSMASQ_VERSION=2.91 -t lanlord-server-component:network-services .
ARG ALPINE=3.22
FROM alpine:${ALPINE}
ARG DNSMASQ_VERSION=2.91
LABEL org.opencontainers.image.title="lanlord-server-component network-services" \
      org.opencontainers.image.source="https://github.com/muncherelli/LANlord" \
      org.opencontainers.image.version="${DNSMASQ_VERSION}"
# dnsmasq-dnssec is not wanted: plain dnsmasq is smaller
RUN apk add --no-cache "dnsmasq~=${DNSMASQ_VERSION}" && rm -rf /etc/dnsmasq.d /usr/share/doc /usr/share/man
ENTRYPOINT ["dnsmasq", "--keep-in-foreground", "--conf-file=/etc/dnsmasq.conf", "--log-facility=-"]
