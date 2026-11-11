#!/usr/bin/env python3
"""Execute a bounded, reviewed change plan against current Cargo test inventories."""
import argparse
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
PLAN_PATH = '.github/ci/targeted-tests.json'


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def select_plan(policy, changed):
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
    targets, python_tests = [], set()
    for name in sorted(selected):
        group = groups[name]
        if set(group) != {'reason', 'python_tests', 'rust_targets'} or not group['reason']:
            raise ValueError('invalid target group')
        python_tests.update(group['python_tests'])
        targets.extend(group['rust_targets'])
    if len(python_tests) > 8 or any(not re.fullmatch(r'test_[a-z_]+', n) for n in python_tests):
        raise ValueError('invalid Python modules')
    if len(targets) > 4 or not (targets or python_tests):
        raise ValueError('target selection must be bounded and nonempty')
    packages = []
    for target in targets:
        if set(target) != {'package', 'manifest', 'prefixes', 'max_tests'}:
            raise ValueError('invalid Rust target fields')
        if not re.fullmatch(r'[a-z][a-z0-9-]+', target['package']):
            raise ValueError('invalid package name')
        if not re.fullmatch(r'(src|tools)/[a-z0-9_-]+/Cargo.toml', target['manifest']):
            raise ValueError('invalid manifest path')
        prefixes = target['prefixes']
        if (not isinstance(prefixes, list) or not prefixes or len(prefixes) > 16
                or any(not re.fullmatch(r'(?:[a-z_][a-z0-9_]*::)+', p) for p in prefixes)
                or len(set(prefixes)) != len(prefixes)):
            raise ValueError('select nonempty module prefixes, not fixed test names')
        if type(target['max_tests']) is not int or not 1 <= target['max_tests'] <= 160:
            raise ValueError('test count budget must be between 1 and 160')
        packages.append(target['package'])
    if len(set(packages)) != len(packages):
        raise ValueError('duplicate Rust package')
    return {'groups': sorted(selected), 'changed_paths': sorted(changed),
            'python_tests': sorted(python_tests), 'rust_targets': targets,
            'unexecuted': policy['unexecuted']}


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
    if not set(ignored) <= set(names):
        raise ValueError('ignored inventory is outside complete inventory')
    eligible = set(names) - set(ignored)
    selected = set()
    for prefix in target['prefixes']:
        matching = {name for name in eligible if name.startswith(prefix)}
        if not matching:
            raise ValueError(f'empty selected module: {prefix}')
        selected.update(matching)
    if len(selected) > target['max_tests']:
        raise ValueError('selected inventory exceeds reviewed budget')
    return sorted(selected)


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


class Run:
    def __init__(self, root):
        self.root = Path(root)
        self.directory = self.root / 'target/targeted-ci'
        self.directory.mkdir(parents=True, exist_ok=True)
        self.summary = {'status': 'incomplete', 'commands': [], 'artifacts': [], 'passed': []}
        self.environment = dict(os.environ, CARGO_INCREMENTAL='0')
        # Disk admission needs a real filesystem rather than the runner's tmpfs.
        fixtures = self.directory / 'fixtures'
        fixtures.mkdir(exist_ok=True)
        self.environment['TMPDIR'] = str(fixtures)
        self.save()

    def save(self):
        (self.directory / 'summary.json').write_text(json.dumps(self.summary, indent=2) + '\n')

    def command(self, args, timeout=60):
        index = len(self.summary['commands'])
        record = {'command': args, 'status': 'incomplete'}
        self.summary['commands'].append(record)
        self.save()
        print('+ ' + ' '.join(args), flush=True)
        stdout = self.directory / f'{index:03}.stdout.log'
        stderr = self.directory / f'{index:03}.stderr.log'
        with stdout.open('w') as out, stderr.open('w') as err:
            result = subprocess.run(args, cwd=self.root, env=self.environment,
                                    stdout=out, stderr=err, timeout=timeout)
        record.update(returncode=result.returncode, status='passed' if result.returncode == 0 else 'failed')
        self.save()
        if result.returncode:
            print(stderr.read_text()[-12000:], file=sys.stderr)
            raise subprocess.CalledProcessError(result.returncode, args)
        return stdout.read_text()

    def plan(self):
        policy = json.loads((self.root / PLAN_PATH).read_text())
        self.summary['head'] = self.command(['git', 'rev-parse', 'HEAD']).strip()
        event_name = os.environ.get('GITHUB_EVENT_NAME')
        if event_name == 'push':
            before = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())['before']
        elif event_name == 'workflow_dispatch':
            before = self.command(['git', 'rev-parse', 'HEAD^']).strip()
        else:
            raise ValueError('targeted runner requires push or manual dispatch context')
        if not isinstance(before, str) or not re.fullmatch(r'[0-9a-f]{40}', before):
            raise ValueError('missing exact comparison commit')
        # Deleted/unavailable history and force pushes require an explicit reviewed baseline.
        self.command(['git', 'merge-base', '--is-ancestor', before, 'HEAD'])
        changed = self.command(['git', 'diff', '--name-only', '--no-renames', '-z',
                                before, 'HEAD']).split('\0')
        value = select_plan(policy, [p for p in changed if p])
        self.summary.update(base=before, policy_sha256=digest(self.root / PLAN_PATH), plan=value)
        self.save()
        return value

    def rust(self, target):
        build = self.directory / 'build'
        output = self.command(['cargo', 'test', '--locked', '--offline', '-p', target['package'],
                               '--lib', '--no-run', '--message-format=json',
                               '--target-dir', str(build)], 1800)
        artifacts, finished = [], []
        for line in output.splitlines():
            item = json.loads(line)
            if item.get('reason') == 'build-finished':
                finished.append(item['success'])
            if (item.get('reason') == 'compiler-artifact'
                    and item['profile']['test'] and item['target']['kind'] == ['lib']
                    and Path(item['manifest_path']).resolve() == self.root / target['manifest']):
                artifacts.append(item)
        if finished != [True] or len(artifacts) != 1:
            raise ValueError('missing unique current test artifact')
        binary = Path(artifacts[0]['executable']).resolve()
        if not binary.is_relative_to(build) or not binary.is_file():
            raise ValueError('test executable outside current build')
        sha = digest(binary)
        names = inventory(self.command([str(binary), '--list', '--format', 'terse']))
        ignored = inventory(self.command([str(binary), '--list', '--ignored', '--format', 'terse']))
        selected = selected_tests(names, ignored, target)
        evidence = {'package': target['package'], 'path': str(binary), 'sha256': sha,
                    'inventory': names, 'ignored': ignored, 'selected': selected,
                    'unexecuted': sorted(set(names) - set(selected))}
        self.summary['artifacts'].append(evidence)
        self.save()
        for name in selected:
            if digest(binary) != sha:
                raise ValueError('test executable changed after build')
            output = self.command([str(binary), '--exact', name, '--test-threads=1'], 300)
            passed_test(output, len(names))
            self.summary['passed'].append({'package': target['package'], 'name': name})
            self.save()

    def execute(self, action):
        try:
            plan = self.plan()
            if action == 'plan':
                self.summary['status'] = 'planned'
                return
            lock = digest(self.root / 'Cargo.lock')
            self.summary['lockfile_sha256'] = lock
            for name in plan['python_tests']:
                # Count first: unittest's successful zero-test exit cannot satisfy a plan.
                if not (self.root / 'test/ci' / (name + '.py')).is_file():
                    raise ValueError(f'missing Python module: {name}')
                sys.path.insert(0, str(self.root / 'test/ci'))
                suite = unittest.defaultTestLoader.loadTestsFromName(name)
                if suite.countTestCases() == 0:
                    raise ValueError(f'empty Python module: {name}')
                index = len(self.summary['commands'])
                self.command([sys.executable, '-m', 'unittest', 'discover',
                              '-s', 'test/ci', '-p', name + '.py', '-v'], 120)
                passed_python((self.directory / f'{index:03}.stderr.log').read_text(),
                              suite.countTestCases())
                self.summary['passed'].append({'python_module': name, 'tests': suite.countTestCases()})
            for target in plan['rust_targets']:
                self.rust(target)
            if digest(self.root / 'Cargo.lock') != lock:
                raise ValueError('Cargo.lock changed during targeted run')
            self.summary['status'] = 'selected-tests-passed'
        except Exception as error:
            self.summary.update(status='failed', error=f'{type(error).__name__}: {error}')
            raise
        finally:
            self.save()
            if os.environ.get('GITHUB_STEP_SUMMARY'):
                with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as stream:
                    stream.write('### Targeted CI scope and result\n\n```json\n'
                                 + json.dumps(self.summary, indent=2) + '\n```\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('plan', 'run'))
    Run(ROOT).execute(parser.parse_args().action)
