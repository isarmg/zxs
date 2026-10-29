#!/usr/bin/env python3
"""Compare two actual 500-file observer loads; no final product acceptance."""
import argparse
import json
import os
from pathlib import Path

from summarize import PHASES, SYSCALLS, audit_pair, trace_counts


def collect_pair(root, mode):
    directory = root / f'namespace-observer-{mode}-{os.environ["GITHUB_RUN_ATTEMPT"]}'
    path = directory / 'results/summary.json'
    entry = dict(observer=mode, status='incomplete', evidence=str(path.relative_to(root)))
    if not path.is_file():
        entry['reason'] = 'missing_observer_pair'
        return entry
    try:
        summary = json.loads(path.read_text())
        errors = audit_pair(summary, 500, 3, expected_mode='observer-validation')
        if summary.get('trace_observer') != mode:
            errors.append('observer mode differs from artifact name')
        entry.update(status=summary['status'], measurement_status=summary['status'], errors=errors,
                     selected_inputs=summary.get('artifact_identity', {}).get('selected_inputs'),
                     binary_sha256=summary.get('binary_sha256'),
                     runs=[dict(implementation=run['implementation'], status=run['status'],
                                reason=run.get('reason'), attempted_phases=run['attempted_phases'],
                                not_run_phases=run['not_run_phases'],
                                observer_probe_admission=run.get('observer_probe_admission'))
                           for run in summary['runs']], counts={})
        for run in summary['runs']:
            counts = {}
            for phase in run['phases']:
                trace = directory / 'results' / f'500-3-{run["implementation"]}' / f'{phase["phase"]}.strace'
                if phase['status'] == 'completed' and not trace.is_file():
                    errors.append(f'completed phase lacks raw trace: {trace.relative_to(directory)}')
                if trace.is_file():
                    counts[phase['phase']] = trace_counts(trace)
            entry['counts'][run['implementation']] = counts
        probe_path = directory / 'observer-probe/probe.json'
        if probe_path.is_file():
            probe = json.loads(probe_path.read_text())
            entry['deterministic_probe'] = dict(status=probe['status'], strace=probe.get('strace'),
                                                probes=[dict(mode=p['mode'], status=p['status'],
                                                             counts=p.get('counts'), observer=p.get('observer'))
                                                        for p in probe['probes']])
        if entry.get('deterministic_probe', {}).get('status') != 'completed_probe':
            entry.update(status='incomplete', reason='same_runner_observer_contract_unavailable')
        if errors:
            entry['status'] = 'failed'
    except (OSError, KeyError, TypeError, ValueError, IndexError) as error:
        entry.update(status='failed', reason='invalid_observer_evidence', error=str(error))
    return entry


def compare_counts(plain, filtered):
    rows = []
    for implementation in ('baseline', 'candidate'):
        for phase in PHASES:
            a = plain['counts'].get(implementation, {}).get(phase, {})
            b = filtered['counts'].get(implementation, {}).get(phase, {})
            for name in sorted(SYSCALLS):
                before = a.get(name, dict(calls=0, errors=0))
                after = b.get(name, dict(calls=0, errors=0))
                rows.append(dict(implementation=implementation, phase=phase, syscall=name,
                                 plain=before, seccomp_bpf=after,
                                 calls_delta=after['calls']-before['calls'],
                                 errors_delta=after['errors']-before['errors']))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(status='incomplete', validation_mode='observer-validation',
                  scope='observer contract and full old 500-file fixtures only; not final candidate acceptance',
                  build_result=os.environ['ZXS_BUILD_RESULT'],
                  github_run_id=os.environ['GITHUB_RUN_ID'], carrier_sha=os.environ['GITHUB_SHA'],
                  artifact_manifest_sha256=os.environ['ZXS_EXPECTED_MANIFEST_SHA256'],
                  observers=[collect_pair(args.input, mode) for mode in ('plain', 'seccomp-bpf')],
                  timing_scope='all runs traced; no untraced performance conclusion',
                  count_scope='entire child process, including startup/recovery; same fixed nine syscall names',
                  equality_scope='exact count/error equality is required for the deterministic contract probe; independent product pools contain fresh operation/version IDs, so every product count delta is reported separately')
    if all(row['status'] == 'completed_measurements' for row in report['observers']):
        plain, filtered = report['observers']
        if (plain['binary_sha256'] != filtered['binary_sha256'] or
                plain['selected_inputs'] != filtered['selected_inputs']):
            report.update(status='failed', reason='observer_modes_used_different_product_inputs')
        else:
            report['product_count_comparison'] = compare_counts(plain, filtered)
            report['product_counts_identical'] = all(not row['calls_delta'] and not row['errors_delta']
                                                     for row in report['product_count_comparison'])
            report['status'] = 'completed_observer_validation' if report['build_result'] == 'success' else 'incomplete'
    elif report['build_result'] == 'failure' or any(row['status'] == 'failed' for row in report['observers']):
        report['status'] = 'failed'
    text = ('Observer validation only; final candidate is not measured here.\n\n'
            '| Observer | Contract and full 500-file pair |\n| --- | --- |\n' +
            '\n'.join(f'| {row["observer"]} | {row["status"]} |' for row in report['observers']) +
            f'\n\nStatus: `{report["status"]}`. Product count equality: '
            f'`{report.get("product_counts_identical", "not established")}`. '
            'All deltas, exact deterministic 54-call/27-error contracts and raw activation evidence are retained. '
            'Trace duration is excluded from product performance conclusions.\n')
    (args.output / 'summary.json').write_text(json.dumps(report, indent=2)+'\n')
    (args.output / 'summary.md').write_text(text)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as output:
            output.write(text)
    print(text)
    return 0 if report['status'] == 'completed_observer_validation' else 1


if __name__ == '__main__':
    raise SystemExit(main())
