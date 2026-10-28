"""Exact, owner-scoped current test material accounting, separate from origins."""
import hashlib
import json
import os
import re
import external_fixtures
import source_layout
from pathlib import Path

MAPPINGS = 'compliance/external-tests'


def safe(root, relative):
    if (not isinstance(relative, str) or not relative or '\\' in relative
            or any(p in ('', '.', '..') or ':' in p for p in relative.split('/'))):
        raise ValueError(f'unsafe external test path: {relative}')
    current = root
    for part in relative.split('/'):
        current /= part
        if current.is_symlink():
            raise ValueError(f'external test symlink: {relative}')
    if not current.resolve().is_relative_to(root.resolve()):
        raise ValueError(f'external test escape: {relative}')
    return current


def inventory(root, relative):
    directory = safe(root, relative)
    if not directory.is_dir():
        raise ValueError(f'missing external test root: {relative}')
    result = {}
    for base, dirs, names in os.walk(directory, followlinks=False):
        for name in dirs + names:
            path = Path(base) / name
            rel = path.relative_to(root).as_posix()
            safe(root, rel)
            if path.is_file():
                result[rel] = hashlib.sha256(path.read_bytes()).hexdigest()
            elif not path.is_dir():
                raise ValueError(f'external test special file: {rel}')
    return result


def _owns_source(owner, entry, origin, prefixes, transfers, root=None):
    from external_transfers import permits
    if external_fixtures.claims(entry):
        return (external_fixtures.permits(root, owner, entry, origin)
                and not permits(transfers, owner['directory'], entry, origin))
    if 'source_kind' in entry:
        # Local work is not an upstream extraction or a cross-owner transfer.
        return (entry['source_kind'] == 'project-authored'
                and entry.get('origin_directory', origin) == origin
                and entry['source'].startswith('test/')
                and any(entry['source'].startswith(p + '/') for p in owner['roots'])
                and not permits(transfers, owner['directory'], entry, origin))
    native = entry.get('origin_directory', origin) == origin and entry['source'].startswith(prefixes)
    return native or permits(transfers, owner['directory'], entry, origin)


def _identity(row):
    return (row.get('source_kind'), row.get('origin_directory'), row['source'],
            row['source_sha256_before'], row.get('source_receipt'))


@source_layout.operation
def load(root, packages, renames=None, transfers=None):
    """Never learn new source bytes during refresh: verify explicit receipts first."""
    transfers = source_layout.current_grants(root, transfers or {})
    directory = safe(root, MAPPINGS)
    if not directory.exists():
        return {}, {}
    owners, receipts, all_roots, paths = {}, {}, [], set()
    for file in sorted(directory.iterdir()):
        safe(root, file.relative_to(root).as_posix())
        if not file.is_file() or file.suffix != '.json':
            raise ValueError(f'unexpected external test mapping: {file}')
        content = file.read_bytes()
        receipts[file.relative_to(root).as_posix()] = hashlib.sha256(content).hexdigest()
        mapping = json.loads(content)
        if mapping.get('schema_version') != 1:
            raise ValueError('unsupported external test mapping schema')
        for owner in mapping['owners']:
            identity = owner['directory']
            if identity not in packages or identity in owners:
                raise ValueError(f'unknown/duplicate external test owner: {identity}')
            from source_naming import source_prefixes
            prefixes = source_layout.historical_prefixes(root, identity, packages[identity]['origin_directory'], renames or {})
            expected = {}
            for entry in owner['files']:
                source, path = entry['source'], entry['path']
                if not re.fullmatch(r'[0-9a-f]{64}', entry.get('source_sha256_before', '')):
                    raise ValueError(f'missing original test source hash: {source}')
                safe(root, source)
                safe(root, path)
                primary_origin = packages[identity]['origin_directory']
                if (not _owns_source(owner, entry, primary_origin, prefixes, transfers, root)
                        or not path.startswith('test/') or path in paths):
                    raise ValueError(f'invalid external test ownership: {path}')
                paths.add(path)
                expected[path] = entry['sha256']
            actual = {}
            for relative in owner['roots']:
                safe(root, relative)
                if not relative.startswith('test/') or any(relative == p or relative.startswith(p + '/') or p.startswith(relative + '/') for p in all_roots):
                    raise ValueError(f'overlapping/unsafe external test roots: {relative}')
                all_roots.append(relative)
                actual.update(inventory(root, relative))
            if actual != expected:
                raise ValueError(f'external test inventory/content drift: {identity}')
            owners[identity] = dict(owner, origin_directory=packages[identity]['origin_directory'],
                                   file_count=len(expected), bytes=sum(safe(root, p).stat().st_size for p in expected))
    # Any new file beneath an attributed foundation package must be inventoried,
    # including files placed next to (rather than inside) a declared subdirectory.
    foundation = root / 'test/foundation'
    if foundation.exists():
        actual = inventory(root, 'test/foundation')
        if set(actual) != {p for p in paths if p.startswith('test/foundation/')}:
            raise ValueError('unattributed external foundation test material')
    return owners, receipts


def check_removed(previous, current, owners, renames=None, trim_grants=None, root=None):
    for directory, package in previous.items():
        if directory not in current or package.get('origin_directory') != current[directory].get('origin_directory'):
            continue
        removed = set(package.get('files_sha256', {})) - set(current[directory]['files_sha256'])
        from source_naming import source_prefixes
        prefixes = (source_layout.historical_prefixes(root, directory, package.get('origin_directory'), renames or {})
                    if root is not None else source_prefixes(renames or {}, directory, package.get('origin_directory')))
        attributed = {path for path, value in (trim_grants or {}).get(directory, {}).items()
                      if package.get("files_sha256", {}).get(path) == value}
        for entry in owners.get(directory, {}).get('files', []):
            if entry.get('source_kind') in ('project-authored', 'upstream-fixture'):
                continue
            for prefix in prefixes:
                if entry['source'].startswith(prefix):
                    relative = entry['source'][len(prefix):]
                    if package.get('files_sha256', {}).get(relative) == entry['source_sha256_before']:
                        attributed.add(relative)
        # Prior external receipts remain explicit even if a test is relocated again.
        if not removed <= attributed:
            raise ValueError(f'unaccounted source removal during test separation: {directory}: {sorted(removed - attributed)}')


def check_previous(previous, current, root=None, packages=None, previous_packages=None):
    """Allow exact whole-file restoration, never disappearance of test fragments."""
    from collections import Counter
    for directory, owner in previous.items():
        # Legacy rows predate per-origin transfers and may carry a corrected
        # aggregate owner. Explicit row origins are immutable transfer identity.
        old = Counter(_identity(row) for row in owner['files'])
        next_owner = current.get(directory, {})
        new = Counter(_identity(row) for row in next_owner.get('files', []))
        missing = old - new
        for (kind, origin, source, before_hash, source_receipt), count in list(missing.items()):
            candidates = [row for row in owner['files']
                          if _identity(row) == (kind, origin, source, before_hash, source_receipt)]
            # Only a single whole-file move can be reversed this way. Multiple
            # extracted fragments cannot collapse into one restored production file.
            if kind is not None or count != 1 or len(candidates) != 1 or root is None:
                continue
            row = candidates[0]
            package = (packages or {}).get(directory, {})
            prior = (previous_packages or {}).get(directory, {})
            if (origin not in (None, owner.get('origin_directory')) or not source.startswith(directory + '/')
                    or package.get('origin_directory') != owner.get('origin_directory')
                    or prior.get('origin_directory') != owner.get('origin_directory')
                    or current.get(directory, {}).get('origin_directory', owner.get('origin_directory')) != owner.get('origin_directory')
                    or row['sha256'] != before_hash):
                continue
            relative = source[len(directory) + 1:]
            if (relative in prior.get('files_sha256', {})
                    or package.get('files_sha256', {}).get(relative) != row['sha256']):
                continue
            restored, external = safe(root, source), safe(root, row['path'])
            if (external.exists() or not restored.is_file()
                    or hashlib.sha256(restored.read_bytes()).hexdigest() != row['sha256']):
                continue
            del missing[(kind, origin, source, before_hash, source_receipt)]
        if missing:
            raise ValueError(f'external test source accounting shrank: {directory}')
    for directory, owner in current.items():
        old = {_identity(row) for row in previous.get(directory, {}).get('files', [])}
        seen = set()
        for row in owner['files']:
            if row.get('source_kind') != 'project-authored':
                continue
            identity = _identity(row)
            if identity in seen:
                raise ValueError(f'duplicate project-authored test identity: {row["source"]}')
            seen.add(identity)
            if identity in old:
                continue
            if (row['source'] != row['path'] or row['source_sha256_before'] != row['sha256']
                    or root is None or not safe(root, row['path']).is_file()
                    or hashlib.sha256(safe(root, row['path']).read_bytes()).hexdigest() != row['sha256']):
                raise ValueError(f'invalid initial project-authored test source: {row["path"]}')


def validate_archived(root, owner, package, removed=False, renames=None, transfers=None):
    """Validate a last-current receipt without learning bytes from absent files."""
    directory = package['directory']
    if (not isinstance(owner, dict) or owner.get('directory') != directory
            or owner.get('origin_directory') != package['origin_directory']
            or not isinstance(owner.get('files'), list) or not owner['files']
            or owner.get('file_count') != len(owner['files'])
            or type(owner.get('bytes')) is not int or owner['bytes'] < 0
            or not isinstance(owner.get('roots'), list) or not owner['roots']):
        raise ValueError(f'incomplete archived external test receipt: {directory}')
    from source_naming import source_prefixes
    prefixes = source_layout.historical_prefixes(root, directory, package['origin_directory'], renames or {})
    roots, paths = [], set()
    for relative in owner['roots']:
        path = safe(root, relative)
        if not relative.startswith('test/') or any(relative == p or relative.startswith(p + '/') or p.startswith(relative + '/') for p in roots):
            raise ValueError(f'invalid archived external test root: {relative}')
        if removed and path.exists():
            raise ValueError(f'external test removal requires absent root: {relative}')
        roots.append(relative)
    for entry in owner['files']:
        source, path = entry['source'], entry['path']
        safe(root, source); safe(root, path)
        if (not _owns_source(owner, entry, package['origin_directory'], prefixes, transfers or {}, root) or path in paths
                or not any(path.startswith(p + '/') for p in roots)
                or any(not re.fullmatch(r'[0-9a-f]{64}', entry.get(key, ''))
                       for key in ('sha256', 'source_sha256_before'))):
            raise ValueError(f'invalid archived external test source: {path}')
        paths.add(path)


def archive_removal(root, previous, item, package, renames=None):
    """Only a matching reviewed plan can dispose of the last external inventory."""
    directory = package['directory']
    owner = previous.get('external_tests', {}).get(directory)
    if owner is None:
        if 'external_tests' in item or 'external_action' in item:
            raise ValueError(f'no prior external test receipt for disposition: {directory}')
        return None
    if item.get('external_action') != 'remove' or item.get('external_tests') != owner:
        raise ValueError(f'explicit external test removal required for {directory}: '
                         'set external_action=remove and external_tests to the exact last catalog receipt; '
                         'remove its mapping and roots. For an upgrade, restore successor tests in '
                         'the authenticated source_prefixes before a separate reviewed relocation.')
    validate_archived(root, owner, package, removed=True, renames=renames)
    return owner


def transition_previous(previous, retired, superseded, current):
    """Exclude only exact archived dispositions, never all owners in a plan."""
    retained = {}
    for directory, owner in previous.items():
        archived = retired.get(directory, {}).get('retirement', {}).get('external_tests')
        if archived != owner:
            archived = superseded.get(owner['origin_directory'], {}).get('external_tests')
        if archived == owner:
            if directory in current:
                raise ValueError(f'removed external test mapping remains active: {directory}')
        else:
            retained[directory] = owner
    return retained


def has_history(catalog):
    return bool(catalog.get('initial_test_relocation_sha256') or catalog.get('external_tests') or any(
        'external_tests' in package.get('retirement', {}) for package in catalog.get('retired', {}).values()) or any(
        'external_tests' in record for record in catalog.get('superseded', {}).values()))
