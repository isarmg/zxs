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
import shutil
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
with (snapshot / 'zxs').open('rb') as source:
    validator['elf_identity'](source.read(64))
if 'materials-archive-sha256' in fields:
    raise ValueError('native build receipt already contains archive identity')
if hashlib.sha256((snapshot.parent / 'web/build-manifest.json').read_bytes()).hexdigest() != fields['native-web-manifest-sha256']:
    raise ValueError('embedded Web manifest differs from verified current Web')
vfs_dir = os.open('target/smb-debian13/product', os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
try:
    vfs_bytes = validator['read_metadata'](vfs_dir, 'VFS-BUILD.json')
    vfs = validator['vfs_identity'](vfs_bytes, fields)
    with validator['open_regular'](vfs_dir, 'zxs.so') as source, (snapshot / 'zxs.so').open('xb') as output:
        shutil.copyfileobj(source, output)
finally:
    os.close(vfs_dir)
if hashlib.sha256((snapshot / 'zxs.so').read_bytes()).hexdigest() != vfs['module_sha256']:
    raise ValueError('VFS output checksum mismatch')
with (snapshot / 'zxs.so').open('rb') as source:
    validator['elf_identity'](source.read(64), module=True)
(snapshot / 'VFS-BUILD.json').write_bytes(vfs_bytes)
(snapshot / 'BUILD.txt').write_bytes(receipt)
PYNATIVE
material=(compliance README.md docs deploy)
tar --owner=0 --group=0 --numeric-owner --mode='u=rwX,go=rX' -czf "$work/materials.tar.gz" \
  -C "$work/snapshot/native" zxs-rescue -C "$work/snapshot/material" "${material[@]}"
printf 'materials-archive-sha256=%s\n' "$(sha256sum "$work/materials.tar.gz" | cut -d' ' -f1)" >> "$work/snapshot/native/BUILD.txt"
[[ ! -L release-assets ]]
mkdir -p release-assets
mv -T -- "$work/snapshot/native/zxs" "release-assets/zxs-${RELEASE_VERSION}-debian13-amd64"
mv -T -- "$work/snapshot/native/zxs.so" "release-assets/zxs-vfs-${RELEASE_VERSION}-debian13-amd64.so"
mv -T -- "$work/materials.tar.gz" "release-assets/zxs-materials-${RELEASE_VERSION}.tar.gz"
mv -T -- "$work/snapshot/native/VFS-BUILD.json" release-assets/VFS-BUILD.json
mv -T -- "$work/snapshot/native/BUILD.txt" release-assets/BUILD.txt
