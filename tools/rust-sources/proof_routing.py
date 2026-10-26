"""Bounded, explicitly pinned routing of active proof-owned source trees.

No missing-file archive fallback. Every historical file has one present current
file, and only unique reviewed replacements may reverse its routing edits.
"""
import base64
import gzip
import hashlib
import io
import json
import os
import tomllib
from pathlib import Path

from external_tests import safe, inventory
import source_layout

# Append only. Each digest names compliance/proof-routing/<digest>.json.gz.
# Register only reviewed, exact routing transactions.
RECEIPTS = ('42cbfc30f0e5db58801c1806abba6926e34a3279dd0697bc1b4cdf2c98fbcce0', 'f22401d8de23ba2f0f8af6447b300206aa5257f8c9a173b1f47541ec587d3a21', '1c36e75fd2f038976efc3d62f0137291aa8129c58d904c0052be5197bbb32174', '21b9ca69f0feeb8a6c30ad45ab630d900f1a6fdab01478182f5a9c6f0a1bbbee', '441ceffc9c12e0bd2cd7daf2f248e1d4b94e76b8ebcfe67d22258a6500ba2d86')
MAX_COMPRESSED = 2 * 1024 * 1024
MAX_EXPANDED = 16 * 1024 * 1024
MAX_FILES = 2048
MAX_REPLACEMENTS = 512
MAX_PATCH_BYTES = 2 * 1024 * 1024
MAX_LIVE_BYTES = 64 * 1024 * 1024
FAMILIES = {
    'platform': ('compliance/platform-test-relocation.json', '6adb981c33cc4058bf9225072fe24a66047ef17f7020348d23152e75fb7169ca'),
    'ring': ('compliance/ring-test-relocation.json', '178ec0e2f7a74909cd037f3d2eda20ef50911d305effcd78311267addb273a5c'),
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def record_digest(record):
    return digest(json.dumps(record, sort_keys=True, ensure_ascii=False).encode())


def _hash(value):
    return isinstance(value, str) and len(value) == 64 and all(c in '0123456789abcdef' for c in value)


def _package(root, package, owner, baseline=None):
    required = ('directory', 'origin_directory', 'name', 'version', 'upstream', 'files_sha256')
    if not isinstance(package, dict) or any(k not in package for k in required):
        raise ValueError('incomplete proof routing package')
    directory = package['directory']
    safe(root, directory)
    if not directory.startswith('src/foundation/') or len(directory.split('/')) != 3:
        raise ValueError('proof routing requires direct foundation owner')
    files = package['files_sha256']
    if not isinstance(files, dict) or not files:
        raise ValueError('empty proof routing source inventory')
    result = {}
    for path, sha in files.items():
        full = directory + '/' + path
        safe(root, full)
        if not _hash(sha):
            raise ValueError('invalid proof routing source hash')
        result[full] = sha
    roots = [directory]
    if owner is not None:
        from external_tests import validate_archived
        validate_archived(root, owner, package, renames={baseline: {'destination_directory': directory, 'package': package}} if baseline and baseline != directory else None)
        roots += owner['roots']
        for row in owner['files']:
            if row['path'] in result:
                raise ValueError('overlapping proof routing inventory')
            result[row['path']] = row['sha256']
    if len(result) > MAX_FILES:
        raise ValueError('proof routing file bound exceeded')
    for index, path in enumerate(roots):
        if any(path == other or path.startswith(other + '/') or other.startswith(path + '/') for other in roots[:index]):
            raise ValueError('overlapping proof routing roots')
    return result, roots


def _read(root, sha):
    if not _hash(sha):
        raise ValueError('invalid proof routing receipt ID')
    path = safe(root, 'compliance/proof-routing/' + sha + '.json.gz')
    if path.stat().st_size > MAX_COMPRESSED:
        raise ValueError('proof routing compressed bound exceeded')
    content = path.read_bytes()
    if digest(content) != sha:
        raise ValueError('immutable proof routing receipt changed')
    with gzip.GzipFile(fileobj=io.BytesIO(content)) as stream:
        data = stream.read(MAX_EXPANDED + 1)
        if len(data) > MAX_EXPANDED or stream.read(1):
            raise ValueError('proof routing expansion bound exceeded')
    row = json.loads(data)
    fields = {'schema', 'parent', 'family', 'reason', 'before', 'after',
              'external_before', 'external_after', 'binding', 'files'}
    if (not isinstance(row, dict) or set(row) != fields or type(row['schema']) is not int or row['schema'] != 1
            or row['family'] not in FAMILIES or not isinstance(row['reason'], str) or not row['reason'].strip()
            or not isinstance(row['files'], list) or not 1 <= len(row['files']) <= MAX_FILES):
        raise ValueError('invalid proof routing receipt schema')
    proof_path, proof_hash = FAMILIES[row['family']]
    if digest(safe(root, proof_path).read_bytes()) != proof_hash:
        raise ValueError('proof routing baseline proof changed')
    return row


def _inverse(data, row):
    if digest(data) != row['after_sha256']:
        raise ValueError('proof routing current content drift')
    return inverse_replacements(data, row)


def inverse_replacements(data, row):
    """Restore the before image from already authenticated after bytes."""
    if not data and row['before_sha256'] != digest(b''):
        raise ValueError('empty proof routing replacement is not a routing edit')
    replacements = row['replacements']
    if not isinstance(replacements, list) or len(replacements) > MAX_REPLACEMENTS:
        raise ValueError('proof routing replacement bound exceeded')
    total = 0
    for edit in reversed(replacements):
        if not isinstance(edit, dict) or set(edit) != {'old', 'new'}:
            raise ValueError('invalid proof routing replacement')
        old, new = (base64.b64decode(edit[k], validate=True) for k in ('old', 'new'))
        total += len(old) + len(new)
        if total > MAX_PATCH_BYTES or not old or not new or old == new:
            raise ValueError('invalid/bounded proof routing replacement')
        if len(new) >= len(data) or data.count(new) != 1:
            raise ValueError('proof routing replacement is full-file or ambiguous')
        previous = data.replace(new, old)
        if previous.count(old) != 1 or previous.replace(old, new) != data:
            raise ValueError('proof routing replacement is not reversible')
        data = previous
    if digest(data) != row['before_sha256']:
        raise ValueError('proof routing historical content mismatch')
    return data


class View:
    def __init__(self, root):
        self.root = root
        self.bytes = {}
        self.forward = {}
        self.backward = {}
        self.roots = set()
        self.packages = {}
        self.records = []
        self.maintenance_records = []

    def _touched(self, path):
        return any(path == r or path.startswith(r + '/') for r in self.roots)

    def read(self, path):
        path = str(path); safe(self.root, path)
        if path in self.bytes:
            return self.bytes[path]
        if self._touched(path):
            raise ValueError('unmapped path in active proof owner')
        return source_layout.read(self.root, path)

    def inventory(self, root):
        root = str(root); safe(self.root, root)
        if root in self.roots:
            return {p: digest(b) for p, b in self.bytes.items() if p.startswith(root + '/')}
        return source_layout.inventory(self.root, root)

    def current_path(self, path):
        path = str(path); safe(self.root, path)
        if path in self.forward:
            return source_layout.current_path(self.root, self.forward[path])
        # Roots are used by Cargo and development-marker checks too.
        if path in self.packages:
            return source_layout.current_path(self.root, self.packages[path]['directory'])
        matches = set()
        for old, current in self.forward.items():
            if old.startswith(path + '/'):
                suffix = old[len(path):]
                if not current.endswith(suffix):
                    raise ValueError('nonuniform current proof directory mapping')
                matches.add(current[:-len(suffix)])
        if len(matches) == 1:
            return source_layout.current_path(self.root, matches.pop())
        if self._touched(path):
            raise ValueError('unmapped current proof path')
        return source_layout.current_path(self.root, path)

    def baseline_path(self, path):
        path = str(path); safe(self.root, path)
        layout = source_layout.load(self.root)
        path = layout.historical_path(path) if layout else path
        return self.backward.get(path, path)

    def current_package(self, directory):
        directory = str(directory)
        if directory in self.packages:
            return dict(self.packages[directory], directory=source_layout.current_path(self.root, self.packages[directory]['directory']))
        manifest = tomllib.loads(source_layout.read(self.root, directory + '/Cargo.toml').decode())['package']
        return dict(directory=source_layout.current_path(self.root, directory), name=manifest['name'], version=manifest['version'])


@source_layout.operation
def load(root, catalog=None, prospective=False):
    """Return authenticated baseline live bytes; prospective is only for refresh authorization."""
    root = Path(root); view = View(root)
    if not RECEIPTS:
        if catalog is None and safe(root, 'compliance/rust-sources.json').is_file():
            catalog = json.loads(safe(root, 'compliance/rust-sources.json').read_bytes())
        if catalog and catalog.get('proof_routing_receipts'):
            raise ValueError('pinned proof routing history was removed')
        return view
    if len(set(RECEIPTS)) != len(RECEIPTS) or len(RECEIPTS) > 64:
        raise ValueError('invalid proof routing chain length/duplicates')
    if catalog is None:
        catalog = json.loads(safe(root, 'compliance/rust-sources.json').read_bytes())
    if not prospective and (type(catalog.get('schema')) is not int or catalog['schema'] < 7):
        raise ValueError('active proof routing requires catalog schema 7')
    registered = catalog.get('proof_routing_receipts', [])
    if (not isinstance(registered, list) or list(RECEIPTS[:len(registered)]) != registered
            or (not prospective and registered != list(RECEIPTS))):
        raise ValueError('pinned proof routing catalog history differs')
    chains, tips = {}, {}
    for sha in RECEIPTS:
        row = _read(root, sha)
        before, after = row['before'], row['after']
        if not isinstance(before, dict) or not isinstance(after, dict):
            raise ValueError('invalid proof routing package structure')
        origin = before['origin_directory']
        baseline = chains[origin][0][1]['before']['directory'] if origin in chains else before['directory']
        old, old_roots = _package(root, before, row['external_before'], baseline)
        new, new_roots = _package(root, after, row['external_after'], baseline)
        from collections import Counter
        labels = lambda owner: Counter((r['source'], r['source_sha256_before']) for r in (owner or {}).get('files', []))
        if labels(row['external_before']) != labels(row['external_after']):
            raise ValueError('proof routing external historical attribution changed')
        if (after['origin_directory'] != origin or before['upstream'] != after['upstream']
                or before['version'] != after['version'] or row['parent'] != tips.get(origin)):
            raise ValueError('proof routing origin/version/parent mismatch')
        chain = chains.setdefault(origin, [])
        if not chain:
            proof = json.loads(safe(root, FAMILIES[row['family']][0]).read_bytes())
            test_root = 'test/foundation/' + before['directory'].rsplit('/', 1)[1]
            covered = (before['directory'] == 'src/foundation/ring' if row['family'] == 'ring'
                       else test_root in proof.get('roots', {}) or any(
                           item.get('source', '').startswith(before['directory'] + '/')
                           for item in proof.get('files', [])))
            if not covered:
                raise ValueError('proof routing owner not covered by pinned family')
        if chain and chain[-1][1]['binding']['kind'] == 'consolidation':
            raise ValueError('editing consolidated pinned sources requires a new authenticated adapter')
        if chain and (row['family'] != chain[-1][1]['family'] or before != chain[-1][1]['after'] or row['external_before'] != chain[-1][1]['external_after']):
            raise ValueError('proof routing ancestry discontinuity')
        paths_old, paths_new = set(), set()
        replacement_count = replacement_bytes = 0
        for file in row['files']:
            if not isinstance(file, dict) or set(file) != {'before', 'after', 'before_sha256', 'after_sha256', 'replacements'}:
                raise ValueError('invalid proof routing file record')
            edits = file['replacements']
            if not isinstance(edits, list):
                raise ValueError('invalid proof routing replacements')
            replacement_count += len(edits)
            for edit in edits:
                if not isinstance(edit, dict) or set(edit) != {'old', 'new'}:
                    raise ValueError('invalid proof routing replacement')
                replacement_bytes += sum(len(base64.b64decode(edit[k], validate=True)) for k in ('old', 'new'))
            if replacement_count > MAX_REPLACEMENTS or replacement_bytes > MAX_PATCH_BYTES:
                raise ValueError('aggregate proof routing replacement bound exceeded')
            a, b = file['before'], file['after']; safe(root, a); safe(root, b)
            if (a in paths_old or b in paths_new or old.get(a) != file['before_sha256'] or new.get(b) != file['after_sha256']):
                raise ValueError('proof routing file bijection/hash mismatch')
            paths_old.add(a); paths_new.add(b)
        if paths_old != set(old) or paths_new != set(new):
            raise ValueError('proof routing file inventory mismatch')
        binding = row['binding']
        if not isinstance(binding, dict) or set(binding) != {'kind', 'record'} or binding['kind'] not in ('rename', 'edit', 'consolidation'):
            raise ValueError('invalid proof routing transition binding')
        if binding['kind'] == 'edit':
            if before['directory'] != after['directory'] or before['name'] != after['name'] or binding['record'] is not None:
                raise ValueError('same-root proof edit changed identity')
        elif binding['kind'] == 'consolidation':
            # Authenticate these together below, before returning any live view.
            pass
        else:
            bound = binding['record']
            if (not isinstance(bound, dict) or bound.get('package') != before
                    or bound.get('destination_directory') != after['directory'] or bound.get('destination_name') != after['name']
                    or bound.get('files_sha256') != after['files_sha256'] or bound.get('external_tests') != row['external_before']
                    or bound.get('destination_external_tests') != row['external_after']):
                raise ValueError('proof routing rename binding mismatch')
            if not prospective and catalog.get('renames', {}).get(before['directory']) != bound:
                raise ValueError('proof routing registered rename differs')
        tips[origin] = sha; chain.append((sha, row, old_roots, new_roots)); view.records.append((sha, row))
    from proof_consolidation import validate_bindings
    bound = [(sha, row) for sha, row in view.records if row['binding']['kind'] == 'consolidation']
    projections = dict(zip((sha for sha, _ in bound),
                           validate_bindings(root, (row for _, row in bound), catalog, prospective)))
    catalog = source_layout.project_catalog(root, catalog)
    view.maintenance_records = [item for result in projections.values()
                                for item in result.get('maintenance', {}).get('records', [])]
    all_current = set()
    live_bytes = 0
    for origin, chain in chains.items():
        last_sha, last, _, live_roots = chain[-1]
        package = last['after']; directory = package['directory']
        expected, _ = _package(root, package, last['external_after'], chain[0][1]['before']['directory'])
        projection = projections.get(last_sha)
        if not prospective and projection is None:
            active = catalog.get('packages', {}).get(directory)
            if active != package or catalog.get('external_tests', {}).get(directory) != last['external_after']:
                raise ValueError('proof routing final catalog owner mismatch')
        if directory in catalog.get('retired', {}):
            raise ValueError('active proof routing cannot substitute for retired archive')
        physical_expected = projection['inventory'] if projection else expected
        live_roots = projection['roots'] if projection else live_roots
        maintenance = projection.get('maintenance') if projection else None
        if maintenance is not None:
            # The maintenance boundary already authenticated the entire live union.
            paths = set(maintenance['inventory'])
            current = maintenance['bytes']
            live_bytes += maintenance['live_bytes']
            if paths & all_current or live_bytes > MAX_LIVE_BYTES:
                raise ValueError('overlapping/bounded active proof owners')
        else:
            paths = set()
            current = {}
            for base in live_roots:
                logical = source_layout.inventory(root, base)
                for p in logical:
                    if p in paths or p in all_current:
                        raise ValueError('overlapping active proof owners')
                    data = source_layout.read(root, p)
                    paths.add(p); live_bytes += len(data)
                    if len(paths) > MAX_FILES or live_bytes > MAX_LIVE_BYTES:
                        raise ValueError('active proof live material bound exceeded')
                    current[p] = data
            if paths != set(physical_expected) or any(digest(data) != physical_expected[p] for p, data in current.items()):
                raise ValueError('active proof current inventory/content drift')
        all_current.update(paths)
        # Foreign-origin imports remain authenticated physical material, never
        # members of the pinned primary's historical inverse bijection.
        current = {p: current[p] for p in expected}
        manifest = tomllib.loads(current[directory + '/Cargo.toml'].decode())['package']
        if manifest.get('name') != package['name'] or manifest.get('version') != package['version']:
            raise ValueError('active proof manifest identity drift')
        latest = {p: p for p in current}
        for _, row, old_roots, new_roots in reversed(chain):
            previous, prior_latest = {}, {}
            for file in row['files']:
                a, b = file['before'], file['after']
                previous[a] = _inverse(current[b], file)
                prior_latest[a] = latest[b]
            for old_root in set(old_roots) - set(new_roots):
                if safe(root, old_root).exists():
                    raise ValueError('old active proof root has returned')
            for file in row['files']:
                if file['before'] != file['after'] and file['before'] not in expected and safe(root, file['before']).exists():
                    raise ValueError('old active proof file has returned')
            current, latest = previous, prior_latest
        baseline_roots = chain[0][2]
        if set(current) & set(view.bytes) or any(r in view.roots for r in baseline_roots):
            raise ValueError('overlapping historical proof owners')
        view.roots.update(baseline_roots); view.bytes.update(current); view.forward.update(latest)
        view.backward.update({v: k for k, v in latest.items()})
        view.packages[chain[0][1]['before']['directory']] = package
    return view


def authorize(root, previous, rename_item):
    view = load(root, previous, prospective=True)
    wanted = {k: v for k, v in rename_item.items() if k != 'directory'}
    for sha, row in view.records:
        if sha in previous.get('proof_routing_receipts', []):
            continue
        if (row['binding'] == {'kind': 'rename', 'record': wanted}
                and previous.get('packages', {}).get(rename_item['directory']) == row['before']
                and previous.get('external_tests', {}).get(rename_item['directory']) == row['external_before']):
            return True
    raise ValueError('pinned source rename lacks exact authenticated historical-view live-routing adapter')



def authorize_consolidation(root, previous, record):
    """Only a freshly pinned exact primary projection may authorize this plan."""
    if not isinstance(record, dict):
        raise ValueError('missing pinned consolidation record')
    view = load(root, previous, prospective=True)
    primary = record['primary_directory']
    for sha, row in view.records:
        if sha in previous.get('proof_routing_receipts', []):
            continue
        if (row['binding'] == {'kind': 'consolidation', 'record': record}
                and previous.get('packages', {}).get(primary) == row['before']
                and previous.get('external_tests', {}).get(primary) == row['external_before']):
            return True
    raise ValueError('pinned source consolidation lacks exact authenticated origin projection')


def validate_refresh(root, previous, current):
    # No newly pinned receipt may quietly authorize a different prior state.
    view = load(root, current)
    import proof_maintenance
    proof_maintenance.validate_refresh(root, previous, current, view.maintenance_records)
    registered = previous.get('proof_routing_receipts', [])
    if list(RECEIPTS[:len(registered)]) != registered:
        raise ValueError('immutable proof routing history drift')
    fresh = [(sha, row) for sha, row in view.records if sha not in registered]
    seen = set()
    for _, row in fresh:
        origin = row['before']['origin_directory']
        if origin in seen:
            raise ValueError('one reviewed proof routing step per owner per refresh')
        seen.add(origin)
        d = row['before']['directory']
        if previous.get('packages', {}).get(d) != row['before'] or previous.get('external_tests', {}).get(d) != row['external_before']:
            raise ValueError('proof routing previous catalog differs')
    return view
