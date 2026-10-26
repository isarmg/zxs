"""Reviewed one-time import of the three binding-test text responsibilities.

The pinned receipt binds the predecessor catalog and layout, complete original
archives, centralized bytes, initial current trees and external-test ownership.
It is not a general package installer or a replacement for normal refresh.
"""
from copy import deepcopy
import io
import json
import tarfile

from source_layout import canonical, digest, safe, _stable

KEY = 'binding_text_import_receipt'
RECEIPTS = ("66c5ee126c07c070103703a02df428eb58c4556abdad585b596f97664111b86b",)  # Append the reviewed content-addressed import receipt.



def content(root, path):
    target = safe(root, path)
    if not target.is_file() or target.stat().st_size > 32 * 1024 * 1024:
        raise ValueError('missing, special or oversized import material')
    return target.read_bytes()


def read(root, marker):
    if marker not in RECEIPTS:
        raise ValueError('unreviewed binding text import receipt')
    data = content(root, 'compliance/source-imports/' + marker + '.json')
    if digest(data) != marker:
        raise ValueError('binding text import receipt changed')
    plan = json.loads(data)
    if (set(plan) != {'schema', 'reason', 'predecessor_catalog_sha256', 'predecessor_layout', 'packages'}
            or plan['schema'] != 1 or not isinstance(plan['reason'], str) or not plan['reason'].strip()
            or not isinstance(plan['packages'], list) or not plan['packages']):
        raise ValueError('invalid binding text import receipt')
    names = [row['package']['name'] for row in plan['packages']]
    origins = [row['origin']['origin_directory'] for row in plan['packages']]
    directories = [row['package']['directory'] for row in plan['packages']]
    if any(len(set(values)) != len(values) for values in (names, origins, directories)):
        raise ValueError('duplicate imported responsibility, origin or directory')
    return plan


def validate_material(root, row):
    """Read only regular archive members; never extract or execute source tools."""
    package, origin = row['package'], row['origin']
    import manage
    upstream = origin['upstream']
    name, version, checksum = manage.upstream_identity(upstream)
    identity = name + '-' + version
    directory = package['directory']
    safe(root, directory)
    parts = directory.split('/')
    if len(parts) != 4 or parts[:2] != ['src', 'foundation'] or parts[-1] != package['name']:
        raise ValueError('imported responsibility requires a grouped foundation directory')
    if (package['version'] != version
            or origin['origin_directory'] != 'vendor/rust/' + identity
            or package['upstream'] != upstream
            or package['origin_directory'] != origin['origin_directory']
            or package['license_materials'] != origin['central_license_materials']
            or any(package.get(k) != origin.get(k) for k in ('license', 'repository', 'vcs'))
            or not row['omissions'] or not row['source_disposition']):
        raise ValueError('binding text import identity or disposition mismatch')
    archive_path = 'compliance/provenance/packages/' + identity + '/release.crate'
    if row['archive'] != archive_path:
        raise ValueError('binding text import archive path mismatch')
    archive = content(root, archive_path)
    if digest(archive) != checksum:
        raise ValueError('binding text import archive checksum mismatch')
    original = {}
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as source:
        for member in source:
            if not member.isfile() or not member.name.startswith(identity + '/'):
                raise ValueError('binding text import archive contains non-file or foreign root')
            path = member.name[len(identity) + 1:]
            safe(root, path)
            if path in original or member.size > 32 * 1024 * 1024:
                raise ValueError('binding text import archive duplicate or oversized file')
            original[path] = digest(source.extractfile(member).read())
    if original != origin['original_files_sha256'] or set(original) != set(row['original_paths']):
        raise ValueError('binding text import original file coverage mismatch')
    if set(row['source_disposition']) != set(original):
        raise ValueError('binding text import missing source disposition')
    attributed = []
    for disposition in row['source_disposition'].values():
        if (set(disposition) != {'current_paths', 'reason'} or not disposition['reason']
                or not isinstance(disposition['current_paths'], list)):
            raise ValueError('incomplete import source disposition')
        for path in disposition['current_paths']:
            safe(root, path)
            attributed.append(path)
    for entry in row['external_tests']['files']:
        if entry.get('source_kind') == 'project-authored':
            continue
        source = entry['source'].removeprefix(directory + '/')
        if (entry['source'] != directory + '/' + source
                or original.get(source) != entry['source_sha256_before']
                or entry['path'] not in row['source_disposition'].get(source, {}).get('current_paths', [])):
            raise ValueError('import external source differs from original attribution')
    current_paths = {directory + '/' + path for path in package['files_sha256']}
    current_paths.update(entry['path'] for entry in row['external_tests']['files'])
    local_paths = {entry['path'] for entry in row['external_tests']['files']
                   if entry.get('source_kind') == 'project-authored'}
    authored = row['project_authored_files']
    for path, checksum in authored.items():
        safe(root, path)
        if (not path.startswith(directory + '/src/')
                or package['files_sha256'].get(path[len(directory) + 1:]) != checksum):
            raise ValueError('import project-authored production source mismatch')
    if (len(attributed) != len(set(attributed)) or set(attributed) & set(authored)
            or set(attributed) | local_paths | set(authored) != current_paths):
        raise ValueError('import current source disposition coverage mismatch')
    paths = set()
    for path, central in row['original_paths'].items():
        if (central in paths or not central.startswith(('compliance/provenance/packages/' + identity + '/',
                'compliance/licenses/upstream/' + identity + '/', 'docs/upstream/' + identity + '/'))):
            raise ValueError('binding text import original material destination mismatch')
        paths.add(central)
        if digest(content(root, central)) != original[path]:
            raise ValueError('binding text import original material changed')
    for path, checksum in row['materials'].items():
        if not path.startswith(('compliance/licenses/', 'compliance/provenance/packages/' + identity + '/', 'docs/upstream/' + identity + '/')):
            raise ValueError('binding text import material outside source scope')
        if digest(content(root, path)) != checksum:
            raise ValueError('binding text import license/metadata checksum mismatch')
    required = set(origin['central_license_materials'] + origin['central_original_metadata'] + origin['central_original_documentation'])
    if not origin['central_license_materials'] or not required <= paths | set(row['materials']):
        raise ValueError('binding text import missing centralized legal/metadata material')


def current(layout, catalog):
    marker = catalog.get(KEY)
    if marker is None:
        return {}, set()
    plan = read(layout.root, marker)
    if plan['predecessor_layout'] != layout.receipt_id:
        raise ValueError('binding text import predecessor layout mismatch')
    owners, origins = {}, set()
    for row in plan['packages']:
        validate_material(layout.root, row)
        before = row['package']; directory = before['directory']; origin = row['origin']['origin_directory']
        package = catalog.get('packages', {}).get(directory)
        if (not isinstance(package, dict) or _stable(package) != _stable(before)
                or layout.appended_origins.get(origin) != row['origin']
                or directory in layout.reverse or directory in catalog.get('retired', {})
                or any(p.get('directory') == directory for p in catalog.get('merges', []))):
            raise ValueError('binding text import current identity or origin drift')
        owners[directory] = row
        origins.add(origin)
    return owners, origins


def prepare(root, previous, marker, previous_raw):
    """Validate all staged inputs before appending origins; no old bytes change."""
    import manage
    plan = read(root, marker)
    if (KEY in previous or digest(previous_raw) != plan['predecessor_catalog_sha256']
            or previous.get('foundation_layout_receipt') != plan['predecessor_layout']):
        raise ValueError('stale binding text import predecessor')
    records = manage.origins(root)
    if manage.origin_hashes(records) != previous['origin_records_sha256']:
        raise ValueError('binding text import existing origin changed')
    for path, checksum in previous['provenance_sha256'].items():
        if digest(content(root, path)) != checksum:
            raise ValueError('binding text import existing original material changed')
    occupied = set(previous['packages']) | set(previous.get('retired', {}))
    old_names = {p['name'] for p in [*previous['packages'].values(), *previous.get('retired', {}).values()]}
    for row in plan['packages']:
        validate_material(root, row)
        package = row['package']; directory = package['directory']
        if directory in occupied or package['name'] in old_names or row['origin']['origin_directory'] in records:
            raise ValueError('binding text import duplicate or retired identity')
        if manage.files(safe(root, directory)) != package['files_sha256']:
            raise ValueError('binding text import initial current inventory mismatch')
        owner = row['external_tests']
        if owner['roots'] != ['test/foundation/' + package['name']]:
            raise ValueError('binding text import external owner mismatch')
        import external_tests
        external_tests.validate_archived(root, owner, package)
        actual = external_tests.inventory(root, owner['roots'][0])
        if actual != {entry['path']: entry['sha256'] for entry in owner['files']}:
            raise ValueError('binding text import external inventory mismatch')
    policy = manage.read_json(root / manage.POLICY)
    manage.structure(root, policy)
    actual = {path.parent.relative_to(root).as_posix()
              for path in safe(root, 'src/foundation').glob('*/*/Cargo.toml')}
    expected = {p for p in previous['packages'] if p.startswith('src/foundation/')}
    expected.update(row['package']['directory'] for row in plan['packages'])
    if actual != expected:
        raise ValueError('binding text import unregistered physical source directory')
    # The normal refresh still checks the exact physical source set, feature
    # edges, source coverage, all previous external labels and closure reachability.
    result = deepcopy(previous)
    result[KEY] = marker
    for row in plan['packages']:
        package = row['package']; directory = package['directory']
        result['packages'][directory] = deepcopy(package)
        result['external_tests'][directory] = deepcopy(row['external_tests'])
    origins_path = safe(root, manage.ORIGINS)
    original = json.loads(origins_path.read_bytes())
    original['packages'].extend(row['origin'] for row in plan['packages'])
    manage.atomic_write(origins_path, (json.dumps(original, ensure_ascii=False, indent=2) + '\n').encode())
    return result
