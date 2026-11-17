# dnsmasq for the retro network, on a pinned alpine. only upstream software is in the image;
# lanlord's dnsmasq.conf is mounted when the container runs.
#   docker build --build-arg IMAGE_VERSION=1.0.0 -t lanlord-dhcp-dns-components:1.0.0 .
# VERSION is this image's X.Y.Z (the tag); the dnsmasq version is the ARG below and a label
# the multi-arch index digest of alpine 3.22, so a rebuild cannot pick up a moved tag
ARG ALPINE_IMAGE=alpine:3.22@sha256:49340b346798875b5592e5d420241f2f9cfba5f47e62ebb1446a1b34734e898f
FROM ${ALPINE_IMAGE}
ARG DNSMASQ_VERSION=2.91
ARG IMAGE_VERSION=dev
LABEL org.opencontainers.image.title="lanlord-dhcp-dns-components" \
      org.opencontainers.image.source="https://github.com/muncherelli/LANlord" \
      org.opencontainers.image.version="${IMAGE_VERSION}" \
      io.lanlord.dnsmasq.version="${DNSMASQ_VERSION}"
# dnsmasq-dnssec is not wanted: plain dnsmasq is smaller
RUN apk add --no-cache "dnsmasq~=${DNSMASQ_VERSION}" && rm -rf /etc/dnsmasq.d /usr/share/doc /usr/share/man
ENTRYPOINT ["dnsmasq", "--keep-in-foreground", "--conf-file=/etc/dnsmasq.conf", "--log-facility=-"]
