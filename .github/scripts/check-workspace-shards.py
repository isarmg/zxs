#!/usr/bin/env python3
"""Partition the complete native Cargo test inventory; never select from source grep.

The unsharded check.sh workspace recipe remains the reference. Cargo's original
no-run selection supplies native harnesses; doctests and every explicit I/O gate
run once in separate groups. Historical durations affect placement, not coverage.
"""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
TEST_SHARDS = 8
SHARDS = tuple(f'tests-{i}' for i in range(TEST_SHARDS)) + (
    'checks-docs', 'io-0', 'io-1', 'io-2', 'ignored')
CARGO = ['cargo', 'test', '--locked', '--offline']
IO = (
    ('history-object', 'platform-atomic', 'directory-retry', 'smb-budget'),
    ('engine-fault', 'auth-fault', 'entropy-fault', 'telemetry-enumeration'),
    ('disk-observation', 'helper-publication', 'independent-disk-publication'),
)
IGNORED = (
    'history_package::graph::tests::distributed_history_larger_than_real_reader_limit_roundtrips',
    'disk::admission_tests::immutable_existing_bind_leaf_is_rejected_before_read_or_sync',
    'disk::admission_tests::cold_directory_admission_keeps_held_identity_after_mount_replacement',
)
SUMMARY = re.compile(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;', re.M)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def auxiliary_commands(shard):
    if shard == 'checks-docs':
        return [['python3', '.github/scripts/check-native-consumer-features.py'],
                ['cargo', 'check', '--locked', '--offline', '-p', 'storage-engine', '-p', 'storage-server', '-p', 'storage-helper'],
                ['cargo', 'check', '--locked', '--offline', '-p', 'storage-server', '--no-default-features', '--all-targets'],
                CARGO + ['--doc']]
    if shard in ('io-0', 'io-1', 'io-2'):
        return [['python3', f'.github/scripts/check-{name}-io.py']
                if name != 'directory-retry' else ['python3', '.github/scripts/check-directory-retry.py']
                for name in IO[int(shard[-1])]]
    if shard == 'ignored':
        return [CARGO + ['-p', 'storage-engine', '--lib', name, '--', '--ignored', '--exact', '--test-threads=1']
                for name in IGNORED]
    raise ValueError('not an auxiliary shard: ' + shard)


def listed(output):
    rows = []
    for line in output.splitlines():
        match = re.fullmatch(r'(.+): (test|benchmark)', line)
        if match:
            rows.append((match[1], match[2]))
        elif line.strip() and not re.fullmatch(r'\d+ tests?, \d+ benchmarks?', line):
            raise ValueError('unexpected libtest inventory line: ' + line)
    if len(rows) != len({name for name, _ in rows}):
        raise ValueError('duplicate libtest name')
    return rows


def allocate(inventory, weights):
    """Deterministic longest-first placement; identity is target plus exact name."""
    identities = [(target['id'], test['name']) for target in inventory for test in target['tests']]
    if len(identities) != len(set(identities)):
        raise ValueError('duplicate target/test identity')
    if any(not isinstance(name, str) or type(weight) is not int or weight <= 0 for name, weight in weights.items()):
        raise ValueError('invalid placement weight')
    costs = [0] * TEST_SHARDS
    result = {}
    for identity in sorted(identities, key=lambda item: (-weights.get(item[1], 1), item)):
        index = min(range(TEST_SHARDS), key=lambda i: (costs[i], i))
        result[identity] = index
        costs[index] += weights.get(identity[1], 1)
    return result


def validate_result(output, selected, total, returncode, expected_ignored):
    summaries = SUMMARY.findall(output)
    if not summaries:
        raise ValueError('missing libtest completion summary')
    # Isolated namespace children print their own summaries before the parent.
    state, passed, failed, ignored, measured, filtered = summaries[-1]
    counts = dict(zip(('passed', 'failed', 'ignored', 'measured', 'filtered'),
                      map(int, (passed, failed, ignored, measured, filtered))))
    if sum(counts[key] for key in ('passed', 'failed', 'ignored', 'measured')) != selected:
        raise ValueError('libtest executed count differs from assigned inventory')
    if counts['ignored'] != expected_ignored:
        raise ValueError('ignored count differs from exact inventory')
    if counts['filtered'] != total - selected:
        raise ValueError('libtest filtered count differs from full inventory')
    if returncode != 0 or state != 'ok' or counts['failed']:
        raise ValueError('libtest reported a failing assigned test')
    return counts



def exact_ignored_result(output):
    summaries = SUMMARY.findall(output)
    if not summaries:
        raise ValueError('missing exact ignored completion')
    state, passed, failed, ignored, measured, filtered = summaries[-1]
    if (state, passed, failed, ignored, measured) != ('ok', '1', '0', '0', '0'):
        raise ValueError('exact ignored test did not complete once')
    return {'passed': 1, 'failed': 0, 'ignored': 0, 'measured': 0, 'filtered': int(filtered)}


def artifacts(output, metadata, root):
    packages = {p['id']: p for p in metadata['packages']}
    members = set(metadata['workspace_default_members'])
    found, features = {}, {}
    success = False
    for line in output.splitlines():
        message = json.loads(line)
        if message.get('reason') == 'build-finished':
            success = message.get('success') is True
        if message.get('reason') != 'compiler-artifact' or message.get('package_id') not in members:
            continue
        package = packages[message['package_id']]
        features.setdefault(package['id'], set()).update(message['features'])
        if not message.get('profile', {}).get('test') or not message.get('executable'):
            continue
        target = message['target']
        declaration = tomllib.loads(Path(package['manifest_path']).read_text())
        section = target['kind'][0] if target['kind'][0] in ('bin', 'test', 'example', 'bench') else 'lib'
        entries = declaration.get(section, [] if section != 'lib' else {})
        entries = entries if isinstance(entries, list) else [entries]
        if any(entry.get('harness') is False and (section == 'lib' or entry.get('name') == target['name']) for entry in entries):
            raise ValueError('custom test harness needs an explicit full-coverage adapter')
        manifest = Path(package['manifest_path']).relative_to(root).as_posix()
        identity = manifest + ':' + ','.join(target['kind']) + ':' + target['name']
        if identity in found:
            raise ValueError('duplicate Cargo test executable: ' + identity)
        binary = Path(message['executable']).resolve()
        if not binary.is_file() or root not in binary.parents:
            raise ValueError('missing or out-of-repository Cargo executable')
        found[identity] = {'id': identity, 'manifest': manifest, 'binary': str(binary),
                           'target': target['name'], 'kind': target['kind']}
    if not success or not found:
        raise ValueError('Cargo did not complete the native inventory build')
    expected = set()
    for member in members:
        package = packages[member]
        manifest = Path(package['manifest_path']).relative_to(root).as_posix()
        for target in package['targets']:
            if target['test'] and set(target.get('required-features', [])) <= features.get(member, set()):
                expected.add(manifest + ':' + ','.join(target['kind']) + ':' + target['name'])
    if set(found) != expected:
        raise ValueError(f'Cargo target inventory differs: missing={sorted(expected-set(found))}, extra={sorted(set(found)-expected)}')
    return [found[key] for key in sorted(found)]


class Runner:
    def __init__(self, shard):
        self.shard = shard
        self.output = ROOT / 'target/workspace-shards' / shard
        self.output.mkdir(parents=True, exist_ok=True)
        self.environment = dict(os.environ, CARGO_TERM_COLOR='never')
        self.receipt = {'schema': 1, 'shard': shard, 'status': 'incomplete', 'commands': [], 'results': []}
        self.save()

    def save(self):
        (self.output / 'receipt.json').write_text(json.dumps(self.receipt, indent=2) + '\n')

    def run(self, command, *, cwd=ROOT, env=None, timeout=5400):
        index = len(self.receipt['commands'])
        record = {'command': command, 'cwd': str(cwd.relative_to(ROOT)), 'timeout_seconds': timeout}
        self.receipt['commands'].append(record)
        self.save()
        start = time.monotonic()
        print('+ ' + ' '.join(command), flush=True)
        out, err = self.output / f'{index:03}.stdout.log', self.output / f'{index:03}.stderr.log'
        try:
            with out.open('w') as stdout, err.open('w') as stderr:
                child = subprocess.Popen(command, cwd=cwd, env=env or self.environment,
                                         stdout=stdout, stderr=stderr, start_new_session=True)
                try:
                    record['returncode'] = child.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    record['timed_out'] = True
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    child.wait()
                    raise
            output = out.read_text()
            if record['returncode']:
                print(output[-24000:], file=sys.stderr)
                print(err.read_text()[-24000:], file=sys.stderr)
            return record['returncode'], output
        finally:
            record['seconds'] = round(time.monotonic() - start, 3)
            self.save()

    def require(self, command, **options):
        code, output = self.run(command, **options)
        if code:
            raise subprocess.CalledProcessError(code, command)
        return output

    def tests(self):
        metadata = json.loads(self.require(['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version=1'], timeout=120))
        output = self.require(CARGO + ['--no-run', '--message-format=json'], timeout=1800)
        binaries = artifacts(output, metadata, ROOT)
        inventory = []
        for target in binaries:
            cwd = (ROOT / target['manifest']).parent
            environment = dict(self.environment, CARGO_MANIFEST_DIR=str(cwd))
            binary = Path(target['binary'])
            # Cargo adds target output directories for dynamic native consumers.
            environment['LD_LIBRARY_PATH'] = os.pathsep.join([str(binary.parent), str(binary.parent.parent), environment.get('LD_LIBRARY_PATH', '')])
            target['environment'] = environment
            all_tests = listed(self.require([str(binary), '--list', '--format', 'terse'], cwd=cwd, env=environment, timeout=120))
            ignored = set(listed(self.require([str(binary), '--list', '--ignored', '--format', 'terse'], cwd=cwd, env=environment, timeout=120)))
            if not ignored <= set(all_tests):
                raise ValueError('ignored inventory is not a subset')
            inventory.append({'id': target['id'], 'manifest': target['manifest'],
                              'tests': [{'name': name, 'kind': kind, 'ignored': (name, kind) in ignored}
                                        for name, kind in sorted(all_tests)]})
        weights = json.loads((ROOT / '.github/workspace-test-weights.json').read_text())['seconds_lower_bound']
        assignments = allocate(inventory, weights)
        self.receipt.update(inventory=inventory, inventory_sha256=digest(inventory), weights_sha256=digest(weights))
        self.save()
        index = int(self.shard.split('-')[1])
        failures = []
        for target, listed_target in zip(binaries, inventory):
            selected = [test['name'] for test in listed_target['tests'] if assignments[(target['id'], test['name'])] == index]
            if not selected:
                continue
            result = {'target': target['id'], 'assigned': selected, 'status': 'incomplete'}
            self.receipt['results'].append(result)
            self.save()
            try:
                code, output = self.run([target['binary'], '--exact', '--format', 'pretty', *selected],
                                        cwd=(ROOT / target['manifest']).parent, env=target['environment'])
                result['counts'] = validate_result(output, len(selected), len(listed_target['tests']), code,
                                                    sum(test['ignored'] for test in listed_target['tests'] if test['name'] in selected))
                result['status'] = 'success'
            except (ValueError, subprocess.TimeoutExpired) as error:
                result.update(status='failure', error=str(error))
                failures.append(str(error))
            finally:
                self.save()
        if not self.receipt['results'] or failures:
            raise ValueError('native shard incomplete or failed: ' + '; '.join(failures))

    def execute(self):
        lock = hashlib.sha256((ROOT / 'Cargo.lock').read_bytes()).hexdigest()
        try:
            self.receipt.update(head=self.require(['git', 'rev-parse', 'HEAD'], timeout=30).strip(), lockfile_sha256=lock)
            if self.receipt['head'] != os.environ.get('GITHUB_SHA', self.receipt['head']):
                raise ValueError('checkout does not match workflow commit')
            if self.shard.startswith('tests-'):
                self.tests()
            else:
                failures = []
                for command in auxiliary_commands(self.shard):
                    try:
                        code, output = self.run(command)
                        if self.shard == 'ignored':
                            counts = exact_ignored_result(output)
                            self.receipt['results'].append({'name': command[7], 'counts': counts, 'status': 'success'})
                            self.save()
                        if code:
                            failures.append(command)
                    except (subprocess.TimeoutExpired, ValueError):
                        failures.append(command)
                if failures:
                    raise ValueError('auxiliary commands failed: ' + repr(failures))
            if hashlib.sha256((ROOT / 'Cargo.lock').read_bytes()).hexdigest() != lock:
                raise ValueError('root lockfile changed')
            self.receipt['status'] = 'success'
        except BaseException as error:
            self.receipt.update(status='failure', error=str(error))
            raise
        finally:
            self.save()


def verify(directory, head, weights):
    receipts = [json.loads(path.read_text()) for path in sorted(directory.rglob('receipt.json'))]
    if Counter(r['shard'] for r in receipts) != Counter(SHARDS):
        raise ValueError('missing, duplicate or unexpected shard receipt')
    if any(r['schema'] != 1 or r['head'] != head or r['status'] != 'success' for r in receipts):
        raise ValueError('unsuccessful or wrong-commit shard')
    if len({r['lockfile_sha256'] for r in receipts}) != 1:
        raise ValueError('different root lockfiles')
    native = [r for r in receipts if r['shard'].startswith('tests-')]
    inventory = native[0]['inventory']
    expected = allocate(inventory, weights)
    observed = []
    for receipt in native:
        if receipt['inventory'] != inventory or receipt['inventory_sha256'] != digest(inventory) or receipt['weights_sha256'] != digest(weights):
            raise ValueError('native inventory or placement differs across shards')
        index = int(receipt['shard'].split('-')[1])
        actual = [(result['target'], name) for result in receipt['results'] for name in result['assigned']]
        required = [identity for identity, assigned in expected.items() if assigned == index]
        if Counter(actual) != Counter(required) or any(r['status'] != 'success' for r in receipt['results']):
            raise ValueError('shard results omit or repeat assigned tests')
        targets = {target['id']: target for target in inventory}
        for result in receipt['results']:
            tests = targets[result['target']]['tests']
            counts = result['counts']
            ignored = sum(test['ignored'] for test in tests if test['name'] in result['assigned'])
            if (counts['failed'] or counts['ignored'] != ignored
                    or counts['passed'] + counts['ignored'] + counts['measured'] != len(result['assigned'])
                    or counts['filtered'] != len(tests) - len(result['assigned'])):
                raise ValueError('receipt completion count differs from inventory')
        if any(command.get('returncode') != 0 for command in receipt['commands']):
            raise ValueError('failed native command in successful receipt')
        observed.extend(actual)
    if Counter(observed) != Counter(expected.keys()):
        raise ValueError('native union is not exactly the full Cargo inventory')
    for receipt in receipts:
        if not receipt['shard'].startswith('tests-'):
            actual = [row['command'] for row in receipt['commands']][1:]
            if actual != auxiliary_commands(receipt['shard']) or any(row.get('returncode') != 0 for row in receipt['commands']):
                raise ValueError('auxiliary command coverage differs')
            if receipt['shard'] == 'ignored':
                if [result['name'] for result in receipt['results']] != list(IGNORED):
                    raise ValueError('missing exact ignored execution results')
                for result in receipt['results']:
                    if result['status'] != 'success' or any(result['counts'][key] != count for key, count in
                            (('passed', 1), ('failed', 0), ('ignored', 0), ('measured', 0))):
                        raise ValueError('invalid exact ignored completion receipt')
    print(f'Full workspace coverage: {len(expected)} native test identities, {TEST_SHARDS} disjoint shards, all auxiliary groups complete.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('shard', choices=(*SHARDS, 'verify'))
    parser.add_argument('directory', nargs='?', type=Path)
    args = parser.parse_args()
    if args.shard == 'verify':
        if args.directory is None:
            parser.error('verify requires the downloaded receipt directory')
        weights = json.loads((ROOT / '.github/workspace-test-weights.json').read_text())['seconds_lower_bound']
        verify(args.directory, os.environ['GITHUB_SHA'], weights)
    else:
        if args.directory is not None:
            parser.error('execution does not accept a receipt directory')
        Runner(args.shard).execute()


if __name__ == '__main__':
    main()
