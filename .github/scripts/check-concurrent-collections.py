#!/usr/bin/env python3
"""Check retained concurrency behavior and feature isolation with real providers."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

import runpy

ROOT = Path(__file__).resolve().parents[2]
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']
TARGETS = ('x86_64-unknown-linux-gnu', 'wasm32-unknown-unknown')
FEATURES = ((), ('alloc',), ('std',))
INTEGRATIONS = tuple(('utilities', name) for name in
    ('atomic_cell', 'cache_padded', 'parker', 'thread', 'wait_group')) + (('deque', 'steal'),)
# The queue, deque and sharded-lock integrations use the local fastrand and run
# with exact name inventories in check-foundation-behaviour.py.
BEHAVIOUR_GATE = ('deque/tests/fifo.rs', 'deque/tests/injector.rs', 'deque/tests/lifo.rs',
                  'queue/tests/array_queue.rs', 'queue/tests/seg_queue.rs', 'utilities/tests/sharded_lock.rs')
PROFILE = '\n[profile.dev]\ndebug=0\nincremental=false\n'


def run(args, root=ROOT, negative=False):
    result = subprocess.run(args, cwd=root, text=True, capture_output=negative,
                            check=not negative, timeout=600)
    if negative and not result.returncode:
        raise AssertionError('Expected feature-isolation compilation failure')
    return result


def check_features(root, temporary, target):
    work = temporary / 'features'
    (work / 'src').mkdir(parents=True)
    for features in FEATURES:
        manifest = work / 'Cargo.toml'
        manifest.write_text('[package]\nname="concurrency-feature-probe"\nversion="0.0.0"\n'
            'edition="2021"\n[workspace]\n[dependencies.concurrent-collections]\npath='
            + json.dumps(str(root / 'src/foundation/concurrency/concurrent-collections'))
            + '\ndefault-features=false\nfeatures=' + json.dumps(features) + '\n' + PROFILE)
        source = '#![no_std]\npub fn padding()->usize {core::mem::align_of::<concurrent_collections::utilities::CachePadded<u8>>()}\n'
        if 'alloc' in features or 'std' in features:
            source += 'pub fn allocated()->usize {let q=concurrent_collections::queue::ArrayQueue::<u8>::new(2);let _c=concurrent_collections::epoch::Collector::new();q.capacity()}\n'
        if 'std' in features:
            source += 'pub fn deque()->bool {concurrent_collections::deque::Worker::<u8>::new_fifo().is_empty()}\n'
        (work / 'src/lib.rs').write_text(source)
        run(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)])
        for triple in TARGETS:
            command = ['cargo', 'check', '--locked', '--offline', '--manifest-path', str(manifest),
                       '--target-dir', str(target), '--target', triple, '--jobs', '2']
            run(command)
            print(f'PASS concurrency features {features} on {triple}', flush=True)
        if not features:
            (work / 'src/lib.rs').write_text('pub fn unavailable(){let _=concurrent_collections::queue::ArrayQueue::<u8>::new(1);}\n')
            result = run(command, negative=True)
            if 'E0433' not in result.stderr or 'ArrayQueue' not in result.stderr:
                raise AssertionError('Feature-negative probe failed for an unrelated reason: ' + result.stderr)


def check_removed_feature(root, temporary):
    work = temporary / 'removed-feature'
    (work / 'src').mkdir(parents=True)
    (work / 'src/lib.rs').write_text('')
    manifest = work / 'Cargo.toml'
    manifest.write_text('[package]\nname="removed-feature-probe"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies.concurrent-collections]\npath='
        + json.dumps(str(root / 'src/foundation/concurrency/concurrent-collections'))
        + '\ndefault-features=false\nfeatures=["nightly"]\n')
    result = run(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)], negative=True)
    if 'nightly' not in result.stderr or 'does not have' not in result.stderr:
        raise AssertionError('Removed-feature probe failed for an unrelated reason: ' + result.stderr)


def check_retained(root, temporary, target):
    mirror = temporary / 'retained'
    package = mirror / 'src/foundation/concurrency/concurrent-collections'
    tests = mirror / 'test/foundation/concurrent-collections'
    shutil.copytree(root / 'src/foundation/concurrency/concurrent-collections', package)
    shutil.copytree(root / 'test/foundation/concurrent-collections', tests)
    manifest = package / 'Cargo.toml'
    manifest.write_text(manifest.read_text() + '\n[workspace]\n' + PROFILE)
    run(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)])
    common = ['--locked', '--offline', '--manifest-path', str(manifest), '--target-dir', str(target), '--jobs', '2']
    run(['cargo', 'test', *common, '--lib'])
    run(['cargo', 'test', *common, '--doc'])
    # The independent harness adds only actual local providers, never fake rand,
    # Loom, Miri or scheduling coverage.
    work = temporary / 'integrations'
    (work / 'src').mkdir(parents=True)
    (work / 'src/lib.rs').write_text('')
    text = '[package]\nname="concurrency-integration-probe"\nversion="0.0.0"\nedition="2021"\n[workspace]\n' + PROFILE
    for name in ('concurrent-collections', 'rayon', 'rustversion'):
        text += '\n[dependencies.' + name + ']\npath=' + json.dumps(str(foundation_path(root, name))) + '\n'
    for module, name in INTEGRATIONS:
        text += '\n[[test]]\nname="retained_' + name + '"\npath=' + json.dumps(str(root / 'test/foundation/concurrent-collections' / module / 'tests' / (name + '.rs'))) + '\n'
    text += '\n[[test]]\nname="consumer_semantics"\npath=' + json.dumps(str(root / 'test/tools/concurrent-collections/semantics.rs')) + '\n'
    manifest = work / 'Cargo.toml'
    manifest.write_text(text)
    run(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)])
    run(['cargo', 'test', '--locked', '--offline', '--manifest-path', str(manifest), '--target-dir', str(target), '--jobs', '2'])
    for path in BEHAVIOUR_GATE:
        if not (root / 'test/foundation/concurrent-collections' / path).is_file():
            raise AssertionError('Missing retained integration: ' + path)
    print('Queue, deque and sharded-lock integrations run in check-foundation-behaviour.py; Loom/Miri not claimed.',
          flush=True)


def check_consumers():
    # Preserve the real default consumers, then explicitly compile SQLite's
    # optional queue-backed pool through its workspace platform consumer. The
    # foundation package is excluded, so root feature selection must use that
    # declared workspace capability rather than a feature on a non-member.
    run(['cargo', 'check', '--locked', '--offline', '-p', 'rayon', '-p', 'sqlite-access'])
    run(['cargo', 'check', '--locked', '--offline', '-p', 'platform-io',
         '--features', 'platform-io/sqlite-pool'])


def main():
    parent = ROOT / 'target/scope-consumers'
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='concurrent-collections-', dir=parent) as directory:
        temporary = Path(directory)
        target = temporary / 'target'
        check_removed_feature(ROOT, temporary)
        check_features(ROOT, temporary, target)
        check_retained(ROOT, temporary, target)
    check_consumers()


if __name__ == '__main__':
    main()
