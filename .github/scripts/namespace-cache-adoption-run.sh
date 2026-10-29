#!/usr/bin/env bash
set -euo pipefail
# One invocation owns one finite budget, including profiler post-processing.
exec python3 - "$@" <<'PY'
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

binary = Path(sys.argv[1]).resolve(strict=True)
evidence = Path(sys.argv[2]).resolve()
widths = [int(value) for value in sys.argv[3:]] or [1000, 10000, 100000]
if any(width not in (1000, 10000, 100000) for width in widths):
    raise ValueError('widths must be 1000, 10000 or 100000')
evidence.mkdir(parents=True, exist_ok=True)
for command in ('heaptrack', 'heaptrack_print'):
    if shutil.which(command) is None:
        raise RuntimeError(f'missing prerequisite: {command}')
started = time.monotonic()
deadline = started + 2700
summary = dict(status='running', total_budget_seconds=2700, stages=[])

def save():
    summary['elapsed_seconds'] = time.monotonic() - started
    (evidence / 'execution-summary.json').write_text(json.dumps(summary, indent=2) + '\n')

def interrupt(number, frame):
    raise SystemExit(128 + number)

signal.signal(signal.SIGTERM, interrupt)

def stop(child):
    # This process group contains only this owned stage and its descendants.
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()

def run(label, maximum, command, log_name):
    remaining = deadline - time.monotonic()
    record = dict(label=label, command=command, status='running',
                  timeout_seconds=max(0, min(maximum, remaining)))
    summary['stages'].append(record)
    save()
    if remaining <= 0:
        record['status'] = summary['status'] = 'incomplete'
        record['reason'] = 'total_execution_budget'
        save()
        raise SystemExit(2)
    child = None
    try:
        with (evidence / log_name).open('wb') as log:
            child = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                     start_new_session=True)
            try:
                status = child.wait(timeout=record['timeout_seconds'])
            except subprocess.TimeoutExpired:
                stop(child)
                record['status'] = summary['status'] = 'incomplete'
                record['reason'] = 'stage_or_total_time_budget'
                raise SystemExit(2)
        record['returncode'] = status
        if status:
            record['status'] = summary['status'] = 'failed'
            raise SystemExit(1)
        record['status'] = 'completed'
    finally:
        if child is not None and child.poll() is None:
            stop(child)
        if record['status'] == 'running':
            record['status'] = 'interrupted'
        save()

try:
    run('binary-identity', 60, ['sha256sum', str(binary)], 'binary.sha256')
    run('heaptrack-version', 60, ['heaptrack', '--version'], 'heaptrack-version.txt')
    for width in widths:
        name = f'segment_store::tests::namespace_adoption::namespace_cache_adoption_{width}'
        run(f'{width}-list', 300, [str(binary), '--exact', name, '--list'], f'{width}-list.txt')
        listed = (evidence / f'{width}-list.txt').read_text().splitlines()
        if listed.count(f'{name}: test') != 1:
            raise RuntimeError(f'missing exact test inventory: {name}')
        arguments = [str(binary), '--exact', name, '--ignored', '--nocapture', '--test-threads=1']
        run(f'{width}-unprofiled', 1200, arguments, f'{width}-unprofiled.log')
        output = (evidence / f'{width}-unprofiled.log').read_text()
        if '1 passed; 0 failed; 0 ignored;' not in output:
            raise RuntimeError(f'no passing exact test body: {name}')
        rows = [json.loads(line.split('NAMESPACE_ADOPTION ', 1)[1])
                for line in output.splitlines() if 'NAMESPACE_ADOPTION ' in line]
        if (len(rows) != 26 or len({row['phase'] for row in rows}) != 26
                or any(row['width'] != width or row['cache_canonical_bytes'] > 8 * 1024 * 1024
                       for row in rows)):
            raise RuntimeError(f'invalid cache evidence for width {width}')
        (evidence / f'{width}-unprofiled.log.json').write_text(json.dumps(rows, indent=2) + '\n')
        run(f'{width}-profiled', 1800,
            ['heaptrack', '-o', str(evidence / f'{width}-heaptrack'), *arguments],
            f'{width}-profiled.log')
        if '1 passed; 0 failed; 0 ignored;' not in (evidence / f'{width}-profiled.log').read_text():
            raise RuntimeError(f'no passing profiled test body: {name}')
        traces = [path for path in evidence.glob(f'{width}-heaptrack*')
                  if path.suffix in ('.gz', '.zst') and path.is_file()]
        if len(traces) != 1:
            raise RuntimeError(f'expected one heaptrack trace for width {width}')
        run(f'{width}-allocation-summary', 600, ['heaptrack_print', str(traces[0])],
            f'{width}-allocations.txt')
        if not (evidence / f'{width}-allocations.txt').stat().st_size:
            raise RuntimeError('empty allocation summary')
    summary['status'] = 'completed_measurements'
finally:
    if summary['status'] == 'running':
        summary['status'] = 'failed_or_interrupted'
    save()
PY
