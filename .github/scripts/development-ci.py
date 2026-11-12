#!/usr/bin/env python3
"""Targeted automatic CI and explicitly requested full manual CI.

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

BACKUP_PROFILER = 'backup-profiler-capability.yml'
# Exact manual diagnostics only; no project workload or general manual exemption.
BACKUP_PROFILER_WORKFLOW = r"""name: Backup profiler capability

on:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: backup-profiler-capability-${{ github.sha }}
  cancel-in-progress: false

jobs:
  capability:
    runs-on: ubuntu-latest
    timeout-minutes: 2
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.sha }}
          persist-credentials: false
      - name: Probe existing tools with fixed non-business children
        shell: bash
        run: |
          set -euo pipefail
          test "$(git rev-parse HEAD)" = "$GITHUB_SHA"
          python3 .github/scripts/backup-profiler-capability.py \
            --head "$GITHUB_SHA" \
            --output "$RUNNER_TEMP/backup-profiler-capability.json"
      - name: Retain capability evidence, not project test results
        if: always()
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: backup-profiler-capability-${{ github.sha }}
          path: ${{ runner.temp }}/backup-profiler-capability.json
          if-no-files-found: error
          retention-days: 7
"""

PAUSE = '    if: ${{ false }}\n'
PUBLISH_CONDITION = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"
HEADER = '# Development short CI: long checks are paused, not passed. See docs/ci/development-short-ci.md.\n'
FULL_HEADER = '# Full CI: main pushes and manual runs; pull requests, schedules and Release remain paused.\n'


WORKSPACE_SHARDS = tuple(f'tests-{i}' for i in range(8)) + (
    'checks-docs', 'io-0', 'io-1', 'io-2', 'ignored')
WORKSPACE_COVERAGE_JOB = "\n  workspace-coverage:\n    name: Complete workspace shard coverage\n    needs: [static, tests]\n    if: always() && needs.static.result == 'success'\n    runs-on: ubuntu-24.04\n    timeout-minutes: 10\n    steps:\n      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with:\n          persist-credentials: false\n      - name: Fetch every workspace receipt from this commit\n        uses: actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333 # v8.0.2\n        with:\n          pattern: workspace-shard-*-${{ github.sha }}\n          path: target/workspace-shards/collected\n      - name: Prove complete non-overlapping workspace execution\n        run: python3 .github/scripts/check-workspace-shards.py verify target/workspace-shards/collected\n"
WORKSPACE_UPLOAD = "      - name: Retain workspace shard inventory and execution evidence\n        if: always() && matrix.suite == 'workspace'\n        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2\n        with:\n          name: workspace-shard-${{ matrix.shard }}-${{ github.sha }}\n          path: target/workspace-shards/${{ matrix.shard }}/\n          if-no-files-found: error\n          retention-days: 7\n"


def execution_matrix():
    return [dict(suite=suite, shard=shard)
            for suite in FULL_SUITES
            for shard in (WORKSPACE_SHARDS if suite == 'workspace' else ('single',))]


def workspace_projection(source):
    source = source.replace('      suites: ${{ steps.suites.outputs.suites }}\n',
                            '      suites: ${{ steps.suites.outputs.suites }}\n'
                            '      matrix: ${{ steps.suites.outputs.matrix }}\n', 1)
    source = source.replace('    name: Tests (${{ matrix.suite }})',
                            '    name: Tests (${{ matrix.suite }}, ${{ matrix.shard }})', 1)
    source = source.replace('        suite: ${{ fromJSON(needs.static.outputs.suites) }}',
                            '        include: ${{ fromJSON(needs.static.outputs.matrix) }}', 1)
    source = source.replace("      ZXS_REQUIRE_BROWSER: '1'\n",
                            "      ZXS_REQUIRE_BROWSER: '1'\n"
                            '      ZXS_WORKSPACE_SHARD: ${{ matrix.shard }}\n', 1)
    source = source.replace('      - name: Retain PKI Valgrind positive and negative evidence\n',
                            WORKSPACE_UPLOAD + '      - name: Retain PKI Valgrind positive and negative evidence\n', 1)
    return source + WORKSPACE_COVERAGE_JOB


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
                                '# Explicit manual full runs execute the complete matrix without reuse.')
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
    return targeted_projection(workspace_projection(source)) if name == 'ci.yml' else source


FULL_CONDITION = "github.event_name == 'workflow_dispatch' && inputs.scope == 'full'"
TARGETED_JOB = """
  targeted-plan:
    name: Admit complete targeted selection
    if: github.event_name == 'push' || inputs.scope == 'targeted'
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    outputs:
      matrix: ${{ steps.plan.outputs.matrix }}
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          fetch-depth: 0
      - name: Partition the complete same-commit selection
        id: plan
        run: python3 .github/scripts/check-targeted-ci.py matrix
      - name: Retain complete selected scope
        if: always()
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: targeted-plan-${{ github.sha }}
          path: target/targeted-ci/summary.json
          if-no-files-found: error
          retention-days: 7

  targeted:
    name: Selected change tests (${{ matrix.index }})
    if: github.event_name == 'push' || inputs.scope == 'targeted'
    needs: targeted-plan
    runs-on: ubuntu-24.04
    timeout-minutes: 90
    strategy:
      fail-fast: false
      max-parallel: 4
      matrix: ${{ fromJSON(needs.targeted-plan.outputs.matrix) }}
    env:
      CARGO_BUILD_JOBS: '1'
      CARGO_PROFILE_DEV_DEBUG: '0'
      CARGO_PROFILE_TEST_DEBUG: '0'
      ZXS_TARGETED_SHARD: ${{ toJSON(matrix) }}
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          fetch-depth: 0
      - name: Validate change scope before building
        run: bash .github/scripts/check.sh targeted-plan
      - name: Initialize isolated Cargo home
        run: bash .github/scripts/initialize-cargo-home.sh
      - name: Install repository Rust toolchain
        run: rustup show active-toolchain
      - name: Run selected tests and retain exact execution evidence
        run: bash .github/scripts/check.sh targeted
      - name: Retain selected tests and unexecuted scope
        if: always()
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: targeted-tests-${{ matrix.index }}-${{ github.sha }}
          path: |
            target/targeted-ci/summary.json
            target/targeted-ci/*.log
            target/targeted-ci/wire/
          if-no-files-found: error
          retention-days: 7

  targeted-coverage:
    name: Complete same-commit targeted coverage
    needs: [targeted-plan, targeted]
    if: always() && needs.targeted-plan.result == 'success'
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          fetch-depth: 0
      - name: Require every selected shard job to succeed
        if: always()
        env:
          ZXS_TARGETED_JOB_RESULT: ${{ needs.targeted.result }}
        run: test "$ZXS_TARGETED_JOB_RESULT" = success
      - name: Fetch every targeted shard from this run
        uses: actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333 # v8.0.2
        with:
          pattern: targeted-tests-*-${{ github.sha }}
          path: target/targeted-shards
      - name: Prove complete non-overlapping selected execution
        run: python3 .github/scripts/check-targeted-ci.py verify-shards --receipts target/targeted-shards
      - name: Retain targeted coverage evidence
        if: always()
        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: targeted-coverage-${{ github.sha }}
          path: |
            target/targeted-ci/summary.json
            target/targeted-ci/coverage.json
          if-no-files-found: error
          retention-days: 7
"""


def targeted_projection(source):
    source = source.replace(FULL_HEADER,
        '# Main pushes run a reviewed change plan; full CI requires explicit manual selection.\n', 1)
    source = source.replace('  workflow_dispatch:\n',
        '  workflow_dispatch:\n'
        '    inputs:\n'
        '      scope:\n'
        '        description: Explicitly select targeted tests or the complete CI workload\n'
        '        type: choice\n'
        '        required: true\n'
        '        default: targeted\n'
        '        options: [targeted, full]\n'
        '      comparison_base:\n'
        '        description: Optional full ancestor commit SHA for targeted retry (ignored by full scope)\n'
        '        type: string\n'
        '        required: false\n'
        "        default: ''\n", 1)
    for job in ('static', 'tests', 'web-production', 'browser-smoke', 'smb-sdk', 'phase-one-core'):
        source = source.replace(f'  {job}:\n',
            f'  {job}:\n    if: {FULL_CONDITION}\n', 1)
    source = source.replace("    if: always() && needs.static.result == 'success'",
        "    if: always() && (" + FULL_CONDITION + ") && needs.static.result == 'success'", 1)
    source = source.replace('      - name: Retain workspace shard inventory and execution evidence\n',
                            "      - name: Retain measured first-party Clippy warnings\n        if: always() && matrix.suite == 'quality'\n        uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2\n        with:\n          name: clippy-warnings-quality-${{ github.sha }}\n          path: target/clippy-warnings/quality.json\n          if-no-files-found: error\n          retention-days: 7\n"
                            + '      - name: Retain workspace shard inventory and execution evidence\n', 1)
    return source + TARGETED_JOB


def check_workflows(root):
    """Fail on any unreviewed entrypoint, override, ignored failure or recipe drift."""
    live = root / '.github/workflows'
    names = {path.name for path in live.iterdir() if path.suffix in {'.yml', '.yaml'}}
    manual = {
        SQLITE_MAINTENANCE: SQLITE_WORKFLOW,
        SQLITE_IMPORT: SQLITE_IMPORT_WORKFLOW,
        BACKUP_PROFILER: BACKUP_PROFILER_WORKFLOW,
    }
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
        'automatic_triggers': False,
        'push_branches': [],
        'suites': list(FULL_SUITES),
        'matrix': execution_matrix(),
        'workspace_shards': list(WORKSPACE_SHARDS),
        'paused_suites': [],
        'paused_jobs': {name: list(jobs) for name, jobs in PAUSED_JOBS.items()},
        'reused': [],
        'reason': 'Full CI only after explicit manual selection; Release remains paused. Selection is not execution evidence.',
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
            stream.write('matrix=' + json.dumps(value['matrix'], separators=(',', ':')) + '\n')
    if summary is not None:
        with open(summary, 'a') as stream:
            stream.write('### Full CI: selected, not yet passed; Release paused\n\n```json\n'
                         + json.dumps(value, indent=2) + '\n```\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('check', 'select'))
    args = parser.parse_args()
    check_workflows(ROOT)
    if args.action == 'select':
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
        if (os.environ['GITHUB_EVENT_NAME'] != 'workflow_dispatch'
                or event.get('inputs', {}).get('scope') != 'full'):
            raise ValueError('full matrix requires explicit manual scope=full')
    value = receipt()
    check_receipt(value)
    emit(value, output=os.environ['GITHUB_OUTPUT'] if args.action == 'select' else None,
         summary=os.environ.get('GITHUB_STEP_SUMMARY'))


if __name__ == '__main__':
    main()
