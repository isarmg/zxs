#!/usr/bin/env python3
"""Snapshot a verified production Web distribution into Cargo OUT_DIR, never runtime disk."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import subprocess

MAX_MANIFEST_BYTES = 4 * 1024 * 1024
MAX_FILES = 4096
MAX_FILE_BYTES = 64 * 1024 * 1024
MAX_TOTAL_BYTES = 256 * 1024 * 1024


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()


def identity(root):
    commit, tree = git(root, 'rev-parse', 'HEAD'), git(root, 'rev-parse', 'HEAD^{tree}')
    if (not re.fullmatch(r'[0-9a-f]{40}', commit)
            or not re.fullmatch(r'[0-9a-f]{40}', tree)
            or commit != os.environ.get('ZXS_EMBED_WEB_COMMIT')
            or tree != os.environ.get('ZXS_EMBED_WEB_TREE')):
        raise ValueError('embedded Web expected source commit/tree mismatch')
    if git(root, 'status', '--porcelain', '--untracked-files=normal'):
        raise ValueError('embedded Web release requires a clean source tree')
    return commit, tree


def generate(root, out):
    root = Path(root).resolve(strict=True)
    out = Path(out).resolve(strict=True)
    before = identity(root)
    verifier = runpy.run_path(str(root / '.github/scripts/verify-web-dist.py'))
    # Bound artifact reads before the existing full checksum verifier opens them.
    dist = verifier['input_path'](root, 'build/web', directory=True)
    manifest_path = verifier['input_path'](dist, 'build-manifest.json')
    if manifest_path.stat().st_size > MAX_MANIFEST_BYTES:
        raise ValueError('embedded Web manifest byte limit exceeded')
    preliminary = verifier['document'](manifest_path.read_bytes())
    if not isinstance(preliminary, dict):
        raise ValueError('embedded Web manifest must be an object')
    declared = verifier['hash_inventory'](preliminary.get('files'), 'output')
    if len(declared) + 1 > MAX_FILES:
        raise ValueError('embedded Web file count exceeds limit')
    total = manifest_path.stat().st_size
    for relative in declared:
        info = verifier['input_path'](dist, relative).stat()
        if info.st_nlink != 1 or info.st_size > MAX_FILE_BYTES:
            raise ValueError(f'embedded Web file link/size limit: {relative}')
        total += info.st_size
        if total > MAX_TOTAL_BYTES:
            raise ValueError('embedded Web total byte limit exceeded')
    manifest, manifest_bytes = verifier['verify_web'](root)
    outputs = dict(manifest['files'])
    outputs['build-manifest.json'] = hashlib.sha256(manifest_bytes).hexdigest()
    if len(outputs) > MAX_FILES:
        raise ValueError('embedded Web file count exceeds limit')
    entries = []
    total = 0
    asset_dir = out / 'web-assets'
    asset_dir.mkdir(exist_ok=True)
    for index, (relative, expected) in enumerate(sorted(outputs.items())):
        path = verifier['input_path'](root / 'build/web', relative)
        info = path.stat()
        if info.st_nlink != 1 or info.st_size > MAX_FILE_BYTES:
            raise ValueError(f'embedded Web file link/size limit: {relative}')
        total += info.st_size
        if total > MAX_TOTAL_BYTES:
            raise ValueError('embedded Web total byte limit exceeded')
        data = path.read_bytes()
        if len(data) != info.st_size or hashlib.sha256(data).hexdigest() != expected:
            raise ValueError(f'embedded Web changed during snapshot: {relative}')
        (asset_dir / str(index)).write_bytes(data)
        entries.append(f'Asset {{ path: {json.dumps(relative, ensure_ascii=False)}, bytes: include_bytes!(concat!(env!("OUT_DIR"), "/web-assets/{index}")) }}')
        print(f'cargo:rerun-if-changed={path}')
    for relative in manifest['inputs']:
        print(f'cargo:rerun-if-changed={root / relative}')
    if identity(root) != before or verifier['source_inventory'](root) != manifest['inputs']:
        raise ValueError('embedded Web sources changed during snapshot')
    if verifier['read_input'](root / 'build/web', 'build-manifest.json') != manifest_bytes:
        raise ValueError('embedded Web manifest changed during snapshot')
    seconds = int(git(root, 'show', '-s', '--format=%ct', 'HEAD'))
    if not 0 <= seconds <= 253402300799:
        raise ValueError('embedded Web commit time outside HTTP date range')
    generated = ('static BUNDLE: Bundle = Bundle {\n'
                 f'source_commit: "{before[0]}", source_tree: "{before[1]}",\n'
                 f'manifest_sha256: "{outputs["build-manifest.json"]}", modified_seconds: {seconds},\n'
                 'files: &[\n' + ',\n'.join(entries) + '\n] };\n')
    (out / 'embedded_web.rs').write_text(generated)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    generate(args.root, args.out)


if __name__ == '__main__':
    main()
