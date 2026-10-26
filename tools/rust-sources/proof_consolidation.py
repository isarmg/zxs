"""Terminal primary-origin projection for an exact reviewed consolidation.

This adapter does not grant source prefixes, load proof chains, or recover absent
primary files. The caller must scan/hash the complete returned inventory, enforce
all routing inverse proofs, and reject successors after a consolidation binding.
"""
from copy import deepcopy
import hashlib
import json
from pathlib import Path

import external_tests
import source_layout
import external_transfers
import source_consolidation

MAX_FILES = 2048
MAX_LIVE_BYTES = 64 * 1024 * 1024
MAX_PROOF_BYTES = 16 * 1024 * 1024


def _reject_pinned_secondary(root, inputs, primary):
    # The current routing row authenticates only its own family. Classification
    # also depends on other families, so none may be missing or merely trusted
    # as local JSON. Import lazily to avoid the routing adapter import cycle.
    from proof_routing import FAMILIES
    others = set(inputs) - {primary}
    if 'src/foundation/ring' in others:
        raise ValueError('pinned secondary origin needs its own proof projection adapter')
    for proof_path, proof_hash in FAMILIES.values():
        path = source_consolidation._safe(root, proof_path)
        if not path.exists():
            raise ValueError('missing pinned projection family proof')
        if not path.is_file() or path.stat().st_size > MAX_PROOF_BYTES:
            raise ValueError('invalid/bounded projection family proof')
        with path.open('rb') as stream:
            data = stream.read(MAX_PROOF_BYTES + 1)
        if len(data) > MAX_PROOF_BYTES:
            raise ValueError('projection family proof exceeds bound')
        if hashlib.sha256(data).hexdigest() != proof_hash:
            raise ValueError('pinned projection family proof changed')
        proof = json.loads(data)
        if not isinstance(proof, dict) or not isinstance(proof.get('files', []), list):
            raise ValueError('invalid projection family proof')
        paths = []
        for row in proof.get('files', []):
            if not isinstance(row, dict):
                raise ValueError('invalid projection family file')
            paths.extend(row[key] for key in ('source', 'path') if isinstance(row.get(key), str))
        for key in ('roots', 'external_files'):
            value = proof.get(key, {})
            if not isinstance(value, dict):
                raise ValueError('invalid projection family root inventory')
            paths.extend(value)
        for directory in others:
            test_root = 'test/foundation/' + directory.rsplit('/', 1)[1]
            if any(path == directory or path.startswith(directory + '/')
                   or path == test_root or path.startswith(test_root + '/') for path in paths):
                raise ValueError('pinned secondary origin needs its own proof projection adapter')


def projection(root, record, historical_bytes=None):
    """Return exactly package/external/inventory/roots/foreign.

package and external are only the primary origin's after snapshots. inventory
contains the full combined production/test union; roots are its physical roots.
foreign contains every nonprimary file, including explicitly reviewed new glue.
All inventories use repo-relative paths except package.files_sha256. Current
primary test files are safely statted to derive the projected owner's byte count;
full content and live-inventory equality remain the proof loader's obligation.
"""
    root = Path(root)
    inputs = source_consolidation._record(root, record)
    primary, destination = record['primary_directory'], record['destination_directory']
    _reject_pinned_secondary(root, inputs, primary)
    external_transfers.validate(root, record, check_current=False)
    origin = inputs[primary]['origin_directory']
    after_files = record['destination_files_sha256']
    primary_routes = record['file_routes'][primary]
    targets = []
    for route in primary_routes.values():
        if set(route) != {'destination'}:
            raise ValueError('pinned primary files cannot be archived')
        targets.append(route['destination'])
    if (len(set(targets)) != len(targets)
            or primary_routes['Cargo.toml'] != {'destination': 'Cargo.toml'}):
        raise ValueError('pinned primary production routes must preserve a manifest-rooted bijection')
    owned = {path: origin for path in targets}
    for directory, package in inputs.items():
        if directory == primary:
            continue
        destinations = {route['destination'] for route in record['file_routes'][directory].values()
                        if 'destination' in route}
        if set(owned) & destinations:
            raise ValueError('foreign production imports overlap another origin projection')
        owned.update({path: package['origin_directory'] for path in destinations})
    if set(targets) & set(record['new_files']):
        raise ValueError('new consolidation glue overlaps pinned primary projection')
    projected = deepcopy(inputs[primary])
    projected.update(directory=destination, name=record['destination_name'],
                     files_sha256={path: after_files[path] for path in targets})
    full = {destination + '/' + path: sha for path, sha in after_files.items()}
    primary_paths = {destination + '/' + path for path in targets}
    roots, owner, projected_owner = [destination], record['destination_external_tests'], None
    if owner is not None:
        roots.extend(owner['roots'])
        for row in owner['files']:
            if row['path'] in full:
                raise ValueError('projection full production/test inventory overlaps')
            full[row['path']] = row['sha256']
        rows = [deepcopy(row) for row in owner['files'] if row.get('origin_directory') == origin]
        if rows:
            projected_owner = deepcopy(owner)
            projected_owner['files'] = rows
            projected_owner['file_count'] = len(rows)
            projected_owner['roots'] = [base for base in owner['roots']
                                        if any(row['path'].startswith(base + '/') for row in rows)]
            size = 0
            for row in rows:
                path = source_consolidation._safe(root, row['path'])
                if not path.is_file():
                    raise ValueError('missing active primary external projection file')
                size += (len(historical_bytes[row['path']]) if historical_bytes is not None
                         else source_layout.logical_size(root, row['path']))
                if size > MAX_LIVE_BYTES:
                    raise ValueError('primary external projection byte bound exceeded')
                primary_paths.add(row['path'])
            if size > owner['bytes']:
                raise ValueError('primary external projection exceeds full owner byte count')
            projected_owner['bytes'] = size
    if not 1 <= len(full) <= MAX_FILES:
        raise ValueError('full consolidation projection inventory bound exceeded')
    for index, relative in enumerate(roots):
        source_consolidation._safe(root, relative)
        if any(relative == p or relative.startswith(p + '/') or p.startswith(relative + '/') for p in roots[:index]):
            raise ValueError('consolidation projection physical roots overlap')
    return dict(package=projected, external=projected_owner, inventory=full, roots=roots,
                foreign={path: sha for path, sha in full.items() if path not in primary_paths})


def validate_binding(root, row, catalog=None, prospective=False):
    """Validate the exact primary projection and, normally, full registration.

Prospective mode accepts no registration as authorization: the caller must match
before against the previous catalog and bind the exact pending reviewed plan.
Maintenance may supply an authenticated terminal snapshot; ordinary routing
successors remain forbidden. This function never calls proof_routing.load.
"""
    return validate_bindings(root, (row,), catalog, prospective)[0]


@source_layout.operation
def validate_bindings(root, rows, catalog=None, prospective=False):
    """Check every binding and authenticate shared history once for this call."""
    rows = tuple(rows)
    import proof_maintenance
    restored = {}
    if catalog is not None:
        catalog, restored, _ = proof_maintenance.restore(root, catalog, prospective)
        catalog = source_layout.project_catalog(root, catalog)
    if rows and not prospective:
        if not isinstance(catalog, dict):
            raise ValueError('consolidation proof requires registered current catalog')
        source_consolidation.validate_history(Path(root), catalog, validate_external=external_transfers.validate)
    results = []
    for row in rows:
        binding = row.get('binding') if isinstance(row, dict) else None
        record = binding.get('record') if isinstance(binding, dict) else None
        maintenance = restored.get(record.get('destination_directory')) if isinstance(record, dict) else None
        result = _validate_binding(root, row, catalog, prospective, maintenance)
        if maintenance is not None:
            result['maintenance'] = maintenance
        results.append(result)
    return results


def _validate_binding(root, row, catalog, prospective, maintenance=None):
    if not isinstance(row, dict):
        raise ValueError('invalid consolidation proof binding row')
    binding = row.get('binding')
    if (not isinstance(binding, dict) or set(binding) != {'kind', 'record'}
            or binding['kind'] != 'consolidation'):
        raise ValueError('invalid consolidation proof binding')
    record = binding['record']
    result = projection(root, record, maintenance['bytes'] if maintenance is not None else None)
    primary = next(item for item in record['inputs']
                   if item['package']['directory'] == record['primary_directory'])
    if (row.get('before') != primary['package'] or row.get('external_before') != primary['external_tests']
            or row.get('after') != result['package'] or row.get('external_after') != result['external']):
        raise ValueError('consolidation proof primary before/after projection mismatch')
    routes = {}
    directory = record['primary_directory']
    for relative, sha in primary['package']['files_sha256'].items():
        target = record['file_routes'][directory][relative]['destination']
        routes[directory + '/' + relative] = (record['destination_directory'] + '/' + target,
                                             sha, record['destination_files_sha256'][target])
    after_rows = {entry['path']: entry for entry in (result['external'] or {}).get('files', [])}
    for entry in (primary['external_tests'] or {}).get('files', []):
        target = record['external_file_routes'][entry['path']]['destination']
        routes[entry['path']] = (target, entry['sha256'], after_rows[target]['sha256'])
    files = row.get('files')
    if not isinstance(files, list) or len(files) != len(routes):
        raise ValueError('consolidation proof must cover exact primary file routes')
    seen = set()
    for entry in files:
        if not isinstance(entry, dict) or not isinstance(entry.get('before'), str):
            raise ValueError('invalid consolidation proof primary file route')
        before = entry['before']
        if before in seen or routes.get(before) != (entry.get('after'), entry.get('before_sha256'), entry.get('after_sha256')):
            raise ValueError('consolidation proof route differs from exact bound primary route')
        seen.add(before)
    if not prospective:
        if not isinstance(catalog, dict):
            raise ValueError('consolidation proof requires registered current catalog')
        key = source_consolidation.receipt_id(record)
        if catalog.get('consolidations', {}).get(key) != record:
            raise ValueError('consolidation proof registered immutable binding differs')
        destination = record['destination_directory']
        expected = deepcopy(primary['package'])
        expected.update(directory=destination, name=record['destination_name'],
                        files_sha256=deepcopy(record['destination_files_sha256']))
        if (catalog['packages'].get(destination) != expected
                or catalog.get('external_tests', {}).get(destination) != record['destination_external_tests']):
            raise ValueError('consolidation proof final full active owner differs')
        for item in record['inputs']:
            before = item['package']
            if before['directory'] == record['primary_directory']:
                continue
            expected = deepcopy(before)
            expected.update(directory=destination, files_sha256=deepcopy(record['destination_files_sha256']),
                            merged_into=destination)
            matching = [p for p in catalog.get('merges', []) if p.get('origin_directory') == before['origin_directory']]
            if matching != [expected]:
                raise ValueError('consolidation proof final full merged owner differs')
    return result
