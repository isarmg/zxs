#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_VERSION:?}"
version="$(python3 -c 'import json;print(json.load(open(".github/samba-debian-target.json"))["package_version"])')"
docker build --build-arg "SAMBA_VERSION=$version" \
  --file .github/containers/debian13-runtime.Dockerfile --tag zxs-debian13-runtime .github/containers
docker run --rm --init --user "$(id -u):$(id -g)" --network none --volume "$PWD:/work" zxs-debian13-runtime \
  python3 .github/scripts/check-debian13-single-binary.py \
  --binary "release-assets/zxs-${RELEASE_VERSION}-debian13-amd64" --web build/web \
  --output artifacts/debian13-single-binary.json
