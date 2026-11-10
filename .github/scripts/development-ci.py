#!/usr/bin/env python3
"""Reviewed complete CI policy; automatic main pushes without Release restoration.

The archived workflows are restoration recipes, not executable CI or evidence.
Only the exact reviewed projection below is allowed in the active directory.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FULL_SUITES = ('rust-sources', 'quality', 'security', 'workspace',
               'foundation-platform', 'foundation-crypto', 'foundation-data',
               'foundation-language', 'foundation-async', 'web', 'contracts',
               'build-tools', 'smb-protocol')
PAUSED_JOBS = {
    'ci.yml': (),
    'release.yml': ('version', 'verify', 'linux-web', 'smb-sdk', 'assemble', 'publish'),
    'smb-sdk.yml': (),
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

SQLITE_IMPORT = 'sqlite-import.yml'
# Separately authorized, exact artifact import; no commit/ref publication path.
SQLITE_IMPORT_WORKFLOW = """# Temporary, fixed-artifact import. Creates Git blobs/tree only, never a commit/ref.
name: SQLite 0.39.1 artifact import
on:
  workflow_dispatch:

permissions: {}

concurrency:
  group: sqlite-0391-maintenance
  cancel-in-progress: false

jobs:
  import:
    if: ${{ github.repository == 'isarmg/zxs' && github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main' }}
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    permissions:
      actions: read
      contents: write
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
      - name: Import only the approved SQLite artifact into unreferenced Git objects
        env:
          GH_TOKEN: ${{ github.token }}
        run: python3 .github/scripts/sqlite-import-maintenance.py
      - name: Retain import evidence, including any failed stage
        if: ${{ always() }}
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: sqlite-0391-import-evidence-${{ github.sha }}-${{ github.run_attempt }}
          path: ${{ runner.temp }}/zxs-sqlite-import/result.json
          if-no-files-found: error
          retention-days: 7
"""

PAUSE = '    if: ${{ false }}\n'
PUBLISH_CONDITION = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"
HEADER = '# Development short CI: long checks are paused, not passed. See docs/ci/development-short-ci.md.\n'
FULL_HEADER = '# Full CI: main pushes and manual runs; pull requests, schedules and Release remain paused.\n'


def development_workflow(name, original):
    """The complete original jobs remain; only these reviewed edits are allowed."""
    source = (HEADER if name == 'release.yml' else FULL_HEADER) + original
    # Enable full CI for main pushes; retain manual dispatch, no schedule or PR trigger.
    if name == 'ci.yml':
        source = source.replace('on:\n  push:\n  pull_request:\n',
                                'on:\n  push:\n    branches: [main]\n')
        # Each run has its own group so later dispatches cannot replace a queued run.
        source = source.replace(
            '  group: ci-${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}\n',
            '  group: ci-${{ github.workflow }}-${{ github.sha }}-${{ github.run_id }}\n', 1)
        source = source.replace('  cancel-in-progress: true\n',
                                '  cancel-in-progress: false\n', 1)
    elif name == 'release.yml':
        source = source.replace("on:\n  push:\n    tags: ['v*']\n",
                                'on:\n  # Automatic runs paused by user request; restore explicitly before re-enabling.\n')
    if name == 'release.yml':
        source = source.replace('    timeout-minutes: 10\n', '    timeout-minutes: 5\n', 1)
    if name == 'ci.yml':
        source = source.replace('# Missing older push bases safely select the full matrix.',
                                '# Every main push and manual run executes the complete matrix without reuse.')
        source = source.replace('Select evidenced CI suites', 'Select all CI suites without reuse')
        source = source.replace('python3 .github/scripts/select-ci-suites.py',
                                'python3 .github/scripts/development-ci.py select')
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
    manual = {SQLITE_MAINTENANCE: SQLITE_WORKFLOW, SQLITE_IMPORT: SQLITE_IMPORT_WORKFLOW}
    required = set(SNAPSHOT_SHA256)
    if not required <= names or not names <= required | set(manual):
        raise ValueError('development workflow entrypoints changed')
    for name, expected in manual.items():
        if name in names and (live / name).read_text() != expected:
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
        'mode': 'full-ci',
        'automatic_triggers': True,
        'push_branches': ['main'],
        'suites': list(FULL_SUITES),
        'paused_suites': [],
        'paused_jobs': {name: list(jobs) for name, jobs in PAUSED_JOBS.items()},
        'reused': [],
        'reason': 'Full CI on main pushes and manual dispatch; Release remains paused. Selection is not execution evidence.',
        'static_minutes': 10,
        'suite_minutes': {suite: 120 if suite == 'workspace' else
                         60 if suite == 'foundation-crypto' else 30
                         for suite in FULL_SUITES},
        'timing_scope': 'execution budgets; excludes platform queue and runner teardown',
    }


def check_receipt(value):
    # Validate the actual emitted matrix, not just an unused inventory constant.
    if value != receipt():
        raise ValueError('full CI selection must be the exact thirteen suites without reuse')
    if len(value['suites']) != 13 or len(set(value['suites'])) != 13:
        raise ValueError('full CI selection must contain thirteen unique suites')


def emit(value, *, output=None, summary=None):
    check_receipt(value)
    print(json.dumps(value, indent=2, sort_keys=True))
    if output is not None:
        with open(output, 'a') as stream:
            stream.write('suites=' + json.dumps(value['suites'], separators=(',', ':')) + '\n')
    if summary is not None:
        with open(summary, 'a') as stream:
            stream.write('### Full CI: selected, not yet passed; Release paused\n\n```json\n'
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
