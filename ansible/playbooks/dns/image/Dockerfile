# dnsmasq for the retro network, on a pinned alpine. only upstream software is in the image;
# lanlord's dnsmasq.conf is mounted when the container runs.
#   docker build --build-arg IMAGE_VERSION=1.0.0 -t lanlord-network-services:1.0.0 .
# VERSION is this image's X.Y.Z (the tag); the dnsmasq version is the ARG below and a label
ARG ALPINE=3.22
FROM alpine:${ALPINE}
ARG DNSMASQ_VERSION=2.91
ARG IMAGE_VERSION=dev
LABEL org.opencontainers.image.title="lanlord-network-services" \
      org.opencontainers.image.source="https://github.com/muncherelli/LANlord" \
      org.opencontainers.image.version="${IMAGE_VERSION}" \
      io.lanlord.dnsmasq.version="${DNSMASQ_VERSION}"
# dnsmasq-dnssec is not wanted: plain dnsmasq is smaller
RUN apk add --no-cache "dnsmasq~=${DNSMASQ_VERSION}" && rm -rf /etc/dnsmasq.d /usr/share/doc /usr/share/man
ENTRYPOINT ["dnsmasq", "--keep-in-foreground", "--conf-file=/etc/dnsmasq.conf", "--log-facility=-"]
