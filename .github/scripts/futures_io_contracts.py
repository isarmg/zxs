"""Dedicated minimal-feature futures IO contracts, called by consolidation."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import hashlib
import json
from pathlib import Path
import runpy
import subprocess
import tempfile


SUITES = {
    'seek-relative': ('foundation-futures-seek-relative.rs', ['io'], 14),
    'string-retention': ('foundation-futures-string-retention.rs', ['io'], 16),
    'stable-bytes': ('foundation-futures-stable-bytes.rs', ['io', 'sink'], 8),
    'write-zero': ('foundation-futures-write-zero.rs', ['io', 'sink'], 9),
}
REJECTIONS = (
    ('asref-only', 'foundation-futures-asref-only-rejection.rs', 'E0277'),
    ('borrowed-block', 'foundation-futures-borrowed-block-rejection.rs', 'E0597'),
)
TARGET = 'x86_64-unknown-linux-gnu'
IDENTITY = (
    '.github/scripts/futures_io_contracts.py',
    '.github/scripts/check-futures-consolidation.py',
    'test/ci/fixtures/foundation-futures-io-tests.json',
    'src/foundation/concurrency/futures/Cargo.toml',
    'src/foundation/concurrency/futures/src/io/mod.rs',
    'src/foundation/concurrency/futures/src/io/buf_reader.rs',
    'src/foundation/concurrency/futures/src/io/read_to_string.rs',
    'src/foundation/concurrency/futures/src/io/read_line.rs',
    'src/foundation/concurrency/futures/src/io/into_sink.rs',
    *(f'test/ci/fixtures/{recipe[0]}' for recipe in SUITES.values()),
    *(f'test/ci/fixtures/{filename}' for _, filename, _ in REJECTIONS),
)


def source_hashes(root):
    return {path: hashlib.sha256((root / path).read_bytes()).hexdigest() for path in IDENTITY}


def check_rejections(root, manifest, target_dir, evidence):
    with manifest.open('a') as handle:
        for name, filename, _ in REJECTIONS:
            handle.write('\n[[bin]]\nname=' + json.dumps(name) + '\npath='
                         + json.dumps(str(root / 'test/ci/fixtures' / filename)) + '\n')
    verified = []
    for name, filename, code in REJECTIONS:
        fixture = root / 'test/ci/fixtures' / filename
        command = ['cargo', 'check', '--locked', '--offline', '--manifest-path', str(manifest),
                   '--target', TARGET, '--target-dir', str(target_dir), '--jobs', '1',
                   '--bin', name, '--message-format=json']
        result = subprocess.run(command, cwd=root, text=True, capture_output=True, timeout=180)
        (evidence / (name + '.log')).write_text(
            '$ ' + ' '.join(command) + '\n' + result.stdout + '\n' + result.stderr)
        errors = [item['message'] for line in result.stdout.splitlines()
                  if (item := json.loads(line)).get('reason') == 'compiler-message'
                  and item['message']['level'] == 'error']
        assert result.returncode == 101 and errors, f'{name}: expected compiler rejection'
        assert all((error.get('code') or {}).get('code') == code and any(
            Path(span['file_name']) == fixture and span['is_primary']
            for span in error['spans']) for error in errors), f'{name}: unrelated diagnostic: {errors}'
        verified.append({'fixture': str(fixture.relative_to(root)), 'code': code})
    return verified


def check(root, temporary, target_dir):
    root, temporary, target_dir = Path(root), Path(temporary), Path(target_dir)
    common = runpy.run_path(str(root / '.github/scripts/check-foundation-behaviour.py'))
    expected = json.loads((root / 'test/ci/fixtures/foundation-futures-io-tests.json').read_text())
    assert expected['schema'] == 1 and expected['target'] == TARGET
    assert set(expected['suites']) == set(SUITES)
    commit, dirty, toolchain = common['source_state']()
    before = source_hashes(root)
    header = (f'commit: {commit}\nworktree: {"modified" if dirty else "clean"}\n'
              f'rustc: {toolchain}\nsource_sha256: {json.dumps(before, sort_keys=True)}\n')
    evidence_root = root / 'target/foundation-behaviour/logs/futures-io'
    evidence_root.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix='run-', dir=evidence_root))
    results = {}
    for key, (filename, features, count) in SUITES.items():
        record = expected['suites'][key]
        fixture = 'test/ci/fixtures/' + filename
        assert record['fixture'] == fixture and record['features'] == features
        names = record['tests']
        assert names == sorted(set(names)) and len(names) == count
        member = temporary / key
        member.mkdir(parents=True)
        manifest = member / 'Cargo.toml'
        manifest.write_text(
            '[package]\nname="futures-io-' + key + '"\nversion="0.0.0"\n'
            'edition="2024"\nautotests=false\nautoexamples=false\nautobins=false\n'
            '[workspace]\nresolver="2"\n[dependencies.futures]\n'
            'path=' + json.dumps(str(root / 'src/foundation/concurrency/futures')) + '\n'
            'default-features=false\nfeatures=' + json.dumps(features) + '\n'
            '[[test]]\nname="contracts"\npath=' + json.dumps(str(root / fixture)) + '\n'
            '[profile.dev]\ndebug=0\nincremental=false\n'
            '[profile.test]\ndebug=0\nincremental=false\n'
        )
        log = common['Log'](evidence / (key + '.log'), header)
        try:
            log.run(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)], root)
            metadata = json.loads(log.run(
                ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
                 '--filter-platform', TARGET, '--manifest-path', str(manifest)], root))
            subject = next(package for package in metadata['packages']
                           if Path(package['manifest_path']).resolve()
                           == (root / 'src/foundation/concurrency/futures/Cargo.toml').resolve())
            resolved = sorted(next(node['features'] for node in metadata['resolve']['nodes']
                                   if node['id'] == subject['id']))
            assert resolved == record['resolved_features'], f'{key}: resolved features differ: {resolved}'
            command = ['cargo', '--color', 'never', 'test', '--locked', '--offline',
                       '--manifest-path', str(manifest), '--target', TARGET,
                       '--target-dir', str(target_dir), '--jobs', '2', '--test', 'contracts']
            actual = common['listed'](log.run([*command, '--', '--list'], root))
            assert actual == names, f'{key}: listed tests differ from exact inventory: {actual}'
            common['check_run'](log.run(command, root), actual, 'futures-io:' + key)
            results[key] = {'fixture': fixture, 'features': features,
                            'resolved_features': resolved, 'tests': actual}
            assert results[key] == record, f'{key}: executed inventory record differs'
        finally:
            log.close()
        print(f'PASS futures-io:{key}: {len(actual)} tests; inventory equal', flush=True)
    rejections = check_rejections(root, temporary / 'stable-bytes/Cargo.toml', target_dir, evidence)
    assert source_hashes(root) == before, 'futures IO source/fixture changed during validation'
    summary = {'schema': 1, 'commit': commit, 'dirty': dirty, 'rustc': toolchain,
               'source_sha256': before, 'target': TARGET, 'suites': results, 'rejections': rejections}
    (evidence / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(f'Futures IO contracts passed: 4 suites, 47 tests, 2 compile rejections; evidence: {evidence}', flush=True)
