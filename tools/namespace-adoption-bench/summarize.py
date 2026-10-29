#!/usr/bin/env python3
"""Account for all twelve fresh pairs; incomplete evidence cannot become success."""
import argparse
import json
import os
from pathlib import Path
import statistics

SIZES = (500, 1500, 3000)
PHASES = ('build', 'edit', 'read', 'open', 'verify')
SYSCALLS = {'fsync', 'fdatasync', 'rename', 'renameat', 'renameat2',
            'mkdir', 'mkdirat', 'unlink', 'unlinkat'}


def trace_counts(path):
    counts = {}
    for line in path.read_text().splitlines():
        parts = line.split()
        if parts and parts[-1] in SYSCALLS:
            counts[parts[-1]] = dict(calls=int(parts[3]),
                                    errors=int(parts[4]) if len(parts) == 6 else 0)
    return counts


def audit_pair(summary, files, pair, expected_mode='final'):
    expected_order = ['baseline', 'candidate'] if pair % 2 == 0 else ['candidate', 'baseline']
    errors = []
    expected = dict(files=files, sizes=[files], pair_index=pair, order=expected_order,
                    validation_mode=expected_mode,
                    github_run_id=os.environ['GITHUB_RUN_ID'],
                    github_run_attempt=os.environ['GITHUB_RUN_ATTEMPT'],
                    carrier_sha=os.environ['GITHUB_SHA'],
                    artifact_manifest_sha256=os.environ['ZXS_EXPECTED_MANIFEST_SHA256'])
    for field, value in expected.items():
        if summary.get(field) != value:
            errors.append(f'{field}: expected {value!r}, observed {summary.get(field)!r}')
    identity = summary.get('artifact_identity', {})
    if (identity.get('status') != 'verified' or identity.get('mode') != expected_mode or
            identity.get('manifest_sha256') !=
            os.environ['ZXS_EXPECTED_MANIFEST_SHA256']):
        errors.append('artifact identity audit is not verified for the current manifest')
    runs = summary['runs']
    if [run['implementation'] for run in runs] != expected_order:
        errors.append('implementation order differs from the fixed pair')
    for run in runs:
        if (run['files'], run['repetition'], run['traced']) != (files, pair, pair == 3):
            errors.append('run workload/order/trace identity differs')
        if run.get('binary_sha256') != summary['binary_sha256'][run['implementation']]:
            errors.append('run binary identity differs')
        if run['status'] == 'completed':
            if [phase['phase'] for phase in run['phases']] != list(PHASES):
                errors.append('completed run lacks all five executed phases')
            if run['attempted_phases'] != list(PHASES) or run['not_run_phases']:
                errors.append('completed run has an inconsistent phase plan')
            for phase in run['phases']:
                if phase['status'] != 'completed' or phase['returncode'] != 0:
                    errors.append('completed run contains a noncompleted child')
                if pair == 3:
                    observer = phase.get('observer', {})
                    mode = summary.get('trace_observer')
                    if (mode not in ('plain', 'seccomp-bpf') or
                            observer.get('requested_mode') != mode or observer.get('effective_mode') != mode or
                            set(observer.get('syscalls', [])) != SYSCALLS or
                            observer.get('full_argv') != phase.get('command')):
                        errors.append('completed traced phase lacks its actual observer/argv/syscall identity')
                    if mode == 'seccomp-bpf' and (
                            observer.get('activation_status') != 'enabled_and_observed' or
                            not observer.get('enabled_marker_count') or not observer.get('seccomp_event_count') or
                            observer.get('fallback_diagnostics') or
                            not run.get('observer_preflight', {}).get('unfiltered') or
                            run.get('observer_probe_admission', {}).get('status') != 'verified'):
                        errors.append('completed BPF phase lacks real activation or same-job contract admission')
    if summary['status'] == 'completed_measurements' and (
            len(runs) != 2 or any(run['status'] != 'completed' for run in runs)):
        errors.append('pair claims completion without both complete runs')
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(status='incomplete', build_result=os.environ['ZXS_BUILD_RESULT'],
                  github_run_id=os.environ['GITHUB_RUN_ID'],
                  github_run_attempt=os.environ['GITHUB_RUN_ATTEMPT'],
                  carrier_sha=os.environ['GITHUB_SHA'],
                  artifact_manifest_sha256=os.environ['ZXS_EXPECTED_MANIFEST_SHA256'],
                  validation_mode='final',
                  scope='final-mode fresh same-runner paired execution; no product adoption verdict',
                  metric_scope=dict(rchar='process bytes read through syscalls; not device I/O',
                                    read_bytes='Linux process storage read accounting',
                                    vmhwm_kb='process resident high-water mark; not allocation count',
                                    trace='separate syscall counts; excluded from untraced timing',
                                    pools='fresh logical directories on runner storage; not independent disks'),
                  pairs=[], sizes={})
    rows = ['| Files | Complete pairs | Failed pairs | Incomplete/missing pairs | Status |',
            '| --- | --- | --- | --- | --- |']
    for files in SIZES:
        entries = []
        metrics = {}
        for pair in range(4):
            directory = args.input / f'namespace-pair-{files}-{pair}-{os.environ["GITHUB_RUN_ATTEMPT"]}'
            path = directory / 'results/summary.json'
            entry = dict(files=files, pair=pair, traced=pair == 3, evidence=str(path.relative_to(args.input)))
            if not path.is_file():
                entry.update(status='incomplete', reason='missing_pair_summary')
            else:
                try:
                    summary = json.loads(path.read_text())
                    errors = audit_pair(summary, files, pair)
                    entry.update(status=summary['status'], errors=errors,
                                 binary_sha256=summary.get('binary_sha256'),
                                 runs=[dict(implementation=run['implementation'], status=run['status'],
                                            reason=run.get('reason'), phases=run['phases'],
                                            attempted_phases=run['attempted_phases'],
                                            not_run_phases=run['not_run_phases']) for run in summary['runs']])
                    if pair == 3:
                        for run in summary['runs']:
                            for phase in run['phases']:
                                trace = directory / 'results' / f'{files}-{pair}-{run["implementation"]}' / f'{phase["phase"]}.strace'
                                if phase['status'] == 'completed' and not trace.is_file():
                                    errors.append(f'completed traced phase lacks {trace.relative_to(directory)}')
                    if errors:
                        entry['status'] = 'failed'
                    if entry['status'] == 'completed_measurements' and pair < 3:
                        measured = {}
                        for run in summary['runs']:
                            measured[run['implementation']] = {
                                row['phase']: row['wall_ns']
                                for phase in run['phases'] if phase['phase'] != 'verify'
                                for row in phase['measurements']}
                        for phase, baseline in measured['baseline'].items():
                            candidate = measured['candidate'][phase]
                            metrics.setdefault(phase, []).append(dict(pair=pair, baseline_ns=baseline,
                                                                      candidate_ns=candidate,
                                                                      candidate_over_baseline=candidate / baseline))
                    if pair == 3:
                        entry['syscall_counts'] = {
                            str(trace.relative_to(directory)): trace_counts(trace)
                            for trace in sorted((directory / 'results').glob('*/*.strace'))}
                except (OSError, ValueError, KeyError, TypeError, ZeroDivisionError) as error:
                    entry.update(status='failed', reason='malformed_pair_evidence', error=str(error))
            entries.append(entry)
            report['pairs'].append(entry)
        completed = sum(entry['status'] == 'completed_measurements' for entry in entries)
        failed = sum(entry['status'] == 'failed' for entry in entries)
        status = 'completed_measurements' if completed == 4 else ('failed' if failed else 'incomplete')
        report['sizes'][str(files)] = dict(status=status, complete_pairs=completed,
                                         failed_pairs=failed, incomplete_pairs=4 - completed - failed,
                                         validated_untraced_pairs=metrics,
                                         medians={phase: dict(
                                             baseline_ns=statistics.median(row['baseline_ns'] for row in values),
                                             candidate_ns=statistics.median(row['candidate_ns'] for row in values),
                                             paired_ratio=statistics.median(row['candidate_over_baseline'] for row in values),
                                             pair_count=len(values)) for phase, values in metrics.items()})
        rows.append(f'| {files} | {completed}/4 | {failed} | {4-completed-failed} | {status} |')
    statuses = [size['status'] for size in report['sizes'].values()]
    if report['build_result'] == 'success' and all(status == 'completed_measurements' for status in statuses):
        report['status'] = 'completed_measurements'
    elif 'failed' in statuses or report['build_result'] == 'failure':
        report['status'] = 'failed'
    text = ('Final-mode frozen identities are authenticated in each pair artifact.\n\n' + '\n'.join(rows) +
            '\n\nAll measurements require both complete implementations and their full semantic verification. '
            'Three untraced AB/BA/AB pairs and one separate traced BA pair per size. '
            'Raw partial phases remain in the pair artifacts. This is not an adoption verdict.\n')
    (args.output / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
    (args.output / 'summary.md').write_text(text)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as output:
            output.write(text)
    print(text)
    return 0 if report['status'] == 'completed_measurements' else 1


if __name__ == '__main__':
    raise SystemExit(main())
