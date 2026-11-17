# samba for retro windows clients, built from the upstream source tarball (checked against the
# samba release key in this folder) as a classic, non-ad server: smbd, nmbd, tdbsam, smbpasswd,
# pdbedit and net, with smb1 (nt1) still available. only upstream software is in the image;
# lanlord's entrypoint.sh, lanlord-user and smb.conf are mounted when the container runs.
#   docker build --build-arg SAMBA_VERSION=4.22.11 -t lanlord-server-component:nt4 .
ARG ALPINE=3.22
FROM alpine:${ALPINE} AS build
ARG SAMBA_VERSION=4.22.11
RUN apk add --no-cache build-base python3 perl pkgconf bison flex gnupg curl tar \
      gnutls-dev popt-dev readline-dev ncurses-dev zlib-dev acl-dev attr-dev libcap-dev \
      libarchive-dev linux-headers libxslt rpcsvc-proto-dev gettext-dev perl-parse-yapp \
      e2fsprogs-dev bsd-compat-headers libtirpc-dev jansson-dev lmdb-dev
COPY samba-pubkey.asc /tmp/samba-pubkey.asc
WORKDIR /src
RUN set -eux; \
    base=https://download.samba.org/pub/samba/stable/samba-${SAMBA_VERSION}; \
    curl -fsSLO "$base.tar.gz"; curl -fsSLO "$base.tar.asc"; \
    gpg --batch --import /tmp/samba-pubkey.asc; \
    gunzip -k "samba-${SAMBA_VERSION}.tar.gz"; \
    gpg --batch --status-fd 1 --verify "samba-${SAMBA_VERSION}.tar.asc" "samba-${SAMBA_VERSION}.tar" | grep -q '^\[GNUPG:\] VALIDSIG .* 81F5E2832BD2545A1897B713AA99442FB680B620$'; \
    tar xf "samba-${SAMBA_VERSION}.tar"
WORKDIR /src/samba-${SAMBA_VERSION}
RUN LDFLAGS=-lintl ./configure --prefix=/usr --sysconfdir=/etc/samba --localstatedir=/var \
      --with-configdir=/etc/samba --with-privatedir=/var/lib/samba/private \
      --with-lockdir=/var/lib/samba --with-statedir=/var/lib/samba --with-cachedir=/var/cache/samba \
      --with-piddir=/run/samba --with-logfilebase=/var/log/samba \
      --without-ad-dc --without-ads --without-ldap --without-pam --without-systemd \
      --without-json --without-winbind --without-regedit --without-libunwind \
      --disable-python --disable-cups --disable-avahi --disable-rpath-install \
      --bundled-libraries=ALL --enable-fhs --with-shared-modules='!vfs_snapper' \
 && make -j"$(nproc)" \
 && make DESTDIR=/out install \
 && rm -rf /out/usr/share /out/usr/include /out/usr/lib/pkgconfig /out/usr/lib/python3* /out/etc \
 && find /out -type f \( -perm -u+x -o -name '*.so*' \) -exec sh -c 'strip --strip-unneeded "$1" 2>/dev/null || true' _ {} \;

FROM alpine:${ALPINE}
ARG SAMBA_VERSION=4.22.11
LABEL org.opencontainers.image.title="lanlord-server-component nt4" \
      org.opencontainers.image.source="https://github.com/muncherelli/LANlord" \
      org.opencontainers.image.version="${SAMBA_VERSION}"
# only what smbd and nmbd link against, plus bash and shadow for the mounted scripts
RUN apk add --no-cache bash shadow gnutls acl-libs libcap libarchive gettext-libs lmdb readline libtirpc
COPY --from=build /out/ /
# samba's own libraries live in /usr/lib/samba; musl finds them through this path file
RUN echo /usr/lib/samba:/lib:/usr/local/lib:/usr/lib > /etc/ld-musl-$(uname -m).path \
 && cd /usr/bin && rm -f cifsdd dbwrap_tool gentest ldb* locktest masktest mdsearch mvxattr ndrdump oLschema2ldif \
      profiles regdiff regpatch regshell regtree rpcclient samba-log-parser sharesec smbcacls smbcquotas \
      smbget smbspool smbtar smbtree wbinfo wspsearch ntlm_auth \
 && { [ -e /usr/sbin/nologin ] || ln -s /sbin/nologin /usr/sbin/nologin; } \
 && mkdir -p /run/samba /var/log/samba /var/lib/samba /var/cache/samba /etc/samba
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
