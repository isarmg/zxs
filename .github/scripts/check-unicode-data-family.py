#!/usr/bin/env python3
"""Check ICU runtime/table consolidation without changing root source or lock files."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

import runpy

ROOT = Path(__file__).resolve().parents[2]
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']
TARGETS = ('x86_64-unknown-linux-gnu', 'wasm32-unknown-unknown')
RETIRED_FEATURES = {
    'unicode-normalizer': ('utf16_iter', 'write16', 'harfbuzz_traits', 'datagen'),
    'unicode-properties': ('harfbuzz_traits', 'unicode_bidi', 'datagen'),
}
SUPPORTED_FEATURES = {
    'unicode-normalizer': ('compiled_data', 'default', 'serialization', 'unicode-properties', 'utf8_iter'),
    'unicode-properties': ('alloc', 'compiled_data', 'default', 'log', 'serialization', 'unstable'),
}
# Minimum/compiled-data profiles are exercised independently by check_family.
# Keep serialization isolated from baked data and optional normalizer properties.
CAPABILITY_PROFILES = {
    'unicode-normalizer': (('utf8_iter',), ('unicode-properties',), ('serialization',),
                           ('serialization', 'unicode-properties'), SUPPORTED_FEATURES['unicode-normalizer']),
    'unicode-properties': (('serialization',), ('serialization', 'alloc'),
                           ('alloc', 'unstable'), SUPPORTED_FEATURES['unicode-properties']),
}



def run(root, args, *, capture=False, env=None, negative=False):
    result = subprocess.run(['cargo', *args, '--color', 'never'], cwd=root,
                            capture_output=capture, text=True, env=env, timeout=600)
    if not negative:
        result.check_returncode()
    elif result.returncode == 0:
        raise AssertionError('negative Cargo probe unexpectedly succeeded')
    return result


def manifest_capability_violations(manifest, workspace_dependencies=None):
    """Resolve aliases and weak forwarding, including workspace-inherited edges."""
    aliases = {}
    violations = []
    inherited = workspace_dependencies or {}

    def dependencies(value):
        if not isinstance(value, dict):
            return
        for key, child in value.items():
            if key in ('dependencies', 'dev-dependencies', 'build-dependencies') and isinstance(child, dict):
                for alias, dependency in child.items():
                    if not isinstance(dependency, dict):
                        dependency = {}
                    parent = inherited.get(alias, {}) if dependency.get('workspace') else {}
                    if not isinstance(parent, dict):
                        parent = {}
                    name = dependency.get('package', parent.get('package', alias))
                    aliases.setdefault(alias, set()).add(name)
                    removed = set(RETIRED_FEATURES.get(name, ()))
                    selected = set(dependency.get('features', ())) | set(parent.get('features', ()))
                    for feature in sorted(removed & selected):
                        violations.append(f'{alias}/{feature}')
            else:
                dependencies(child)

    dependencies(manifest)
    own = manifest.get('package', {}).get('name')
    for feature, forwards in manifest.get('features', {}).items():
        if feature in RETIRED_FEATURES.get(own, ()):
            violations.append(feature)
        for forward in forwards:
            if '/' in forward:
                alias, selected = forward.split('/', 1)
                names = aliases.get(alias.rstrip('?'), {alias.rstrip('?')})
                if any(selected in RETIRED_FEATURES.get(name, ()) for name in names):
                    violations.append(forward)
            elif forward in RETIRED_FEATURES.get(own, ()):
                violations.append(forward)
    return sorted(set(violations))


def validate_capability_surface(root):
    workspace = tomllib.loads((root / 'Cargo.toml').read_text()).get('workspace', {}).get('dependencies', {})
    for name, expected in SUPPORTED_FEATURES.items():
        manifest = tomllib.loads((foundation_path(root, name) / 'Cargo.toml').read_text())
        if set(manifest['features']) != set(expected):
            raise AssertionError('Unicode supported capability surface drift: ' + name)
    # Historical provenance manifests are immutable records, not retained consumers.
    manifests = [root / 'Cargo.toml']
    for directory in ('src', 'test', 'tools', 'rescue'):
        manifests.extend(path for path in (root / directory).rglob('Cargo.toml')
                         if 'target' not in path.relative_to(root).parts)
    for path in manifests:
        violations = manifest_capability_violations(tomllib.loads(path.read_text()), workspace)
        if violations:
            raise AssertionError('retired Unicode feature consumer in ' + str(path.relative_to(root))
                                 + ': ' + ', '.join(violations))


def validate_unknown_feature(result, name, feature):
    # Cargo dependency-resolution diagnostics are intentionally required. An
    # unresolved crate/provider or Rust compile failure is not a passing rejection.
    diagnostic = result.stderr
    expected = (f'with feature `{feature}` but `{name}` does not have that feature',
                f'package `{name}` does not contain this feature: {feature}')
    if result.returncode == 0 or not any(text in diagnostic for text in expected):
        raise AssertionError('retired feature did not fail cleanly at Cargo resolution: '
                             + name + '/' + feature + '\n' + diagnostic)
    if 'error[E' in diagnostic or 'could not compile' in diagnostic:
        raise AssertionError('retired feature reached Rust compilation: ' + name + '/' + feature)


def capability_probe(root, temporary, name, label, features):
    work = temporary / (name + '-capability-' + label)
    (work / 'src').mkdir(parents=True)
    (work / 'src/lib.rs').write_text('#![no_std]\npub use ' + name.replace('-', '_') + '::provider;\n')
    manifest = ('[package]\nname = "unicode-capability-probe"\nversion = "0.0.0"\n'
                'edition = "2024"\n[workspace]\n[dependencies.' + name + ']\n'
                'path = ' + json.dumps(str(foundation_path(root, name))) + '\n'
                'default-features = false\nfeatures = ' + json.dumps(features) + '\n'
                '[profile.dev]\ndebug = 0\nincremental = false\n')
    (work / 'Cargo.toml').write_text(manifest)
    return ['--offline', '--manifest-path', str(work / 'Cargo.toml')]


def check_capabilities(root, temporary, target, family):
    name = 'unicode-' + family
    for features in CAPABILITY_PROFILES[name]:
        common = capability_probe(root, temporary, name, '-'.join(features), features)
        run(root, ['generate-lockfile', *common])
        for platform in TARGETS:
            run(root, ['check', '--locked', *common, '--jobs', '1', '--target', platform,
                       '--target-dir', str(target)])
    for feature in RETIRED_FEATURES[name]:
        common = capability_probe(root, temporary, name, 'retired-' + feature, [feature])
        result = run(root, ['check', *common, '--jobs', '1', '--target-dir', str(target)],
                     capture=True, negative=True)
        validate_unknown_feature(result, name, feature)
        print('Clean retired-feature rejection: ' + name + '/' + feature)


def validate_graph(metadata, name, compiled, tree):
    nodes = {row['id']: row for row in metadata['resolve']['nodes']}
    packages = {row['name']: row for row in metadata['packages'] if row['id'] in nodes}
    if name.replace('unicode-', 'icu_') + '_data' in packages:
        raise AssertionError('absorbed generated-data package remains active')
    own = nodes[packages[name]['id']]['features']
    if ('compiled_data' in own) != compiled:
        raise AssertionError('compiled_data feature isolation failed')
    if ('baked' in nodes[packages['icu_provider']['id']]['features']) != compiled:
        raise AssertionError('provider-only build activates baked support')
    # Cargo metadata includes weak optional feature edges; cargo tree reports
    # the actually built normal dependencies without this over-approximation.
    if name == 'unicode-normalizer' and 'unicode-properties' in tree:
        raise AssertionError('minimal normalizer unexpectedly activates optional properties')


def validate_table_inputs(root, family):
    """Table bytes and build selection stay equal to their exact input receipt."""
    catalog = json.loads((root / 'compliance/rust-sources.json').read_text())
    destination = 'src/foundation/unicode-' + family
    matches = [record for record in catalog['consolidations'].values()
               if record['destination_directory'] == destination]
    if len(matches) != 1:
        raise AssertionError('Unicode data consolidation receipt missing or ambiguous')
    record = matches[0]
    old = 'src/foundation/icu_' + family + '_data'
    package = next(row['package'] for row in record['inputs'] if row['package']['directory'] == old)
    for source, expected in package['files_sha256'].items():
        if source.startswith('data/') or source == 'build.rs':
            target = record['file_routes'][old][source]['destination']
            actual = hashlib.sha256((foundation_path(root, 'unicode-' + family) / target).read_bytes()).hexdigest()
            if actual != expected:
                raise AssertionError('generated Unicode table/build input changed: ' + source)


def check_family(root, temporary, target, family):
    validate_table_inputs(root, family)
    name = 'unicode-' + family
    crate = 'unicode_' + family
    if (foundation_path(root, name.replace('unicode-', 'icu_') + '_data')).exists():
        raise AssertionError('retired generated-data source package remains')
    feature_sets = ((), ('compiled_data',)) if family == 'normalizer' else ((), ('alloc',), ('compiled_data',), ('compiled_data', 'alloc'))
    for features in feature_sets:
        compiled = 'compiled_data' in features
        work = temporary / (family + '-' + ('-'.join(features) or 'provider'))
        (work / 'src').mkdir(parents=True)
        lib = work / 'src/lib.rs'
        lib.write_text('#![no_std]\npub use ' + crate + '::provider;\n')
        manifest = ('[package]\nname = "unicode-data-probe"\nversion = "0.0.0"\n'
                    'edition = "2024"\n[workspace]\n[dependencies.' + name + ']\n'
                    'path = ' + json.dumps(str(foundation_path(root, name))) + '\n'
                    'default-features = false\nfeatures = ' + json.dumps(features) + '\n'
                    '[profile.dev]\ndebug = 0\nincremental = false\n'
                    '[profile.test]\ndebug = 0\nincremental = false\n')
        (work / 'Cargo.toml').write_text(manifest)
        common = ['--offline', '--manifest-path', str(work / 'Cargo.toml')]
        run(root, ['generate-lockfile', *common])
        metadata = run(root, ['metadata', '--locked', '--format-version', '1', *common], capture=True)
        tree = run(root, ['tree', '--locked', *common, '--edges', 'normal', '--prefix', 'none', '--format', '{p}'], capture=True)
        validate_graph(json.loads(metadata.stdout), name, compiled, tree.stdout)
        for platform in TARGETS:
            check = ['check', '--locked', *common, '--jobs', '1', '--target', platform,
                     '--target-dir', str(target)]
            run(root, check)
        if compiled:
            # Use the real unchanged generated tables, never a fabricated provider.
            custom = dict(os.environ, ICU4X_DATA_DIR=str(foundation_path(root, name) / 'src/data'))
            run(root, check, env=custom)
        else:
            lib.write_text('#![no_std]\npub use ' + crate + '::provider::Baked;\n')
            failure = run(root, check, capture=True, negative=True)
            if 'E0432' not in failure.stderr or 'Baked' not in failure.stderr:
                raise AssertionError('negative baked-data probe failed for an unrelated reason')
    work = temporary / (family + '-behavior')
    work.mkdir()
    features = ['compiled_data', 'utf8_iter'] if family == 'normalizer' else ['compiled_data', 'alloc']
    manifest = ('[package]\nname = "unicode-data-behavior"\nversion = "0.0.0"\n'
                'edition = "2024"\n[workspace]\n[dependencies.' + name + ']\n'
                'path = ' + json.dumps(str(foundation_path(root, name))) + '\n'
                'default-features = false\nfeatures = ' + json.dumps(features) + '\n')
    source = root / ('test/unicode-data/' + family + '.rs')
    manifest += ('[[test]]\nname = "unicode_behavior"\npath = ' + json.dumps(str(source)) + '\n'
                 '[profile.dev]\ndebug = 0\nincremental = false\n'
                 '[profile.test]\ndebug = 0\nincremental = false\n')
    (work / 'Cargo.toml').write_text(manifest)
    common = ['--offline', '--manifest-path', str(work / 'Cargo.toml')]
    run(root, ['generate-lockfile', *common])
    run(root, ['test', '--locked', *common, '--jobs', '1', '--target-dir', str(target)])
    if family == 'properties':
        run(root, ['test', '--locked', *common, '--lib', '-p', name, '--jobs', '1', '--target-dir', str(target)])


    # Documented public parser APIs require unstable; keep minimal graph probes above separate.
    doc_features = ['compiled_data', 'utf8_iter', 'unicode-properties'] if family == 'normalizer' else ['compiled_data', 'alloc', 'unstable']
    (work / 'Cargo.toml').write_text(manifest.replace('features = ' + json.dumps(features), 'features = ' + json.dumps(doc_features)))
    run(root, ['generate-lockfile', *common])
    run(root, ['test', '--locked', *common, '--doc', '-p', name, '--jobs', '1', '--target-dir', str(target)])


def check(root=ROOT, families=('normalizer', 'properties'), capabilities_only=False):
    root = Path(root).resolve()
    lock = hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest()
    validate_capability_surface(root)
    scratch = root / 'target/scope-consumers'
    scratch.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('ICU_FAMILY_TARGET_DIR', str(root / 'target/unicode-data-consumers')))
    with tempfile.TemporaryDirectory(prefix='unicode-data-', dir=scratch) as directory:
        for family in families:
            check_capabilities(root, Path(directory), target, family)
            if not capabilities_only:
                check_family(root, Path(directory), target, family)
    if hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest() != lock:
        raise AssertionError('Unicode gate changed the product lock file')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--family', choices=('normalizer', 'properties'))
    parser.add_argument('--capabilities-only', action='store_true',
                        help='run supported profiles and clean retired-feature rejections only')
    arguments = parser.parse_args()
    check(families=(arguments.family,) if arguments.family else ('normalizer', 'properties'),
          capabilities_only=arguments.capabilities_only)
