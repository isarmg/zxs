#!/usr/bin/env python3
"""Count first-party boundary debt without prescribing source-code shape."""
import argparse
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SOURCES = (
    'src/engine/src', 'src/server/src', 'src/web/src',
    'src/contract/src', 'src/naming/src', 'helper/src',
)
PATTERNS = {
    'error_new': r'Error::new\(',
    'validate_id': r'validate_id\(',
    'parse_discard': r'::parse\([^;\n]*\)\?;',
    'id_string': r'_id:\s*String\b',
    'hash_string': r'_hash:\s*String\b',
    'json_value': r'json!\(|from_value\(|to_value\(',
    'checked_add_1': r'checked_add\(1\)',
    'unwrap_or_default': r'unwrap_or_default\(\)',
    'join_literal': r'\.join\("[^"]+"\)',
    'cfg_not_test': r'cfg\(not\(test\)\)',
}


def counts(root):
    result = {}
    for source in SOURCES:
        directory = root / source
        if not directory.is_dir():
            raise FileNotFoundError(directory)
        totals = dict.fromkeys(PATTERNS, 0)
        for path in sorted(directory.rglob('*.rs')):
            text = path.read_text(encoding='utf-8')
            for name, pattern in PATTERNS.items():
                totals[name] += len(re.findall(pattern, text))
        result[source] = totals
    return result


def increases(current, baseline):
    if current.keys() != baseline.keys():
        raise ValueError('boundary metric source inventory changed; review the baseline')
    risen = []
    for source, metrics in current.items():
        if metrics.keys() != baseline[source].keys():
            raise ValueError(f'boundary metric inventory changed for {source}')
        for metric, value in metrics.items():
            previous = baseline[source][metric]
            if value > previous:
                risen.append((source, metric, previous, value))
    return risen


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--update', action='store_true', help='write a reviewed count baseline')
    args = parser.parse_args()
    current = counts(ROOT)
    path = ROOT / '.github/boundary-metrics-baseline.json'
    if args.update:
        path.write_text(json.dumps(current, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        return 0
    baseline = json.loads(path.read_text(encoding='utf-8'))
    risen = increases(current, baseline)
    for source, metric, previous, value in risen:
        print(f'{source}:{metric}: {previous} -> {value}')
    return bool(risen)


if __name__ == '__main__':
    raise SystemExit(main())
