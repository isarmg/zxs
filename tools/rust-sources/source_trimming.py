"""Explicit production-file removal, independent of test relocation receipts.

Receipts retain the complete pre-trim owner snapshot. They are append-only and
content-addressed; original origin/license records remain managed by manage.py.
Owner lifecycle changes require a future chain-aware adapter, not lost receipts.
"""
from copy import deepcopy
import hashlib
import json
import re

import external_tests
import source_layout

KEY = 'source_trims'
HASHES = 'source_trim_records_sha256'
HEX = re.compile(r'[0-9a-f]{64}')
STABLE = ('origin_directory', 'upstream', 'version', 'license', 'license_materials',
          'repository', 'vcs')


def receipt_id(record):
    return hashlib.sha256(json.dumps(record, sort_keys=True, ensure_ascii=False,
                                    allow_nan=False).encode('utf-8')).hexdigest()


def hashes(records):
    return {key: receipt_id(row) for key, row in sorted(records.items())}


def owners(catalog):
    return {row['directory'] for row in catalog.get(KEY, {}).values()}


def _validate_record(root, row, packages, forbidden, imported=()):
    if not isinstance(row, dict) or set(row) != {'directory', 'package', 'files'}:
        raise ValueError('invalid source trim receipt fields')
    directory = row['directory']
    if (not isinstance(directory, str) or not directory.startswith('src/foundation/')
            or (len(directory.split('/')) != 3 and directory not in imported)
            or directory not in packages or directory in forbidden):
        raise ValueError('source trimming requires an active standalone foundation owner')
    external_tests.safe(root, directory)
    before, current = row['package'], packages[directory]
    if (not isinstance(before, dict) or before.get('directory') != directory
            or not before.get('origin_directory')
            or before.get('origin_directory') != current.get('origin_directory')):
        raise ValueError('source trim owner identity drift')
    if any(before.get(field) != current.get(field) for field in STABLE):
        raise ValueError('source trim stable origin metadata drift')
    entries = row['files']
    if not isinstance(entries, list) or not entries:
        raise ValueError('source trim needs explicit files')
    seen = set()
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {'path', 'sha256_before', 'reason'}:
            raise ValueError('invalid source trim file receipt')
        path = entry['path']
        if (not isinstance(path, str) or path in seen
                or not (path == 'build.rs' or (path.startswith('src/') and path.endswith('.rs')))):
            raise ValueError('duplicate or non-production source trim path')
        target = (external_tests.safe(root, directory + '/' + path) if directory in imported
                  else source_layout.physical(root, directory + '/' + path))
        if any(part in ('test', 'tests', 'benches', 'examples') for part in path.split('/')):
            raise ValueError('test material cannot use production source trimming')
        value = entry['sha256_before']
        if (not isinstance(value, str) or not HEX.fullmatch(value)
                or before.get('files_sha256', {}).get(path) != value):
            raise ValueError('source trim before hash mismatch')
        if not isinstance(entry['reason'], str) or not entry['reason'].strip():
            raise ValueError('source trim requires a review reason for each file')
        if target.exists():
            raise ValueError('trimmed source must remain absent: ' + str(target))
        seen.add(path)
    return {(directory, path) for path in seen}


def forbidden_owners(catalog):
    return ({row['destination_directory'] for row in catalog.get('consolidations', {}).values()}
            | {row['directory'] for row in catalog.get('merges', [])})


@source_layout.historical_catalog
def validate(root, catalog):
    records = catalog.get(KEY, {})
    if not isinstance(records, dict) or hashes(records) != catalog.get(HASHES, {}):
        raise ValueError('source trim receipt integrity drift')
    seen = set()
    for key, row in records.items():
        if key != receipt_id(row):
            raise ValueError('source trim content-address identity drift')
        paths = _validate_record(root, row, catalog['packages'], forbidden_owners(catalog),
                                 source_layout.imported_directories(root))
        if seen & paths:
            raise ValueError('duplicate historical source trim')
        seen |= paths
        if any(path in catalog['packages'][directory].get('files_sha256', {}) for directory, path in paths):
            raise ValueError('trimmed source remains in current inventory')
    return records


def prepare(root, previous, plan=None):
    # During a fresh transition the previous inventory still contains removed
    # files. Historical receipts, however, must already be absent from it.
    records = deepcopy(validate(root, previous))
    grants = {}
    if plan is None:
        return records, grants
    if not isinstance(plan, dict) or set(plan) != {'schema', 'packages'} or type(plan['schema']) is not int or plan['schema'] != 1:
        raise ValueError('invalid source trim plan schema')
    if not isinstance(plan['packages'], list) or not plan['packages']:
        raise ValueError('source trim plan needs packages')
    seen = set()
    old_paths = {(row['directory'], item['path']) for row in records.values() for item in row['files']}
    for row in plan['packages']:
        paths = _validate_record(root, row, previous['packages'], forbidden_owners(previous),
                                 source_layout.imported_directories(root))
        directory = row['directory']
        if directory in seen or old_paths & paths:
            raise ValueError('duplicate source trim owner/file')
        if row['package'] != previous['packages'][directory]:
            raise ValueError('stale source trim package snapshot')
        seen.add(directory)
        records[receipt_id(row)] = deepcopy(row)
        grants[directory] = {entry['path']: entry['sha256_before'] for entry in row['files']}
    return records, grants


def check_append_only(previous, current):
    for key, row in previous.get(KEY, {}).items():
        if current.get(KEY, {}).get(key) != row:
            raise ValueError('source trim history cannot be removed or modified')
