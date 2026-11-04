# Isolated acceptance image: no compiler, Cargo, SDK headers, or custom Samba build.
FROM debian:13-slim
ARG SAMBA_VERSION
RUN printf '#!/bin/sh\nexit 101\n' > /usr/sbin/policy-rc.d \
 && chmod 755 /usr/sbin/policy-rc.d \
 && apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
      python3 "samba=${SAMBA_VERSION}" "smbclient=${SAMBA_VERSION}" \
 && rm -rf /var/lib/apt/lists/*
# Synthetic identities exist only in this disposable CI image, never on the host.
RUN useradd --no-create-home --uid 21001 --user-group --shell /usr/sbin/nologin zxs-engine-test \
 && useradd --no-create-home --uid 21002 --user-group --shell /usr/sbin/nologin zxs-smb-test \
 && useradd --no-create-home --uid 21003 --user-group --shell /usr/sbin/nologin zxs-smb-next-test
WORKDIR /work
