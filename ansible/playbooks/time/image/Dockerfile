# chrony for the lanlord networks, on a pinned alpine. only upstream software is in the image;
# lanlord's chrony.conf is mounted when the container runs.
#   docker build --build-arg IMAGE_VERSION=1.0.0 -t lanlord-time-server-components:1.0.0 .
# VERSION is this image's X.Y.Z (the tag); the chrony version is the ARG below and a label
# the multi-arch index digest of alpine:3.22, so a rebuild cannot pick up a moved tag
ARG ALPINE_IMAGE=alpine@sha256:49340b346798875b5592e5d420241f2f9cfba5f47e62ebb1446a1b34734e898f
FROM ${ALPINE_IMAGE}
ARG CHRONY_VERSION=4.6.1
ARG IMAGE_VERSION=dev
LABEL org.opencontainers.image.title="lanlord-time-server-components" \
      org.opencontainers.image.source="https://github.com/muncherelli/LANlord" \
      org.opencontainers.image.version="${IMAGE_VERSION}" \
      io.lanlord.chrony.version="${CHRONY_VERSION}"
# the chrony package creates the chrony user; fail the build if it ever stops doing so
RUN apk add --no-cache "chrony~=${CHRONY_VERSION}" && rm -rf /etc/chrony /usr/share/doc /usr/share/man && getent passwd chrony >/dev/null
# -x: serve time only, the host's clock is never touched (so no CAP_SYS_TIME); -d: stay in the
# foreground and log to stdout; -u chrony: root only reads the config and binds udp 123, then
# chronyd drops to the chrony user. no driftfile or log file is configured, and chronyd makes its
# own runtime directory, so nothing else has to be writable
ENTRYPOINT ["chronyd", "-d", "-x", "-u", "chrony", "-f", "/etc/chrony/chrony.conf"]
