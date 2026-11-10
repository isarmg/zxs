#!/usr/bin/env bash
# Exercise the exact standalone ELF in a different directory, without a Web directory.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_VERSION:?}"
[[ "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?$ ]] || exit 1
foreign_review="${ZXS_SMOKE_FOREIGN_REVIEW-0}"
case "$foreign_review" in
  0|1) ;;
  *) printf 'ZXS_SMOKE_FOREIGN_REVIEW must be 0 or 1\n' >&2; exit 1 ;;
esac
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/server"
cp "release-assets/zxs-${RELEASE_VERSION}-debian13-amd64" "$work/server/zxs"
chmod 755 "$work/server/zxs"
test "$(find "$work/server" -mindepth 1 -maxdepth 1 | wc -l)" = 1
screenshot_args=()
if [[ -n "${ZXS_SMOKE_SCREENSHOT:-}" ]]; then
  screenshot_args=(--screenshot "$ZXS_SMOKE_SCREENSHOT")
fi
# The actual extracted/copied executable must serve all browser flows from
# embedded bytes, not a --help stub or a beside-the-binary public directory.
cargo run --locked --offline --release -p browser-smoke -- \
  --cli-bin "$work/server/zxs" --embedded-web --server-container-image zxs-debian13-service-runtime "${screenshot_args[@]}"

# CI explicitly opts into a separate review-only scenario after the full suite.
# Keep the same copied ELF and do not pass incompatible screenshot arguments.
if [[ "$foreign_review" == 1 ]]; then
  cargo run --locked --offline --release -p browser-smoke -- \
    --cli-bin "$work/server/zxs" --embedded-web --server-container-image zxs-debian13-service-runtime --foreign-review-only
fi
