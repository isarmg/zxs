from pathlib import Path
import subprocess, hashlib, json, datetime
root=Path('/workspace/scratch/08a6fde47383'); out=root/'r5-recovery-measurement'; base='3fd13d3e205e59b7b8f840ae164ad947e9a06533'
variants={}
for name,directory in [('baseline','zxs-r5-recovery-baseline'),('recovery','zxs-r5-recovery')]:
 repo=root/directory
 git=lambda *args: subprocess.check_output(['git','-C',str(repo),*args])
 assert not git('status','--porcelain').strip()
 patch=git('diff','--binary',base,'HEAD')
 (out/f'{name}-overlay.patch').write_bytes(patch)
 binary=out/f'{name}-startup'
 variants[name]={'commit':git('rev-parse','HEAD').decode().strip(),'tree':git('rev-parse','HEAD^{tree}').decode().strip(),'base':base,'clean_worktree':True,'overlay_sha256':hashlib.sha256(patch).hexdigest(),'fixture_sha256':hashlib.sha256((repo/'tools/perf-check/src/bin/recovery-startup.rs').read_bytes()).hexdigest(),'cargo_lock_sha256':hashlib.sha256((repo/'Cargo.lock').read_bytes()).hexdigest(),'target':str(root/('targets/r5-recovery-baseline' if name=='baseline' else 'targets/r5-recovery')),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest()}
assert variants['baseline']['fixture_sha256']==variants['recovery']['fixture_sha256']
(out/'source-provenance.json').write_text(json.dumps({'created_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'remote_base':base,'toolchain':'rustc 1.99.0 (b940084d7 2026-09-28); cargo 1.99.0 (5f94df478 2026-08-27)','build_command':'CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo build --release -p perf-check --bin recovery-startup','variants':variants,'scope':'Development fixture only; full Engine::open, not payload/physical redundancy evidence; one fixture per mode/variant, three opens at each history size; static audit, no profiler attribution','fixture_origin':'original state-sharing actual transaction workload, with unrelated clone/write/cursor/replay microbenchmarks removed identically from both variants; existing workload labels and default budgets unchanged'},indent=2)+'\n')
