#!/usr/bin/env python3
"""Exercise the owned calendar capability and actual serialization/tracing consumers."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import argparse
import json
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']
TARGETS = ('x86_64-unknown-linux-gnu', 'wasm32-unknown-unknown')
FEATURES = ('std', 'alloc', 'formatting', 'parsing', 'large-dates', 'serialization',
            'serialization-human-readable', 'serialization-well-known', 'wasm-bindgen')
REMOVED = ('macros', 'quickcheck', 'rand', 'rand08', 'rand09', 'local-offset')
ABSORBED = ('time', 'time-core', 'deranged', 'powerfmt', 'num-conv')
PRIVATE = ('calendar_core', 'ranged', 'display', 'num_conv')
MATRIX = ((), ('std',), ('alloc',), ('parsing',), ('alloc', 'parsing'),
          ('formatting',), ('formatting', 'parsing'), ('large-dates',),
          ('parsing', 'large-dates'), ('serialization',), ('serialization', 'alloc'),
          ('serialization-human-readable',), ('serialization-well-known',),
          ('wasm-bindgen',), FEATURES)
TEST_PROFILES = ((), ('alloc', 'parsing'), ('formatting', 'parsing'),
                 ('large-dates',), ('formatting', 'parsing', 'large-dates'))
SERIALIZATION_PROFILES = (('serialization',), ('serialization-human-readable',),
                          ('serialization-well-known',))


def structure(root):
    manifest = tomllib.loads((root / 'src/foundation/core-types/date-time/Cargo.toml').read_text())
    assert manifest['package']['name'] == 'date-time'
    assert manifest['lib']['name'] == 'date_time'
    assert set(manifest['features']) == {'default', *FEATURES}
    assert manifest['features']['default'] == ['std']
    for old in ABSORBED:
        assert not (foundation_path(root, old)).exists(), old
        assert old not in manifest.get('dependencies', {}), old
    source = (root / 'src/foundation/core-types/date-time/src/lib.rs').read_text()
    assert '#![no_std]' in source
    for module in PRIVATE:
        assert re.search(r'^mod ' + module + ';$', source, re.M), module


def manifest(root, fixture='contracts'):
    dependencies = {
        'date-time': {'path': str(root / 'src/foundation/core-types/date-time'), 'default-features': False},
        'data-serialization': {'path': str(root / 'src/foundation/data-storage/data-serialization'),
                               'default-features': False, 'optional': fixture == 'contracts'},
    }
    if fixture != 'contracts':
        dependencies['date-time']['features'] = ['std']
        dependencies['data-serialization']['features'] = ['derive']
        dependencies['json-codec'] = {'path': str(root / 'src/foundation/data-storage/json-codec')}
    if fixture == 'tracing':
        dependencies['date-time']['features'] += ['formatting', 'parsing']
        dependencies['tracing'] = {'path': str(root / 'src/foundation/diagnostics/tracing'),
                                   'default-features': False, 'features': ['time', 'fmt', 'json']}
    lines = ['[package]', f'name = "date-time-{fixture}-probe"', 'version = "0.0.0"',
             'edition = "2024"', '[workspace]', '[features]', 'default = []']
    for feature in FEATURES:
        selected = ['date-time/' + feature]
        if feature.startswith('serialization') and fixture == 'contracts':
            selected.append('dep:data-serialization')
        lines.append(f'{feature} = {json.dumps(selected)}')
    for name, values in dependencies.items():
        lines.append(f'[dependencies.{name}]')
        for key, value in values.items():
            lines.append(f'{key} = {json.dumps(value)}')
    lines += ['[[test]]', f'name = "{fixture}"',
              'path = ' + json.dumps(str(root / 'test/date-time' / f'{fixture}.rs'))]
    return '\n'.join(lines) + '\n'


def run(work, environment, command, *args, success=True):
    arguments = ['cargo', command, '--color', 'never', '--offline',
                 '--manifest-path', str(work / 'Cargo.toml')]
    if command != 'generate-lockfile':
        arguments.append('--locked')
    if command in ('build', 'check', 'test'):
        arguments += ['-j2']
    result = subprocess.run([*arguments, *args], cwd=work, env=environment,
                            capture_output=True, text=True)
    if success and (result.returncode or re.search(r'^warning(?:\[|:)', result.stderr, re.M)):
        raise AssertionError(result.stdout + result.stderr)
    return result


def prepare(root, parent, fixture):
    work = parent / fixture
    (work / 'src').mkdir(parents=True)
    shutil.copyfile(root / 'rust-toolchain.toml', work / 'rust-toolchain.toml')
    (work / 'Cargo.toml').write_text(manifest(root, fixture))
    source = (root / 'test/date-time/no_std.rs').read_text() if fixture == 'contracts' else ''
    (work / 'src/lib.rs').write_text(source)
    return work


def feature_flags(features):
    return ['--features', ','.join(features)] if features else []


def reject_features_and_private_api(work, environment):
    source = work / 'src/lib.rs'
    original = source.read_text()
    source.write_text('#![no_std]\n')
    for feature in REMOVED:
        result = run(work, environment, 'check', '--features', 'date-time/' + feature, success=False)
        assert result.returncode and feature in result.stderr and 'does not have' in result.stderr, result.stderr
        print(f'date-time rejected feature: {feature}', flush=True)
    for module in PRIVATE:
        source.write_text(f'pub use date_time::{module};\n')
        result = run(work, environment, 'check', success=False)
        assert result.returncode and 'error[E0603]' in result.stderr and module in result.stderr, result.stderr
    for expression, name in (
        ('date_time::UtcOffset::current_local_offset()', 'current_local_offset'),
        ('date_time::UtcOffset::local_offset_at(date_time::OffsetDateTime::UNIX_EPOCH)', 'local_offset_at'),
        ('date_time::OffsetDateTime::now_local()', 'now_local'),
    ):
        source.write_text('pub fn rejected() { let _ = ' + expression + '; }\n')
        result = run(work, environment, 'check', '--features', 'std', success=False)
        assert result.returncode and 'error[E0599]' in result.stderr and name in result.stderr, result.stderr
    for module in ('formatting', 'parsing', 'data_serialization', 'macros'):
        source.write_text(f'pub use date_time::{module};\n')
        result = run(work, environment, 'check', success=False)
        assert result.returncode and 'error[E0432]' in result.stderr and module in result.stderr, result.stderr
    source.write_text('pub fn rejected() { let _ = date_time::Date::MIN.format(&date_time::format_description::well_known::Rfc3339); }\n')
    result = run(work, environment, 'build', '--features', 'formatting', success=False)
    assert result.returncode and 'error[E0080]' in result.stderr and 'Rfc3339 requires date, time, and offset' in result.stderr, result.stderr
    source.write_text(original)
    print('date-time private modules, disabled APIs, and removed local-time APIs rejected', flush=True)


def check_parsing_units(root, parent, environment):
    behaviour = runpy.run_path(str(root / '.github/scripts/check-foundation-behaviour.py'))
    work = parent / 'parsing-units'
    work.mkdir()
    behaviour['standalone_member'](behaviour['Suite']('date-time', ()), work)
    run(work, environment, 'generate-lockfile')
    required = ('nine_digits_checks_every_byte_before_numeric_conversion',
                'fixed_nine_digit_subseconds_return_parse_errors_for_invalid_input',
                'period_case_sensitivity_is_preserved_for_each_letter_case')
    for defaults in ([], ['--no-default-features']):
        result = run(work, environment, 'test', '-p', 'date-time', '--lib',
                     '--features', 'parsing', *defaults, 'parsing::combinator::tests')
        print(result.stdout, end='', flush=True)
        for name in required:
            assert re.search(r'test parsing::combinator::tests::' + name + r' \.\.\. ok', result.stdout), name


def check(root=ROOT):
    structure(root)
    environment = runpy.run_path(str(root / '.github/scripts/backend_environment.py'))['matrix_environment']()
    with tempfile.TemporaryDirectory(prefix='date-time-capabilities-') as directory:
        parent = Path(directory)
        environment.update(CARGO_BUILD_JOBS='2', CARGO_INCREMENTAL='0',
                           CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')
        environment.setdefault('CARGO_TARGET_DIR', str(parent / 'target'))
        check_parsing_units(root, parent, environment)
        work = prepare(root, parent, 'contracts')
        run(work, environment, 'generate-lockfile')
        for features in ((), ('alloc',), ('parsing',), ('formatting',), ('large-dates',)):
            # Cargo metadata includes weak optional feature edges that are not built.
            # Cargo tree reflects the actual active build closure.
            tree = run(work, environment, 'tree', '--prefix', 'none', '--format', '{p}',
                       '--edges', 'normal,build', *feature_flags(features)).stdout
            names = {line.split()[0] for line in tree.splitlines() if line.strip()}
            assert names == {'date-time', 'date-time-contracts-probe'}, (features, names)
        for features in MATRIX:
            for target in TARGETS:
                run(work, environment, 'check', '--target', target, *feature_flags(features))
                print(f'date-time matrix PASS: {target}: {",".join(features) or "core/no_std"}', flush=True)
        for features in TEST_PROFILES:
            result = run(work, environment, 'test', '--tests', '--target', TARGETS[0], *feature_flags(features))
            print(result.stdout, end='', flush=True)
            # WASM tests are compiled, never reported as executed on a native host.
            run(work, environment, 'check', '--tests', '--target', TARGETS[1], *feature_flags(features))
        reject_features_and_private_api(work, environment)
        for fixture, profiles in (('serialization', SERIALIZATION_PROFILES), ('tracing', ((),))):
            work = prepare(root, parent, fixture)
            run(work, environment, 'generate-lockfile')
            for features in profiles:
                result = run(work, environment, 'test', '--tests', '--target', TARGETS[0], *feature_flags(features))
                print(result.stdout, end='', flush=True)
                run(work, environment, 'check', '--tests', '--target', TARGETS[1], *feature_flags(features))
    print(f'Date/time: {len(MATRIX) * len(TARGETS)} native/WASM capability checks; native calendar, formatting, parsing, real JSON and tracing tests passed; WASM test fixtures compile only; removed features and private APIs rejected.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--structure-only', action='store_true')
    args = parser.parse_args()
    if args.structure_only:
        structure(args.root.resolve())
    else:
        check(args.root.resolve())
