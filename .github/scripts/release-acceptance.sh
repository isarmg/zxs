#!/usr/bin/env bash
# Exercise exactly the archives that will be uploaded, with the release binary.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_VERSION:?}"
[[ "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?$ ]] || exit 1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/server" "$work/web"
tar -xzf "release-assets/zxs-${RELEASE_VERSION}-x86_64-unknown-linux-gnu.tar.gz" -C "$work/server"
tar -xzf "release-assets/web-interface-${RELEASE_VERSION}.tar.gz" -C "$work/web"
test -f "$work/server/deploy/systemd/zxs.service"
cmp "$work/server/deploy/systemd/zxs.service" "$work/web/deploy/systemd/zxs.service"
test -f "$work/server/deploy/samba/smb.conf.example"
cmp "$work/server/deploy/samba/smb.conf.example" "$work/web/deploy/samba/smb.conf.example"
test -f "$work/web/public/build-manifest.json"
test -f "$work/web/docs/release.md"
test -f "$work/web/compliance/NOTICE.md"
"$work/server/zxs-rescue" --help
# This starts the real extracted service, checks health/assets and exercises the
# browser flows. A --help-only stub is not a release acceptance test.
screenshot_args=()
if [[ -n "${ZXS_SMOKE_SCREENSHOT:-}" ]]; then
  screenshot_args=(--screenshot "$ZXS_SMOKE_SCREENSHOT")
fi
cargo run --locked --offline --release -p browser-smoke -- \
  --cli-bin "$work/server/zxs" --web-dir "$work/web/public" "${screenshot_args[@]}"
