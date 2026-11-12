"""Partition admitted selections, then prove same-commit complete shard execution."""
import hashlib
import json
from pathlib import PurePosixPath
import re

MAX_SHARDS = 16
RUST_PER_SHARD = 16
PYTHON_PER_SHARD = 8


def fingerprint(value):
    encoded = json.dumps(value, sort_keys=True, separators=(',', ':')).encode()
    return hashlib.sha256(encoded).hexdigest()


def partition(plan):
    rust, python = plan['rust_targets'], plan['python_tests']
    batches, pending = [], []
    for target in rust:
        # Scheduling is separate from Cargo identity. An isolated selection
        # carries every overlapping selection of that build into the same job.
        isolated = any(selection.get('isolate', False)
                       for selection in target.get('selections', [target]))
        if isolated:
            if pending:
                batches.append(pending)
                pending = []
            batches.append([target])
        else:
            pending.append(target)
            if len(pending) == RUST_PER_SHARD:
                batches.append(pending)
                pending = []
    if pending:
        batches.append(pending)
    count = max(len(batches),
                (len(python) + PYTHON_PER_SHARD - 1) // PYTHON_PER_SHARD)
    if not 1 <= count <= MAX_SHARDS:
        raise ValueError('complete targeted selection exceeds reviewed shard budget')
    shards = []
    for index in range(count):
        modules = python[index * PYTHON_PER_SHARD:(index + 1) * PYTHON_PER_SHARD]
        shard = dict(plan)
        shard['rust_targets'] = batches[index] if index < len(batches) else []
        shard['python_tests'] = modules
        shard['python_timeouts'] = {name: plan['python_timeouts'][name] for name in modules}
        shard['python_selections'] = [selection for selection in plan['python_selections']
                                      if selection['module'] in modules]
        shards.append(shard)
    return shards


def identity(summary, plan):
    return {key: summary[key] for key in ('head', 'base', 'policy_sha256')} | {
        'plan_sha256': fingerprint(plan), 'count': len(partition(plan)),
    }


def matrix(summary, plan):
    binding = identity(summary, plan)
    return {'include': [dict(binding, index=index) for index in range(binding['count'])]}


def select(summary, plan, request):
    binding = identity(summary, plan)
    if (set(request) != set(binding) | {'index'}
            or any(request[key] != value for key, value in binding.items())
            or type(request.get('count')) is not int
            or type(request['index']) is not int
            or not 0 <= request['index'] < binding['count']):
        raise ValueError('shard request does not match the current complete selection')
    summary['shard'] = dict(request)
    return partition(plan)[request['index']]


def current_file(record):
    path = record.get('path')
    checksum = record.get('sha256')
    if (not isinstance(path, str) or not PurePosixPath(path).is_absolute()
            or '..' in PurePosixPath(path).parts or '/target/targeted-ci/build/' not in path
            or not isinstance(checksum, str) or not re.fullmatch(r'[0-9a-f]{64}', checksum)):
        raise ValueError('incomplete current-build artifact identity')


def verify_execution(receipt, plan, selection_evidence, test_timeouts):
    if (receipt.get('status') != 'selected-tests-passed'
            or receipt.get('plan') != plan
            or any(receipt.get(key) != [] for key in ('failed', 'skipped', 'blocked'))):
        raise ValueError('shard is incomplete or differs from its admitted plan')
    passed = receipt.get('passed', [])
    python = [item for item in passed if 'python_module' in item]
    if (sorted(item['python_module'] for item in python) != plan['python_tests']
            or any(type(item.get('tests')) is not int or item['tests'] <= 0
                   or item.get('timeout_seconds') != plan['python_timeouts'][item['python_module']]
                   for item in python)):
        raise ValueError('Python execution does not cover its exact shard selection')
    checks = [target for target in plan['rust_targets'] if target.get('mode') == 'check']
    if receipt.get('compiled') != checks:
        raise ValueError('compile checks do not cover their exact shard selection')
    artifacts = receipt.get('artifacts', [])
    if [item.get('configuration') for item in artifacts] != plan['rust_targets']:
        raise ValueError('current artifacts do not cover their exact shard selection')
    expected = []
    for target, artifact in zip(plan['rust_targets'], artifacts):
        if target.get('mode') == 'check':
            files = artifact.get('files')
            if not isinstance(files, list) or not files:
                raise ValueError('missing compile artifact evidence')
            for record in files:
                current_file(record)
            if len({record['path'] for record in files}) != len(files):
                raise ValueError('duplicate compile artifact evidence')
            continue
        current_file(artifact)
        if artifact.get('package') != target['package']:
            raise ValueError('test artifact package differs from selected build')
        selections = selection_evidence(artifact['inventory'], artifact['ignored'], target)
        timeouts = test_timeouts(selections)
        names = sorted(timeouts)
        if (artifact.get('selections') != selections or artifact.get('test_timeouts') != timeouts
                or artifact.get('selected') != names
                or artifact.get('unexecuted') != sorted(set(artifact['inventory']) - set(names))):
            raise ValueError('shard inventory or budget differs from its selected tests')
        expected.extend({'configuration': target, 'name': name} for name in names)
    if [item for item in passed if 'python_module' not in item] != expected:
        raise ValueError('Rust execution does not cover its exact shard inventory')


def verify(summary, plan, receipts, selection_evidence, test_timeouts):
    binding = identity(summary, plan)
    shards = partition(plan)
    if len(receipts) != len(shards):
        raise ValueError('missing or additional targeted shard receipts')
    seen = set()
    for receipt in receipts:
        request = receipt.get('shard', {})
        # Reuse the same boundary as execution; no receipt may choose its own scope.
        selected = select(dict(summary), plan, request)
        index = request['index']
        if index in seen:
            raise ValueError('duplicate targeted shard receipt')
        seen.add(index)
        if any(receipt.get(key) != binding[key] for key in ('head', 'base', 'policy_sha256')):
            raise ValueError('targeted receipt belongs to another source or policy')
        verify_execution(receipt, selected, selection_evidence, test_timeouts)
    if seen != set(range(len(shards))):
        raise ValueError('incomplete targeted shard coverage')
    return dict(binding, status='all-selected-shards-passed', shards=sorted(seen))
