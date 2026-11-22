# samba for retro windows clients, built from the upstream source tarball (checked against the
# samba release key in this folder) as a classic, non-ad server: smbd, nmbd, winbindd (ids for
# local and builtin groups), tdbsam, smbpasswd, pdbedit, net, rpcclient and wbinfo, with smb1
# (nt1) still available. only upstream software is in the image;
# lanlord's entrypoint.sh, lanlord-user and smb.conf are mounted when the container runs.
#   docker build --build-arg IMAGE_VERSION=0.1.0 -t lanlord-nt4-server-components:0.1.0 .
# IMAGE_VERSION is the lanlord release (the tag); the samba version is the ARG below and a label
# the multi-arch index digest of alpine:3.22, so a rebuild cannot pick up a moved tag
ARG ALPINE_IMAGE=alpine@sha256:49340b346798875b5592e5d420241f2f9cfba5f47e62ebb1446a1b34734e898f
FROM ${ALPINE_IMAGE} AS build
ARG SAMBA_VERSION=4.22.11
RUN apk add --no-cache \
      acl-dev \
      attr-dev \
      bison \
      bsd-compat-headers \
      build-base \
      cmocka-dev \
      curl \
      e2fsprogs-dev \
      flex \
      gettext-dev \
      gnupg \
      gnutls-dev \
      jansson-dev \
      libarchive-dev \
      libcap-dev \
      libtirpc-dev \
      libxslt \
      linux-headers \
      lmdb-dev \
      ncurses-dev \
      perl \
      perl-parse-yapp \
      pkgconf \
      popt-dev \
      python3 \
      readline-dev \
      rpcsvc-proto-dev \
      tar \
      zlib-dev
COPY samba-pubkey.asc /tmp/samba-pubkey.asc
COPY patches /patches
WORKDIR /src
RUN set -eux; \
    base=https://download.samba.org/pub/samba/stable/samba-${SAMBA_VERSION}; \
    curl -fsSLO --proto "=https" --tlsv1.2 "$base.tar.gz"; \
    curl -fsSLO --proto "=https" --tlsv1.2 "$base.tar.asc"; \
    gpg --batch --import /tmp/samba-pubkey.asc; \
    gunzip -k "samba-${SAMBA_VERSION}.tar.gz"; \
    gpg --batch --status-fd 1 --verify \
      "samba-${SAMBA_VERSION}.tar.asc" "samba-${SAMBA_VERSION}.tar" \
      | grep -q '^\[GNUPG:\] VALIDSIG .* 81F5E2832BD2545A1897B713AA99442FB680B620$'; \
    tar xf "samba-${SAMBA_VERSION}.tar"
WORKDIR /src/samba-${SAMBA_VERSION}
# lanlord's patches (see their headers); the build fails when one does not apply to this samba version
RUN patch -p1 < /patches/samba-4.22-bare-ntlm-session-key.patch
RUN patch -p1 < /patches/samba-4.22-mask-account-script-names.patch \
 && test "$(grep -c 'talloc_string_sub2' source3/passdb/pdb_interface.c)" -ge 2 \
 && ! grep -n 'talloc_all_string_sub(.*\(add_script\|del_script\)' source3/passdb/pdb_interface.c
RUN LDFLAGS=-lintl ./configure --prefix=/usr --sysconfdir=/etc/samba --localstatedir=/var \
      --with-configdir=/etc/samba --with-privatedir=/var/lib/samba/private \
      --with-lockdir=/var/lib/samba --with-statedir=/var/lib/samba --with-cachedir=/var/cache/samba \
      --with-piddir=/run/samba --with-logfilebase=/var/log/samba \
      --without-ad-dc --without-ads --without-ldap --without-pam --without-systemd \
      --without-json --without-regedit --without-libunwind \
      --disable-python --disable-cups --disable-avahi --disable-rpath-install \
      --bundled-libraries='ALL,!cmocka' --enable-fhs --with-shared-modules='!vfs_snapper' \
 && make -j"$(nproc)" \
 && make DESTDIR=/out install \
 && rm -rf /out/usr/share /out/usr/include /out/usr/lib/pkgconfig /out/usr/lib/python3* /out/etc \
 && find /out -type f \( -perm -u+x -o -name '*.so*' \) \
      -exec sh -c 'strip --strip-unneeded "$1" 2>/dev/null || true' _ {} \;

# the service helper lanlord-exec copies to an nt4 machine: a plain win32 exe with no c runtime. it is
# cross compiled with clang and lld (they exist for every build host architecture, unlike a mingw gcc)
# against import libraries made from the few kernel32 and advapi32 functions it calls; the build
# fails unless the result is an i386 pe with subsystem 4.0 that imports only those two dlls. it is built
# for a 486 with no sse, mmx or p6-only instructions (cmov): nt4 does not enable sse and runs on a 486,
# so such an instruction traps as illegal; the build fails if the disassembly shows one
FROM ${ALPINE_IMAGE} AS svc
RUN apk add --no-cache clang lld llvm
COPY lanlord-svc/lanlord-svc.c /src/lanlord-svc.c
WORKDIR /src
RUN set -eux; \
    printf 'LIBRARY KERNEL32.dll\nEXPORTS\nCloseHandle@4\nCreateProcessA@40\nEnterCriticalSection@4\nExitProcess@4\nExpandEnvironmentStringsA@12\nGetCommandLineA@0\nGetExitCodeProcess@8\nGetSystemDirectoryA@8\nInitializeCriticalSection@4\nLeaveCriticalSection@4\nTerminateProcess@8\nWaitForSingleObject@8\n' > kernel32.def; \
    printf 'LIBRARY ADVAPI32.dll\nEXPORTS\nRegisterServiceCtrlHandlerA@8\nSetServiceStatus@8\nStartServiceCtrlDispatcherA@4\n' > advapi32.def; \
    llvm-dlltool -m i386 -k -d kernel32.def -l libkernel32.a; \
    llvm-dlltool -m i386 -k -d advapi32.def -l libadvapi32.a; \
    mkdir -p /out/usr/lib/lanlord; \
    clang --target=i686-w64-windows-gnu -march=i486 -mno-mmx -mno-sse -mno-sse2 \
      -Os -Wall -Wextra -ffreestanding -fno-stack-protector -nostdlib \
      -fuse-ld=lld -Wl,-e,_svc_entry -Wl,--subsystem,console:4.0 -Wl,--major-os-version,4 \
      -Wl,--minor-os-version,0 -Wl,--major-subsystem-version,4 -Wl,--minor-subsystem-version,0 -Wl,-s \
      -o /out/usr/lib/lanlord/lanlord-svc.exe lanlord-svc.c -L. -lkernel32 -ladvapi32; \
    exe=/out/usr/lib/lanlord/lanlord-svc.exe; \
    dump=$(llvm-objdump -p "$exe"); \
    llvm-readobj --file-headers "$exe" | grep -E 'Format|Machine|Subsystem|OperatingSystemVersion|SubsystemVersion'; \
    llvm-readobj --file-headers "$exe" | grep -q 'Format: COFF-i386'; \
    llvm-readobj --file-headers "$exe" | grep -q 'Subsystem: IMAGE_SUBSYSTEM_WINDOWS_CUI'; \
    echo "$dump" | grep -Eq '^MajorOSystemVersion[[:space:]]+4$'; \
    echo "$dump" | grep -Eq '^MinorOSystemVersion[[:space:]]+0$'; \
    echo "$dump" | grep -Eq '^MajorSubsystemVersion[[:space:]]+4$'; \
    echo "$dump" | grep -Eq '^MinorSubsystemVersion[[:space:]]+0$'; \
    dlls=$(echo "$dump" | grep 'DLL Name' | sed 's/.*DLL Name: //' | sort | tr '\n' ' '); \
    test "$dlls" = "ADVAPI32.dll KERNEL32.dll "; \
    dis=$(llvm-objdump -d "$exe"); \
    if echo "$dis" | grep -Eq '%(x|y|z)?mm[0-9]|\bcmov|\bf(u)?comip?\b'; then \
      echo "lanlord-svc.exe uses instructions nt4 or a 486 cannot run" >&2; exit 1; fi

FROM ${ALPINE_IMAGE}
ARG SAMBA_VERSION=4.22.11
ARG IMAGE_VERSION=dev
ARG IMAGE_REVISION=unknown
LABEL org.opencontainers.image.title="lanlord-nt4-server-components" \
      org.opencontainers.image.source="https://github.com/muncherelli/LANlord" \
      org.opencontainers.image.version="${IMAGE_VERSION}" \
      org.opencontainers.image.revision="${IMAGE_REVISION}" \
      org.opencontainers.image.description="samba ${SAMBA_VERSION} built from the upstream source with two lanlord patches (GPLv3: the patches are in the repository)" \
      org.opencontainers.image.licenses="GPL-3.0-or-later" \
      io.lanlord.samba.version="${SAMBA_VERSION}" \
      io.lanlord.samba.patched="true" \
      io.lanlord.samba.patches="https://github.com/muncherelli/LANlord/tree/trunk/ansible/playbooks/nt4-domain/image/patches"
# only what smbd and nmbd link against, plus bash and shadow for the mounted scripts
RUN apk add --no-cache acl-libs bash gettext-libs gnutls libarchive libcap libtirpc lmdb readline shadow
COPY --from=build /out/usr/ /usr/
COPY --from=svc /out/usr/lib/lanlord/lanlord-svc.exe /usr/lib/lanlord/lanlord-svc.exe
# samba's own libraries live in /usr/lib/samba; musl finds them through this path file
RUN echo /usr/lib/samba:/lib:/usr/local/lib:/usr/lib > /etc/ld-musl-$(uname -m).path \
 && cd /usr/bin && rm -f cifsdd dbwrap_tool gentest ldb* locktest masktest mdsearch mvxattr ndrdump oLschema2ldif \
      profiles regdiff regpatch regshell regtree samba-log-parser sharesec smbcquotas \
      smbget smbspool smbtar smbtree wspsearch ntlm_auth \
 && { [ -e /usr/sbin/nologin ] || ln -s /sbin/nologin /usr/sbin/nologin; } \
 && mkdir -p /run/samba /var/log/samba /var/lib/samba /var/cache/samba /etc/samba
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
