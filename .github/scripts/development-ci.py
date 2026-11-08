#!/usr/bin/env python3
"""Fixed development CI policy; there is no event/input/env full-run override.

The archived workflows are restoration recipes, not executable CI or evidence.
Only the exact reviewed projection below is allowed in the active directory.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SHORT_SUITES = ('quality', 'security', 'web', 'contracts', 'build-tools',
                'smb-protocol', 'foundation-platform')
PAUSED_SUITES = ('rust-sources', 'workspace', 'foundation-crypto', 'foundation-data',
                 'foundation-language', 'foundation-async')
PAUSED_JOBS = {
    'ci.yml': ('web-production', 'browser-smoke', 'smb-sdk', 'phase-one-core'),
    'release.yml': ('version', 'verify', 'linux-web', 'smb-sdk', 'assemble', 'publish'),
    'smb-sdk.yml': ('smb-sdk',),
}
SNAPSHOT_SHA256 = {
    'ci.yml': 'c076365d617c9cb94a97c8dfc8c2c5248af7ebae08d510d8209036f781ef177d',
    'release.yml': '121aecec6acdcd34f2024b6e6e96a27c82c91d981c69c80fbb5f3bf5df4971ce',
    'smb-sdk.yml': '795075fb6a8adfe3ea88df18a84e0e631c9ed06b5f8a2ed611fda194e31cd062',
}
PAUSE = '    if: ${{ false }}\n'
PUBLISH_CONDITION = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"
HEADER = '# Development short CI: long checks are paused, not passed. See docs/ci/development-short-ci.md.\n'


def development_workflow(name, original):
    """The complete original jobs remain; only these reviewed edits are allowed."""
    source = HEADER + original
    # Preserve jobs and history while disabling every automatic entrypoint.
    if name == 'ci.yml':
        source = source.replace('on:\n  push:\n  pull_request:\n',
                                'on:\n  # Automatic runs paused by user request; restore explicitly before re-enabling.\n')
    elif name == 'release.yml':
        source = source.replace("on:\n  push:\n    tags: ['v*']\n",
                                'on:\n  # Automatic runs paused by user request; restore explicitly before re-enabling.\n')
    if name in ('ci.yml', 'release.yml'):
        source = source.replace('    timeout-minutes: 10\n', '    timeout-minutes: 5\n', 1)
    if name == 'ci.yml':
        source = source.replace('# Missing older push bases safely select the full matrix.',
                                '# Development selection is fixed even without prior evidence.')
        source = source.replace('Select evidenced CI suites', 'Select fixed development CI suites')
        source = source.replace('python3 .github/scripts/select-ci-suites.py',
                                'python3 .github/scripts/development-ci.py select')
        start = source.index('    # Cold crypto runs')
        end = source.index('    strategy:\n', start)
        source = source[:start] + ('    # Static (5) + each parallel short suite (10) <= 15 minutes.\n'
                                  '    # Timeout is a failure; no partial or empty success.\n'
                                  '    timeout-minutes: 10\n') + source[end:]
    for job in PAUSED_JOBS[name]:
        if job == 'publish':
            # Preserve the tag-only publication restriction even while impossible to run.
            source = source.replace('    if: ' + PUBLISH_CONDITION + '\n',
                                    '    if: ${{ false && (' + PUBLISH_CONDITION + ') }}\n')
        else:
            source = source.replace(f'  {job}:\n', f'  {job}:\n' + PAUSE, 1)
    return source


def check_workflows(root):
    """Fail on any unreviewed entrypoint, override, ignored failure or recipe drift."""
    live = root / '.github/workflows'
    names = {path.name for path in live.iterdir() if path.suffix in {'.yml', '.yaml'}}
    if names != set(SNAPSHOT_SHA256):
        raise ValueError('development workflow entrypoints changed')
    for name, expected_hash in SNAPSHOT_SHA256.items():
        original = (root / '.github/ci/full-workflows' / name).read_bytes()
        if hashlib.sha256(original).hexdigest() != expected_hash:
            raise ValueError(f'retained full recipe changed: {name}')
        expected = development_workflow(name, original.decode('utf-8'))
        if (live / name).read_text() != expected:
            raise ValueError(f'development policy or retained job body changed: {name}')


def receipt():
    return {
        'mode': 'development-short-ci',
        'automatic_triggers': False,
        'suites': list(SHORT_SUITES),
        'paused_suites': list(PAUSED_SUITES),
        'paused_jobs': {name: list(jobs) for name, jobs in PAUSED_JOBS.items()},
        'reused': [],
        'reason': 'Long checks are paused until explicit restoration after all tasks finish; not passed or reused.',
        'static_minutes': 5,
        'suite_minutes': 10,
        'dependency_path_minutes': 15,
        'timing_scope': 'execution dependencies; excludes platform queue and runner teardown',
    }


def check_receipt(value):
    # Validate the actual emitted matrix, not just an unused inventory constant.
    if value != receipt():
        raise ValueError('development selection must be the exact seven suites and explicit pause receipt')
    if len(value['suites']) != 7 or len(set(value['suites'])) != 7:
        raise ValueError('development selection must contain seven unique suites')
    if set(value['suites']) & set(value['paused_suites']):
        raise ValueError('paused suite leaked into development selection')


def emit(value, *, output=None, summary=None):
    check_receipt(value)
    print(json.dumps(value, indent=2, sort_keys=True))
    if output is not None:
        with open(output, 'a') as stream:
            stream.write('suites=' + json.dumps(value['suites'], separators=(',', ':')) + '\n')
    if summary is not None:
        with open(summary, 'a') as stream:
            stream.write('### Development CI: long checks paused, not passed\n\n```json\n'
                         + json.dumps(value, indent=2) + '\n```\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('check', 'select'))
    args = parser.parse_args()
    check_workflows(ROOT)
    value = receipt()
    check_receipt(value)
    emit(value, output=os.environ['GITHUB_OUTPUT'] if args.action == 'select' else None,
         summary=os.environ.get('GITHUB_STEP_SUMMARY'))


if __name__ == '__main__':
    main()
