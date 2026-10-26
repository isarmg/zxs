import datetime, hashlib, json, os, pathlib, shutil, subprocess, time
root = pathlib.Path('/workspace/scratch/08a6fde47383')
out = root / 'r5-recovery-measurement'
binaries = {name: out / f'{name}-startup' for name in ('baseline','recovery')}

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def run(variant, mode, maximum, suffix):
    name = f'{variant}-{mode}-{maximum}-{suffix}'
    start = time.monotonic()
    binary = binaries[variant]
    print(f'{datetime.datetime.now(datetime.timezone.utc).isoformat()} starting {name}; free={shutil.disk_usage(root).free/2**30:.2f} GiB', flush=True)
    with (out / f'{name}.jsonl').open('x') as stdout, (out / f'{name}.stderr').open('x') as stderr:
        process = subprocess.Popen([str(binary), mode, str(maximum)], stdout=stdout, stderr=stderr)
        _, status, usage = os.wait4(process.pid, 0)
        process.returncode = os.waitstatus_to_exitcode(status)
        resource = {
            'binary_sha256':digest(binary), 'argv':[str(binary),mode,str(maximum)],
            'exit_code':process.returncode, 'wall_seconds':time.monotonic()-start,
            'user_cpu_seconds':usage.ru_utime, 'system_cpu_seconds':usage.ru_stime,
            'max_rss_kib':usage.ru_maxrss, 'minor_faults':usage.ru_minflt,
            'major_faults':usage.ru_majflt, 'voluntary_context_switches':usage.ru_nvcsw,
            'involuntary_context_switches':usage.ru_nivcsw,
        }
        (out / f'{name}.resource.json').write_text(json.dumps(resource,indent=2)+'\n')
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, process.args)
    rows = [json.loads(line) for line in (out / f'{name}.jsonl').read_text().splitlines()]
    assert [row['history'] for row in rows] == [n for n in (64,256,1024) if n <= maximum]
    for row in rows:
        assert len(row['restart_ns']) == len(row['restart_evidence']) == 3
        assert all(row['checks'].values())
        for ns,evidence in zip(row['restart_ns'],row['restart_evidence']):
            assert ns == evidence['engine_open_ns']
            assert type(evidence['checkpoint_hit']) is bool
            assert (evidence['checkpoint_sequence'] is not None) == evidence['checkpoint_hit']
    print(f'finished {name}: {resource["wall_seconds"]:.3f} s; checkpoints=' + str([(r['history'],[e['checkpoint_sequence'] for e in r['restart_evidence']]) for r in rows]),flush=True)

if __name__ == '__main__':
    import sys
    if sys.argv[1:] == ['smoke']:
        for variant in ('baseline','recovery'): run(variant,'fixed',64,'smoke')
    elif sys.argv[1:] == ['full']:
        for mode,maximum in [('fixed',1024),('growing',256)]:
            for variant in ('baseline','recovery'): run(variant,mode,maximum,'r1')
    else:
        raise SystemExit('usage: run.py smoke|full')
