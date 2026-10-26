"""Append-only standalone upgrades after a pinned foundation layout.

The original origins bytes remain authenticated by the layout's existing hash.
New identities require the ordinary explicit upgrade proof; they never replace
or reinterpret any pre-layout origin, receipt, or external-test source label.
"""
import json

ORIGINS = 'compliance/provenance/origins.json'


def frozen_origins(layout, expected):
    from source_layout import safe, digest
    snapshot = safe(layout.root, 'compliance/source-layout/origins-' + expected + '.json')
    if not snapshot.is_file() or digest(snapshot.read_bytes()) != expected:
        raise ValueError('missing or changed immutable pre-layout origins snapshot')
    before = json.loads(snapshot.read_bytes())
    after = json.loads(safe(layout.root, ORIGINS).read_bytes())
    count = len(before.get('packages', []))
    if (not count or {k:v for k,v in before.items() if k != 'packages'} !=
            {k:v for k,v in after.items() if k != 'packages'}
            or not isinstance(after.get('packages'), list)
            or after['packages'][:count] != before['packages']):
        raise ValueError('pre-layout origins are not an exact append-only extension')
    added = after['packages'][count:]
    keys = [p['origin_directory'] for p in after['packages']]
    if len(keys) != len(set(keys)):
        raise ValueError('duplicate appended layout origin')
    layout.appended_origins = {p['origin_directory']:p for p in added}


def chains(layout, catalog, imported_origins=frozenset()):
    from source_layout import _stable, digest, canonical
    additions = {}
    upgraded = {}
    used = set(imported_origins)
    for old, row in layout.rows.items():
        directory = row['destination_directory']
        current = catalog.get('packages', {}).get(directory)
        before = row['package']
        origin = before['origin_directory']
        if current is None or current.get('origin_directory') == origin:
            continue
        layout.require_standalone(catalog, old)
        first = True
        seen = set()
        while origin != current.get('origin_directory'):
            if origin in seen:
                raise ValueError('cyclic grouped source upgrade')
            seen.add(origin)
            receipt = catalog.get('superseded', {}).get(origin)
            if (not isinstance(receipt, dict) or receipt.get('package', {}).get('directory') != directory
                    or receipt['package'].get('origin_directory') != origin
                    or catalog.get('superseded_records_sha256', {}).get(origin) != digest(canonical(receipt))):
                raise ValueError('missing authenticated grouped upgrade chain')
            if first:
                if (_stable(receipt['package']) != _stable(before)
                        or receipt['package'].get('files_sha256') != row['files_sha256']):
                    raise ValueError('grouped upgrade changed pre-layout package identity')
                if receipt.get('external_tests') != row['destination_external_tests']:
                    raise ValueError('grouped upgrade lost pre-layout test attribution')
            else:
                prior = layout.appended_origins[origin]
                if receipt['package'].get('upstream') != prior['upstream']:
                    raise ValueError('grouped upgrade predecessor origin mismatch')
            successor = receipt.get('successor_origin_directory')
            record = layout.appended_origins.get(successor)
            if (record is None or successor in used or record['upstream']['name'] != before['upstream']['name']
                    or record['upstream']['version'] == receipt['package']['upstream']['version']):
                raise ValueError('invalid grouped upgrade successor')
            if not isinstance(receipt.get('reason'), str) or not receipt['reason'].strip():
                raise ValueError('missing grouped upgrade reason')
            additions[origin] = receipt; used.add(successor)
            origin = successor; first = False
        record = layout.appended_origins[origin]
        if (current.get('name') != before['name'] or current.get('version') != record['upstream']['version']
                or current.get('upstream') != record['upstream']
                or current.get('license_materials') != record['central_license_materials']
                or any(current.get(k) != record.get(k) for k in ('license', 'repository', 'vcs'))):
            raise ValueError('grouped current upgrade identity mismatch')
        upgraded[directory] = current
    pending = set()
    plan = layout.pending_upgrade
    if plan is not None and (not isinstance(plan, dict) or plan.get('schema') != 1
                             or not isinstance(plan.get('packages'), list)):
        raise ValueError('invalid explicit grouped source upgrade plan')
    for item in (plan or {}).get('packages', []):
        if not isinstance(item, dict):
            raise ValueError('invalid grouped source upgrade item')
        directory = item.get('directory')
        if item.get('successor_origin_directory') in used:
            continue
        if directory not in layout.reverse or item.get('package') != catalog.get('packages', {}).get(directory):
            raise ValueError('stale pending grouped upgrade')
        layout.require_standalone(catalog, layout.reverse[directory])
        successor = item.get('successor_origin_directory')
        if successor not in layout.appended_origins:
            raise ValueError('pending grouped upgrade requires appended origin')
        pending.add(successor)
    if set(layout.appended_origins) != used | pending:
        raise ValueError('unattributed appended layout origin')
    return additions, upgraded, used


def history_value(layout, field, value, additions, used):
    if field in ('superseded', 'superseded_records_sha256') and isinstance(value, dict):
        return {k:v for k,v in value.items() if k not in additions}
    if field == 'origin_records_sha256' and isinstance(value, dict):
        return {k:v for k,v in value.items() if k not in used}
    if field == 'upstream_identities' and isinstance(value, list):
        added = [layout.appended_origins[k]['upstream'] for k in used]
        return [v for v in value if v not in added]
    return value
