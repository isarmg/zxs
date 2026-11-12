#!/usr/bin/env python3
"""Execute a bounded, reviewed change plan against current Cargo test inventories."""
import argparse
import fnmatch
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
PLAN_PATH = '.github/ci/targeted-tests.json'


def observe_backup_process(target, name):
    """Only this already-selected current-build test gets read-only observation."""
    return (target.get('mode', 'test') == 'test'
            and target.get('package') == 'storage-engine'
            and target.get('manifest') == 'src/engine/Cargo.toml'
            and target.get('kind') == 'test'
            and target.get('name') == 'recursive_backup_paths'
            and name == (
                'backup_preparation::transaction_serving::'
                'same_backup_switch_serves_and_replays_normal_successor_publications'))


def backup_process_sampler():
    path = ROOT / '.github/scripts/backup-process-sampling.py'
    spec = importlib.util.spec_from_file_location('backup_process_sampling', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def step_summary(summary):
    # Full inventories, configuration provenance and logs remain in the artifact.
    # GitHub rejects a step summary above 1 MiB; bound this presentation separately.
    short = lambda value, limit=2000: str(value)[:limit]
    failed = summary.get('failed', [])
    passed = summary.get('passed', [])
    projection = {
        'status': short(summary.get('status', 'unknown'), 80),
        'head': short(summary.get('head', ''), 40),
        'base': short(summary.get('base', ''), 40),
        'rust_tests_passed': sum('name' in item for item in passed),
        'python_tests_passed': sum(item.get('tests', 0) for item in passed
                                   if 'python_module' in item),
        'compile_checks_passed': len(summary.get('compiled', [])),
        'selected_groups': [short(group, 80) for group in
                            summary.get('plan', {}).get('groups', [])[:50]],
        'unexecuted_scope': short(summary.get('plan', {}).get('unexecuted', '')),
        'failed': len(failed),
        'skipped': len(summary.get('skipped', [])),
        'blocked': len(summary.get('blocked', [])),
        'first_failures': [
            {key: short(item[key], 500) for key in ('stage', 'name', 'python_module', 'error')
             if key in item}
            for item in failed[:20]
        ],
    }
    if 'error' in summary:
        projection['error'] = short(summary['error'])
    return ('### Targeted CI scope and result\n\n'
            'Complete scope, inventories, all failures, skipped/blocked work and logs are '
            'retained in the targeted-tests artifact (summary.json and *.log).\n\n'
            '~~~json\n' + json.dumps(projection, indent=2, ensure_ascii=False) + '\n~~~\n')


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def select_plan(policy, changed, *, shardable=False):
    if set(policy) != {'groups', 'rules', 'unexecuted'} or not policy['unexecuted']:
        raise ValueError('invalid targeted policy')
    groups, rules = policy['groups'], policy['rules']
    if not isinstance(groups, dict) or not groups or not isinstance(rules, list) or not rules:
        raise ValueError('empty targeted policy')
    selected, unknown = set(), []
    for path in changed:
        matches = [rule for rule in rules if fnmatch.fnmatchcase(path, rule['path'])]
        if not matches:
            unknown.append(path)
        for rule in matches:
            if set(rule) != {'path', 'groups'} or not isinstance(rule['groups'], list):
                raise ValueError('invalid path rule')
            selected.update(rule['groups'])
    if unknown:
        raise ValueError(f'unmapped changes need a reviewed targeted rule: {unknown}')
    if not changed or not selected or not selected <= set(groups):
        raise ValueError('empty or unknown targeted selection')
    targets, origins, python_tests = [], [], set()
    python_timeouts, python_selections = {}, []
    for name in sorted(selected):
        group = groups[name]
        required = {'reason', 'python_tests', 'rust_targets'}
        if (not required <= set(group) or set(group) - required - {'python_timeouts'}
                or not group['reason']):
            raise ValueError('invalid target group')
        modules = group['python_tests']
        timeouts = group.get('python_timeouts', {})
        if (not isinstance(modules, list)
                or any(not isinstance(module, str)
                       or not re.fullmatch(r'test_[a-z0-9_]+', module) for module in modules)
                or len(set(modules)) != len(modules)
                or not isinstance(timeouts, dict) or not set(timeouts) <= set(modules)
                or any(type(value) is not int or not 1 <= value <= 900
                       for value in timeouts.values())):
            raise ValueError('invalid Python modules or timeout budget')
        for module in modules:
            timeout = timeouts.get(module, 120)
            python_timeouts[module] = min(python_timeouts.get(module, timeout), timeout)
            python_selections.append({'group': name, 'module': module,
                                      'timeout_seconds': timeout})
        python_tests.update(modules)
        targets.extend(group['rust_targets'])
        origins.extend([name] * len(group['rust_targets']))
    python_limit = 128 if shardable else 8
    if len(python_tests) > python_limit or any(not re.fullmatch(r'test_[a-z0-9_]+', n) for n in python_tests):
        raise ValueError('invalid Python modules')
    if not (targets or python_tests):
        raise ValueError('target selection must be bounded and nonempty')
    identities = []
    for target in targets:
        required = {'package', 'manifest'}
        optional = {'prefixes', 'exact', 'whole', 'max_tests', 'kind', 'name', 'features',
                    'default_features', 'mode', 'compile_target', 'timeout_seconds'}
        if not required <= set(target) or set(target) - required - optional:
            raise ValueError('invalid Rust target fields')
        if not re.fullmatch(r'[a-z][a-z0-9-]+', target['package']):
            raise ValueError('invalid package name')
        if not re.fullmatch(r'(?:(?:src|tools)/[a-z0-9_-]+|helper)/Cargo.toml', target['manifest']):
            raise ValueError('invalid manifest path')
        kind, mode = target.get('kind', 'lib'), target.get('mode', 'test')
        if kind not in {'lib', 'test', 'bin'} or mode not in {'test', 'check'}:
            raise ValueError('invalid target kind or execution mode')
        if kind != 'lib' and not re.fullmatch(r'[a-z][a-z0-9_-]+', target.get('name', '')):
            raise ValueError('named target requires its exact Cargo name')
        if kind == 'lib' and 'name' in target:
            raise ValueError('library name comes from the package')
        features = target.get('features', [])
        if (not isinstance(features, list) or len(set(features)) != len(features)
                or any(not re.fullmatch(r'[a-z][a-z0-9_-]*', f) for f in features)):
            raise ValueError('invalid features')
        if type(target.get('default_features', True)) is not bool:
            raise ValueError('default_features must be boolean')
        cross = target.get('compile_target')
        if cross not in {None, 'wasm32-unknown-unknown'} or (cross and mode != 'check'):
            raise ValueError('only compile checks may select the reviewed WASM target')
        if mode == 'check':
            if set(target) & {'prefixes', 'exact', 'whole', 'max_tests'}:
                raise ValueError('compile-only targets cannot claim test selection')
        else:
            prefixes, exact = target.get('prefixes', []), target.get('exact', [])
            whole = target.get('whole', False)
            if (type(whole) is not bool or not isinstance(prefixes, list)
                    or not isinstance(exact, list) or len(prefixes) > 16 or len(exact) > 16
                    or len(set(prefixes)) != len(prefixes) or len(set(exact)) != len(exact)
                    or any(not re.fullmatch(r'(?:[a-z_][a-z0-9_]*::)+', n) for n in prefixes)
                    or any(not re.fullmatch(r'[a-z_][a-z0-9_:]*', n) for n in exact)
                    or not (whole or prefixes or exact) or (whole and (prefixes or exact))):
                raise ValueError('invalid or empty test selection')
            # The compiled held-layout inventory has 187 runnable tests; each target
            # still enforces its own reviewed cap below this admission ceiling.
            if type(target.get('max_tests')) is not int or not 1 <= target['max_tests'] <= 192:
                raise ValueError('test count budget must be between 1 and 192')
        if (type(target.get('timeout_seconds', 300)) is not int
                or not 1 <= target.get('timeout_seconds', 300) <= 900):
            raise ValueError('per-test timeout must be between 1 and 900 seconds')
        identities.append((target['package'], kind, target.get('name'), tuple(sorted(features)),
                           target.get('default_features', True), mode, cross))
    builds = {}
    for identity, target, origin in zip(identities, targets, origins):
        if identity not in builds:
            builds[identity] = dict(target, selection_groups=[origin])
            continue
        combined = builds[identity]
        if combined['manifest'] != target['manifest']:
            raise ValueError('same Cargo identity has conflicting manifests')
        if 'selections' not in combined:
            original = {key: value for key, value in combined.items()
                        if key != 'selection_groups'}
            combined['selections'] = [original]
            for key in ('prefixes', 'exact', 'whole', 'max_tests', 'timeout_seconds'):
                combined.pop(key, None)
        combined['selections'].append(dict(target))
        combined['selection_groups'].append(origin)
    targets = list(builds.values())
    if len(targets) > (256 if shardable else 16):
        raise ValueError('target selection must be bounded and nonempty')
    return {'groups': sorted(selected), 'changed_paths': sorted(changed),
            'python_tests': sorted(python_tests), 'python_timeouts': python_timeouts,
            'python_selections': python_selections, 'rust_targets': targets,
            'unexecuted': policy['unexecuted']}


def cargo_command(target, build):
    mode, kind = target.get('mode', 'test'), target.get('kind', 'lib')
    command = ['cargo', mode, '--locked', '--offline', '-p', target['package'], '--' + kind]
    if kind != 'lib':
        command.append(target['name'])
    if mode == 'test':
        command.append('--no-run')
    if target.get('features'):
        command += ['--features', ','.join(target['features'])]
    if not target.get('default_features', True):
        command.append('--no-default-features')
    if target.get('compile_target'):
        command += ['--target', target['compile_target'], '--release']
    return command + ['--message-format=json', '--target-dir', str(build)]


def current_artifact(output, target, root):
    artifacts, finished = [], []
    kind = target.get('kind', 'lib')
    name = target['package'].replace('-', '_') if kind == 'lib' else target['name']
    for line in output.splitlines():
        item = json.loads(line)
        if item.get('reason') == 'build-finished':
            finished.append(item['success'])
        if item.get('reason') != 'compiler-artifact':
            continue
        actual_kind = item['target']['kind']
        matches_kind = (bool(actual_kind) and set(actual_kind) <= {'lib', 'rlib', 'cdylib'}
                        if kind == 'lib' else actual_kind == [kind])
        if (not matches_kind or item['target']['name'] != name
                or Path(item['manifest_path']).resolve() != root / target['manifest']
                or item['profile']['test'] != (target.get('mode', 'test') == 'test')):
            continue
        if not set(target.get('features', [])) <= set(item['features']):
            raise ValueError('current artifact lacks requested features')
        artifacts.append(item)
    if finished != [True] or len(artifacts) != 1:
        raise ValueError('missing unique current Cargo target artifact')
    return artifacts[0]


def inventory(output):
    names = []
    for line in output.splitlines():
        if line.endswith(': test'):
            names.append(line[:-6])
        elif line and not re.fullmatch(r'\d+ tests?, \d+ benchmarks?', line):
            raise ValueError(f'unexpected test inventory line: {line}')
    if len(set(names)) != len(names):
        raise ValueError('duplicate test inventory')
    return names


def selected_tests(names, ignored, target):
    if 'selections' in target:
        return sorted({name for selection in target['selections']
                       for name in selected_tests(names, ignored, selection)})
    if not set(ignored) <= set(names):
        raise ValueError('ignored inventory is outside complete inventory')
    eligible = set(names) - set(ignored)
    selected = set(eligible) if target.get('whole', False) else set()
    for name in target.get('exact', []):
        if name not in eligible:
            raise ValueError(f'missing or ignored exact test: {name}')
        selected.add(name)
    for prefix in target.get('prefixes', []):
        matching = {name for name in eligible if name.startswith(prefix)}
        if not matching:
            raise ValueError(f'empty selected module: {prefix}')
        selected.update(matching)
    if not selected or len(selected) > target['max_tests']:
        raise ValueError('selected inventory exceeds reviewed budget')
    return sorted(selected)


def selection_evidence(names, ignored, target):
    selections = target.get('selections', [target])
    return [{'group': group, 'configuration': selection,
             'selected': selected_tests(names, ignored, selection)}
            for group, selection in zip(target['selection_groups'], selections)]


def test_timeouts(selections):
    timeouts = {}
    for selection in selections:
        timeout = selection['configuration'].get('timeout_seconds', 300)
        for name in selection['selected']:
            timeouts[name] = min(timeouts.get(name, timeout), timeout)
    return timeouts


def passed_test(output, total):
    summaries = re.findall(
        r'^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; '
        r'(\d+) measured; (\d+) filtered out;.*$', output, re.MULTILINE)
    if (not summaries or summaries[-1][:4] != ('1', '0', '0', '0')
            or int(summaries[-1][4]) != total - 1):
        raise ValueError('exact test did not finish with one pass')


def passed_python(output, expected):
    counts = re.findall(r'^Ran (\d+) tests? in [0-9.]+s$', output, re.MULTILINE)
    if (expected <= 0 or counts != [str(expected)]
            or not re.search(r'^OK$', output, re.MULTILINE)):
        raise ValueError('Python module did not execute the expected nonzero tests')


def shard_tools():
    path = ROOT / ".github/scripts/targeted-ci-shards.py"
    spec = importlib.util.spec_from_file_location("targeted_ci_shards", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Run:
    def __init__(self, root, *, shard_request=None):
        self.root = Path(root)
        self.shard_request = shard_request
        self.directory = self.root / 'target/targeted-ci'
        self.directory.mkdir(parents=True, exist_ok=True)
        self.summary = {'status': 'incomplete', 'commands': [], 'artifacts': [], 'passed': [],
                        'compiled': [], 'failed': [], 'skipped': [], 'blocked': []}
        self.environment = dict(os.environ, CARGO_INCREMENTAL='0')
        # Disk admission needs a real filesystem rather than the runner's tmpfs.
        fixtures = self.directory / 'fixtures'
        fixtures.mkdir(exist_ok=True)
        self.environment['TMPDIR'] = str(fixtures)
        wire = self.directory / 'wire'
        wire.mkdir(exist_ok=True)
        self.environment['ZXS_TEST_WIRE_EVIDENCE_DIR'] = str(wire)
        self.summary['wire_evidence_directory'] = str(wire)
        self.save()

    def save(self):
        (self.directory / 'summary.json').write_text(json.dumps(self.summary, indent=2) + '\n')

    def command(self, args, timeout=60, *, observe_process=False):
        index = len(self.summary['commands'])
        record = {'command': args, 'status': 'incomplete'}
        self.summary['commands'].append(record)
        self.save()
        print('+ ' + ' '.join(args), flush=True)
        stdout = self.directory / f'{index:03}.stdout.log'
        stderr = self.directory / f'{index:03}.stderr.log'
        record.update(stdout=str(stdout), stderr=str(stderr))
        try:
            with stdout.open('w') as out, stderr.open('w') as err:
                if observe_process:
                    diagnostic = self.directory / f'{index:03}.sampling.log'
                    record['process_sampling'] = {
                        'path': str(diagnostic),
                        'role': 'external observation, not test success evidence',
                        'status': 'requested; inspect observation log for availability',
                    }
                    self.save()
                    # Do not retry execution if observation reports a failure.
                    # The sampler owns exactly one child and the original deadline.
                    result = backup_process_sampler().run(
                        args, cwd=self.root, env=self.environment, stdout=out,
                        stderr=err, timeout=timeout, diagnostic_path=diagnostic)
                else:
                    result = subprocess.run(args, cwd=self.root, env=self.environment,
                                            stdout=out, stderr=err, timeout=timeout)
        except Exception as error:
            record.update(status='timed-out' if isinstance(error, subprocess.TimeoutExpired)
                          else 'error', error=f'{type(error).__name__}: {error}')
            self.save()
            raise
        record.update(returncode=result.returncode, status='passed' if result.returncode == 0 else 'failed')
        self.save()
        if result.returncode:
            print(stderr.read_text()[-12000:], file=sys.stderr)
            raise subprocess.CalledProcessError(result.returncode, args)
        return stdout.read_text()

    def plan(self, *, shardable=False):
        policy = json.loads((self.root / PLAN_PATH).read_text())
        self.summary['head'] = self.command(['git', 'rev-parse', 'HEAD']).strip()
        event_name = os.environ.get('GITHUB_EVENT_NAME')
        if event_name == 'push':
            before = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())['before']
        elif event_name == 'workflow_dispatch':
            inputs = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text()).get('inputs', {})
            if inputs.get('scope') != 'targeted':
                raise ValueError('targeted runner requires explicit targeted scope')
            before = inputs.get('comparison_base') or self.command(
                ['git', 'rev-parse', 'HEAD^']).strip()
        else:
            raise ValueError('targeted runner requires push or manual dispatch context')
        if not isinstance(before, str) or not re.fullmatch(r'[0-9a-f]{40}', before):
            raise ValueError('missing exact comparison commit')
        # Deleted/unavailable history and force pushes require an explicit reviewed baseline.
        self.command(['git', 'merge-base', '--is-ancestor', before, 'HEAD'])
        changed = self.command(['git', 'diff', '--name-only', '--no-renames', '-z',
                                before, 'HEAD']).split('\0')
        value = select_plan(policy, [p for p in changed if p],
                            shardable=shardable or self.shard_request is not None)
        self.summary.update(base=before, policy_sha256=digest(self.root / PLAN_PATH), plan=value)
        if self.shard_request is not None:
            value = shard_tools().select(self.summary, value, self.shard_request)
            self.summary['plan'] = value
        self.save()
        return value

    def verify_shards(self, directory):
        try:
            plan = self.plan(shardable=True)
            receipts = [json.loads(path.read_text())
                        for path in sorted(directory.rglob('summary.json'))]
            result = shard_tools().verify(
                self.summary, plan, receipts, selection_evidence, test_timeouts)
            self.summary.update(status=result['status'], coverage=result)
            (self.directory / 'coverage.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(result, indent=2))
        except Exception as error:
            self.summary.update(status='failed', error=f'{type(error).__name__}: {error}')
            raise
        finally:
            self.save()

    def failure(self, stage, error, **scope):
        record = dict(scope, stage=stage, error=f'{type(error).__name__}: {error}',
                      kind='timeout' if isinstance(error, subprocess.TimeoutExpired)
                      else type(error).__name__)
        self.summary['failed'].append(record)
        self.save()

    def block_after_timeout(self, **scope):
        # subprocess.run reaps its direct child, not every descendant. Do not reuse
        # shared build/fixture state after a timeout with unproven process cleanup.
        self.summary['blocked'].append(dict(scope, reason='timeout descendant cleanup unproven'))
        self.save()

    def rust(self, target):
        stage = 'compile'
        try:
            build = self.directory / 'build'
            mode = target.get('mode', 'test')
            command = cargo_command(target, build)
            output = self.command(command, 1800)
            stage = 'artifact'
            artifact = current_artifact(output, target, self.root)
            if mode == 'check':
                files = []
                for filename in artifact['filenames']:
                    path = Path(filename).resolve()
                    if not path.is_relative_to(build) or not path.is_file():
                        raise ValueError('compile artifact outside current build')
                    files.append({'path': str(path), 'sha256': digest(path)})
                if not files:
                    raise ValueError('compile check emitted no target artifact')
                self.summary['artifacts'].append({'configuration': target, 'files': files})
                self.summary['compiled'].append(target)
                self.save()
                return
            binary = Path(artifact['executable']).resolve()
            if not binary.is_relative_to(build) or not binary.is_file():
                raise ValueError('test executable outside current build')
            sha = digest(binary)
            stage = 'inventory'
            names = inventory(self.command([str(binary), '--list', '--format', 'terse']))
            ignored = inventory(self.command([str(binary), '--list', '--ignored', '--format', 'terse']))
            stage = 'selection'
            selections = selection_evidence(names, ignored, target)
            timeouts = test_timeouts(selections)
            selected = sorted(timeouts)
            evidence = {'configuration': target, 'package': target['package'], 'path': str(binary), 'sha256': sha,
                        'inventory': names, 'ignored': ignored, 'selected': selected,
                        'selections': selections, 'test_timeouts': timeouts,
                        'unexecuted': sorted(set(names) - set(selected))}
            self.summary['artifacts'].append(evidence)
            self.save()
        except Exception as error:
            self.failure(stage, error, configuration=target)
            if isinstance(error, subprocess.TimeoutExpired):
                self.block_after_timeout(configuration=target, stage=stage)
                raise
            self.summary['skipped'].append({'configuration': target, 'stage': stage,
                                            'reason': 'target prerequisite failed'})
            self.save()
            return
        for index, name in enumerate(selected):
            try:
                if digest(binary) != sha:
                    raise ValueError('test executable changed after build')
            except Exception as error:
                self.failure('integrity', error, configuration=target, name=name)
                self.summary['skipped'].append({'configuration': target,
                                                'names': selected[index:],
                                                'reason': 'test executable integrity failed'})
                self.save()
                return
            try:
                arguments = [str(binary), '--exact', name, '--test-threads=1']
                if observe_backup_process(target, name):
                    output = self.command(arguments, timeouts[name], observe_process=True)
                else:
                    output = self.command(arguments, timeouts[name])
                passed_test(output, len(names))
            except Exception as error:
                self.failure('test', error, configuration=target, name=name)
                if isinstance(error, subprocess.TimeoutExpired):
                    if selected[index + 1:]:
                        self.block_after_timeout(configuration=target, names=selected[index + 1:])
                    raise
                continue
            self.summary['passed'].append({'configuration': target, 'name': name})
            self.save()

    def python(self, name, timeout=120):
        # Count first: unittest's successful zero-test exit cannot satisfy a plan.
        if not (self.root / 'test/ci' / (name + '.py')).is_file():
            raise ValueError(f'missing Python module: {name}')
        sys.path.insert(0, str(self.root / 'test/ci'))
        suite = unittest.defaultTestLoader.loadTestsFromName(name)
        if suite.countTestCases() == 0:
            raise ValueError(f'empty Python module: {name}')
        index = len(self.summary['commands'])
        self.command([sys.executable, '-m', 'unittest', 'discover',
                      '-s', 'test/ci', '-p', name + '.py', '-v'], timeout)
        passed_python((self.directory / f'{index:03}.stderr.log').read_text(),
                      suite.countTestCases())
        self.summary['passed'].append({'python_module': name, 'tests': suite.countTestCases(),
                                       'timeout_seconds': timeout})
        self.save()

    def execute(self, action):
        remaining_python, remaining_targets = [], []
        try:
            plan = self.plan(shardable=True) if action == 'matrix' else self.plan()
            if action == 'matrix':
                matrix = shard_tools().matrix(self.summary, plan)
                self.summary['matrix'] = matrix
                self.summary['status'] = 'planned'
                with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
                    stream.write('matrix=' + json.dumps(matrix, separators=(',', ':')) + '\n')
                return
            if action == 'plan':
                self.summary['status'] = 'planned'
                return
            lock = digest(self.root / 'Cargo.lock')
            self.summary['lockfile_sha256'] = lock
            remaining_python = list(plan['python_tests'])
            remaining_targets = list(plan['rust_targets'])
            for index, name in enumerate(plan['python_tests']):
                remaining_python = plan['python_tests'][index + 1:]
                try:
                    self.python(name, plan.get('python_timeouts', {}).get(name, 120))
                except Exception as error:
                    self.failure('python', error, python_module=name)
                    if isinstance(error, subprocess.TimeoutExpired):
                        raise
            cross_ready = True
            if any(t.get('compile_target') for t in plan['rust_targets']):
                try:
                    self.command(['rustup', 'target', 'add', 'wasm32-unknown-unknown'], 300)
                except Exception as error:
                    self.failure('toolchain', error, compile_target='wasm32-unknown-unknown')
                    if isinstance(error, subprocess.TimeoutExpired):
                        raise
                    cross_ready = False
            for index, target in enumerate(plan['rust_targets']):
                remaining_targets = plan['rust_targets'][index + 1:]
                if target.get('compile_target') and not cross_ready:
                    self.summary['skipped'].append({'configuration': target,
                                                    'reason': 'toolchain setup failed'})
                    self.save()
                    continue
                self.rust(target)
            if digest(self.root / 'Cargo.lock') != lock:
                raise ValueError('Cargo.lock changed during targeted run')
            if self.summary['failed']:
                raise RuntimeError('selected targeted work failed; see failed and skipped evidence')
            self.summary['status'] = 'selected-tests-passed'
        except Exception as error:
            if (isinstance(error, subprocess.TimeoutExpired)
                    and (remaining_python or remaining_targets)):
                self.block_after_timeout(python_modules=remaining_python,
                                         rust_targets=remaining_targets)
            self.summary.update(status='failed', error=f'{type(error).__name__}: {error}')
            raise
        finally:
            self.save()
            if os.environ.get('GITHUB_STEP_SUMMARY'):
                with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as stream:
                    stream.write(step_summary(self.summary))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('plan', 'run', 'matrix', 'verify-shards'))
    parser.add_argument('--receipts', type=Path)
    args = parser.parse_args()
    if args.action == 'verify-shards':
        if args.receipts is None:
            parser.error('verify-shards requires --receipts')
        Run(ROOT).verify_shards(args.receipts)
    else:
        request = os.environ.get('ZXS_TARGETED_SHARD')
        Run(ROOT, shard_request=json.loads(request) if request else None).execute(args.action)
