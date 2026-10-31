#!/usr/bin/env python3
"""Run one retained PKI secret-input test and a real-SDK negative control."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization.')

import hashlib
import json
from pathlib import Path
import re
import runpy
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
TEST = 'base64::tests::codepoint_decode_secret_does_not_branch_or_index_on_secret_input'


def run(command, log, environment, cwd=ROOT, expected=0):
    result = subprocess.run(command, cwd=cwd, env=environment, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=1200)
    log.write_text('$ ' + ' '.join(map(str, command)) + '\n' + result.stdout)
    if result.returncode != expected:
        raise RuntimeError(f'{log}: expected exit {expected}, got {result.returncode}\n{result.stdout}')
    return result.stdout


def check():
    output = ROOT / 'target/pki-valgrind'
    logs = output / 'logs'
    logs.mkdir(parents=True, exist_ok=True)
    environment = runpy.run_path(str(ROOT / '.github/scripts/backend_environment.py'))['matrix_environment']()
    include = Path(environment.get('ZXS_VALGRIND_INCLUDE', '/usr/include')).resolve()
    headers = [include / 'valgrind' / name for name in ('valgrind.h', 'memcheck.h')]
    for header in headers:
        if not header.is_file():
            raise RuntimeError(f'real Valgrind SDK header required: {header}')
    environment['ZXS_VALGRIND_INCLUDE'] = str(include)
    environment['ZXS_VALGRIND_FIXTURES'] = str(ROOT / 'test/ci/fixtures')
    environment['CARGO_TARGET_DIR'] = str(output / 'build')
    valgrind = environment.get('ZXS_VALGRIND', 'valgrind')
    version = run([valgrind, '--version'], logs / 'valgrind-version.log', environment).strip()
    compiler = run(['rustc', '--version', '--verbose'], logs / 'rustc.log', environment)
    prefix = [valgrind, '--tool=memcheck', '--error-exitcode=97', '--track-origins=yes']
    if environment.get('ZXS_VALGRIND_DEBUGINFO'):
        prefix += ['--extra-debuginfo-path=' + str(Path(environment['ZXS_VALGRIND_DEBUGINFO']).resolve())]
    fixtures = ROOT / 'test/ci/fixtures'
    with tempfile.TemporaryDirectory(prefix='fixture-', dir=output) as temporary:
        mirror = Path(temporary)
        # The retained include! paths are relative to their original package depth.
        # No production source or test byte is copied, transformed, or substituted.
        (mirror / 'test').symlink_to(ROOT / 'test', target_is_directory=True)
        work = mirror / 'src/foundation/networking/rustls-pki-types'
        work.mkdir(parents=True)
        source = ROOT / 'src/foundation/networking/rustls-pki-types'
        manifest = f'''[package]
name="pki-valgrind-tests"
version="0.0.0"
edition="2021"
build={json.dumps(str(fixtures / 'valgrind-sdk-build.rs'))}
[workspace]
[lib]
path={json.dumps(str(source / 'src/lib.rs'))}
[features]
default=["std"]
alloc=["dep:zeroize"]
std=["alloc"]
[dependencies]
zeroize={{path={json.dumps(str(ROOT / 'src/foundation/cryptography/zeroize'))},optional=true}}
[build-dependencies]
cc={{path={json.dumps(str(ROOT / 'src/foundation/tooling/cc'))}}}
bindgen={{path={json.dumps(str(ROOT / 'src/foundation/tooling/bindgen'))}}}
[[bin]]
name="valgrind-negative"
path={json.dumps(str(fixtures / 'valgrind-negative.rs'))}
'''
        (work / 'Cargo.toml').write_text(manifest)
        common = ['--offline', '--manifest-path', str(work / 'Cargo.toml')]
        run(['cargo', 'generate-lockfile', *common], logs / 'lock.log', environment)
        built = run(['cargo', 'test', '--locked', *common, '--release', '--lib', '--no-run',
                     '--message-format=json'], logs / 'build-test.log', environment)
        executables = [json.loads(line)['executable'] for line in built.splitlines()
                       if line.startswith('{') and json.loads(line).get('reason') == 'compiler-artifact'
                       and json.loads(line).get('executable')]
        if len(executables) != 1:
            raise RuntimeError(f'exactly one test executable required: {executables}')
        binary = executables[0]
        listed = run([binary, TEST, '--exact', '--list'], logs / 'test-list.log', environment)
        if [line for line in listed.splitlines() if line.endswith(': test')] != [TEST + ': test']:
            raise RuntimeError('the exact retained codepoint test was not enumerated')
        native = run([binary, TEST, '--exact', '--test-threads=1', '--nocapture'],
                     logs / 'native-rejected.log', environment, expected=101)
        if 'Valgrind is required' not in native or '0 passed; 1 failed' not in native:
            raise RuntimeError('native execution must fail rather than report a skipped pass')
        passed = run([*prefix, binary, TEST, '--exact', '--test-threads=1', '--nocapture'],
                     logs / 'positive.log', environment)
        if ('SKIPPED' in passed or '1 passed; 0 failed; 0 ignored' not in passed
                or not re.search(r'ERROR SUMMARY: 0 errors from 0 contexts', passed)):
            raise RuntimeError('positive probe must run exactly once under clean Memcheck')
        built = run(['cargo', 'build', '--locked', *common, '--release', '--bin',
                     'valgrind-negative', '--message-format=json'], logs / 'build-negative.log', environment)
        binaries = [json.loads(line)['executable'] for line in built.splitlines()
                    if line.startswith('{') and json.loads(line).get('reason') == 'compiler-artifact'
                    and json.loads(line).get('executable')]
        if len(binaries) != 1:
            raise RuntimeError('exactly one negative-control executable required')
        rejected = run([*prefix, binaries[0]], logs / 'negative.log', environment, expected=97)
        if ('Conditional jump or move depends on uninitialised value(s)' not in rejected
                or not re.search(r'ERROR SUMMARY: [1-9][0-9,]* errors', rejected)):
            raise RuntimeError('the deliberately tainted branch must be detected by Memcheck')
        identity = {'valgrind': version, 'rustc': compiler, 'test': TEST,
                    'headers': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in headers},
                    'test_binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
                    'sources': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                                for p in [Path(__file__).resolve(), *sorted(fixtures.glob('valgrind-*')),
                                          ROOT / 'test/foundation/rustls-pki-types/unit/src/base64/all_tests_533.rs',
                                          source / 'src/base64.rs']},
                    'positive_tests': 1, 'negative_exit': 97, 'native_exit': 101,
                    'scope': 'one retained codepoint test; not a general constant-time proof'}
        (logs / 'receipt.json').write_text(json.dumps(identity, indent=2) + '\n')
    print('PKI Valgrind SDK: retained codepoint test passed; tainted-branch control rejected with exit 97.')


if __name__ == '__main__':
    check()
