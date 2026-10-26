import json, pathlib, statistics
out=pathlib.Path(__file__).resolve().parent
provenance=json.loads((out/'source-provenance.json').read_text())
rows={}
resources={}
for mode, maximum in [('fixed',1024),('growing',256)]:
    for variant in ('baseline','recovery'):
        name=f'{variant}-{mode}-{maximum}-r1'
        resource=json.loads((out/f'{name}.resource.json').read_text())
        assert resource['exit_code']==0, (name,'not complete')
        assert resource['binary_sha256']==provenance['variants'][variant]['binary_sha256']
        data=[json.loads(line) for line in (out/f'{name}.jsonl').read_text().splitlines()]
        assert [r['history'] for r in data]==[n for n in (64,256,1024) if n<=maximum]
        assert all(all(r['checks'].values()) for r in data)
        resources[name]=resource
        for row in data:
            assert len(row['restart_ns'])==len(row['restart_evidence'])==3
            rows[variant,mode,row['history']]=row
comparisons=[]
for mode, maximum in [('fixed',1024),('growing',256)]:
    for history in (n for n in (64,256,1024) if n<=maximum):
        result={'mode':mode,'history':history}
        for variant in ('baseline','recovery'):
            row=rows[variant,mode,history]
            seconds=[ns/1e9 for ns in row['restart_ns']]
            result[variant]={
                'engine_open_seconds':seconds,
                'selected_checkpoint_sequences':[e['checkpoint_sequence'] for e in row['restart_evidence']],
                'checkpoint_load_warning_codes':[None if e['checkpoint_load_warning'] is None else e['checkpoint_load_warning']['code'] for e in row['restart_evidence']],
                'checkpoint_save_warning_codes':[None if e['checkpoint_save_warning'] is None else e['checkpoint_save_warning']['code'] for e in row['restart_evidence']],
                'median_open_seconds':statistics.median(seconds),
                'state_json_bytes':row['state_json_bytes'],
                'post_probe_memory':row['post_probe_memory'],
            }
        result['median_ratio_recovery_over_baseline']=result['recovery']['median_open_seconds']/result['baseline']['median_open_seconds']
        comparisons.append(result)
summary={'complete':True,'scope':'one fresh fixture per variant/mode, three complete Engine::open observations per history; process exit verifies final integrity; no profiler attribution or production timing gate','comparisons':comparisons,'process_resources':resources}
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
for row in comparisons:
    print(row['mode'],row['history'],'baseline',row['baseline']['engine_open_seconds'],'recovery',row['recovery']['engine_open_seconds'],'median ratio',row['median_ratio_recovery_over_baseline'])
