"""Bounded synthetic220-commit trend; no adoption/capacity verdict."""
import json
import os
from pathlib import Path
import sys
from summarize import audit_pair

root, out = map(Path, sys.argv[1:])
out.mkdir(parents=True, exist_ok=False)
report = dict(schema='namespace-history-trend-v1', status='incomplete',
    scope='500live files;218edits;220commits;one unchanged branch;AB/BA;synthetic small bodies',
    exclusions=['not real video/photo/document workload','not physical12disk I/O','not user capacity certification',
                'not individual endpoint P99','not adoption approval'],
    adoption_gate='BLOCK unchanged',
    reference_run='41026367235', reference_commits=22,
    comparison_caveat='prior point cross-run; timed event request limit200->22, equal22 returned events; all220 verified separately; outside-timer metadata inventories may warm directory/inode caches before read/open, so not pure history scaling or OS-cold evidence',
    user_categories=['video sequential media','photo batch browsing/rename','document saves/history recovery','minority high file-branch count'],
    run_id=os.environ['GITHUB_RUN_ID'], controller=os.environ['GITHUB_SHA'],
    manifest_sha256=os.environ['ZXS_EXPECTED_MANIFEST_SHA256'], pairs=[])
try:
    for pair in (0,1):
        directory = root / f'namespace-pair-500-{pair}-{os.environ["GITHUB_RUN_ATTEMPT"]}'
        data = json.loads((directory / 'results/summary.json').read_text())
        errors = audit_pair(data,500,pair,expected_mode='history-trend')
        if errors or data['status'] != 'completed_measurements':
            raise RuntimeError(f'pair{pair} incomplete/invalid: {errors}')
        for run in data['runs']:
            phases = {p['phase']:p['measurements'] for p in run['phases']}
            oracle = phases['verify'][0]
            expected = dict(verified_current_files=500, verified_bodies=718,
                verified_operations=220, verified_commit_namespaces=220,
                receipt_api='operation_record', full_health_api_calls=0)
            if any(oracle.get(k) != v for k,v in expected.items()):
                raise RuntimeError('incomplete full history/body/receipt oracle')
            if phases['edit'][0]['extra']['edits'] != 218 or len(phases['edit'][0]['extra']['operation_ns']) != 218:
                raise RuntimeError('edit work differs')
            if phases['open'][0]['extra']['commits'] != 220:
                raise RuntimeError('reopened history differs')
            for read in phases['read']:
                if read['extra'] != dict(events=22,lookups=1000,page_len=6):
                    raise RuntimeError('timed output volume differs')
            for phase in ('build','edit','verify'):
                inventory = run['inventory_after_phase'][phase]
                if any(inventory[k]['files'] <= 0 for k in ('pool','state','metadata','immutable_objects')):
                    raise RuntimeError('missing metadata/object inventory')
        report['pairs'].append(dict(pair=pair,order=data['order'],binaries=data['binary_sha256'],
            artifact_identity=data['artifact_identity'],fixture=data['fixture'],runs=data['runs']))
    report['status']='completed_synthetic_trend_only'
finally:
    (out/'history-trend.json').write_text(json.dumps(report,indent=2)+'\n')
    print('HISTORY_TREND_RESULTS',json.dumps(report))

