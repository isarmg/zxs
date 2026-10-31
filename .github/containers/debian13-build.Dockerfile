# Build/test environment only. Never installed on a user's Debian host.
FROM debian:13-slim
ARG SAMBA_VERSION
RUN printf '#!/bin/sh\nexit 101\n' > /usr/sbin/policy-rc.d \
 && chmod 755 /usr/sbin/policy-rc.d \
 && sed -i 's/^Types: deb$/Types: deb deb-src/' /etc/apt/sources.list.d/debian.sources \
 && apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
      ca-certificates git build-essential clang libclang-dev pkg-config python3 \
      "samba=${SAMBA_VERSION}" "samba-common-bin=${SAMBA_VERSION}" \
      "samba-dev=${SAMBA_VERSION}" "smbclient=${SAMBA_VERSION}" \
 && DEBIAN_FRONTEND=noninteractive apt-get build-dep --arch-only -y "samba=${SAMBA_VERSION}"
WORKDIR /work
