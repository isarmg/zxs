#!/usr/bin/env python3
"""Watch first-party Rust patterns; counts are candidates, not semantic violations."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
BASELINE = '.github/metrics-baseline.json'
SOURCES = (
    'src/engine', 'src/server', 'src/web', 'src/contract', 'src/naming',
    'src/platform', 'src/interface', 'src/macros', 'src/auth', 'src/client',
    'src/smb', 'src/smb-vfs', 'helper', 'tools',
)
PATTERNS = {
    'error_new': r'\bError::new\(',
    'validate_fn': r'\bvalidate_id\(|\bvalidate_(?:name|lookup_name|path|lookup_path|slot)\(',
    'parse_discard': r'::parse\([^;\n]*\)\?;',
    'id_string': r'_ids?: (?:Option<|Vec<|BTreeSet<)?String\b',
    'string_key_map': r'BTreeMap<String,',
    'hash_string': r'_hash: (?:Option<)?String\b',
    'json_value': r'\bjson!\(|\bfrom_value\(|\bto_value\(|json_codec::Value',
    'checked_add_1': r'checked_add\(1\)',
    'unwrap_or_default': r'unwrap_or_default\(\)',
    'invariant_as_error': r'ErrorCode::(?:RecoveryRequired|StateInvalid|ObjectCorrupt)',
    'wildcard_arm': r'\b_ =>',
    'unreachable': r'unreachable!',
    'erase_error': r'map_err\(\|_\|',
    'join_literal': r'\.join\("[A-Za-z0-9_.\-]+(?:/[A-Za-z0-9_.\-]+)*"\)',
    'disk_name_const': r'const [A-Z_]+: &str = "[A-Za-z0-9_.\-/]+\.(?:json|bin|lock|sqlite)"',
    'assert_fn_def': r'\bfn (?:validate|require|verify|check|ensure)_\w+',
    'state_leak': r'\.state\(\)\.',
    'api_path_literal': r'"/api/',
    'clock_read': r'\bnow\(\)\?',
    'cfg_not_test': r'cfg\(not\(test\)\)',
    'fault_cfg': r'feature = "fault-injection"',
    'glob_super': r'use super::\*|super::super::',
}
WIRING = re.compile(r'#\[path|(?:pub(?:\(crate\))? )?mod \w+;')
SAMPLING = {
    'files': '**/*.rs',
    'excluded_lines': 'leading-whitespace-then-//',
    'long_line_over': 100,
    'cfg_test_wiring': WIRING.pattern,
}


def empty_counts():
    return {source: dict.fromkeys((*PATTERNS, 'long_line', 'cfg_test_item'), 0)
            for source in SOURCES}


def add_counts(totals, contents):
    lines = [line for line in contents.splitlines() if not line.lstrip().startswith('//')]
    text = '\n'.join(lines)
    totals['long_line'] += sum(len(line) > 100 for line in lines)
    for index, line in enumerate(lines):
        if line.strip() == '#[cfg(test)]':
            following = lines[index + 1].strip() if index + 1 < len(lines) else ''
            totals['cfg_test_item'] += 0 if WIRING.match(following) else 1
    for name, pattern in PATTERNS.items():
        totals[name] += len(re.findall(pattern, text))


def counts(root):
    result = empty_counts()
    for source in SOURCES:
        directory = root / source
        if not directory.is_dir():
            raise FileNotFoundError(directory)
        for path in sorted(directory.rglob('*.rs')):
            add_counts(result[source], path.read_text(encoding='utf-8'))
    return result


def git(root, *args, data=None):
    return subprocess.check_output(['git', '-C', str(root), *args], input=data)


def committed_counts(root, reference):
    # A working-tree edit must never be attributed to an unchanged commit SHA.
    commit = git(root, 'rev-parse', '--verify', '--end-of-options',
                 reference + '^{commit}').decode().strip()
    result = empty_counts()
    for source in SOURCES:
        kind = git(root, 'cat-file', '-t', commit + ':' + source).decode().strip()
        if kind != 'tree':
            raise ValueError(f'missing committed source directory: {source}')
    entries = git(root, 'ls-tree', '-r', '-z', commit, '--', *SOURCES).split(b'\0')
    blobs = []
    for entry in filter(None, entries):
        metadata, name = entry.split(b'\t', 1)
        mode, kind, oid = metadata.split()
        path = name.decode('utf-8')
        if path.endswith('.rs'):
            if kind != b'blob' or mode not in (b'100644', b'100755'):
                raise ValueError(f'non-regular committed Rust source: {path}')
            source = next(source for source in SOURCES if path.startswith(source + '/'))
            blobs.append((source, oid))
    output = git(root, 'cat-file', '--batch', data=b''.join(oid + b'\n' for _, oid in blobs))
    offset = 0
    for source, oid in blobs:
        end = output.index(b'\n', offset)
        actual, kind, size = output[offset:end].split()
        length = int(size)
        if actual != oid or kind != b'blob':
            raise ValueError('unexpected Git source object')
        offset = end + 1
        add_counts(result[source], output[offset:offset + length].decode('utf-8'))
        offset += length + 1
    return commit, result


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate baseline key: {key}')
        result[key] = value
    return result


def validate_counts(value):
    expected = empty_counts()
    if not isinstance(value, dict) or value.keys() != expected.keys():
        raise ValueError('boundary metric source inventory changed; review and resample the baseline')
    for source, metrics in value.items():
        if not isinstance(metrics, dict) or metrics.keys() != expected[source].keys():
            raise ValueError(f'boundary metric inventory changed for {source}')
        if any(type(count) is not int or count < 0 for count in metrics.values()):
            raise ValueError(f'invalid boundary metric count for {source}')


def validate_snapshot(value):
    required = {'version', 'mode', 'commit', 'script_sha256', 'captured_at', 'scope',
                'patterns', 'sampling', 'counts'}
    if not isinstance(value, dict) or not required <= value.keys():
        raise ValueError('baseline lacks provenance; use --update --migrate for unversioned counts')
    if value.keys() - required - {'migration'}:
        raise ValueError('unknown baseline fields')
    if type(value['version']) is not int or value['version'] != 1 or value['mode'] != 'watch':
        raise ValueError('unsupported boundary metric snapshot')
    for field, width in [('commit', 40), ('script_sha256', 64)]:
        if not isinstance(value[field], str) or not re.fullmatch('[0-9a-f]{%d}' % width,
                                                               value[field]):
            raise ValueError(f'invalid baseline {field}')
    captured = datetime.fromisoformat(value['captured_at'])
    if captured.utcoffset() != timezone.utc.utcoffset(captured):
        raise ValueError('baseline captured_at must be UTC')
    if (value['scope'] != list(SOURCES) or value['patterns'] != PATTERNS
            or value['sampling'] != SAMPLING):
        raise ValueError('sampling changed; review scope and resample with --update')
    if 'migration' in value:
        migration = value['migration']
        if (not isinstance(migration, dict)
                or set(migration) != {'from', 'previous_baseline_sha256', 'method'}
                or migration['from'] != 'unversioned-counts'
                or migration['method'] != 'resampled-from-commit'
                or not isinstance(migration['previous_baseline_sha256'], str)
                or not re.fullmatch('[0-9a-f]{64}', migration['previous_baseline_sha256'])):
            raise ValueError('invalid baseline migration provenance')
    validate_counts(value['counts'])
    return value


def script_digest():
    return hashlib.sha256(Path(__file__).read_bytes()).hexdigest()


def snapshot(root, reference):
    commit, current = committed_counts(root, reference)
    return {'version': 1, 'mode': 'watch', 'commit': commit,
            'script_sha256': script_digest(),
            'captured_at': datetime.now(timezone.utc).isoformat(),
            'scope': list(SOURCES), 'patterns': PATTERNS, 'sampling': SAMPLING,
            'counts': current}


def changes(current, baseline):
    validate_counts(current)
    validate_counts(baseline)
    return [(source, metric, baseline[source][metric], value)
            for source, metrics in current.items() for metric, value in metrics.items()
            if value != baseline[source][metric]]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--update', action='store_true', help='snapshot reviewed committed sources')
    parser.add_argument('--ref', help='source commit for --update (default: HEAD)')
    parser.add_argument('--migrate', action='store_true', help='explicitly replace unversioned counts')
    args = parser.parse_args(argv)
    if (args.ref or args.migrate) and not args.update:
        parser.error('--ref and --migrate require --update')
    path = args.root / BASELINE
    try:
        if args.update:
            migration = None
            if path.exists():
                raw = path.read_bytes()
                previous = json.loads(raw, object_pairs_hook=unique_object)
                if isinstance(previous, dict) and 'version' not in previous:
                    validate_counts(previous)
                    if not args.migrate:
                        raise ValueError('unversioned baseline requires explicit --update --migrate')
                    migration = {'from': 'unversioned-counts', 'method': 'resampled-from-commit',
                                 'previous_baseline_sha256': hashlib.sha256(raw).hexdigest()}
                elif args.migrate:
                    raise ValueError('--migrate requires an unversioned count baseline')
            elif args.migrate:
                raise ValueError('--migrate requires an existing unversioned count baseline')
            value = snapshot(args.root, args.ref or 'HEAD')
            if migration:
                value['migration'] = migration
            path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n', encoding='utf-8')
            print(f'watch snapshot: committed sources at {value["commit"]}; '
                  'uncommitted source edits are excluded')
            return 0
        value = validate_snapshot(json.loads(path.read_bytes(), object_pairs_hook=unique_object))
        current = counts(args.root)
        print(f'watch baseline: {value["commit"]} captured {value["captured_at"]}; '
              f'script_sha256={value["script_sha256"]}')
        if value['script_sha256'] != script_digest():
            print('watch: script digest changed; review implementation separately from count changes')
        for source, metric, previous, count in changes(current, value['counts']):
            direction = 'rose' if count > previous else 'fell'
            print(f'{direction} {source}:{metric}: {previous} -> {count}')
        print('watch only: review changed candidates and their proof scopes; '
              'semantic, safety and source gates remain independent')
        return 0
    except (OSError, ValueError, TypeError, subprocess.CalledProcessError) as error:
        print(f'boundary metrics input error: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
