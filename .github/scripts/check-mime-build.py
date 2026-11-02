#!/usr/bin/env python3
"""Verify MIME build-output completion and the actual feature-gated host build."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[2]
TARGET = 'x86_64-unknown-linux-gnu'
COMPONENT = 'src/foundation/networking/mime_guess'
FIXTURE = 'test/ci/fixtures/mime-build-output-contracts.rs'
TESTS = {
    'tests::accepting_writer_retains_generated_reverse_definitions',
    'tests::buffered_final_write_failure_is_returned_with_its_original_cause',
    'tests::flush_failure_after_accepted_writes_is_returned_with_its_original_cause',
    'tests::partial_write_error_is_primary_and_prevents_a_later_flush',
    'tests::successful_buffered_generation_flushes_before_returning',
}
MODES = (
    ('default', True, [], ['default', 'rev-mappings']),
    ('explicit', False, ['rev-mappings'], ['rev-mappings']),
    ('forward-only', False, [], []),
)
IDENTITY = (COMPONENT + '/build.rs', COMPONENT + '/src/mime_types.rs',
            COMPONENT + '/Cargo.toml', FIXTURE, '.github/scripts/check-mime-build.py')


def source_hashes(root):
    return {path: hashlib.sha256((root / path).read_bytes()).hexdigest() for path in IDENTITY}


def private_subject(root, member):
    """Keep the real build source as an exact prefix and its data at the same depth."""
    (member / 'src').mkdir(parents=True)
    source = (root / COMPONENT / 'build.rs').read_bytes()
    (member / 'subject.rs').write_bytes(source + b'\n' + (root / FIXTURE).read_bytes())
    shutil.copy(root / COMPONENT / 'src/mime_types.rs', member / 'src/mime_types.rs')
    assert (member / 'subject.rs').read_bytes()[:len(source)] == source
    assert (member / 'src/mime_types.rs').read_bytes() == (root / COMPONENT / 'src/mime_types.rs').read_bytes()
    (member / 'Cargo.toml').write_text(
        '[package]\nname="mime-private-build-contracts"\nversion="0.0.0"\nedition="2015"\n'
        'build=false\nautobins=false\nautotests=false\npublish=false\n[workspace]\nresolver="2"\n'
        '[features]\ndefault=[]\nrev-mappings=[]\n[lib]\npath="subject.rs"\n'
        '[dependencies.unicase]\npath=' + json.dumps(str(root / 'src/foundation/text-unicode/unicase'))
        + '\n[profile.dev]\ndebug=0\nincremental=false\n[profile.test]\ndebug=0\nincremental=false\n'
    )


def check(root):
    root = Path(root).resolve()
    common = runpy.run_path(str(root / '.github/scripts/check-foundation-behaviour.py'))
    commit, dirty, rustc = common['source_state']()
    before = source_hashes(root)
    evidence_root = root / 'target/foundation-behaviour/logs/mime-build'
    evidence_root.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix='run-', dir=evidence_root))
    log = common['Log'](evidence / 'contracts.log',
                        f'commit: {commit}\nmodified: {dirty}\nrustc: {rustc}\n'
                        + json.dumps(before, sort_keys=True) + '\n')
    scratch = root / 'target/scope-consumers'
    scratch.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', root / 'target/mime-build-contracts'))
    builds = {}
    try:
        with tempfile.TemporaryDirectory(prefix='mime-build-', dir=scratch) as folder:
            work = Path(folder)
            private_subject(root, work / 'private')
            manifest = work / 'private/Cargo.toml'
            log.run(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)], root)
            command = ['cargo', 'test', '--offline', '--locked', '--manifest-path', str(manifest),
                       '--target', TARGET, '--target-dir', str(target), '--jobs', '2',
                       '--lib', '--features', 'rev-mappings']
            names = common['listed'](log.run([*command, '--', '--list'], root))
            assert names == sorted(TESTS), f'MIME generator test inventory differs: {names}'
            common['check_run'](log.run(command, root), names, 'mime-private-build')
            for mode, defaults, features, expected in MODES:
                member = work / mode
                member.mkdir()
                api = 'pub use mime_guess::from_path;\n'
                if expected:
                    api += 'pub use mime_guess::get_mime_extensions_str;\n'
                (member / 'lib.rs').write_text(api)
                manifest = member / 'Cargo.toml'
                manifest.write_text(
                    '[package]\nname="mime-build-' + mode + '"\nversion="0.0.0"\n'
                    'edition="2024"\npublish=false\n[workspace]\nresolver="2"\n'
                    '[lib]\npath="lib.rs"\n[dependencies.mime_guess]\npath='
                    + json.dumps(str(root / COMPONENT)) + '\ndefault-features='
                    + str(defaults).lower() + '\nfeatures=' + json.dumps(features) + '\n'
                    '[profile.dev]\ndebug=0\nincremental=false\n'
                )
                log.run(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)], root)
                metadata = json.loads(log.run(['cargo', 'metadata', '--offline', '--locked',
                    '--manifest-path', str(manifest), '--format-version', '1', '--filter-platform', TARGET], root))
                subject = next(p for p in metadata['packages'] if Path(p['manifest_path']).resolve()
                               == (root / COMPONENT / 'Cargo.toml').resolve())
                resolved = sorted(next(n['features'] for n in metadata['resolve']['nodes']
                                       if n['id'] == subject['id']))
                assert resolved == expected, f'{mode}: MIME features differ: {resolved}'
                log.run(['cargo', 'check', '--offline', '--locked', '--manifest-path', str(manifest),
                         '--target', TARGET, '--target-dir', str(target), '--jobs', '2', '--lib'], root)
                builds[mode] = resolved
        assert source_hashes(root) == before, 'MIME source or fixture changed during validation'
        (evidence / 'summary.json').write_text(json.dumps({
            'schema': 1, 'commit': commit, 'dirty': dirty, 'rustc': rustc,
            'source_sha256': before, 'target': TARGET, 'tests': names,
            'feature_builds': builds, 'scope': '5 private generator tests;3 host-build/API compile controls',
        }, indent=2) + '\n')
    finally:
        log.close()
    print(f'MIME build contracts passed: 5 tests; 3 exact feature builds; evidence: {evidence}', flush=True)


if __name__ == '__main__':
    check(ROOT)
