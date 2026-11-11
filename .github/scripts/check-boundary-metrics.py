#!/usr/bin/env python3
"""Count first-party boundary debt without prescribing source-code shape."""
import argparse
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
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


def counts(root):
    result = {}
    for source in SOURCES:
        directory = root / source
        if not directory.is_dir():
            raise FileNotFoundError(directory)
        totals = dict.fromkeys((*PATTERNS, 'long_line', 'cfg_test_item'), 0)
        for path in sorted(directory.rglob('*.rs')):
            lines = [line for line in path.read_text(encoding='utf-8').splitlines()
                     if not line.lstrip().startswith('//')]
            text = '\n'.join(lines)
            totals['long_line'] += sum(len(line) > 100 for line in lines)
            for index, line in enumerate(lines):
                if line.strip() == '#[cfg(test)]':
                    following = lines[index + 1].strip() if index + 1 < len(lines) else ''
                    totals['cfg_test_item'] += 0 if WIRING.match(following) else 1
            for name, pattern in PATTERNS.items():
                totals[name] += len(re.findall(pattern, text))
        result[source] = totals
    return result


def changes(current, baseline):
    if current.keys() != baseline.keys():
        raise ValueError('boundary metric source inventory changed; review the baseline')
    changed = []
    for source, metrics in current.items():
        if metrics.keys() != baseline[source].keys():
            raise ValueError(f'boundary metric inventory changed for {source}')
        for metric, value in metrics.items():
            previous = baseline[source][metric]
            if value != previous:
                changed.append((source, metric, previous, value))
    return changed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--update', action='store_true', help='write a reviewed count baseline')
    args = parser.parse_args()
    current = counts(ROOT)
    path = ROOT / '.github/metrics-baseline.json'
    if args.update:
        path.write_text(json.dumps(current, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        return 0
    baseline = json.loads(path.read_text(encoding='utf-8'))
    changed = changes(current, baseline)
    for source, metric, previous, value in changed:
        direction = 'rose' if value > previous else 'fell'
        print(f'{direction} {source}:{metric}: {previous} -> {value}')
    return bool(changed)


if __name__ == '__main__':
    raise SystemExit(main())
