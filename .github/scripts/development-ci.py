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
SQLITE_MAINTENANCE = 'sqlite-upgrade.yml'
# An exact, removable manual maintenance exception, not a CI/full-run override.
SQLITE_WORKFLOW = """# Temporary, manual, fixed-source patch preparation. Never publishes a commit.
name: SQLite 0.39.1 upgrade patch
on:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: sqlite-0391-maintenance
  cancel-in-progress: false

jobs:
  prepare:
    if: ${{ github.repository == 'isarmg/zxs' && github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main' }}
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    env:
      CARGO_NET_OFFLINE: 'true'
      PYTHONDONTWRITEBYTECODE: '1'
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.sha }}
          fetch-depth: 1
          persist-credentials: false
      - name: Initialize isolated Cargo home
        run: bash .github/scripts/initialize-cargo-home.sh
      - name: Select the repository Rust toolchain for offline metadata only
        run: rustup show active-toolchain
      - name: Prepare fixed SQLite source upgrade, never compile or publish
        run: python3 .github/scripts/sqlite-upgrade-maintenance.py
      - name: Retain source evidence, including an explicit failed stage
        if: ${{ always() }}
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: sqlite-0391-source-evidence-${{ github.sha }}-${{ github.run_attempt }}
          path: ${{ runner.temp }}/zxs-sqlite-0391/evidence/
          if-no-files-found: error
          retention-days: 7
      - name: Retain complete patch only after all source gates succeed
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: sqlite-0391-upgrade-patch-${{ github.sha }}-${{ github.run_attempt }}
          path: ${{ runner.temp }}/zxs-sqlite-0391/candidate.patch
          if-no-files-found: error
          retention-days: 7
"""

PAUSE = '    if: ${{ false }}\n'
PUBLISH_CONDITION = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"
HEADER = '# Development short CI: long checks are paused, not passed. See docs/ci/development-short-ci.md.\n'


def development_workflow(name, original):
    """The complete original jobs remain; only these reviewed edits are allowed."""
    source = HEADER + original
    # Recurring checks are dispatched externally, never by GitHub schedule.
    if name == 'ci.yml':
        source = source.replace('on:\n  push:\n  pull_request:\n',
                                'on:\n  # Manual dispatch only; GitHub automatic triggers remain paused.\n')
        # A later manual check must not cancel one already in progress.
        source = source.replace('  cancel-in-progress: true\n',
                                '  cancel-in-progress: false\n', 1)
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
    if names not in (set(SNAPSHOT_SHA256), set(SNAPSHOT_SHA256) | {SQLITE_MAINTENANCE}):
        raise ValueError('development workflow entrypoints changed')
    if SQLITE_MAINTENANCE in names and (live / SQLITE_MAINTENANCE).read_text() != SQLITE_WORKFLOW:
        raise ValueError('development policy: SQLite maintenance scope changed')
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
