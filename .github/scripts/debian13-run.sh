#!/usr/bin/env bash
# Build/verify in Debian 13; host Rust toolchain is read-only and source-only Cargo stays offline.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
network="${ZXS_BUILD_NETWORK:-online}"
case "$network" in
  online|offline) ;;
  *) printf 'ZXS_BUILD_NETWORK must be online or offline\n' >&2; exit 2 ;;
esac
user_args=()
browser_args=()
network_args=()
image=zxs-debian13-build
file=.github/containers/debian13-build.Dockerfile
if [[ "${1:-}" == --native ]]; then
  user_args=(--user "$(id -u):$(id -g)")
  image=zxs-debian13-native
  file=.github/containers/debian13-native.Dockerfile
  shift
fi
if [[ "${1:-}" == --browser ]]; then
  user_args=(--user "$(id -u):$(id -g)")
  network_args=(--network none)
  browser_args=(--env CHROME_BIN=/usr/bin/chromium --env CHROMEDRIVER=/usr/bin/chromedriver --env HOME=/tmp --env "ZXS_SMOKE_SCREENSHOT=${ZXS_SMOKE_SCREENSHOT:-}" --env "ZXS_VIEWPORT_PREVIEWS=${ZXS_VIEWPORT_PREVIEWS:-}")
  image=zxs-debian13-browser
  file=.github/containers/debian13-browser.Dockerfile
  shift
fi
if [[ "$network" == offline ]]; then
  # Do not invoke a rustup proxy or build/pull an image to fill missing inputs.
  toolchain="${ZXS_BUILD_TOOLCHAIN_ROOT:-}"
  if [[ "$toolchain" != /* || "$toolchain" == *:* || "$toolchain" == *$'\n'* ]] \
      || [[ ! -x "$toolchain/bin/rustc" || ! -x "$toolchain/bin/cargo" ]]; then
    printf 'Offline build requires an absolute existing ZXS_BUILD_TOOLCHAIN_ROOT\n' >&2
    exit 2
  fi
  toolchain="$(cd -- "$toolchain" && pwd -P)"
  if [[ "$toolchain" == *:* || "$toolchain" == *$'\n'* ]]; then
    printf 'Offline toolchain path cannot contain volume separators or newlines\n' >&2
    exit 2
  fi
  if ! image_id="$(docker image inspect --format '{{.Id}}' "$image")"; then
    printf 'Offline build requires an already prepared local image: %s\n' "$image" >&2
    exit 1
  fi
  if [[ ! "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    printf 'Offline image inspection did not return one immutable image ID\n' >&2
    exit 1
  fi
  # Bind this invocation to the inspected image even if its tag changes later.
  image="$image_id"
  network_args=(--pull=never --network none)
else
  version="$(python3 -c 'import json;print(json.load(open(".github/samba-debian-target.json"))["package_version"])')"
  docker build --build-arg "SAMBA_VERSION=$version" \
    --file "$file" --tag "$image" .github/containers
  toolchain="$(rustc --print sysroot)"
fi
docker run --rm --init "${user_args[@]}" "${browser_args[@]}" "${network_args[@]}" \
  --volume "$PWD:/work" --volume "$toolchain:/opt/rust-toolchain:ro" \
  --env PATH=/opt/rust-toolchain/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  --env CARGO_HOME=/tmp/zxs-cargo-home --env CARGO_TARGET_DIR=/work/target/debian13-native \
  --env CARGO_INCREMENTAL=0 --env RUST_BACKTRACE=1 \
  --env "ZXS_BUILD_NETWORK=$network" \
  --env GIT_CONFIG_COUNT=1 --env GIT_CONFIG_KEY_0=safe.directory --env GIT_CONFIG_VALUE_0=/work \
  --env "RELEASE_VERSION=${RELEASE_VERSION:-}" \
  "$image" "$@"
