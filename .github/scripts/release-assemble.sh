#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
: "${RELEASE_VERSION:?}"
# Assembly does not regenerate or relabel build/source/installation metadata.
# The producer already bound those bytes inside the complete source archive.
python3 - "$RELEASE_VERSION" <<'PYASSEMBLE'
import hashlib
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import sys

validator = runpy.run_path('.github/scripts/verify-release-assets.py')
version = sys.argv[1]
names = validator['asset_names'](version)
directory = os.open('release-assets', os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
try:
    if set(os.listdir(directory)) != names:
        raise ValueError('unassembled release must contain exactly ZXS, helper and complete source')
    hashes = {}
    bundle = runpy.run_path('.github/scripts/release-source-bundle.py')
    with tempfile.TemporaryDirectory(prefix='zxs-release-assemble-') as temporary:
        snapshot = Path(temporary)
        for name in sorted(names):
            limit = bundle['MAX_BUNDLE_BYTES'] if name.endswith('.tar.gz') else 512 * 1024 * 1024
            digest, size = hashlib.sha256(), 0
            with validator['open_regular'](directory, name) as source, (snapshot / name).open('xb') as output:
                while chunk := source.read(1024 * 1024):
                    size += len(chunk)
                    if size > limit:
                        raise ValueError('release asset exceeds its byte budget: ' + name)
                    digest.update(chunk)
                    output.write(chunk)
            hashes[name] = digest.hexdigest()
        checksums = ''.join(f'{digest}  {name}\n' for name, digest in hashes.items())
        (snapshot / 'SHA256SUMS').write_text(checksums)
        commit = subprocess.check_output(['git', '--no-replace-objects', 'rev-parse', 'HEAD'], text=True).strip()
        validator['verify'](snapshot, version, commit)
    fd = os.open('SHA256SUMS', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 0o644, dir_fd=directory)
    with os.fdopen(fd, 'w') as output:
        output.write(checksums)
    validator['verify_opened'](directory, version, commit, None)
finally:
    os.close(directory)
PYASSEMBLE

