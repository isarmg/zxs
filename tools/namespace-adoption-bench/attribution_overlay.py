"""Verification-only allowlisted instrumentation; never modifies the source branch."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

DIRECTORY = Path(__file__).resolve().parent
PATHS = {
    'src/engine/src/lib.rs',
    'src/engine/src/namespace.rs',
    'src/engine/src/segment_store.rs',
    'src/engine/src/journal.rs',
    'src/engine/src/business/namespace/query.rs',
}

def sha(data):
    return hashlib.sha256(data).hexdigest()

def overlay_for(expected):
    if os.environ.get('ZXS_BENCH_MODE') != 'attribution':
        return None
    if expected['adapter_path'] != 'test/engine/bench/namespace_adoption/candidate_attribution.rs':
        return None
    value = json.loads((DIRECTORY / 'attribution-overlay.json').read_text())
    if value['schema'] != 'namespace-attribution-overlay-v1' or value['candidate'] != expected:
        raise RuntimeError('attribution source/adapter pin differs')
    if set(value['files']) != PATHS:
        raise RuntimeError('attribution changed-path allowlist differs')
    for name, entry in value['files'].items():
        content = entry['content_path']
        if Path(content).name != content or not content.startswith('attribution-') or not content.endswith('.rs'):
            raise RuntimeError('overlay content path is not a confined Rust file')
        if sha((DIRECTORY / content).read_bytes()) != entry['after_sha256']:
            raise RuntimeError('overlay content digest differs: ' + name)
    return value

def install_overlay(product, expected):
    overlay = overlay_for(expected)
    if overlay is None:
        return
    for name, entry in overlay['files'].items():
        original = (product / name).read_bytes()
        if sha(original) != entry['before_sha256']:
            raise RuntimeError('overlay original bytes differ: ' + name)
    for name, entry in overlay['files'].items():
        (product / name).write_bytes((DIRECTORY / entry['content_path']).read_bytes())

def overlay_identity(product, expected):
    overlay = overlay_for(expected)
    if overlay is None:
        return [], None
    for name, entry in overlay['files'].items():
        original = subprocess.check_output(['git', '-C', str(product), 'show', 'HEAD:' + name])
        blob = subprocess.check_output(['git', '-C', str(product), 'rev-parse', 'HEAD:' + name], text=True).strip()
        if sha(original) != entry['before_sha256'] or blob != entry['before_blob']:
            raise RuntimeError('overlay base object differs: ' + name)
        if sha((product / name).read_bytes()) != entry['after_sha256']:
            raise RuntimeError('instrumented source differs: ' + name)
    return sorted(overlay['files']), dict(
        scope='instrumented verification build; product SHA/tree identify BASE ONLY',
        descriptor_sha256=sha((DIRECTORY / 'attribution-overlay.json').read_bytes()),
        files=overlay['files'])

def effective_build_tree(product, paths):
    if os.environ.get('ZXS_BENCH_MODE') not in ('attribution', 'history-trend'):
        return None
    # Separate temporary index: never alters checkout/index/ref identity.
    with tempfile.TemporaryDirectory(prefix='namespace-attribution-index-') as directory:
        env = {**os.environ, 'GIT_INDEX_FILE': str(Path(directory) / 'index')}
        def git(*args):
            return subprocess.check_output(['git', '-C', str(product), *args], env=env, text=True).strip()
        git('read-tree', 'HEAD')
        git('add', '--', 'src/engine/Cargo.toml', 'test/engine/bench/task1_adoption.rs', *paths)
        return git('write-tree')

