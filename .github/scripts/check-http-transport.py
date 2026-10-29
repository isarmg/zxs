#!/usr/bin/env python3
"""Validate HTTP/1 transport/adapters, real duplex IO and native/WASM capabilities."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import json
import os
from pathlib import Path
import re
import shutil
import tomllib
import subprocess
import sys
import tempfile
import runpy

ROOT=Path(__file__).resolve().parents[2]
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']
FEATURES=((),('client','http1'),('server','http1'),('adapters',),('service-adapter',),('adapter-client',),('client-pool',),('client-proxy',),('tokio',),('client-legacy','http1','tokio'),('adapter-server',),('server-auto','tokio'),('server-graceful','http1','tokio'),('full',),('rt-tracing-exec-force',))
NATIVE_ONLY={'tokio','client-legacy','full','rt-tracing-exec-force'}
def run(work,*args,negative=False,env=None):
    try:
        result=subprocess.run(['cargo',args[0],'--color','never','--offline',*args[1:]],cwd=work,text=True,capture_output=True,timeout=600,env=env)
    except subprocess.TimeoutExpired as error:
        print(f'HTTP transport Cargo command timed out after {error.timeout}s in {work}', file=sys.stderr, flush=True)
        # TimeoutExpired may carry bytes even when text=True. Preserve Cargo's
        # lock waits, compilation progress and test output before re-raising;
        # otherwise an ordinary timeout traceback discards all captured clues.
        for label, output in (('stdout', error.stdout), ('stderr', error.stderr)):
            if output:
                if isinstance(output, bytes):
                    output = output.decode('utf-8', errors='replace')
                print(f'--- captured Cargo {label} ---\n{output}', file=sys.stderr, flush=True)
        raise
    if negative:
        assert result.returncode and 'with feature `http2`' in result.stderr and '`http-transport` does not have that feature' in result.stderr,result.stderr
    else:
        if result.returncode:raise RuntimeError(result.stdout+result.stderr)
        if re.search(r'^warning:',result.stderr,re.M):raise AssertionError(result.stderr)
    return result.stdout

def check_retained_primitives(temporary):
    # Compile the actual private production modules, not copies or public facades.
    # Match their manifest-relative include paths in a disposable test workspace.
    mirror = temporary / 'retained'
    work = mirror / 'src/foundation/networking/http-transport'
    (work / 'src').mkdir(parents=True)
    shutil.copytree(ROOT / 'test/foundation/http-transport/primitives',
                    mirror / 'test/foundation/http-transport/primitives')
    (work / 'rust-toolchain.toml').write_bytes((ROOT / 'rust-toolchain.toml').read_bytes())
    (work / 'Cargo.toml').write_text(
        '[package]\nname="transport-private-primitives"\nversion="0.0.0"\nedition="2021"\n'
        '[workspace]\n[dependencies]\n'
        f'futures={{path={json.dumps(str(ROOT/"src/foundation/concurrency/futures"))},default-features=false,features=["std","executor"]}}\n'
        f'tokio={{path={json.dumps(str(ROOT/"src/foundation/concurrency/tokio"))},default-features=false,features=["sync"]}}\n')
    modules = ''.join(f'#[path = {json.dumps(str(ROOT/"src/foundation/networking/http-transport/src/common"/(name+".rs")))}]\n'
                      f'pub(crate) mod {name};\n' for name in ('demand','try_lock','task_waker'))
    (work / 'src/lib.rs').write_text('#![deny(warnings)]\nmod common {\n' + modules + '}\n'
        '#[cfg(test)]\n#[path = "../../../../../test/foundation/http-transport/primitives/atomic-waker/tests/basic.rs"]\nmod retained_atomic_waker;\n'
        f'#[cfg(test)]\n#[path = {json.dumps(str(ROOT/"test/architecture/http-transport/primitives.rs"))}]\nmod contracts;\n')
    run(work, 'generate-lockfile')
    output = run(work, 'test', '--locked', '--lib')
    assert re.search(r'test result: ok\. 18 passed; 0 failed; 0 ignored', output), output
    # Repeat the scheduling-sensitive tests; each iteration covers 128 races.
    for _ in range(3):
        output = run(work, 'test', '--locked', '--lib', 'race', env=dict(os.environ, RUST_TEST_THREADS='1'))
        assert re.search(r'test result: ok\. 2 passed; 0 failed; 0 ignored', output), output


def check_retired_dependencies():
    retired = {'want', 'try-lock', 'atomic-waker'}
    for name in retired:
        assert not (foundation_path(ROOT, name)).exists(), name
        assert not (ROOT / 'test/foundation' / name).exists(), name
    def dependencies(value):
        if not isinstance(value, dict): return
        for key, item in value.items():
            if key in ('dependencies', 'dev-dependencies', 'build-dependencies'):
                for name, specification in item.items():
                    package = specification.get('package', name) if isinstance(specification, dict) else name
                    assert package not in retired, (manifest, key, name)
            else:
                dependencies(item)
    for base in ('src', 'tools', 'test'):
        for manifest in (ROOT / base).rglob('Cargo.toml'):
            dependencies(tomllib.loads(manifest.read_text()))
    dependencies(tomllib.loads((ROOT / 'Cargo.toml').read_text()))

def main():
    check_retired_dependencies()
    os.environ.setdefault('CARGO_BUILD_JOBS','2');os.environ.setdefault('CARGO_PROFILE_DEV_DEBUG','0');os.environ.setdefault('CARGO_INCREMENTAL','0')
    with tempfile.TemporaryDirectory(prefix='http-transport-') as temporary:
        check_retained_primitives(Path(temporary))
        work=Path(temporary);(work/'src').mkdir();(work/'test').mkdir()
        (work / 'rust-toolchain.toml').write_bytes((ROOT / 'rust-toolchain.toml').read_bytes())
        (work/'src/lib.rs').write_text('pub use http_transport::{Error,Request,Response};\n')
        count=0
        for features in FEATURES:
            (work/'Cargo.toml').write_text('[package]\nname="transport-contract"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\n'+f'http-transport={{path={json.dumps(str(ROOT/"src/foundation/networking/http-transport"))},default-features=false,features={json.dumps(features)}}}\n')
            run(work,'generate-lockfile');metadata=json.loads(run(work,'metadata','--locked','--format-version','1'))
            nodes={n['id']:n for n in metadata['resolve']['nodes']};packages={p['name']:p for p in metadata['packages'] if p['id'] in nodes}
            assert not set(packages).intersection({'hyper','hyper-util','h2','want','try-lock','atomic-waker'}),set(packages)
            if not set(features)&NATIVE_ONLY:
                tf=set(nodes[packages['tokio']['id']]['features']);assert not tf.intersection({'rt','rt-multi-thread','net','time'}),tf
                run(work,'check','--locked','--target','wasm32-unknown-unknown');count+=1
            run(work,'check','--locked');count+=1
        run(work,'check','--locked','--features','http-transport/http2',negative=True)
        for extra,flag in [('tracing','hyper_unstable_tracing'),('ffi','hyper_unstable_ffi')]:
            environment=dict(os.environ)
            environment['RUSTFLAGS']=(environment.get('RUSTFLAGS','')+' --cfg '+flag).strip()
            run(work,'check','--locked','--features',f'http-transport/{extra},http-transport/client,http-transport/http1',env=environment)
            count+=1
            if extra == 'ffi':
                with (work/'Cargo.toml').open('a') as manifest:
                    manifest.write('[[test]]\nname="ffi"\npath="test/ffi.rs"\n')
                (work/'test/ffi.rs').write_bytes((ROOT/'test/architecture/http-transport/ffi.rs').read_bytes())
                output=run(work,'test','--locked','--test','ffi','--features','http-transport/ffi,http-transport/client,http-transport/http1',env=environment)
                assert re.search(r'test result: ok\. 2 passed; 0 failed',output),output
        # Real runtime + HTTP client/server over Tokio's in-memory duplex stream, no sockets.
        (work/'Cargo.toml').write_text('[package]\nname="transport-behavior"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\n'+
            f'http-transport={{path={json.dumps(str(ROOT/"src/foundation/networking/http-transport"))},features=["full"]}}\n'+
            f'http-routing={{path={json.dumps(str(ROOT/"src/foundation/networking/http-routing"))},default-features=false,features=["tokio","http1"]}}\n'+
            f'tokio={{path={json.dumps(str(ROOT/"src/foundation/concurrency/tokio"))},features=["rt","macros","io-util","time","test-util"]}}\n'+
            f'http-body={{path={json.dumps(str(ROOT/"src/foundation/networking/http-body"))},features=["utilities"]}}\n'+
            f'service-stack={{path={json.dumps(str(ROOT/"src/foundation/networking/service-stack"))},features=["util"]}}\n'+
            '[[test]]\nname="behavior"\npath="test/behavior.rs"\n')
        (work/'test/behavior.rs').write_bytes((ROOT/'test/architecture/http-transport/behavior.rs').read_bytes() + (ROOT/'test/architecture/http-transport/combined.rs').read_bytes() + (ROOT/'test/architecture/http-transport/upgrade.rs').read_bytes() + (ROOT/'test/architecture/http-transport/client_state.rs').read_bytes() + (ROOT/'test/architecture/http-transport/probe_timeout.rs').read_bytes() + (ROOT/'test/architecture/http-transport/backport.rs').read_bytes())
        run(work, 'generate-lockfile')
        inventory = run(work, 'test', '--locked', '--test', 'behavior', '--', '--list')
        print(inventory, flush=True)
        expected = re.findall(r'^(.+): test$', inventory, re.M)
        assert len(expected) == len(set(expected)) == 30, inventory
        output = run(work, 'test', '--locked', '--test', 'behavior', '--', '--format', 'pretty')
        print(output, flush=True)
        executed = re.findall(r'^test (.+) \.\.\. ok$', output, re.M)
        assert sorted(executed) == sorted(expected), output
        assert re.search(
            r'^test result: ok\. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;',
            output, re.M,
        ), output
        print(f'http-transport: {count} native/WASM feature checks, absent HTTP2 rejection, 30 transport/routing/readiness/upgrade/state/shutdown tests, 2 FFI allocation/ABI tests, 6 retained primitive tests and 12 additional primitive contracts (including repeated races) passed')
if __name__=='__main__':main()
