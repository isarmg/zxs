"""Pinned same-root maintenance reconstructs an exact terminal proof owner."""
import base64
from copy import deepcopy
import gzip
import io
import json
import os
from pathlib import Path

import source_layout

KEY = 'proof_maintenance_receipts'
RECEIPTS = ('f6b00af0dad58bb402937c193974d916d525a790decbe043e679954c03bcbd1e',)


def state(catalog, directory):
    return dict(package=catalog['packages'].get(directory),
                merges=[p for p in catalog.get('merges', []) if p['directory'] == directory],
                external=catalog.get('external_tests', {}).get(directory))


def terminal_state(row):
    record = row['binding']['record']
    directory = record['destination_directory']
    packages = []
    for item in record['inputs']:
        package = deepcopy(item['package'])
        package.update(directory=directory, files_sha256=deepcopy(record['destination_files_sha256']))
        if item['package']['directory'] == record['primary_directory']:
            package['name'] = record['destination_name']
            primary = package
        else:
            package['merged_into'] = directory
            packages.append(package)
    return dict(package=primary, merges=packages, external=deepcopy(record['destination_external_tests']))


def _read(root, sha):
    import proof_routing as routing
    if not routing._hash(sha):
        raise ValueError('invalid maintenance receipt ID')
    path = routing.safe(root, 'compliance/proof-maintenance/' + sha + '.json.gz')
    with path.open('rb') as stream:
        raw = stream.read(routing.MAX_COMPRESSED + 1)
    if len(raw) > routing.MAX_COMPRESSED:
        raise ValueError('maintenance compressed bound exceeded')
    if routing.digest(raw) != sha:
        raise ValueError('immutable maintenance receipt changed')
    with gzip.GzipFile(fileobj=io.BytesIO(raw)) as stream:
        raw = stream.read(routing.MAX_EXPANDED + 1)
        if len(raw) > routing.MAX_EXPANDED or stream.read(1):
            raise ValueError('maintenance expansion bound exceeded')
    row = json.loads(raw)
    if (not isinstance(row, dict) or set(row) != {'schema', 'reason', 'terminal', 'layout', 'parent', 'before', 'after', 'files'}
            or type(row['schema']) is not int or row['schema'] != 1
            or not isinstance(row['reason'], str) or not row['reason'].strip()
            or not isinstance(row['files'], list) or not 1 <= len(row['files']) <= routing.MAX_FILES):
        raise ValueError('invalid maintenance receipt schema')
    return row


def _inventory(root, value):
    import proof_routing as routing
    if not isinstance(value, dict) or set(value) != {'package', 'merges', 'external'}:
        raise ValueError('invalid maintenance owner snapshot')
    package = value['package']
    if (not isinstance(package, dict) or not isinstance(package.get('directory'), str)
            or not isinstance(package.get('files_sha256'), dict) or not package['files_sha256']):
        raise ValueError('invalid maintenance source owner')
    directory = package['directory']; routing.safe(root, directory)
    files, roots = {}, [directory]
    for relative, sha in package['files_sha256'].items():
        path = directory + '/' + relative; routing.safe(root, path)
        if not routing._hash(sha):
            raise ValueError('invalid maintenance source hash')
        files[path] = sha
    if not isinstance(value['merges'], list):
        raise ValueError('invalid maintenance merged owners')
    owner = value['external']
    if owner is not None:
        if (not isinstance(owner, dict) or owner.get('directory') != package['directory']
                or owner.get('origin_directory') != package['origin_directory']
                or not isinstance(owner.get('files'), list) or owner.get('file_count') != len(owner['files'])
                or type(owner.get('bytes')) is not int or owner['bytes'] < 0
                or not isinstance(owner.get('roots'), list) or not owner['roots']):
            raise ValueError('invalid maintenance external owner')
        for base in owner['roots']:
            routing.safe(root, base)
            if not base.startswith('test/') or any(base == p or base.startswith(p + '/') or p.startswith(base + '/') for p in roots):
                raise ValueError('invalid maintenance external root')
            roots.append(base)
        for entry in owner['files']:
            if not isinstance(entry, dict) or not isinstance(entry.get('path'), str):
                raise ValueError('invalid maintenance external file')
            path = entry['path']; routing.safe(root, path)
            if (path in files or not routing._hash(entry.get('sha256'))
                    or not any(path.startswith(base + '/') for base in owner['roots'])):
                raise ValueError('invalid maintenance external inventory')
            files[path] = entry['sha256']
    if len(files) > routing.MAX_FILES:
        raise ValueError('maintenance file bound exceeded')
    return files, roots


def _transition(root, before, after):
    old, roots = _inventory(root, before)
    new, new_roots = _inventory(root, after)
    package = before['package']
    expected = deepcopy(before)
    expected['package']['files_sha256'] = after['package']['files_sha256']
    if set(package['files_sha256']) != set(after['package']['files_sha256']):
        raise ValueError('maintenance production inventory changed')
    for merged in expected['merges']:
        merged['files_sha256'] = after['package']['files_sha256']
    expected['external'] = after['external']
    if expected != after or roots != new_roots:
        raise ValueError('maintenance changed owner identity or roots')
    prior, current = before['external'], after['external']
    if prior is None:
        if current is not None:
            raise ValueError('maintenance requires existing external owner')
    else:
        if current is None or dict(prior, files=current['files'], file_count=current['file_count'], bytes=current['bytes']) != current:
            raise ValueError('maintenance changed external owner metadata')
        rows = {row['path']: row for row in current['files']}
        for row in prior['files']:
            if row['path'] not in rows or dict(row, sha256=rows[row['path']]['sha256']) != rows[row['path']]:
                raise ValueError('maintenance changed historical test attribution')
        for path in set(new) - set(old):
            row = rows[path]
            if (row.get('source_kind') != 'project-authored' or row.get('source') != path
                    or row.get('source_sha256_before') != row['sha256']
                    or row.get('origin_directory', package['origin_directory']) != package['origin_directory']
                    or set(row) - {'path', 'source', 'source_kind', 'source_sha256_before', 'sha256', 'origin_directory'}):
                raise ValueError('maintenance new tests must be project-authored')
    return old, new, roots


def _bytes_match(data, hashes, snapshot, expected):
    if hashes != expected:
        raise ValueError('maintenance inventory/content drift')
    owner = snapshot['external']
    if owner is not None and sum(len(data[row['path']]) for row in owner['files']) != owner['bytes']:
        raise ValueError('maintenance external byte count drift')


def _patches(row, old, new):
    """Validate exact changed-file coverage and the existing receipt patch budget."""
    import proof_routing as routing
    changed = {path for path in old if old[path] != new.get(path)}
    patches = {}
    count = size = 0
    for file in row['files']:
        if (not isinstance(file, dict) or set(file) != {'before', 'after', 'before_sha256', 'after_sha256', 'replacements'}
                or file['before'] != file['after'] or file['before'] in patches
                or file['before'] not in changed or file['before_sha256'] != old[file['before']]
                or file['after_sha256'] != new[file['after']] or not isinstance(file['replacements'], list)):
            raise ValueError('maintenance exact changed-file coverage differs')
        patches[file['before']] = file
        count += len(file['replacements'])
        for edit in file['replacements']:
            if not isinstance(edit, dict) or set(edit) != {'old', 'new'}:
                raise ValueError('invalid maintenance replacement')
            size += sum(len(base64.b64decode(edit[k], validate=True)) for k in ('old', 'new'))
    if set(patches) != changed or count > routing.MAX_REPLACEMENTS or size > routing.MAX_PATCH_BYTES:
        raise ValueError('maintenance patch coverage/bound differs')
    return patches


def restore(root, catalog, prospective=False):
    """Authenticate live tips and return exact terminal catalog and byte views."""
    import proof_routing as routing
    registered = catalog.get(KEY, [])
    if (not isinstance(registered, list) or registered != list(RECEIPTS[:len(registered)])
            or (not prospective and registered != list(RECEIPTS))
            or len(RECEIPTS) > 64 or len(set(RECEIPTS)) != len(RECEIPTS)):
        raise ValueError('pinned maintenance catalog history differs')
    historical = deepcopy(catalog)
    layout = source_layout.load(root)
    import source_layout_corrections
    layout_identity = {key: catalog.get(key) for key in (source_layout.KEY, source_layout_corrections.KEY)}
    chains, records, terminals = {}, [], {}
    for sha in RECEIPTS:
        row = _read(root, sha)
        terminal = row['terminal']
        if terminal not in routing.RECEIPTS or terminal not in catalog.get('proof_routing_receipts', []):
            raise ValueError('maintenance terminal is not registered')
        if terminal not in terminals:
            terminals[terminal] = routing._read(root, terminal)
        bound = terminals[terminal]
        if bound['binding']['kind'] != 'consolidation':
            raise ValueError('maintenance requires terminal consolidation')
        chain = chains.setdefault(terminal, [])
        old, new, roots = _transition(root, row['before'], row['after'])
        before = row['before']
        if not chain:
            before_catalog = dict(packages={before['package']['directory']: before['package']},
                                  merges=before['merges'], external_tests={})
            if before['external'] is not None:
                before_catalog['external_tests'][before['package']['directory']] = before['external']
            projected = source_layout.project_catalog(root, before_catalog)
            logical = bound['binding']['record']['destination_directory']
            before = state(projected, logical)
        expected = chain[-1][1]['after'] if chain else terminal_state(bound)
        if (row['layout'] != layout_identity or row['before']['package']['directory'] != source_layout.current_path(root, bound['after']['directory'])
                or row['parent'] != (chain[-1][0] if chain else None) or before != expected):
            raise ValueError('maintenance ancestry/layout discontinuity')
        _patches(row, old, new)
        chain.append((sha, row, old, new, roots)); records.append((sha, row))
    restored = {}
    for terminal, chain in chains.items():
        _, last, _, live_inventory, roots = chain[-1]
        directory = last['after']['package']['directory']
        logical_directory = layout.historical_path(directory) if layout else directory
        if logical_directory in restored or state(catalog, directory) != last['after']:
            raise ValueError('maintenance final full active owner differs')
        data, hashes = {}, {}; total = 0
        for base in roots:
            target = routing.safe(root, base)
            if not target.is_dir():
                raise ValueError('missing maintenance live root')
            for directory_path, children, names in os.walk(target, followlinks=False):
                for name in children + names:
                    path = str((Path(directory_path) / name).relative_to(root))
                    file = routing.safe(root, path)
                    if file.is_file():
                        with file.open('rb') as stream:
                            value = stream.read(routing.MAX_LIVE_BYTES - total + 1)
                        total += len(value)
                        if total > routing.MAX_LIVE_BYTES or len(data) >= routing.MAX_FILES:
                            raise ValueError('maintenance live material bound exceeded')
                        data[path] = value; hashes[path] = routing.digest(value)
                    elif not file.is_dir():
                        raise ValueError('maintenance special file')
        _bytes_match(data, hashes, last['after'], live_inventory)
        for _, row, old, new, _ in reversed(chain):
            for file in row['files']:
                data[file['before']] = routing.inverse_replacements(data[file['after']], file)
                hashes[file['before']] = file['before_sha256']
            for path in set(new) - set(old):
                del data[path]; del hashes[path]
            _bytes_match(data, hashes, row['before'], old)
        first = chain[0][1]['before']
        historical['packages'][directory] = deepcopy(first['package'])
        historical['merges'] = [p for p in historical.get('merges', []) if p['directory'] != directory] + deepcopy(first['merges'])
        if first['external'] is not None:
            historical['external_tests'][directory] = deepcopy(first['external'])
        if layout:
            data = {layout.historical_path(path): layout.project_bytes(layout.historical_path(path), value)
                    for path, value in data.items()}
            live_inventory = {layout.historical_path(path): sha for path, sha in live_inventory.items()}
        restored[logical_directory] = dict(bytes=data, inventory=live_inventory, live_bytes=total,
                                           records=[(sha, row) for sha, row, _, _, _ in chain])
    return historical, restored, records


def validate_refresh(root, previous, current, records=None):
    if records is None:
        _, _, records = restore(root, current)
    registered = previous.get(KEY, [])
    if registered != list(RECEIPTS[:len(registered)]):
        raise ValueError('immutable maintenance history drift')
    seen = set()
    for sha, row in records:
        if sha in registered:
            continue
        directory = row['before']['package']['directory']
        if directory in seen or state(previous, directory) != row['before']:
            raise ValueError('maintenance previous catalog differs or multiple fresh owner steps')
        seen.add(directory)
