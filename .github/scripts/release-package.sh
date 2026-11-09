#!/usr/bin/env bash
# Package the same-source Web/native products into the four-file release contract.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_VERSION:?}"
[[ "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?$ ]] || {
  echo 'RELEASE_VERSION must be a safe version filename' >&2
  exit 1
}
# Refuse to mix this generation with old or partial assets.
[[ ! -e release-assets && ! -L release-assets ]] || {
  echo 'release-assets must be absent; retain existing output and choose a fresh checkout' >&2
  exit 1
}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 .github/scripts/verify-web-dist.py --root "$PWD" --stage "$work/snapshot"
python3 - "$work/snapshot/native" "$work/assets" "$RELEASE_VERSION" <<'PYNATIVE'
import hashlib
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys

validator = runpy.run_path('.github/scripts/verify-release-assets.py')
directory = os.open('target/native/x86_64-unknown-linux-gnu/release',
                    os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
try:
    receipt = validator['read_metadata'](directory, 'BUILD.txt')
finally:
    os.close(directory)
commit = subprocess.check_output(['git', '--no-replace-objects', 'rev-parse', 'HEAD'], text=True).strip()
fields = validator['build_identity'](receipt, sys.argv[3], commit)
snapshot, assets = Path(sys.argv[1]), Path(sys.argv[2])
assets.mkdir()
for name in ('zxs', 'zxs-helper'):
    with (snapshot / name).open('rb') as source:
        validator['elf_identity'](source.read(64))
        source.seek(0)
        if hashlib.file_digest(source, 'sha256').hexdigest() != fields[f'native-{name}-sha256']:
            raise ValueError(f'native build output checksum mismatch: {name}')
if hashlib.sha256((snapshot.parent / 'web/build-manifest.json').read_bytes()).hexdigest() != fields['native-web-manifest-sha256']:
    raise ValueError('embedded Web manifest differs from verified current Web')
vfs_dir = os.open('target/smb-debian13/product', os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
try:
    vfs_bytes = validator['read_metadata'](vfs_dir, 'VFS-BUILD.json')
    vfs = validator['embedded_vfs_identity'](vfs_bytes, fields)
    with validator['open_regular'](vfs_dir, 'zxs.so') as source:
        if os.fstat(source.fileno()).st_size > 64 * 1024 * 1024:
            raise ValueError('VFS output exceeds embedded module budget')
        validator['elf_identity'](source.read(64), module=True)
        source.seek(0)
        if hashlib.file_digest(source, 'sha256').hexdigest() != vfs['module_sha256']:
            raise ValueError('VFS output checksum mismatch')
finally:
    os.close(vfs_dir)
bundle = runpy.run_path('.github/scripts/release-source-bundle.py')
bundle['prepare'](Path.cwd(), assets / f'zxs-source-{sys.argv[3]}.tar.gz',
                  sys.argv[3], receipt, vfs_bytes)
for name in ('zxs', 'zxs-helper'):
    (snapshot / name).rename(assets / f'{name}-{sys.argv[3]}-debian13-amd64')
# Publish only after all three files have been prepared. Existing destinations
# are never overwritten; an interrupted copy remains explicit partial output.
destination = Path('release-assets')
destination.mkdir()
for source in assets.iterdir():
    with source.open('rb') as incoming, (destination / source.name).open('xb') as outgoing:
        shutil.copyfileobj(incoming, outgoing, length=1024 * 1024)
    if not source.name.endswith('.tar.gz'):
        (destination / source.name).chmod(0o755)
PYNATIVE

