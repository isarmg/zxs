#!/usr/bin/env bash
# Build/verify in Debian 13; host Rust toolchain is read-only and source-only Cargo stays offline.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
version="$(python3 -c 'import json;print(json.load(open(".github/samba-debian-target.json"))["package_version"])')"
docker build --build-arg "SAMBA_VERSION=$version" \
  --file .github/containers/debian13-build.Dockerfile --tag zxs-debian13-build .github/containers
toolchain="$(rustc --print sysroot)"
docker run --rm --init \
  --volume "$PWD:/work" --volume "$toolchain:/opt/rust-toolchain:ro" \
  --env PATH=/opt/rust-toolchain/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  --env CARGO_HOME=/tmp/zxs-cargo-home --env CARGO_TARGET_DIR=/work/target/debian13-native \
  --env CARGO_INCREMENTAL=0 --env RUST_BACKTRACE=1 \
  --env GIT_CONFIG_COUNT=1 --env GIT_CONFIG_KEY_0=safe.directory --env GIT_CONFIG_VALUE_0=/work \
  --env "RELEASE_VERSION=${RELEASE_VERSION:-}" \
  zxs-debian13-build "$@"
