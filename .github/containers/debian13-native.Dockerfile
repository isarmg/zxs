# Native ZXS build only; system Samba belongs to the separate matching-ABI job.
FROM debian:13-slim
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
      ca-certificates git build-essential clang libclang-dev pkg-config python3
WORKDIR /work
