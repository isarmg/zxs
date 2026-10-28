#!/usr/bin/env python3
"""Run retained and project-authored behaviour tests on local foundation sources.

Two kinds of suites run against the locally renamed implementations:

* unit suites: ``cargo test --lib -p <package>`` in the root workspace, so the
  package compiles with the features the root graph resolves for its real
  consumers;
* integration suites: one member package per suite in a temporary workspace.
  The subject package is a path dependency with exactly the features the
  production profiles in ``compliance/rust-sources.json`` resolve for it, and
  the member mirrors the subject's feature names with the same set enabled, so
  retained upstream ``cfg(feature = ...)`` gates keep their meaning.

Every test binary is listed and run; the listed names must equal the generated
inventory and every listed test must report ``ok`` with nothing ignored or
filtered. ``--write`` regenerates the inventory from a passing run and is used
only by the final integration. Nothing is skipped on a source digest: Cargo's
own fingerprints decide what to rebuild.
"""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import runpy
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
INVENTORY = ROOT / 'test/ci/fixtures/foundation-behaviour-tests.json'
CATALOG = ROOT / 'compliance/rust-sources.json'
WORKSPACES = ROOT / 'tools/rust-sources/workspaces.json'
OUTPUT = ROOT / 'target/foundation-behaviour'
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']


@dataclass(frozen=True)
class Suite:
    """Integration tests for one subject package and the local crates they import."""
    subject: str
    tests: tuple
    providers: tuple = ()
    # Subject examples a test spawns from target/<profile>/examples; built before the tests run.
    examples: tuple = ()


# Root-workspace unit suites: lib-wired tests reached through #[cfg(test)] #[path]/include!.
UNIT_SUITES = ('bytes', 'cmov', 'flag-set', 'futures-intrusive', 'guardian', 'httpdate', 'once_cell', 'scopeguard',
               'send_wrapper', 'signal-hook-registry', 'spin', 'thread_local')

FOUNDATION_TESTS = 'test/foundation/'
INTEGRATION_SUITES = (
    Suite('flume', tuple(FOUNDATION_TESTS + 'flume/tests/' + name + '.rs'
                         for name in ('basic', 'iter', 'array', 'list', 'zero'))
          + ('test/architecture/flume/behavior.rs',),
          ('concurrent-collections', 'fastrand')),
    Suite('futures-intrusive', tuple(FOUNDATION_TESTS + 'futures-intrusive/tests/' + name + '.rs' for name in (
        'mutex', 'semaphore', 'manual_reset_event', 'oneshot_channel', 'mpmc_channel',
        'state_broadcast_channel', 'timer')), ('futures', 'synchronization')),
    Suite('concurrent-collections', tuple(FOUNDATION_TESTS + 'concurrent-collections/' + path + '.rs' for path in (
        'queue/tests/array_queue', 'queue/tests/seg_queue', 'deque/tests/fifo', 'deque/tests/lifo',
        'deque/tests/injector', 'utilities/tests/sharded_lock')), ('fastrand',)),
    Suite('parking', (FOUNDATION_TESTS + 'parking/tests/parking.rs',)),
    Suite('signal-hook-registry', (FOUNDATION_TESTS + 'signal-hook-registry/tests/unregister_signal.rs',),
          ('libc',)),
    Suite('tempfile', tuple(FOUNDATION_TESTS + 'tempfile/tests/' + name + '.rs'
                            for name in ('env', 'namedtempfile', 'spooled', 'tempdir', 'tempfile')),
          ('fastrand',)),
    Suite('bytes', tuple(FOUNDATION_TESTS + 'bytes/tests/' + name + '.rs' for name in (
        'test_buf', 'test_buf_mut', 'test_bytes', 'test_bytes_odd_alloc', 'test_bytes_vec_alloc',
        'test_chain', 'test_debug', 'test_iter', 'test_limit', 'test_reader', 'test_take'))),
    Suite('slab', (FOUNDATION_TESTS + 'slab/tests/slab.rs',)),
    Suite('hashlink', tuple(FOUNDATION_TESTS + 'hashlink/tests/' + name + '.rs'
                            for name in ('linked_hash_map', 'linked_hash_set', 'lru_cache')),
          ('rustc-hash',)),
    Suite('once_cell', (FOUNDATION_TESTS + 'once_cell/tests/it/main.rs',),
          examples=('examples/reentrant_init_deadlocks.rs',)),
    Suite('cmov', tuple(FOUNDATION_TESTS + 'cmov/tests/' + name + '.rs'
                       for name in ('core_impls', 'regression'))),
    Suite('constant_time_eq', (FOUNDATION_TESTS + 'constant_time_eq/tests/exhaustive.rs',)),
)

# Retained material that stays unexecuted, with the concrete missing provider.
NOT_EXECUTED = {
    'test/foundation/bytes/tests/test_serialization.rs': 'serde_test is not a local source',
    'test/foundation/cmov/tests/proptests.rs': 'proptest is not a local source',
    'test/foundation/constant_time_eq/tests/count_instructions.rs': 'count_instructions is not a local source',
    'test/foundation/constant_time_eq/tests/count_instructions_classic.rs': 'count_instructions is not a local source',
    'test/foundation/constant_time_eq/tests/count_instructions_generic.rs': 'count_instructions is not a local source',
    'test/foundation/hashlink/tests/data_serialization.rs': 'serde_test is not a local source',
    'test/foundation/parking/tests/loom.rs': 'loom is not a local source',
}

SUMMARY = re.compile(r'^test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored; '
                     r'(\d+) measured; (\d+) filtered out', re.M)
OUTCOME = re.compile(r'^test (.+?)(?: - should panic)? \.\.\. (\w+)$', re.M)


def production_features():
    """Features each package resolves to in the production profiles, keyed by package name."""
    profiles = json.loads(WORKSPACES.read_text())['profiles']
    production = {profile['id'] for profile in profiles if profile['production']}
    resolved = {}
    for resolution in json.loads(CATALOG.read_text())['resolutions']:
        if resolution['profile'] not in production:
            continue
        for directory, package in resolution['packages'].items():
            name = tomllib.loads((ROOT / directory / 'Cargo.toml').read_text())['package']['name']
            resolved.setdefault(name, set()).update(package['features'])
    return {name: sorted(features) for name, features in resolved.items()}


def validation_features(name):
    """Features for a package reachable only from validation profiles (e.g. parking via the SQLite pool)."""
    features = set()
    found = False
    for resolution in json.loads(CATALOG.read_text())['resolutions']:
        for directory, package in resolution['packages'].items():
            if directory.rsplit('/', 1)[1] == name:
                found = True
                features.update(package['features'])
    assert found, f'{name} is not in any registered resolution'
    return sorted(features)


def root_features():
    """Features the root workspace resolves for each package; `cargo test -p` compiles with these."""
    metadata = json.loads(subprocess.run(
        ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
         '--filter-platform', 'x86_64-unknown-linux-gnu'],
        cwd=ROOT, text=True, capture_output=True, check=True).stdout)
    names = {package['id']: package['name'] for package in metadata['packages']}
    return {names[node['id']]: sorted(node['features']) for node in metadata['resolve']['nodes']}


def features_for(name, production):
    return production[name] if name in production else validation_features(name)


class Log:
    def __init__(self, path, header):
        path.parent.mkdir(parents=True, exist_ok=True)
        self.handle = path.open('w')
        self.handle.write(header)

    def run(self, command, cwd):
        self.handle.write('\n$ ' + ' '.join(command) + '\n')
        result = subprocess.run(command, cwd=cwd, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=1800)
        self.handle.write(result.stdout)
        self.handle.flush()
        if result.returncode:
            raise RuntimeError(f'{" ".join(command)} failed in {cwd}:\n{result.stdout}')
        return result.stdout

    def close(self):
        self.handle.close()


def listed(output):
    names = [line[:-len(': test')] for line in output.splitlines() if line.endswith(': test')]
    assert len(names) == len(set(names)), output
    return sorted(names)


def check_run(output, names, label):
    summaries = SUMMARY.findall(output)
    assert len(summaries) == 1, f'{label}: expected exactly one test binary\n{output}'
    status, passed, failed, ignored, measured, filtered = summaries[0]
    assert (status, failed, ignored, measured, filtered) == ('ok', '0', '0', '0', '0'), \
        f'{label}: ignored, filtered or failed tests do not count as executed\n{output}'
    outcomes = dict(OUTCOME.findall(output))
    assert set(outcomes) == set(names) and set(outcomes.values()) == {'ok'} and int(passed) == len(names), \
        f'{label}: every listed test must report ok\n{output}'


def cargo_test(log, cwd, selection, label):
    common = ['cargo', '--color', 'never', 'test', '--locked', '--offline', *selection]
    names = listed(log.run([*common, '--', '--list'], cwd))
    assert names, f'{label}: an empty test binary is not an executed suite'
    check_run(log.run(common, cwd), names, label)
    return names


def dependency(name, features):
    path = json.dumps(str(foundation_path(ROOT, name)))
    return f'{json.dumps(name)} = {{ path = {path}, default-features = false, features = {json.dumps(features)} }}\n'


def member_manifest(suite, production):
    subject = foundation_path(ROOT, suite.subject)
    declared = tomllib.loads((subject / 'Cargo.toml').read_text()).get('features', {})
    enabled = features_for(suite.subject, production)
    mirrored = sorted((set(declared) | set(enabled)) - {'default'})
    text = (f'[package]\nname = "behaviour-{suite.subject}"\nversion = "0.0.0"\nedition = "2021"\n'
            'publish = false\nautotests = false\n\n[lib]\npath = "lib.rs"\n\n[features]\n'
            f'default = {json.dumps([name for name in enabled if name != "default"])}\n')
    text += ''.join(f'{json.dumps(name)} = []\n' for name in mirrored)
    text += '\n[dependencies]\n' + dependency(suite.subject, enabled)
    text += ''.join(dependency(name, features_for(name, production)) for name in suite.providers)
    for path in suite.tests:
        assert (ROOT / path).is_file(), path
        text += f'\n[[test]]\nname = {json.dumps(target_name(path))}\npath = {json.dumps(str(ROOT / path))}\n'
    for path in suite.examples:
        assert (subject / path).is_file(), path
        text += f'\n[[example]]\nname = {json.dumps(Path(path).stem)}\npath = {json.dumps(str(subject / path))}\n'
    return text


def target_name(path):
    path = Path(path)
    return path.parent.name if path.name == 'main.rs' else path.stem


def source_state():
    commit = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True,
                            capture_output=True, check=True).stdout.strip()
    dirty = subprocess.run(['git', 'status', '--porcelain'], cwd=ROOT, text=True,
                           capture_output=True, check=True).stdout
    toolchain = subprocess.run(['rustc', '-V'], cwd=ROOT, text=True, capture_output=True,
                               check=True).stdout.strip()
    return commit, bool(dirty), toolchain


def run_suites(selected):
    commit, dirty, toolchain = source_state()
    header = f'commit: {commit}\nworktree: {"modified" if dirty else "clean"}\nrustc: {toolchain}\n'
    production = production_features()
    resolved = root_features()
    results = {}
    for name in UNIT_SUITES:
        key = 'unit:' + name
        if selected and key not in selected:
            continue
        log = Log(OUTPUT / 'logs' / (key.replace(':', '-') + '.log'), header)
        names = cargo_test(log, ROOT, ['--lib', '-p', name], key)
        log.close()
        results[key] = {'subject': name, 'features': resolved[name], 'targets': {'lib': names}}
        print(f'PASS {key}: {len(names)} tests', flush=True)
    chosen = [suite for suite in INTEGRATION_SUITES if not selected or 'integration:' + suite.subject in selected]
    if chosen:
        with tempfile.TemporaryDirectory(prefix='foundation-behaviour-') as directory:
            work = Path(directory)
            (work / 'rust-toolchain.toml').write_bytes((ROOT / 'rust-toolchain.toml').read_bytes())
            members = []
            for suite in chosen:
                member = work / suite.subject
                member.mkdir()
                (member / 'lib.rs').write_text('')
                (member / 'Cargo.toml').write_text(member_manifest(suite, production))
                members.append(suite.subject)
            (work / 'Cargo.toml').write_text('[workspace]\nresolver = "2"\nmembers = '
                                             + json.dumps(members) + '\n\n[profile.dev]\ndebug = 0\n')
            subprocess.run(['cargo', 'generate-lockfile', '--offline'], cwd=work, check=True,
                           capture_output=True, text=True)
            for suite in chosen:
                key = 'integration:' + suite.subject
                log = Log(OUTPUT / 'logs' / (key.replace(':', '-') + '.log'),
                          header + (work / suite.subject / 'Cargo.toml').read_text())
                targets = {}
                for path in suite.examples:
                    log.run(['cargo', '--color', 'never', 'build', '--locked', '--offline', '-p',
                             'behaviour-' + suite.subject, '--example', Path(path).stem], work)
                for path in suite.tests:
                    target = target_name(path)
                    targets[target] = cargo_test(
                        log, work, ['-p', 'behaviour-' + suite.subject, '--test', target], f'{key}/{target}')
                log.close()
                results[key] = {'subject': suite.subject, 'features': features_for(suite.subject, production),
                                'targets': targets}
                print(f'PASS {key}: {sum(map(len, targets.values()))} tests in {len(targets)} targets',
                      flush=True)
    summary = {'commit': commit, 'worktree_modified': dirty, 'rustc': toolchain,
               'suites': {key: {target: len(names) for target, names in value['targets'].items()}
                          for key, value in results.items()}}
    (OUTPUT / 'summary.json').write_text(json.dumps(summary, indent=2, sort_keys=True) + '\n')
    return results


def inventory_text(results):
    return json.dumps({'schema': 1, 'generator': '.github/scripts/check-foundation-behaviour.py --write',
                       'suites': results}, indent=2, sort_keys=True) + '\n'


def suite_keys():
    return ['unit:' + name for name in UNIT_SUITES] + ['integration:' + suite.subject for suite in INTEGRATION_SUITES]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--write', action='store_true', help='regenerate the inventory from a passing run')
    parser.add_argument('--suite', action='append', choices=suite_keys(), help='run only these suites')
    arguments = parser.parse_args()
    assert not (arguments.write and arguments.suite), 'the inventory is written from a complete run only'
    for path in NOT_EXECUTED:
        assert (ROOT / path).is_file(), path
    os.environ.setdefault('CARGO_TARGET_DIR', str(OUTPUT / 'target'))
    os.environ.setdefault('CARGO_INCREMENTAL', '0')
    results = run_suites(set(arguments.suite or ()))
    if arguments.write:
        INVENTORY.write_text(inventory_text(results))
        print(f'Wrote {INVENTORY.relative_to(ROOT)}', flush=True)
        return
    expected = json.loads(INVENTORY.read_text())
    assert expected['schema'] == 1 and sorted(expected['suites']) == sorted(suite_keys()), \
        'inventory suites differ from the suite table; regenerate with --write'
    for key, value in results.items():
        assert expected['suites'][key] == value, \
            f'{key}: executed tests or features differ from {INVENTORY.relative_to(ROOT)}'
    total = sum(len(names) for value in results.values() for names in value['targets'].values())
    print(f'Foundation behaviour suites passed: {len(results)} suites, {total} tests, names equal the inventory.')


if __name__ == '__main__':
    main()
