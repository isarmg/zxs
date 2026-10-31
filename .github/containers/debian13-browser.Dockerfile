# Same target libc as the delivered ELF; browser runs as the invoking non-root UID.
FROM debian:13-slim
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
      ca-certificates git build-essential clang libclang-dev pkg-config python3 chromium chromium-driver chromium-sandbox
WORKDIR /work
