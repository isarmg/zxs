"""Bounded same-source attribution; never an adoption result."""
import json
import os
from pathlib import Path
import sys
from summarize import audit_pair

root, out = map(Path, sys.argv[1:])
out.mkdir(parents=True, exist_ok=False)
report = dict(schema='namespace-attribution-v1', status='incomplete',
    scope='Same529ef source: original owned-get control vs explicit instrumentation overlay;500files;AB/BA;no adoption verdict',
    adoption_gate='BLOCK unchanged; this is perturbation/component evidence only',
    run_id=os.environ['GITHUB_RUN_ID'], controller=os.environ['GITHUB_SHA'],
    manifest_sha256=os.environ['ZXS_EXPECTED_MANIFEST_SHA256'], pairs=[])
try:
    for pair in (0, 1):
        directory = root / f'namespace-pair-500-{pair}-{os.environ["GITHUB_RUN_ATTEMPT"]}'
        data = json.loads((directory / 'results/summary.json').read_text())
        errors = audit_pair(data, 500, pair, expected_mode='attribution')
        if errors or data['status'] != 'completed_measurements':
            raise RuntimeError(f'pair{pair} incomplete/invalid: {errors}')
        diagnostics = []
        text = (directory / f'results/500-{pair}-candidate/read.stderr').read_text()
        for line in text.splitlines():
            if line.startswith('NAMESPACE_ATTRIBUTION '):
                diagnostics.append(json.loads(line.removeprefix('NAMESPACE_ATTRIBUTION ')))
        if [value['phase'] for value in diagnostics] != ['read_after_open', 'read_warm_repeat']:
            raise RuntimeError('missing first/warm attribution')
        for value in diagnostics:
            if len(value['names']) != 14 or len(set(value['names'])) != 14:
                raise RuntimeError('invalid attribution counter schema')
            for field in ('lookup_inclusive_wall_ns', 'lookup_calls', 'tail_inclusive_wall_ns', 'tail_calls'):
                if len(value[field]) != 14 or any(not isinstance(n, int) or n < 0 for n in value[field]):
                    raise RuntimeError('invalid attribution counters')
            for name in ('lookup_get', 'normalization', 'resolve_raw', 'owned_entry', 'get_key'):
                if value['lookup_calls'][value['names'].index(name)] != 1000:
                    raise RuntimeError('original1000lookup coverage differs: ' + name)
        if diagnostics[0]['lookup_calls'][10] == 0 or any(diagnostics[1]['lookup_calls'][i] for i in (6,7,8,9,10)):
            raise RuntimeError('first/warm cache-miss attribution differs from intended case')
        report['pairs'].append(dict(pair=pair, order=data['order'], binaries=data['binary_sha256'],
            artifact_identity=data['artifact_identity'], runs=data['runs'], diagnostics=diagnostics))
    report['status'] = 'completed_attribution_only'
finally:
    (out / 'attribution.json').write_text(json.dumps(report, indent=2) + '\n')
    print('ATTRIBUTION_RESULTS', json.dumps(report))
