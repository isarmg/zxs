#!/usr/bin/env python3
"""Exercise the single Service/Layer owner and maintained middleware boundaries."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
FEATURES = ((), ('util',), ('filter', 'load-shed', 'steer'), ('full',))

def run(work, *args):
    command = ['cargo', *args, '--color', 'never', '--offline']
    result = subprocess.run(command, cwd=work, text=True, capture_output=True, timeout=600)
    if result.returncode:
        raise RuntimeError(' '.join(command) + '\n' + result.stdout + result.stderr)
    return result.stdout

def main():
    os.environ.setdefault('CARGO_BUILD_JOBS', '2')
    os.environ.setdefault('CARGO_PROFILE_DEV_DEBUG', '0')
    os.environ.setdefault('CARGO_INCREMENTAL', '0')
    with tempfile.TemporaryDirectory(prefix='service-stack-') as temporary:
        work = Path(temporary)
        (work / 'rust-toolchain.toml').write_bytes((ROOT / 'rust-toolchain.toml').read_bytes())
        (work / 'src').mkdir()
        (work / 'src/lib.rs').write_text('pub use service_stack::{Service, Layer, ServiceBuilder};\n')
        (work / 'test').mkdir()
        (work / 'test/behavior.rs').write_bytes((ROOT / 'test/architecture/service-stack/behavior.rs').read_bytes())
        (work / 'test/rng.rs').write_bytes((ROOT / 'test/architecture/service-stack/rng.rs').read_bytes())
        for features in FEATURES:
            (work / 'Cargo.toml').write_text(
                '[package]\nname="service-stack-contract"\nversion="0.0.0"\nedition="2021"\n'
                '[workspace]\n[features]\nbalance=["service-stack/balance"]\n[dependencies.service-stack]\n'
                f'path={json.dumps(str(ROOT / "src/foundation/networking/service-stack"))}\n'
                f'default-features=false\nfeatures={json.dumps(features)}\n'
                '[[test]]\nname="behavior"\npath="test/behavior.rs"\nrequired-features=["service-stack/util"]\n'
                '[[test]]\nname="rng"\npath="test/rng.rs"\nrequired-features=["service-stack/util"]\n'
            )
            run(work, 'generate-lockfile')
            metadata = json.loads(run(work, 'metadata', '--locked', '--format-version', '1'))
            nodes = {node['id'] for node in metadata['resolve']['nodes']}
            names = {p['name'] for p in metadata['packages'] if p['id'] in nodes}
            assert not names.intersection({'tower', 'tower-layer', 'tower-service'}), names
            if not features:
                assert names == {'service-stack-contract', 'service-stack'}, names
            if 'full' not in features:
                assert 'tokio' not in names, names
            run(work, 'check', '--locked')
            # Tokio net/time middleware is native-only; bare traits and pure utilities remain WASM.
            if 'full' not in features:
                run(work, 'check', '--locked', '--target', 'wasm32-unknown-unknown')
            if features == ('util',):
                output = run(work, 'test', '--locked', '--test', 'behavior')
                assert re.search(r'test result: ok\. 4 passed; 0 failed', output), output
            if features in (('util',), ('full',)):
                balance = ('--features', 'balance') if features == ('full',) else ()
                output = run(work, 'test', '--locked', '--test', 'rng', *balance)
                print(output, end='', flush=True)
                expected = 7 if features == ('full',) else 6
                assert re.search(rf'test result: ok\. {expected} passed; 0 failed; 0 ignored', output), output
        (work / 'Cargo.toml').write_text(
            '[package]\nname="service-stack-consumers"\nversion="0.0.0"\nedition="2021"\n'
            '[workspace]\n[dependencies]\n'
            f'http-routing={{path={json.dumps(str(ROOT / "src/foundation/networking/http-routing"))},default-features=false,features=["http1","tokio","json"]}}\n'
            f'reqwest={{path={json.dumps(str(ROOT / "src/foundation/networking/reqwest"))},default-features=false}}\n'
            f'tokio={{path={json.dumps(str(ROOT / "src/foundation/concurrency/tokio"))},default-features=false,features=["rt","net","io-util","time","sync","macros"]}}\n'
            '[[test]]\nname="reqwest_retry"\npath="test/reqwest_retry.rs"\n'
        )
        (work / 'src/lib.rs').write_text(
            'pub fn router() -> http_routing::Router { http_routing::Router::new().route("/", http_routing::routing::get(|| async { "ok" })) }\n'
            'pub fn client() -> reqwest::Client { reqwest::Client::new() }\n'
        )
        run(work, 'generate-lockfile')
        run(work, 'check', '--locked')
        (work / 'test/reqwest_retry.rs').write_bytes(
            (ROOT / 'test/architecture/service-stack/reqwest_retry.rs').read_bytes()
        )
        output = run(work, 'test', '--locked', '--test', 'reqwest_retry')
        print(output, end='', flush=True)
        assert re.search(r'test result: ok\. 3 passed; 0 failed; 0 ignored', output), output
        print('service-stack: 7 native/WASM feature checks, 4 behavior tests, 6 util / 7 full RNG tests, Axum/Reqwest consumers and 3 retry scope tests passed')

if __name__ == '__main__':
    main()
