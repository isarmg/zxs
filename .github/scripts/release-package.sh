#!/usr/bin/env bash
# Package the verified Web snapshot and native bytes bound by release-native.py.
# This is build-step identity, not a reproducible-build attestation.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_VERSION:?}"
# release-tools performs canonical SemVer validation in CI. Independently bound
# the filename here so direct invocations cannot redirect archive output paths.
[[ "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?$ ]] || {
  echo 'RELEASE_VERSION must be a safe version filename' >&2
  exit 1
}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 .github/scripts/verify-web-dist.py --root "$PWD" --stage "$work/snapshot"
python3 - "$work/snapshot/native" "$RELEASE_VERSION" <<'PYNATIVE'
import hashlib
import os
from pathlib import Path
import runpy
import sys

validator = runpy.run_path('.github/scripts/verify-release-assets.py')
directory = os.open('target/native/x86_64-unknown-linux-gnu/release',
                    os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
try:
    receipt = validator['read_metadata'](directory, 'BUILD.txt')
finally:
    os.close(directory)
fields = validator['build_identity'](receipt, sys.argv[2])
snapshot = Path(sys.argv[1])
for name in ('zxs', 'zxs-rescue'):
    with (snapshot / name).open('rb') as source:
        actual = hashlib.file_digest(source, 'sha256').hexdigest()
    if actual != fields[f'native-{name}-sha256']:
        raise ValueError(f'native build output checksum mismatch: {name}')
if 'native-archive-sha256' in fields:
    raise ValueError('native build receipt already contains archive identity')
(snapshot / 'BUILD.txt').write_bytes(receipt)
PYNATIVE
material=(compliance README.md docs deploy)
tar --owner=0 --group=0 --numeric-owner --mode='u=rwX,go=rX' -czf "$work/native.tar.gz" \
  -C "$work/snapshot/native" zxs zxs-rescue -C "$work/snapshot/material" "${material[@]}"
# Keep the verified browser tree (including its hashed legal subset) separate
# from complete documentation/provenance, which is not a static asset tree.
mv "$work/snapshot/web" "$work/snapshot/public"
tar --owner=0 --group=0 --numeric-owner --mode='u=rwX,go=rX' -czf "$work/web.tar.gz" \
  -C "$work/snapshot" public -C "$work/snapshot/material" "${material[@]}"
printf 'native-archive-sha256=%s\n' "$(sha256sum "$work/native.tar.gz" | cut -d' ' -f1)" >> "$work/snapshot/native/BUILD.txt"
[[ ! -L release-assets ]]
mkdir -p release-assets
mv -T -- "$work/native.tar.gz" "release-assets/zxs-${RELEASE_VERSION}-x86_64-unknown-linux-gnu.tar.gz"
mv -T -- "$work/web.tar.gz" "release-assets/web-interface-${RELEASE_VERSION}.tar.gz"
mv -T -- "$work/snapshot/native/BUILD.txt" release-assets/BUILD.txt
