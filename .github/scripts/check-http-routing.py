#!/usr/bin/env python3
"""Check routing/kernel identity, pure WASM boundaries, macros and response behavior."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/rust-sources'))
import source_layout
BASE_FEATURES = ((), ('http1',), ('json', 'form', 'query', 'macros'), ('tokio', 'http1'), ('default',))
# Pinned original inactive source evidence, not compiled or exposed APIs.
# Current wiring may differ only through the authenticated layout receipt.
INACTIVE_SOURCES = {
    'src/extract/multipart.rs': '328db82e6b2ecc75a91dffbaa6cb5cefb103048d77926b7010d897f18160ada0',
    'src/extract/ws.rs': '478a826c84392a62d8329552875fed7264257c319fbbb12f187411612d29cdb1',
    'src/test_helpers/mod.rs': 'fd8a7c14bbf6e90382db3a0f7362866de35405513d4f8a48970b31bc036e18ac',
    'src/test_helpers/test_client.rs': '0cfd9ad0d6426f9c29c0254cfe49c5395ce8ed75a86380e81edca82aa1d70a95',
}
UNSUPPORTED_FEATURES = ('ws', 'multipart', '__private', '__private_docs', 'http2')

@source_layout.operation
def verify_inactive_sources(root):
    historical = 'src/foundation/http-routing'
    component = source_layout.physical(root, historical)
    for relative, expected in INACTIVE_SOURCES.items():
        path = component / relative
        for ancestor in (path, *path.parents):
            if ancestor == root: break
            if ancestor.is_symlink():
                raise ValueError(f'inactive evidence symlink: {relative}')
        if not path.is_file():
            raise ValueError(f'inactive evidence changed or missing: {relative}')
        try:
            original = source_layout.read(root, historical + '/' + relative)
        except (OSError, ValueError) as error:
            raise ValueError(f'inactive evidence changed or missing: {relative}') from error
        if hashlib.sha256(original).hexdigest() != expected:
            raise ValueError(f'inactive evidence changed or missing: {relative}')
    manifest = tomllib.loads((component / 'Cargo.toml').read_text())
    if set(UNSUPPORTED_FEATURES) & manifest['features'].keys():
        raise ValueError('unsupported capability advertised')
    if {'base64', 'reqwest', 'tower-http'} & manifest.get('dependencies', {}).keys():
        raise ValueError('inactive-only dependency restored')
    for relative in ('src/lib.rs', 'src/extract/mod.rs'):
        text = (component / relative).read_text()
        if re.search(r'(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(?:ws|multipart|test_helpers)\s*[;{]', text):
            raise ValueError(f'inactive module restored: {relative}')
    duplicates = root / 'compliance/provenance/retired/http-routing'
    if duplicates.exists() and any(p.is_file() for p in duplicates.rglob('*')):
        raise ValueError('duplicate inactive-source archive introduced')

def run(work, *args, rejected_feature=None):
    result = subprocess.run(['cargo', *args, '--color', 'never', '--offline'], cwd=work,
                            text=True, capture_output=True, timeout=600)
    if rejected_feature:
        assert result.returncode and f'with feature `{rejected_feature}`' in result.stderr and f'`http-routing` does not have that feature' in result.stderr, result.stderr
        return result.stdout
    if result.returncode: raise RuntimeError(result.stdout + result.stderr)
    if re.search(r'^warning:', result.stderr, re.M): raise AssertionError(result.stderr)
    return result.stdout

def main():
    verify_inactive_sources(ROOT)
    os.environ.setdefault('CARGO_BUILD_JOBS','2')
    os.environ.setdefault('CARGO_PROFILE_DEV_DEBUG','0')
    os.environ.setdefault('CARGO_INCREMENTAL','0')
    with tempfile.TemporaryDirectory(prefix='http-routing-') as temporary:
        work=Path(temporary);(work/'src').mkdir();(work/'test').mkdir()
        (work / 'rust-toolchain.toml').write_bytes((ROOT / 'rust-toolchain.toml').read_bytes())
        (work/'src/lib.rs').write_text('pub fn router() -> http_routing::Router { http_routing::Router::new() }\n')
        (work/'test/behavior.rs').write_bytes((ROOT/'test/architecture/http-routing/behavior.rs').read_bytes())
        declared = tuple(sorted(tomllib.loads((ROOT/'src/foundation/networking/http-routing/Cargo.toml').read_text())['features']))
        wasm = tuple(feature for feature in declared if feature not in {'default', 'tokio'})
        count = 0
        for features in (*BASE_FEATURES, declared, wasm):
            (work/'Cargo.toml').write_text(
                '[package]\nname="routing-contract"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\n'
                f'http-routing={{path={json.dumps(str(ROOT/"src/foundation/networking/http-routing"))},default-features=false,features={json.dumps(features)}}}\n'
                f'service-stack={{path={json.dumps(str(ROOT/"src/foundation/networking/service-stack"))},default-features=false,features=["util"]}}\n'
                f'http-body={{path={json.dumps(str(ROOT/"src/foundation/networking/http-body"))},features=["utilities"]}}\n'
                '[[test]]\nname="behavior"\npath="test/behavior.rs"\nrequired-features=["http-routing/query","http-routing/macros"]\n')
            run(work,'generate-lockfile')
            metadata=json.loads(run(work,'metadata','--locked','--format-version','1'))
            nodes={n['id']:n for n in metadata['resolve']['nodes']}
            names={p['name'] for p in metadata['packages'] if p['id'] in nodes}
            assert not names.intersection({'axum','axum-core'}), names
            runtime=bool(set(features)&{'tokio','default'})
            if not runtime:
                assert not names.intersection({'hyper','hyper-util'}), names
                packages={p['name']:p for p in metadata['packages'] if p['id'] in nodes}
                if 'http1' in features:
                    transport_features=set(nodes[packages['http-transport']['id']]['features'])
                    assert not transport_features.intersection({'adapters','adapter-server','service-adapter','tokio'}),transport_features
                    tokio_features=set(nodes[packages['tokio']['id']]['features'])
                    assert not tokio_features.intersection({'net','rt','rt-multi-thread','time'}),tokio_features
                else:
                    assert not names.intersection({'tokio','http-transport'}),names
                run(work,'check','--locked','--target','wasm32-unknown-unknown');count += 1
            run(work,'check','--locked');count += 1
            if features == ('json', 'form', 'query', 'macros'):
                result=run(work,'test','--locked','--test','behavior')
                assert re.search(r'test result: ok\. 8 passed; 0 failed',result), result
        for feature in UNSUPPORTED_FEATURES:
            run(work, 'check', '--locked', '--features', f'http-routing/{feature}', rejected_feature=feature)
        print(f'http-routing: {count} native/WASM feature checks (including all declared features), 8 routing/body/status/macro/path tests and 5 unsupported-feature rejections and 4 exact inactive-source checks passed')
if __name__=='__main__': main()
