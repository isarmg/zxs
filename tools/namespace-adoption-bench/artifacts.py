#!/usr/bin/env python3
"""Seal this verification run's two builds; authenticate them before each pair."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tomllib
from attribution_overlay import install_overlay, overlay_identity, effective_build_tree, overlay_for

MODES = ('final', 'observer-validation', 'attribution', 'history-trend')
HARNESS_FILES = ('artifacts.py', 'run.py', 'summarize.py', 'verification.yml', 'inputs.json',
                 'observer_probe.py', 'observer-validation.yml', 'observer_compare.py',
                 'history-trend-inputs.json', 'history_trend_summary.py', 'attribution_overlay.py', 'attribution-overlay.json', 'attribution_summary.py', 'attribution-inputs.json',
                 'attribution-lib.rs', 'attribution-namespace.rs', 'attribution-segment_store.rs',
                 'attribution-journal.rs', 'attribution-query.rs')
TARGET = 'x86_64-unknown-linux-gnu'
RUST_FLAGS = '-C\x1ftarget-cpu=x86-64'
C_FLAGS = '-march=x86-64 -mtune=generic'
WIRING = (b'\n[[bin]]\nname="task1_adoption"\n'
          b'path="../../test/engine/bench/task1_adoption.rs"\ntest=false\nbench=false\n')


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def command(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True,
                                   env={**os.environ, 'LC_ALL': 'C'}, stderr=subprocess.STDOUT)


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')


def load_inputs(mode, path=None):
    """Attribution has separate pins and cannot enter the final/observer gate."""
    require(mode in MODES, f'explicit validation mode required: {mode}')
    require((mode == 'attribution') == (os.environ.get('ZXS_BENCH_MODE') == 'attribution'),
            'attribution mode and environment must agree in both directions')
    require((mode == 'history-trend') == (os.environ.get('ZXS_BENCH_MODE') == 'history-trend'),
            'history trend mode and environment must agree in both directions')
    filename = ('history-trend-inputs.json' if mode == 'history-trend' else
                'attribution-inputs.json' if mode == 'attribution' else 'inputs.json')
    path = Path(path) if path is not None else Path(__file__).with_name(filename)
    config = json.loads(path.read_text())
    if mode == 'history-trend':
        require(set(config) == {'schema', 'baseline', 'candidate'} and
                config['schema'] == 'namespace-history-trend-inputs-v1', 'invalid trend inputs')
        products = dict(baseline=config['baseline'], candidate=config['candidate'])
    elif mode == 'attribution':
        require(set(config) == {'schema', 'control', 'instrumented'} and
                config['schema'] == 'namespace-attribution-inputs-v1',
                'unsupported attribution configuration')
        products = dict(baseline=config['control'], candidate=config['instrumented'])
    else:
        require(set(config) == {'schema', 'baseline', 'final_candidate', 'observer_validation_candidate'} and
                config['schema'] == 'namespace-adoption-inputs-v1', 'unsupported pinned input configuration')
        candidate = config['final_candidate' if mode == 'final' else 'observer_validation_candidate']
        require(candidate is not None, f'{mode}: candidate SHA/tree/adapter identity is pending; execution forbidden')
        products = dict(baseline=config['baseline'], candidate=candidate)
    for label, identity in products.items():
        require(isinstance(identity, dict) and
                set(identity) == {'sha', 'tree', 'adapter_path', 'adapter_sha256'},
                f'{label}: complete pinned source and adapter identity required')
        for field, length in (('sha', 40), ('tree', 40), ('adapter_sha256', 64)):
            require(isinstance(identity[field], str) and
                    re.fullmatch(f'[0-9a-f]{{{length}}}', identity[field]) is not None,
                    f'{label}: invalid {field}')
        adapter = Path(identity['adapter_path'])
        require(not adapter.is_absolute() and '..' not in adapter.parts and
                adapter.parent == Path('test/engine/bench/namespace_adoption') and adapter.suffix == '.rs',
                f'{label}: adapter must name a confined standalone Rust benchmark')
    return products


def selected_inputs(mode, harness):
    filename = ('history-trend-inputs.json' if mode == 'history-trend' else
                'attribution-inputs.json' if mode == 'attribution' else 'inputs.json')
    config = harness / 'tools/namespace-adoption-bench' / filename
    products = load_inputs(mode, config)
    for label, identity in products.items():
        require(digest(harness / identity['adapter_path']) == identity['adapter_sha256'],
                f'{label}: adapter does not match pinned bytes')
    require(mode != 'attribution' or os.environ.get('ZXS_BENCH_MODE') == 'attribution',
            'attribution mode requires explicit attribution environment')
    return dict(mode=mode, config_sha256=digest(config), products=products,
                diagnostic_overlay_sha256=(digest(config.with_name('attribution-overlay.json'))
                                           if mode == 'attribution' else None))


def prepare_products(workspace, harness, evidence, mode):
    selected = selected_inputs(mode, harness)
    # Authenticate both clean checkouts before installing either standalone adapter.
    for label, identity in selected['products'].items():
        product = workspace / label
        require(command('git', '-C', str(product), 'rev-parse', 'HEAD').strip() == identity['sha'] and
                command('git', '-C', str(product), 'rev-parse', 'HEAD^{tree}').strip() == identity['tree'],
                f'{label}: checkout does not match selected {mode} input')
        require(not command('git', '-C', str(product), 'status', '--porcelain').strip(),
                f'{label}: product checkout must be clean before adapter installation')
    sources = {}
    for label, identity in selected['products'].items():
        product = workspace / label
        adapter = harness / identity['adapter_path']
        install_overlay(product, identity)
        manifest = product / 'src/engine/Cargo.toml'
        original = manifest.read_bytes()
        require(b'task1_adoption' not in original, f'{label}: benchmark wiring already exists')
        destination = product / 'test/engine/bench/task1_adoption.rs'
        destination.parent.mkdir(parents=True, exist_ok=True)
        with destination.open('xb') as output:
            output.write(adapter.read_bytes())
        manifest.write_bytes(original + WIRING)
        sources[label] = source_identity(product, adapter, identity)
    evidence.mkdir(parents=True, exist_ok=True)
    write_json(evidence / 'input-identity.json', dict(selected=selected, products=sources,
                                                   workflow_sha=os.environ['GITHUB_SHA']))


def host_identity():
    release = {}
    for line in Path('/etc/os-release').read_text().splitlines():
        if '=' in line:
            key, value = line.split('=', 1)
            release[key] = value.strip('"')
    identity = dict(system=platform.system(), machine=platform.machine(),
                    distribution=release['ID'], version=release['VERSION_ID'],
                    runner_os=os.environ['RUNNER_OS'], runner_arch=os.environ['RUNNER_ARCH'],
                    image_os=os.environ.get('ImageOS'), image_version=os.environ.get('ImageVersion'),
                    kernel=platform.release(), libc=list(platform.libc_ver()))
    require((identity['system'], identity['machine'], identity['distribution'], identity['version'],
             identity['runner_os'], identity['runner_arch']) ==
            ('Linux', 'x86_64', 'ubuntu', '24.04', 'Linux', 'X64'),
            f'unsupported benchmark runner: {identity}')
    return identity


def build_environment():
    # These names affect compiler selection, code generation or Cargo inputs; no secrets are read.
    names = sorted(name for name in os.environ if name in {
        'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC', 'RUSTC_WRAPPER',
        'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_TARGET', 'CARGO_INCREMENTAL',
        'CARGO_BUILD_JOBS', 'CARGO_NET_OFFLINE', 'CARGO_HOME', 'CC', 'CXX', 'CFLAGS', 'CXXFLAGS',
        'CPPFLAGS', 'LDFLAGS', 'AR', 'LD', 'HOST_CC', 'TARGET_CC', 'HOST_CFLAGS',
        'TARGET_CFLAGS', 'RUSTUP_TOOLCHAIN', 'PKG_CONFIG_PATH', 'PKG_CONFIG_LIBDIR',
    } or name.startswith(('CARGO_PROFILE_', 'CARGO_TARGET_', 'CC_', 'CXX_', 'CFLAGS_',
                          'CXXFLAGS_', 'AR_', 'RANLIB_', 'RUSTFLAGS_')))
    environment = {name: os.environ[name] for name in names}
    require(environment.get('CARGO_ENCODED_RUSTFLAGS') == RUST_FLAGS,
            'build must explicitly use portable target-cpu=x86-64')
    require(not environment.get('RUSTFLAGS'), 'RUSTFLAGS must not override portable flags')
    require(environment.get('CFLAGS') == C_FLAGS, 'C build must use portable x86-64 flags')
    require(environment.get('CARGO_INCREMENTAL') == '0', 'incremental builds are not admitted')
    require(environment.get('CARGO_NET_OFFLINE') == 'true', 'build must remain offline')
    require(environment.get('CARGO_BUILD_TARGET') == TARGET, 'Cargo target differs from fixed target')
    require(not any(value for key, value in environment.items()
                    if key in ('RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER')
                    or key.startswith(('CARGO_PROFILE_', 'CARGO_TARGET_', 'RUSTFLAGS_'))),
            'unreviewed compiler/profile/target override')
    return environment


def source_identity(product, adapter, expected):
    def git(*args):
        return command('git', '-C', str(product), *args).strip()
    label = product.name
    sha, tree = expected['sha'], expected['tree']
    require(git('rev-parse', 'HEAD') == sha and git('rev-parse', 'HEAD^{tree}') == tree,
            f'{label}: product SHA/tree mismatch')
    overlay_paths, diagnostic_overlay = overlay_identity(product, expected)
    require(git('diff', 'HEAD', '--name-only').splitlines() == sorted(['src/engine/Cargo.toml', *overlay_paths]),
            f'{label}: unexpected tracked source changes')
    require(git('ls-files', '--others', '--exclude-standard').splitlines() ==
            ['test/engine/bench/task1_adoption.rs'], f'{label}: unexpected untracked source')
    require(tomllib.loads((product / 'rust-toolchain.toml').read_text())['toolchain']['channel'] ==
            '1.99.0', f'{label}: toolchain pin changed')
    original = subprocess.check_output(['git', '-C', str(product), 'show',
                                        'HEAD:src/engine/Cargo.toml'])
    require((product / 'src/engine/Cargo.toml').read_bytes() == original + WIRING,
            f'{label}: standalone manifest wiring differs')
    require(digest(adapter) == expected['adapter_sha256'] and
            digest(product / 'test/engine/bench/task1_adoption.rs') == expected['adapter_sha256'],
            f'{label}: independently reviewed adapter bytes differ')
    inputs = ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml',
              'compliance/rust-sources.json', 'src/engine/Cargo.toml')
    return dict(sha=sha, tree=tree, adapter_sha256=expected['adapter_sha256'],
                diagnostic_overlay=diagnostic_overlay,
                effective_build_tree=effective_build_tree(product, overlay_paths),
                tracked_inventory_scope='base_git_tree_only; effective_build_tree includes all admitted overlays',
                manifest_before_sha256=hashlib.sha256(original).hexdigest(),
                inputs={name: dict(sha256=digest(product / name),
                                  source_blob=git('rev-parse', f'HEAD:{name}')) for name in inputs},
                tracked_source_inventory_sha256=hashlib.sha256(
                    command('git', '-C', str(product), 'ls-tree', '-r', '--full-tree', 'HEAD').encode()
                ).hexdigest())


def compiler_evidence(product, evidence, binary):
    label = product.name
    rust = command('rustc', '-vV', cwd=product)
    cargo = command('cargo', '--version', '--verbose', cwd=product)
    require(rust.startswith('rustc 1.99.0 ') and cargo.startswith('cargo 1.99.0 '),
            f'{label}: fixed Rust/Cargo 1.99.0 required')
    require(f'host: {TARGET}\n' in rust, f'{label}: rustc host mismatch')
    expected = ['cargo', 'build', '--locked', '--offline', '--release', '--target', TARGET,
                '-p', 'storage-engine', '--bin', 'task1_adoption', '--message-format=json', '-vv']
    actual = shlex.split((evidence / f'{label}-build-command.txt').read_text())
    require(actual == expected, f'{label}: build command differs: {actual}')
    records = [json.loads(line) for line in (evidence / f'{label}-build.jsonl').read_text().splitlines()
               if line.startswith('{')]
    require([row['success'] for row in records if row['reason'] == 'build-finished'] == [True],
            f'{label}: successful build-finished record missing')
    artifacts = [row for row in records if row['reason'] == 'compiler-artifact']
    engine = [row for row in artifacts
              if Path(row['manifest_path']).resolve() == (product / 'src/engine/Cargo.toml').resolve()]
    require(any(row['target']['kind'] == ['lib'] for row in engine),
            f'{label}: actual engine library features missing')
    selected = [row for row in engine if row['target']['kind'] == ['bin'] and
                row['target']['name'] == 'task1_adoption']
    require(len(selected) == 1, f'{label}: expected exactly one executable artifact')
    require(Path(selected[0]['executable']).resolve() == binary.resolve(),
            f'{label}: executable path is not Cargo output')
    for row in engine:
        require(not set(row['features']) & {'fault-injection', 'test-support'},
                f'{label}: development engine feature enabled')
        require(row['profile']['opt_level'] == '3' and not row['profile']['debug_assertions']
                and not row['profile']['test'], f'{label}: executable/library profile is not release')
    return dict(command=actual, target=TARGET, profile='release', rustc_verbose=rust,
                cargo_verbose=cargo, environment=build_environment(),
                c_toolchain={name: command(name, '--version') for name in ('cc', 'c++', 'ld', 'ar')},
                actual_engine_artifacts=engine,
                actual_compilation_units=[dict(package_id=row['package_id'], target=row['target'],
                                               profile=row['profile'], features=row['features'])
                                          for row in artifacts])


def elf_identity(binary):
    with binary.open('rb') as source:
        header = source.read(20)
    require(header[:6] == b'\x7fELF\x02\x01' and header[18:20] == b'\x3e\x00',
            f'{binary}: expected little-endian ELF64 x86-64')
    elf = {name: command('readelf', option, str(binary)) for name, option in
           (('header', '-h'), ('program_headers', '-l'), ('dynamic', '-d'),
            ('versions', '--version-info'), ('notes', '-n'))}
    require('x86-64-v2' not in elf['notes'] and 'x86-64-v3' not in elf['notes'] and
            'x86-64-v4' not in elf['notes'], f'{binary}: nonbaseline x86-64 ISA requirement')
    # Runtime-dispatched SIMD implementations may exist; they do not change the global ISA.
    elf['ldd_verbose'] = command('ldd', '-v', str(binary))
    require('not found' not in elf['ldd_verbose'], f'{binary}: unresolved runtime dependency/version')
    # Version-information headings end in ':'; only address-bearing resolver rows are paths.
    libraries = sorted(set(re.findall(r'(?:=>\s+|^\s*)(/[^\s()]+)\s+\(0x[0-9a-f]+\)',
                                      elf['ldd_verbose'], re.MULTILINE)))
    require(libraries, f'{binary}: dynamic loader dependencies were not reported')
    elf['resolved_libraries'] = {name: digest(name) for name in libraries}
    return elf


def seal_bundle(workspace, harness, evidence, bundle_dir, binaries, mode):
    selected = selected_inputs(mode, harness)
    prepared = json.loads((evidence / 'input-identity.json').read_text())
    require(prepared['selected'] == selected and prepared['workflow_sha'] == os.environ['GITHUB_SHA'],
            'prepared products belong to different pinned inputs or carrier')
    bundle_dir.mkdir(parents=True, exist_ok=False)
    (bundle_dir / 'bin').mkdir()
    (bundle_dir / 'adapters').mkdir()
    source_records = {}
    for label, identity in selected['products'].items():
        product = workspace / label
        adapter = harness / identity['adapter_path']
        source = source_identity(product, adapter, identity)
        require(source == prepared['products'][label], f'{label}: prepared source identity changed during build')
        compiler = compiler_evidence(product, evidence, binaries[label])
        destination = bundle_dir / 'bin' / label
        shutil.copyfile(binaries[label], destination)
        destination.chmod(0o755)
        shutil.copyfile(adapter, bundle_dir / 'adapters' / f'{label}.rs')
        source_records[label] = dict(source=source, compiler=compiler,
                                     binary_sha256=digest(destination), elf=elf_identity(destination))
    shutil.copytree(evidence, bundle_dir / 'evidence')
    manifest = dict(schema='namespace-adoption-build-once-v2', selected_inputs=selected,
                    scope='binary reuse only; every requested measurement executes anew',
                    workflow_sha=os.environ['GITHUB_SHA'], run_id=os.environ['GITHUB_RUN_ID'],
                    run_attempt=os.environ['GITHUB_RUN_ATTEMPT'], build_host=host_identity(),
                    harness={name: digest(harness / 'tools/namespace-adoption-bench' / name)
                             for name in HARNESS_FILES},
                    tools=dict(python=sys.version,
                               strace=command('strace', '--version'),
                               readelf=command('readelf', '--version'), git=command('git', '--version')),
                    products=source_records,
                    evidence={str(path.relative_to(bundle_dir)): digest(path)
                              for path in sorted((bundle_dir / 'evidence').rglob('*')) if path.is_file()})
    write_json(bundle_dir / 'manifest.json', manifest)
    return digest(bundle_dir / 'manifest.json')


def verify_bundle(bundle_dir, adapter_paths, audit_dir, *, mode):
    """Return authenticated executables; preserve a failure audit before propagating errors."""
    bundle_dir, audit_dir = Path(bundle_dir), Path(audit_dir)
    audit_dir.mkdir(parents=True, exist_ok=True)
    audit = dict(status='failed', mode=mode,
                 scope='artifact identity and runtime compatibility; no test-result reuse')
    try:
        directory = Path(__file__).resolve().parent
        harness = directory.parents[1]
        selected = selected_inputs(mode, harness)
        audit['selected_inputs'] = selected
        audit['manifest_sha256'] = digest(bundle_dir / 'manifest.json')
        require(audit['manifest_sha256'] == os.environ['ZXS_EXPECTED_MANIFEST_SHA256'],
                'manifest hash differs from this run\'s build-job output')
        manifest = json.loads((bundle_dir / 'manifest.json').read_text())
        require(manifest['schema'] == 'namespace-adoption-build-once-v2', 'unsupported artifact schema')
        require(manifest['selected_inputs'] == selected,
                'artifact mode or pinned source/adapter/config identity differs')
        require(manifest['workflow_sha'] == os.environ['GITHUB_SHA'] and
                manifest['run_id'] == os.environ['GITHUB_RUN_ID'] and
                manifest['run_attempt'] == os.environ['GITHUB_RUN_ATTEMPT'],
                'artifact belongs to a different workflow commit/run/attempt')
        require(set(manifest['harness']) == set(HARNESS_FILES), 'incomplete harness identity')
        for name, expected in manifest['harness'].items():
            require(digest(directory / name) == expected, f'{name}: harness bytes differ from build job')
        audit['host'] = host_identity()
        for key in ('system', 'machine', 'distribution', 'version', 'runner_os', 'runner_arch'):
            require(audit['host'][key] == manifest['build_host'][key], f'runner {key} differs')
        for relative, expected in manifest['evidence'].items():
            require(digest(bundle_dir / relative) == expected, f'{relative}: raw build evidence changed')
        binaries = {}
        audit['products'] = {}
        require(set(manifest['products']) == set(selected['products']), 'product set differs')
        for label, identity in selected['products'].items():
            sha, tree = identity['sha'], identity['tree']
            entry = manifest['products'][label]
            require((entry['source']['sha'], entry['source']['tree']) == (sha, tree),
                    f'{label}: frozen product identity differs')
            require(Path(adapter_paths[label]).resolve() == (harness / identity['adapter_path']).resolve(),
                    f'{label}: adapter path differs from selected input')
            require(digest(adapter_paths[label]) == identity['adapter_sha256'] ==
                    digest(bundle_dir / 'adapters' / f'{label}.rs') == entry['source']['adapter_sha256'],
                    f'{label}: adapter bytes differ')
            if mode == 'history-trend':
                require(entry['source'].get('diagnostic_overlay') is None,
                        f'{label}: trend forbids production instrumentation')
                require(re.fullmatch('[0-9a-f]{40}', entry['source'].get('effective_build_tree', '')),
                        f'{label}: trend effective build tree missing')
            if mode == 'attribution':
                overlay = overlay_for(identity)
                actual_overlay = entry['source'].get('diagnostic_overlay')
                require((actual_overlay is None) == (overlay is None),
                        f'{label}: diagnostic overlay identity missing or unexpected')
                if overlay is not None:
                    require(actual_overlay['descriptor_sha256'] == selected['diagnostic_overlay_sha256'] and
                            actual_overlay['files'] == overlay['files'],
                            f'{label}: instrumented overlay differs from selected diagnostic')
                require(re.fullmatch('[0-9a-f]{40}', entry['source'].get('effective_build_tree', '')),
                        f'{label}: effective build tree missing')
            binary = bundle_dir / 'bin' / label
            require(digest(binary) == entry['binary_sha256'], f'{label}: binary hash differs')
            binary.chmod(0o755)  # Actions download restores files without executable mode.
            runtime = elf_identity(binary)
            for field in ('header', 'program_headers', 'dynamic', 'versions', 'notes'):
                require(runtime[field] == entry['elf'][field], f'{label}: ELF {field} differs')
            audit['products'][label] = dict(binary_sha256=digest(binary), source_sha=sha,
                                           source_tree=tree, runtime=runtime,
                                           diagnostic_overlay=entry['source'].get('diagnostic_overlay'),
                                           effective_build_tree=entry['source'].get('effective_build_tree'))
            binaries[label] = binary.resolve()
        audit['status'] = 'verified'
        return binaries
    except (OSError, KeyError, TypeError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        audit.update(error_type=type(error).__name__, error=str(error))
        raise
    finally:
        write_json(audit_dir / 'identity-verification.json', audit)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['resolve', 'prepare', 'seal', 'verify'])
    parser.add_argument('--mode', choices=MODES, required=True)
    parser.add_argument('--workspace', type=Path)
    parser.add_argument('--harness', type=Path, required=True)
    parser.add_argument('--evidence', type=Path)
    parser.add_argument('--bundle-dir', type=Path)
    parser.add_argument('--baseline-binary', type=Path)
    parser.add_argument('--candidate-binary', type=Path)
    parser.add_argument('--audit-dir', type=Path)
    args = parser.parse_args()
    if args.action == 'resolve':
        print(json.dumps(selected_inputs(args.mode, args.harness.resolve()), sort_keys=True))
    elif args.action == 'prepare':
        require(args.workspace is not None and args.evidence is not None,
                'prepare requires workspace and evidence paths')
        prepare_products(args.workspace.resolve(), args.harness.resolve(), args.evidence.resolve(), args.mode)
    elif args.action == 'seal':
        require(all((args.workspace, args.evidence, args.bundle_dir, args.baseline_binary, args.candidate_binary)),
                'seal requires workspace, evidence, bundle-dir and both binary paths')
        print(seal_bundle(args.workspace.resolve(), args.harness.resolve(), args.evidence.resolve(),
                          args.bundle_dir.resolve(), {'baseline': args.baseline_binary,
                                                      'candidate': args.candidate_binary}, args.mode))
    else:
        require(args.audit_dir is not None and args.bundle_dir is not None,
                'verify requires audit-dir and bundle-dir')
        products = load_inputs(args.mode)
        paths = {label: args.harness / identity['adapter_path'] for label, identity in products.items()}
        verify_bundle(args.bundle_dir, paths, args.audit_dir, mode=args.mode)


if __name__ == '__main__':
    main()

