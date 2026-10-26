"""Authenticated, identity-preserving foundation root layout.

A layout is a live-filesystem projection, not an archive or compatibility tree.
Old proof readers consume present bytes after reversing layout path edits.
Evolved manifests retain their current non-path contents, not a frozen snapshot.
History objects and source/origin labels are never translated.
"""
from contextlib import contextmanager
from contextvars import ContextVar
from copy import deepcopy
import base64
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import tomllib

KEY = 'foundation_layout_receipt'
PINNED_RECEIPTS = ('5f02def35b57751ea9e54a2f1f22fdfebdbd6dda1ed910de7f8fa805f55e281d',)  # Append the reviewed content-addressed installation receipt.
MAX_COMPRESSED = 8 * 1024 * 1024
MAX_EXPANDED = 64 * 1024 * 1024
MAX_FILE_BYTES = 32 * 1024 * 1024
MAX_FILES = 32768
MAX_PACKAGES = 512
HEX = re.compile(r'[0-9a-f]{64}')
GROUP = re.compile(r'[a-z][a-z0-9]*(?:-[a-z0-9]+)*')
HISTORY_FIELDS = (
    'renames', 'rename_records_sha256', 'source_moves',
    'consolidations', 'consolidation_records_sha256',
    'source_trims', 'source_trim_records_sha256',
    'retired', 'retired_records_sha256', 'superseded', 'superseded_records_sha256',
    'origin_records_sha256', 'upstream_identities', 'proof_routing_receipts',
    'initial_test_relocation_sha256', 'post_consolidation_benchmark_relocation',
)
_FIELDS = {'schema', 'reason', 'categories', 'predecessor_catalog_sha256',
           'history_hashes', 'frozen_evidence', 'packages', 'stationary_owners', 'file_edits'}
_PACKAGE_FIELDS = {'directory', 'destination_directory', 'package', 'merged_packages',
                   'files_sha256', 'external_tests', 'destination_external_tests'}
_EDIT_FIELDS = {'after_path', 'before_sha256', 'after_sha256', 'before_size', 'after_size', 'replacements'}
_ACTIVE = ContextVar('authenticated_foundation_layout', default=None)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False).encode()


def _stamp(path):
    if not path.is_file():
        return None
    value = path.stat()
    return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)


def safe(root, relative):
    if (not isinstance(relative, str) or not relative or '\\' in relative
            or any(p in ('', '.', '..') or ':' in p for p in relative.split('/'))):
        raise ValueError('unsafe layout path')
    path = root
    for part in relative.split('/'):
        path /= part
        if path.is_symlink():
            raise ValueError('layout symlink')
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError('layout path escapes repository')
    return path


def route(path, moves):
    for old, new in moves.items():
        if path == old or path.startswith(old + '/'):
            return new + path[len(old):]
    return path


def _target(base, relative):
    value = os.path.normpath(str(PurePosixPath(base) / relative))
    if value.startswith(('../', '/')) or value == '..':
        raise ValueError('layout path target escapes repository')
    return value



def reference_kind(path, data, offset):
    """Recognize maintained compiler/build inputs; unknown string edits fail."""
    quote = data.rfind(b'"', 0, offset + 1)
    if quote < 0:
        raise ValueError('layout path is not a quoted maintained reference')
    context = data[max(0, quote - 300):quote + 1]
    end = data.find(b'"', quote + 1)
    literal = data[quote + 1:end] if end >= 0 else b''
    qualified = rb'(?:core::)?'
    if re.search(rb'include(?:_str|_bytes)?!\s*\(\s*' + qualified + rb'concat!\s*\(\s*' + qualified + rb'env!\s*\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*"$', context):
        return 'manifest-relative-include'
    if re.search(rb'include(?:_str|_bytes)?!\s*\(\s*(?:' + qualified + rb'concat!\s*\(\s*)?"$', context):
        return 'source-relative-include'
    if re.search(rb'#\s*\[\s*path\s*=\s*"$', context):
        return 'source-module-path'
    if re.search(rb'#\s*include\s*"$', context):
        return 'c-include'
    exceptions = {
        'src/foundation/ring/build.rs': {b'../../../test/foundation/ring/crypto/constant_time_test.c'},
        'src/foundation/rust-syntax/benches/rust.rs': {b'../../../test/foundation/rust-syntax/tests/rust/compiler', b'../../../test/foundation/rust-syntax/tests/rust/library'},
        'src/foundation/rust-syntax/benches/file.rs': {b'../../../test/foundation/rust-syntax/tests/rust/library/core/src/str/mod.rs'},
        'src/foundation/axum-macros/src/lib.rs': {b'../../../test/foundation/axum-macros/tests/{directory}/fail/*.rs', b'../../../test/foundation/axum-macros/tests/{directory}/pass/*.rs'},
        'src/foundation/zerocopy/src/macros.rs': {b'../../../../test/foundation/zerocopy/testdata/include_value/'},
        'src/foundation/http-routing/src/lib.rs': {b'src/foundation/http-routing'},
        'test/tools/web-build/unit/main_tests.rs': {b'src/foundation/reactive-state/src/lib.rs'},
        'test/tools/contract-check/gitignore.rs': {b'src/foundation/cc/src/target/' + name for name in (b'cached.bin', b'apple.rs.bak', b'apple.rs', b'generated.rs', b'llvm.rs', b'parser.rs')},
    }
    if literal in exceptions.get(path, set()):
        return 'reviewed-build-runtime-or-documentation-input'
    raise ValueError('unclassified layout reference requires review: ' + path + ': ' + literal.decode('utf-8', 'replace'))

def rewrite(path, data, moves):
    """The only permitted source edit language: deterministic path rebasing."""
    destination = route(path, moves)
    if path.rsplit('/', 1)[-1] == 'Cargo.toml':
        before = data.decode('utf-8')
        old_base, new_base = str(PurePosixPath(path).parent), str(PurePosixPath(destination).parent)
        def replace(match):
            prefix, value, suffix = match.groups()
            target = route(_target(old_base, value), moves)
            return prefix + os.path.relpath(target, new_base) + suffix
        after = re.sub(r'((?<![A-Za-z_-])(?:path|readme|license-file)\s*=\s*")([^"\n]+)(")', replace, before)
        if path == 'Cargo.toml':
            after = re.sub(r'"(src/foundation/[^"\n]+)"', lambda m: '"' + route(m[1], moves) + '"', after)
        def semantic(value, base):
            if isinstance(value, dict):
                return {k: (route(_target(base, v), moves) if k in ('path', 'readme', 'license-file') and isinstance(v, str)
                            else semantic(v, base)) for k, v in value.items()}
            if isinstance(value, list):
                return [route(v, moves) if isinstance(v, str) and v.startswith('src/foundation/') else semantic(v, base) for v in value]
            return value
        if semantic(tomllib.loads(before), old_base) != semantic(tomllib.loads(after), new_base):
            raise ValueError('layout changed Cargo semantics')
        return after.encode('utf-8')
    if PurePosixPath(path).suffix not in {'.rs', '.c', '.h'}:
        return data
    after = data
    if destination != path:
        source_base = str(PurePosixPath(path).parent)
        package_base = next(old for old in moves if path.startswith(old + '/'))
        def external_path(match):
            lead, anchor = match.groups()
            # Macro concatenations are manifest-relative. Ordinary include/#[path]
            # strings are source-relative, so package-internal docs stay internal.
            context = data[max(0, match.start() - 160):match.start()]
            manifest_relative = bool(re.search(rb'(?:core::)?concat!\s*\(\s*(?:core::)?env!\s*\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*"$', context))
            if lead.startswith(b'/') and not manifest_relative:
                raise ValueError('ambiguous manifest-relative layout path')
            base = package_base if manifest_relative else source_base
            # The only current inline-module path attributes that climb past the
            # physical file directory live in Rustls's client/server modules.
            if path == 'src/foundation/rustls/src/lib.rs' and lead.count(b'../') == len(PurePosixPath(source_base).parts) + 1:
                base += '/inline-module'
            relative = lead.lstrip(b'/').decode() + anchor.decode()
            target = route(_target(base, relative), moves)
            rebased = os.path.relpath(target, route(base, moves)).encode()
            result = (b'/' if lead.startswith(b'/') else b'') + rebased + b'/'
            if result != match[0]:
                reference_kind(path, data, match.start())
            return result
        after = re.sub(rb'(/?(?:\.\./)+)(test|docs|compliance)/', external_path, after)
    if b'src/foundation/' in after:
        for old, new in moves.items():
            original = after
            def replace_root(match):
                reference_kind(path, original, match.start())
                return new.encode()
            after = re.sub(rb'(?<![A-Za-z0-9_-])' + re.escape(old.encode()) + rb'(?=/|["\s])', replace_root, after)
    # One maintained test statement exceeds rustfmt's width only after rebasing.
    # This exact whitespace adjustment is part of its reviewed byte receipt;
    # no other statements or whitespace are normalized.
    if path == 'test/tools/web-build/unit/main_tests.rs':
        statement = b'    let dependency = fixture.0.join("src/foundation/web-runtime/reactive-state/src/lib.rs");\n'
        wrapped = b'    let dependency = fixture\n        .0\n        .join("src/foundation/web-runtime/reactive-state/src/lib.rs");\n'
        if after.count(statement) > 1:
            raise ValueError('ambiguous maintained fixture layout formatting')
        after = after.replace(statement, wrapped)
    return after


def _inventory(value):
    if not isinstance(value, dict) or not value or len(value) > MAX_FILES:
        raise ValueError('invalid layout inventory')
    for path, sha in value.items():
        # Validate relative syntax without touching the filesystem.
        if (not isinstance(path, str) or not path or '\\' in path
                or any(p in ('', '.', '..') or ':' in p for p in path.split('/'))
                or not isinstance(sha, str) or not HEX.fullmatch(sha)):
            raise ValueError('invalid layout inventory path/hash')


def _external_identity(owner):
    if owner is None:
        return None
    value = deepcopy(owner)
    value.pop('bytes', None)
    for row in value.get('files', []):
        row.pop('sha256', None)
    return value


def _stable(package):
    return {k: v for k, v in package.items() if k not in {'directory', 'files_sha256', 'changes', 'merged_into'}}


def _edit_shape(root, path, row, moves):
    safe(root, path)
    if not isinstance(row, dict) or set(row) != _EDIT_FIELDS:
        raise ValueError('invalid layout path-edit fields')
    if row['after_path'] != route(path, moves):
        raise ValueError('layout edited path route mismatch')
    safe(root, row['after_path'])
    for key in ('before_sha256', 'after_sha256'):
        if not isinstance(row[key], str) or not HEX.fullmatch(row[key]):
            raise ValueError('invalid layout edit hash')
    if row['before_sha256'] == row['after_sha256']:
        raise ValueError('layout edit does not change bytes')
    if any(type(row[k]) is not int or not 0 < row[k] <= MAX_FILE_BYTES for k in ('before_size', 'after_size')):
        raise ValueError('layout edit byte bound exceeded')
    edits = row['replacements']
    if not isinstance(edits, list) or not 1 <= len(edits) <= 4096:
        raise ValueError('invalid bounded layout replacements')
    end = 0; delta = 0
    for edit in edits:
        if not isinstance(edit, dict) or set(edit) != {'start', 'end', 'before', 'after'}:
            raise ValueError('invalid layout replacement fields')
        a, b = edit['start'], edit['end']
        if type(a) is not int or type(b) is not int or not end <= a <= b <= row['before_size']:
            raise ValueError('overlapping/invalid layout replacement offsets')
        old, new = (base64.b64decode(edit[k], validate=True) for k in ('before', 'after'))
        if len(old) != b - a or old == new or len(old) + len(new) > 65536:
            raise ValueError('invalid layout replacement bytes')
        delta += len(new) - len(old); end = b
    if row['before_size'] + delta != row['after_size']:
        raise ValueError('layout replacement byte count mismatch')


def inverse(path, data, row, moves):
    if len(data) != row['after_size'] or digest(data) != row['after_sha256']:
        raise ValueError('layout live edited content drift: ' + path)
    offsets = []; delta = 0
    for edit in row['replacements']:
        old, new = (base64.b64decode(edit[k], validate=True) for k in ('before', 'after'))
        start = edit['start'] + delta
        offsets.append((start, old, new)); delta += len(new) - len(old)
    previous = data
    for start, old, new in reversed(offsets):
        if previous[start:start + len(new)] != new:
            raise ValueError('layout inverse context mismatch')
        previous = previous[:start] + old + previous[start + len(new):]
    if len(previous) != row['before_size'] or digest(previous) != row['before_sha256']:
        raise ValueError('layout inverse before hash mismatch')
    if rewrite(path, previous, moves) != data:
        raise ValueError('layout edits are not deterministic path-only changes')
    return previous


class Layout:
    def __init__(self, root, receipt, receipt_id, catalog=None, pending_upgrade=None, prospective_maintenance=False):
        self.legacy_maintenance = None
        self.pending_upgrade = pending_upgrade
        self.appended_origins = {}
        self.upgraded_owners = {}
        self.imported_owners = {}
        self.root = Path(root).resolve()
        self.receipt = receipt
        self.receipt_id = receipt_id
        self.moves = {}
        self.reverse = {}
        self.edits = {}
        self.rows = {}
        self.descendant_files = {}
        self.catalog_digest = digest(canonical(catalog)) if catalog is not None else None
        catalog_file = safe(self.root, 'compliance/rust-sources.json')
        self.disk_catalog_stamp = _stamp(catalog_file)
        self.disk_catalog_sha256 = digest(catalog_file.read_bytes()) if catalog_file.is_file() else None
        if self.disk_catalog_stamp != _stamp(catalog_file):
            raise ValueError('layout catalog changed while authenticating')
        self._validate_shape()
        import source_layout_corrections
        self.corrections = source_layout_corrections.load(self.root, self, catalog) if catalog is not None else None
        if catalog is not None:
            self.check_current_identity(catalog)
            import proof_legacy_maintenance
            self.legacy_maintenance = proof_legacy_maintenance.load(self, catalog, prospective_maintenance)

    def check_disk_catalog(self, force=False):
        path = safe(self.root, 'compliance/rust-sources.json')
        stamp = _stamp(path)
        if force or stamp != self.disk_catalog_stamp:
            actual = digest(path.read_bytes()) if path.is_file() else None
            if actual != self.disk_catalog_sha256 or stamp != _stamp(path):
                raise ValueError('layout operation catalog changed during authenticated context')
            self.disk_catalog_stamp = stamp

    def _validate_shape(self):
        row = self.receipt
        expected_fields = _FIELDS | ({'installation_files'} if isinstance(row, dict) and row.get('schema') == 2 else set())
        if (not isinstance(row, dict) or set(row) != expected_fields or type(row['schema']) is not int
                or row['schema'] not in (1, 2) or not isinstance(row['reason'], str) or not row['reason'].strip()
                or not isinstance(row['categories'], dict) or not row['categories']
                or any(not GROUP.fullmatch(g) or not isinstance(d, str) or not d.strip() for g, d in row['categories'].items())
                or not isinstance(row['packages'], list) or not 1 <= len(row['packages']) <= MAX_PACKAGES
                or not isinstance(row['file_edits'], dict) or len(row['file_edits']) > MAX_FILES
                or not isinstance(row['stationary_owners'], dict)
                or not HEX.fullmatch(row['predecessor_catalog_sha256'])):
            raise ValueError('invalid layout receipt schema')
        if row['schema'] == 2:
            initial = row['installation_files']
            if (not isinstance(initial, dict) or set(initial) != {'Cargo.lock'}
                    or not isinstance(initial['Cargo.lock'], str) or not HEX.fullmatch(initial['Cargo.lock'])):
                raise ValueError('invalid layout installation-only file binding')
        if not isinstance(row['history_hashes'], dict) or set(row['history_hashes']) != set(HISTORY_FIELDS):
            raise ValueError('incomplete layout history bindings')
        if not isinstance(row['frozen_evidence'], dict):
            raise ValueError('invalid layout frozen evidence')
        for path, expected in row['frozen_evidence'].items():
            if not isinstance(expected, str) or not HEX.fullmatch(expected):
                raise ValueError('invalid frozen evidence hash')
            target = safe(self.root, path)
            if not target.is_file() or digest(target.read_bytes()) != expected:
                import source_layout_upgrades
                if path != source_layout_upgrades.ORIGINS:
                    raise ValueError('immutable pre-layout evidence drift: ' + path)
                source_layout_upgrades.frozen_origins(self, expected)
        groups = set()
        for item in row['packages']:
            if not isinstance(item, dict) or set(item) != _PACKAGE_FIELDS:
                raise ValueError('invalid layout package fields')
            old, new = item['directory'], item['destination_directory']
            safe(self.root, old); safe(self.root, new)
            old_parts, new_parts = old.split('/'), new.split('/')
            if (len(old_parts) != 3 or old_parts[:2] != ['src', 'foundation']
                    or len(new_parts) != 4 or new_parts[:2] != ['src', 'foundation']
                    or new_parts[2] not in row['categories'] or new_parts[3] != old_parts[2]
                    or old in self.moves or new in self.reverse):
                raise ValueError('invalid/duplicate layout source/destination')
            before = item['package']
            if (not isinstance(before, dict) or before.get('directory') != old
                    or not all(before.get(k) for k in ('name', 'version', 'origin_directory', 'upstream'))
                    or not isinstance(item['merged_packages'], list)):
                raise ValueError('incomplete layout previous package identity')
            _inventory(before['files_sha256']); _inventory(item['files_sha256'])
            if set(before['files_sha256']) != set(item['files_sha256']):
                raise ValueError('layout file bijection changed relative inventory')
            origins = {before['origin_directory']}
            for merged in item['merged_packages']:
                if (not isinstance(merged, dict) or merged.get('directory') != old
                        or merged.get('origin_directory') in origins
                        or merged.get('files_sha256') != before['files_sha256']):
                    raise ValueError('layout shared owner identity/inventory mismatch')
                origins.add(merged['origin_directory'])
            self.moves[old] = new; self.reverse[new] = old; self.rows[old] = item; groups.add(new_parts[2])
        if groups != set(row['categories']):
            raise ValueError('empty or missing layout category')
        for path, edit in row['file_edits'].items():
            _edit_shape(self.root, path, edit, self.moves)
            self.edits[path] = edit
        for old, item in self.rows.items():
            for relative, before_sha in item['package']['files_sha256'].items():
                after_sha = item['files_sha256'][relative]; edit = self.edits.get(old + '/' + relative)
                if before_sha != after_sha:
                    if edit is None or edit['before_sha256'] != before_sha or edit['after_sha256'] != after_sha:
                        raise ValueError('layout changed file lacks exact path-edit receipt')
                elif edit is not None:
                    raise ValueError('unexpected layout source path edit')
            self._validate_external(item)
        for directory, row in self.receipt['stationary_owners'].items():
            safe(self.root, directory)
            if (directory in self.moves or not isinstance(row, dict)
                    or set(row) != {'package', 'merged_packages', 'files_sha256'}
                    or not isinstance(row['merged_packages'], list)):
                raise ValueError('invalid stationary layout owner')
            owners = ([row['package']] if row['package'] is not None else []) + row['merged_packages']
            if not owners or any(p.get('directory') != directory for p in owners):
                raise ValueError('stationary layout identity mismatch')
            _inventory(row['files_sha256'])
            for owner in owners:
                _inventory(owner['files_sha256'])
                if set(owner['files_sha256']) != set(row['files_sha256']):
                    raise ValueError('stationary layout lost files')
                for path, sha in owner['files_sha256'].items():
                    new_sha = row['files_sha256'][path]; edit = self.edits.get(directory + '/' + path)
                    if sha != new_sha and (edit is None or edit['before_sha256'] != sha or edit['after_sha256'] != new_sha):
                        raise ValueError('stationary source lacks exact path edit')

    def _validate_external(self, item):
        before, after = item['external_tests'], item['destination_external_tests']
        if before is None:
            if after is not None:
                raise ValueError('layout cannot introduce external ownership')
            return
        if (not isinstance(before, dict) or before.get('directory') != item['directory']
                or not isinstance(after, dict) or before.get('origin_directory') != item['package']['origin_directory']):
            raise ValueError('invalid layout external owner identity')
        expected = deepcopy(before); expected['directory'] = item['destination_directory']
        delta = 0
        for entry in expected['files']:
            edit = self.edits.get(entry['path'])
            if edit is not None:
                if edit['before_sha256'] != entry['sha256'] or edit['after_path'] != entry['path']:
                    raise ValueError('layout external edit attribution mismatch')
                entry['sha256'] = edit['after_sha256']; delta += edit['after_size'] - edit['before_size']
        expected['bytes'] += delta
        if expected != after:
            raise ValueError('layout rewrote external source identity or inventory')

    def current_path(self, path):
        safe(self.root, str(path))
        return route(str(path), self.moves)

    def historical_path(self, path):
        safe(self.root, str(path))
        return route(str(path), self.reverse)

    def read(self, path):
        path = str(path)
        for old, item in self.rows.items():
            if (path.startswith(old + '/') and path[len(old) + 1:] not in item['package']['files_sha256']
                    and path not in self.descendant_files
                    and self.moves[old] not in self.upgraded_owners):
                raise ValueError('unmapped logical path in relocated owner: ' + path)
        target = safe(self.root, self.current_path(path))
        if not target.is_file() or target.stat().st_size > MAX_FILE_BYTES:
            raise ValueError('missing or oversized live layout file: ' + path)
        return self.project_bytes(path, target.read_bytes())

    def project_bytes(self, path, data):
        """Apply the original exact layout inverse to supplied physical bytes."""
        if self.legacy_maintenance is not None:
            data = self.legacy_maintenance.read(path, data)
        if self.corrections is not None:
            data = self.corrections.read(path, data)
        if path in self.descendant_files and digest(data) != self.descendant_files[path]:
            raise ValueError('post-layout moved source content differs from authenticated current inventory: ' + path)
        for directory, package in self.upgraded_owners.items():
            prefix = self.reverse[directory] + '/'
            if path.startswith(prefix) and package['files_sha256'].get(path[len(prefix):]) != digest(data):
                raise ValueError('upgraded logical source differs from authenticated current inventory: ' + path)
        if path in self.edits and not any(path.startswith(self.reverse[d] + '/') for d in self.upgraded_owners):
            edit = self.edits[path]
            if PurePosixPath(path).name == 'Cargo.toml' and digest(data) != edit['after_sha256']:
                # Ordinary manifest maintenance changes current bytes, not the
                # immutable installation. Expose those edits to old proof readers.
                data = rewrite(self.current_path(path), data, self.reverse)
            else:
                data = inverse(path, data, edit, self.moves)
        return data

    def inventory(self, directory):
        directory = str(directory); target = safe(self.root, self.current_path(directory))
        if not target.is_dir():
            raise ValueError('missing live layout root: ' + directory)
        result = {}
        for base, children, names in os.walk(target, followlinks=False):
            for name in children + names:
                file = Path(base) / name
                relative = file.relative_to(self.root).as_posix(); safe(self.root, relative)
                if file.is_file():
                    old = self.historical_path(relative)
                    result[old] = digest(self.read(old))
                    if len(result) > MAX_FILES:
                        raise ValueError('layout inventory exceeds bound')
                elif not file.is_dir():
                    raise ValueError('layout special file')
        return result

    def source_prefixes(self, directory, origin):
        old = self.reverse.get(directory)
        if old is None:
            return ()
        row = self.rows[old]
        if row['package']['origin_directory'] != origin:
            return ()
        return (old + '/',)

    def _project_hash(self, path, sha):
        if self.legacy_maintenance is not None:
            sha = self.legacy_maintenance.project_hash(path, sha)
        if self.corrections is not None:
            sha = self.corrections.project_hash(path, sha)
        edit = self.edits.get(path)
        if edit is not None and PurePosixPath(path).name == 'Cargo.toml' and sha != edit['after_sha256']:
            target = safe(self.root, self.current_path(path))
            if not target.is_file() or target.stat().st_size > MAX_FILE_BYTES:
                raise ValueError('missing or oversized live layout manifest: ' + path)
            # Never replace a stale catalog hash with the latest filesystem hash.
            if digest(target.read_bytes()) == sha:
                return digest(self.read(path))
        return edit['before_sha256'] if edit is not None and sha == edit['after_sha256'] else sha

    def _project_package(self, package):
        value = deepcopy(package)
        directory = self.historical_path(value['directory']); value['directory'] = directory
        if isinstance(value.get('merged_into'), str):
            value['merged_into'] = route(value['merged_into'], self.reverse)
        if 'files_sha256' in value:
            value['files_sha256'] = {p: self._project_hash(directory + '/' + p, sha) for p, sha in value['files_sha256'].items()}
        return value

    def project_catalog(self, catalog):
        """Project CURRENT owners only; historical records/source labels stay exact."""
        result = deepcopy(catalog)
        result['packages'] = {self.historical_path(k): self._project_package(v) for k, v in catalog.get('packages', {}).items()}
        if len(result['packages']) != len(catalog.get('packages', {})):
            raise ValueError('layout projected package key collision')
        result['merges'] = [self._project_package(v) for v in catalog.get('merges', [])]
        external = {}
        for key, owner in catalog.get('external_tests', {}).items():
            old = self.historical_path(key); value = deepcopy(owner); value['directory'] = old
            delta = 0
            for row in value['files']:
                if self.corrections is not None:
                    delta += self.corrections.delta(row['path'], row['sha256'])
                    row['sha256'] = self.corrections.project_hash(row['path'], row['sha256'])
                edit = self.edits.get(row['path'])
                if edit is not None and row['sha256'] == edit['after_sha256']:
                    row['sha256'] = edit['before_sha256']; delta += edit['after_size'] - edit['before_size']
            value['bytes'] -= delta
            if old in external:
                raise ValueError('layout projected external owner collision')
            external[old] = value
        result['external_tests'] = external
        return result

    def check_current_identity(self, catalog):
        if self.legacy_maintenance is not None:
            self.legacy_maintenance.validate_catalog(catalog)
        if catalog.get(KEY) != self.receipt_id:
            raise ValueError('missing or changed layout catalog binding')
        if self.corrections is not None:
            self.corrections.validate_catalog(catalog)
        import source_layout_upgrades
        import source_imports
        self.imported_owners, imported_origins = source_imports.current(self, catalog)
        additions, upgraded, used = source_layout_upgrades.chains(self, catalog, imported_origins)
        self.upgraded_owners = upgraded
        appended_moves = self._check_appended_moves(catalog)
        appended_trims = self._check_imported_trims(catalog)
        for field, expected in self.receipt['history_hashes'].items():
            value = source_layout_upgrades.history_value(self, field, catalog.get(field), additions, used)
            if field == 'source_moves' and isinstance(value, dict):
                value = {key: row for key, row in value.items() if key not in appended_moves}
            if field in ('source_trims', 'source_trim_records_sha256') and isinstance(value, dict):
                value = {key: row for key, row in value.items() if key not in appended_trims}
            if field in ('retired', 'retired_records_sha256') and isinstance(value, dict):
                value = {key: row for key, row in value.items() if key not in self.reverse}
            if digest(canonical(value)) != expected:
                raise ValueError('pre-layout immutable history changed: ' + field)
        current = catalog.get('packages', {})
        old_origins = set()
        retired_roots = set()
        for old, row in self.rows.items():
            new = row['destination_directory']; before = row['package']; package = current.get(new)
            if new not in current:
                self._check_retired_owner(catalog, row)
                retired_roots.add(new)
                continue
            if (old in current or not isinstance(package, dict) or package.get('directory') != new
                    or (new not in upgraded and _stable(before) != _stable(package))):
                raise ValueError('layout current package identity drift: ' + old)
            if safe(self.root, old).exists():
                raise ValueError('old relocated source root has returned: ' + old)
            old_origins.add(before['origin_directory'])
            merged = [m for m in catalog.get('merges', []) if m['directory'] == new]
            if (len(merged) != len(row['merged_packages'])
                    or [_stable(m) for m in merged] != [_stable(m) for m in row['merged_packages']]
                    or any(m.get('merged_into') != route(b.get('merged_into', ''), self.moves)
                           or m.get('files_sha256') != package.get('files_sha256') for m, b in zip(merged, row['merged_packages']))):
                raise ValueError('layout current shared origin coverage drift')
            if new in upgraded:
                continue
            before_owner = row['destination_external_tests']
            current_owner = catalog.get('external_tests', {}).get(new)
            if before_owner is None:
                if current_owner is not None:
                    raise ValueError('layout introduced an unattributed external owner')
            else:
                from collections import Counter
                import external_tests
                def labels(entries):
                    return Counter(tuple((key, canonical(value)) for key, value in sorted(entry.items()) if key != 'sha256')
                                   for entry in entries)
                if (not isinstance(current_owner, dict)
                        or {k: v for k, v in before_owner.items() if k not in ('files', 'bytes', 'file_count')}
                        != {k: v for k, v in current_owner.items() if k not in ('files', 'bytes', 'file_count')}
                        or not isinstance(current_owner.get('files'), list)
                        or type(current_owner.get('file_count')) is not int
                        or current_owner['file_count'] != len(current_owner['files'])):
                    raise ValueError('layout current external historical attribution drift')
                historical, present = labels(before_owner['files']), labels(current_owner['files'])
                # Preserve every installation label, including any local rows
                # already in that snapshot. Only later local work is additive;
                # it never becomes part of the immutable relocation receipt.
                local = labels(entry for entry in current_owner['files']
                               if entry.get('source_kind') == 'project-authored'
                               and external_tests._owns_source(current_owner, entry, before_owner['origin_directory'], (), {}))
                if historical - present or (present - historical) - local:
                    raise ValueError('layout current external historical attribution drift')
        for directory, row in self.receipt['stationary_owners'].items():
            before = row['package']; package = current.get(directory)
            if (before is None) != (package is None) or (before is not None and _stable(before) != _stable(package)):
                raise ValueError('layout stationary package identity drift')
            merged = [m for m in catalog.get('merges', []) if m.get('directory') == directory]
            if (len(merged) != len(row['merged_packages'])
                    or [_stable(m) for m in merged] != [_stable(m) for m in row['merged_packages']]
                    or [m.get('merged_into') for m in merged] != [m.get('merged_into') for m in row['merged_packages']]):
                raise ValueError('layout stationary shared origin identity drift')
            inventories = [p['files_sha256'] for p in ([package] if package is not None else []) + merged]
            if any(value != inventories[0] for value in inventories):
                raise ValueError('layout stationary shared inventory drift')
        if ((set(catalog.get('retired', {})) & set(self.reverse)) != retired_roots
                or (set(catalog.get('retired_records_sha256', {})) & set(self.reverse)) != retired_roots):
            raise ValueError('duplicate or unmatched post-layout retirement identity')
        present = {p for p in current if p.startswith('src/foundation/')}
        if present != (set(self.reverse) - retired_roots) | set(self.imported_owners):
            raise ValueError('layout current foundation package inventory drift')
        descendants = {}
        for key in appended_moves:
            record = catalog['source_moves'][key]
            directory = record['directory']
            if directory not in current:
                continue
            inventory = current[directory]['files_sha256']
            _inventory(inventory)
            for destination in record['file_moves'].values():
                if destination in inventory:
                    descendants[self.reverse[directory] + '/' + destination] = inventory[destination]
        self.descendant_files = descendants

    def require_standalone(self, catalog, old):
        row = self.rows[old]
        if self.corrections is not None and row['destination_directory'] in self.corrections.owners:
            raise ValueError('corrected external owner lifecycle requires an explicit descendant adapter')
        protected = {'ring', 'libc', 'memchr', 'mio', 'rustc-demangle', 'tokio', 'system-entropy', 'synchronization'}
        if (old.rsplit('/', 1)[-1] in protected or row['merged_packages']
                or any(p.get('directory') == row['destination_directory'] for p in catalog.get('merges', []))
                or any(item.get('destination_directory') == old for item in catalog.get('renames', {}).values())
                or any(item.get('destination_directory') == old for item in catalog.get('consolidations', {}).values())
                or any(item.get('directory') == old for item in catalog.get('source_trims', {}).values())):
            raise ValueError('post-layout shared or chained lifecycle requires an explicit descendant adapter')

    def require_unedited_moves(self, old, moves):
        if not isinstance(moves, dict) or any(not isinstance(path, str) for pair in moves.items() for path in pair):
            raise ValueError('invalid post-layout internal source-file move map')
        protected = {path for path in self.edits if path.startswith(old + '/')}
        for pair in moves.items():
            for relative in pair:
                path = old + '/' + relative
                if any(path == edited or path.startswith(edited + '/') or edited.startswith(path + '/')
                       for edited in protected):
                    raise ValueError('post-layout move touching a path-edited source requires an explicit descendant inverse adapter')

    def _check_appended_moves(self, catalog):
        import source_moves
        added = set()
        for key, record in catalog.get('source_moves', {}).items():
            directory = record.get('directory') if isinstance(record, dict) else None
            if directory not in self.reverse:
                continue
            old = self.reverse[directory]
            self.require_standalone(catalog, old)
            source_moves._record(self.root, record)
            self.require_unedited_moves(old, record['file_moves'])
            if (key != source_moves.receipt_id(record) or record['merged_packages']
                    or record['external_file_moves']
                    or _stable(record['package']) != _stable(self.rows[old]['package'])):
                raise ValueError('invalid post-layout internal move identity, scope or receipt')
            before_owner = self.rows[old]['destination_external_tests']
            for owner in (record['external_tests'], record['destination_external_tests']):
                if _external_identity(owner) != _external_identity(before_owner):
                    raise ValueError('post-layout internal move changed external historical attribution')
            added.add(key)
        return added

    def _check_imported_trims(self, catalog):
        import source_trimming
        added, seen = set(), set()
        for key, row in catalog.get('source_trims', {}).items():
            if not isinstance(row, dict) or row.get('directory') not in self.imported_owners:
                continue
            paths = source_trimming._validate_record(
                self.root, row, catalog['packages'], source_trimming.forbidden_owners(catalog),
                self.imported_owners)
            if (key != source_trimming.receipt_id(row)
                    or catalog.get('source_trim_records_sha256', {}).get(key) != key
                    or seen & paths
                    or any(path in catalog['packages'][directory]['files_sha256']
                           for directory, path in paths)):
                raise ValueError('invalid imported source trim receipt or absence')
            seen |= paths
            added.add(key)
        return added

    def require_supported_plan(self, previous, retirement=None, moves=None, upgrade=None, trim=None, **unsupported):
        if any(value is not None for value in unsupported.values()):
            raise ValueError('grouped foundation rename, consolidation, upgrade or trim requires an explicit descendant-chain adapter; ordinary refresh, standalone retirement and internal source-file moves are supported')
        if trim is not None:
            if (not isinstance(trim, dict) or not isinstance(trim.get('packages'), list)
                    or any(not isinstance(row, dict) or row.get('directory') not in self.imported_owners
                           for row in trim['packages'])):
                raise ValueError('post-layout trim requires an authenticated imported owner')
        for plan in (retirement, moves, upgrade):
            if plan is None:
                continue
            if not isinstance(plan, dict) or not isinstance(plan.get('packages'), list):
                raise ValueError('invalid grouped foundation lifecycle plan')
            for item in plan['packages']:
                directory = item.get('directory') if isinstance(item, dict) else None
                if directory not in self.reverse:
                    raise ValueError('post-layout lifecycle plan requires a declared grouped owner')
                self.require_standalone(previous, self.reverse[directory])
                if plan is moves:
                    if item.get('external_file_moves'):
                        raise ValueError('post-layout external file moves require a descendant-chain adapter')
                    self.require_unedited_moves(self.reverse[directory], item.get('file_moves'))

    def _check_retired_owner(self, catalog, row):
        old = row['directory']; new = row['destination_directory']
        package = catalog.get('retired', {}).get(new)
        before = row['package']
        self.require_standalone(catalog, old)
        if (not isinstance(package, dict) or package.get('directory') != new
                or _stable({k: v for k, v in package.items() if k != 'retirement'}) != _stable(before)
                or catalog.get('retired_records_sha256', {}).get(new) != digest(canonical(package))
                or new in catalog.get('external_tests', {}) or safe(self.root, new).exists()
                or safe(self.root, old).exists() or old in catalog.get('packages', {})):
            raise ValueError('invalid post-layout retired owner identity, receipt or absence')
        _inventory(package.get('files_sha256'))
        retirement = package.get('retirement', {})
        if (not isinstance(retirement, dict)
                or not {'reason', 'category', 'bytes', 'file_count', 'targets'} <= set(retirement)
                or not retirement.get('reason') or not retirement.get('category')
                or type(retirement.get('bytes')) is not int or retirement['bytes'] < 0
                or retirement.get('file_count') != len(package['files_sha256'])
                or not isinstance(retirement.get('targets'), list) or not retirement['targets']):
            raise ValueError('incomplete post-layout retirement receipt')
        before_owner = row['destination_external_tests']
        archived = retirement.get('external_tests')
        if before_owner is None:
            if archived is not None:
                raise ValueError('post-layout retirement introduced external attribution')
        else:
            def identity(owner):
                value = deepcopy(owner)
                value.pop('bytes', None)
                for item in value.get('files', []):
                    item.pop('sha256', None)
                return value
            if not isinstance(archived, dict) or identity(archived) != identity(before_owner):
                raise ValueError('post-layout retirement lost historical external attribution')
            if any(safe(self.root, root).exists() for root in archived['roots']):
                raise ValueError('post-layout retired external test root has returned')

    def verify_installation(self, previous, catalog, previous_raw_sha256):
        for path, expected in self.receipt.get('installation_files', {}).items():
            target = safe(self.root, path)
            if not target.is_file() or digest(target.read_bytes()) != expected:
                raise ValueError('layout installation changed root lockfile')
        if previous_raw_sha256 != self.receipt['predecessor_catalog_sha256']:
            raise ValueError('stale layout predecessor catalog')
        self.check_current_identity(catalog)
        for field, expected in self.receipt['history_hashes'].items():
            if digest(canonical(previous.get(field))) != expected:
                raise ValueError('layout predecessor history mismatch')
        if {p for p in previous['packages'] if p.startswith('src/foundation/')} != set(self.moves):
            raise ValueError('layout omitted a previous foundation package')
        for old, row in self.rows.items():
            if (previous['packages'].get(old) != row['package']
                    or [m for m in previous.get('merges', []) if m['directory'] == old] != row['merged_packages']
                    or previous.get('external_tests', {}).get(old) != row['external_tests']):
                raise ValueError('layout exact predecessor owner mismatch')
            new = row['destination_directory']
            if safe(self.root, old).exists() or not safe(self.root, new).is_dir():
                raise ValueError('layout requires absent old and present new root')
            actual = self.inventory(old)
            expected = {old + '/' + p: sha for p, sha in row['package']['files_sha256'].items()}
            if actual != expected:
                raise ValueError('layout live file bijection/before inventory mismatch')
            if catalog['packages'][new]['files_sha256'] != row['files_sha256']:
                raise ValueError('layout after package inventory mismatch')
            if catalog.get('external_tests', {}).get(new) != row['destination_external_tests']:
                raise ValueError('layout after external inventory mismatch')
        all_external = {}
        for owner in previous.get('external_tests', {}).values():
            for base in owner['roots']:
                actual = self.inventory(base)
                expected = {row['path']: row['sha256'] for row in owner['files'] if row['path'].startswith(base + '/')}
                if actual != expected:
                    raise ValueError('layout external file coverage drift')
            for row in owner['files']:
                if row['path'] in all_external:
                    raise ValueError('layout duplicate external file attribution')
                all_external[row['path']] = row['sha256']
        foundation_tests = safe(self.root, 'test/foundation')
        if foundation_tests.exists() and self.inventory('test/foundation') != {p: sha for p, sha in all_external.items() if p.startswith('test/foundation/')}:
            raise ValueError('layout unattributed external foundation file')
        for path in self.edits:
            # Installation still certifies exact path-only changes, including
            # manifests outside package inventories such as the root workspace.
            target = safe(self.root, self.current_path(path))
            if not target.is_file() or target.stat().st_size > MAX_FILE_BYTES:
                raise ValueError('missing or oversized live layout file: ' + path)
            inverse(path, target.read_bytes(), self.edits[path], self.moves)
        for directory, row in self.receipt['stationary_owners'].items():
            if (previous['packages'].get(directory) != row['package']
                    or [p for p in previous.get('merges', []) if p['directory'] == directory] != row['merged_packages']):
                raise ValueError('layout stationary predecessor mismatch')
            owners = ([row['package']] if row['package'] is not None else []) + row['merged_packages']
            if self.inventory(directory) != {directory + '/' + p: sha for p, sha in owners[0]['files_sha256'].items()}:
                raise ValueError('layout stationary source content mismatch')


def receipt_path(receipt_id):
    if not isinstance(receipt_id, str) or not HEX.fullmatch(receipt_id):
        raise ValueError('invalid layout receipt ID')
    return 'compliance/source-layout/' + receipt_id + '.json.gz'


def read_receipt(root, receipt_id):
    if receipt_id not in PINNED_RECEIPTS:
        raise ValueError('unregistered layout receipt')
    path = safe(Path(root), receipt_path(receipt_id))
    if not path.is_file() or path.stat().st_size > MAX_COMPRESSED:
        raise ValueError('missing or oversized layout receipt')
    content = path.read_bytes()
    if digest(content) != receipt_id:
        raise ValueError('immutable layout receipt drift')
    with gzip.GzipFile(fileobj=io.BytesIO(content)) as stream:
        data = stream.read(MAX_EXPANDED + 1)
        if len(data) > MAX_EXPANDED or stream.read(1):
            raise ValueError('layout receipt expansion bound exceeded')
    return json.loads(data)


def load(root, catalog=None, pending_upgrade=None, prospective_maintenance=False):
    root = Path(root).resolve()
    active = _ACTIVE.get()
    if active is not None and active.root == root and catalog is None:
        active.check_disk_catalog()
        return active
    if catalog is None:
        path = safe(root, 'compliance/rust-sources.json')
        catalog = json.loads(path.read_bytes()) if path.is_file() else {}
    marker = catalog.get(KEY)
    if marker is None:
        if any(safe(root, receipt_path(key)).exists() for key in PINNED_RECEIPTS):
            raise ValueError('layout receipt present but catalog binding removed')
        return None
    return Layout(root, read_receipt(root, marker), marker, catalog, pending_upgrade, prospective_maintenance)


@contextmanager
def activate(layout):
    if layout is None:
        yield
        return
    if not isinstance(layout, Layout) or layout.catalog_digest is None:
        raise ValueError('layout context requires authenticated current catalog')
    layout.check_disk_catalog(force=True)
    token = _ACTIVE.set(layout)
    try:
        yield
    finally:
        try:
            layout.check_disk_catalog(force=True)
        finally:
            _ACTIVE.reset(token)


def _scoped_layout(root):
    """Reuse only the current exact-root operation, authenticated again on exit."""
    active = _ACTIVE.get()
    if active is not None and active.root == Path(root).resolve():
        active.check_disk_catalog()
        return active
    return load(root)


def imported_directories(root):
    """Current imported owners have no pre-layout descendant files to invert."""
    layout = _scoped_layout(root)
    return set(layout.imported_owners) if layout else set()


def written_catalog(root, data):
    """Bind refresh's exact committed output before its scoped context returns."""
    layout = _ACTIVE.get()
    if layout is None:
        return
    if layout.root != Path(root).resolve() or safe(layout.root, 'compliance/rust-sources.json').read_bytes() != data:
        raise ValueError('layout committed catalog binding mismatch')
    current = json.loads(data)
    layout.check_current_identity(current)
    layout.catalog_digest = digest(canonical(current))
    layout.disk_catalog_sha256 = digest(data)
    layout.disk_catalog_stamp = _stamp(safe(layout.root, 'compliance/rust-sources.json'))


def current_path(root, path):
    layout = _scoped_layout(root)
    return layout.current_path(str(path)) if layout else str(path)


def read(root, path):
    layout = _scoped_layout(root)
    return layout.read(str(path)) if layout else safe(Path(root), str(path)).read_bytes()


def inventory(root, path):
    layout = _scoped_layout(root)
    if layout:
        return layout.inventory(str(path))
    import external_tests
    return external_tests.inventory(root, path)


def project_catalog(root, catalog):
    layout = _scoped_layout(root)
    return layout.project_catalog(catalog) if layout else catalog


def historical_catalog(function):
    """Scope one authenticated boundary around legacy current-owner checks."""
    from functools import wraps
    @wraps(function)
    def wrapped(root, catalog=None, *args, **kwargs):
        layout = load(root)
        with activate(layout):
            if layout is not None:
                if catalog is None:
                    catalog = json.loads(safe(Path(root), 'compliance/rust-sources.json').read_bytes())
                catalog = layout.project_catalog(catalog)
            return function(root, catalog, *args, **kwargs)
    return wrapped


def operation(function):
    """Authenticate once for an entry point that does not project its arguments."""
    from functools import wraps
    from inspect import signature
    parameters = signature(function)
    @wraps(function)
    def wrapped(*args, **kwargs):
        bound = parameters.bind(*args, **kwargs)
        bound.apply_defaults()
        root = bound.arguments['root']
        layout = load(root)
        with activate(layout):
            return function(*args, **kwargs)
    return wrapped


def physical(root, path):
    return safe(Path(root), current_path(root, path))


def logical_size(root, path):
    return len(read(root, path))


def current_grants(root, grants):
    layout = _scoped_layout(root)
    if layout is None:
        return grants
    result = {}
    for path, value in grants.items():
        key = layout.current_path(path)
        if key in result:
            raise ValueError('layout grant key collision')
        result[key] = deepcopy(value)
    return result


def historical_prefixes(root, directory, origin, renames):
    import source_naming
    layout = _scoped_layout(root)
    if layout is None:
        return source_naming.source_prefixes(renames, directory, origin)
    old = layout.historical_path(directory)
    return tuple(dict.fromkeys((directory + '/',) + source_naming.source_prefixes(renames, old, origin)))


def transition_catalog(previous, receipt, receipt_id):
    """Construct only the explicitly reviewed CURRENT owner transition."""
    result = deepcopy(previous)
    result[KEY] = receipt_id; result['schema'] = max(result.get('schema', 0), 9)
    moves = {row['directory']: row['destination_directory'] for row in receipt['packages']}
    rows = {row['directory']: row for row in receipt['packages']}
    active = {}
    for directory, package in result['packages'].items():
        new = route(directory, moves); package['directory'] = new
        if directory in rows:
            package['files_sha256'] = deepcopy(rows[directory]['files_sha256'])
        elif directory in receipt['stationary_owners']:
            package['files_sha256'] = deepcopy(receipt['stationary_owners'][directory]['files_sha256'])
        active[new] = package
    result['packages'] = active
    for package in result.get('merges', []):
        old = package['directory']; package['directory'] = route(old, moves)
        if isinstance(package.get('merged_into'), str):
            package['merged_into'] = route(package['merged_into'], moves)
        row = rows.get(old) or receipt['stationary_owners'].get(old)
        if row is not None:
            package['files_sha256'] = deepcopy(row['files_sha256'])
    external = {}
    for directory, owner in result.get('external_tests', {}).items():
        if directory in rows:
            external[moves[directory]] = deepcopy(rows[directory]['destination_external_tests'])
        else:
            external[directory] = owner
    result['external_tests'] = external
    return result


def prepare(root, previous, plan, previous_raw_sha256):
    if not isinstance(plan, dict) or set(plan) != {'schema', 'receipt_id'} or plan['schema'] != 1:
        raise ValueError('invalid explicit layout installation plan')
    if KEY in previous:
        raise ValueError('layout is already installed; a new chain-aware receipt is required')
    receipt_id = plan['receipt_id']; receipt = read_receipt(root, receipt_id)
    current = transition_catalog(previous, receipt, receipt_id)
    layout = Layout(root, receipt, receipt_id, current)
    layout.verify_installation(previous, current, previous_raw_sha256)
    return layout, current
