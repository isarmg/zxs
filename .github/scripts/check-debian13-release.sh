#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_VERSION:?}"
version="$(python3 -c 'import json;print(json.load(open(".github/samba-debian-target.json"))["package_version"])')"
docker build --build-arg "SAMBA_VERSION=$version" \
  --file .github/containers/debian13-runtime.Dockerfile --tag zxs-debian13-runtime .github/containers
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "release-assets/zxs-helper-${RELEASE_VERSION}-debian13-amd64" "$work/zxs-helper"
docker build --network none --file .github/containers/debian13-service-runtime.Dockerfile \
  --tag zxs-debian13-service-runtime "$work"
# Create the shared output directory as the host runner before container root
# writes receipts, so later host Chrome can create its diagnostic subdirectory.
mkdir -p artifacts
# Root is confined to this disposable container. The checker drops the actual
# daemon to the private service identity after helper preparation and teardown.
docker run --rm --init --network none --env ZXS_ISOLATED_RELEASE_TEST=1 \
  --volume "$PWD:/work:ro" --volume "$PWD/artifacts:/work/artifacts" zxs-debian13-service-runtime \
  python3 .github/scripts/check-debian13-single-binary.py \
  --binary "release-assets/zxs-${RELEASE_VERSION}-debian13-amd64" \
  --helper "release-assets/zxs-helper-${RELEASE_VERSION}-debian13-amd64" --web build/web \
  --output artifacts/debian13-single-binary.json
