"""Pinned HTTP demand maintenance before the existing exact layout inverse."""
from copy import deepcopy
import os
from pathlib import Path

import proof_maintenance as maintenance
import proof_routing as routing
import source_consolidation
import source_layout
import source_layout_corrections

KEY = 'legacy_proof_maintenance_receipts'
RECEIPTS = ('e37e4ee6074dabae4638f15f4926b9af9c67c0b100fd2d7db4e45d323be61509',)
OWNER = 'src/foundation/networking/http-transport'
LOGICAL = 'src/foundation/http-transport'
FILE = 'src/common/demand.rs'
TERMINAL = '9a4a44802e8f493dcc1a1546caa723ea774c956de5d18c474076f85fd1280ac4'


class Maintenance:
    def __init__(self, layout, catalog, sha, prospective):
        self.layout = layout
        self.sha = sha
        self.row = maintenance._read(layout.root, sha)
        row = self.row
        physical = OWNER + '/' + FILE
        installation = layout.rows[LOGICAL]
        terminal = catalog.get('consolidations', {}).get(TERMINAL)
        if (row['terminal'] != TERMINAL or row['parent'] is not None
                or not isinstance(terminal, dict)
                or source_consolidation.receipt_id(terminal) != TERMINAL
                or catalog.get('consolidation_records_sha256', {}).get(TERMINAL) != TERMINAL
                or terminal.get('destination_directory') != LOGICAL
                or installation['destination_directory'] != OWNER
                or row['layout'] != {key: catalog.get(key) for key in
                                     (source_layout.KEY, source_layout_corrections.KEY)}
                or row['layout'][source_layout.KEY] != layout.receipt_id):
            raise ValueError('legacy maintenance terminal/layout binding differs')
        old, new, roots = maintenance._transition(layout.root, row['before'], row['after'])
        expected = dict(package=deepcopy(installation['package']),
                        merges=deepcopy(installation['merged_packages']),
                        external=deepcopy(installation['destination_external_tests']))
        for package in [expected['package'], *expected['merges']]:
            package['directory'] = OWNER
            if package.get('merged_into') == LOGICAL:
                package['merged_into'] = OWNER
            package['files_sha256'] = deepcopy(installation['files_sha256'])
        before = deepcopy(row['before'])
        if layout.corrections is not None and before['external'] is not None:
            for entry in before['external']['files']:
                before['external']['bytes'] -= layout.corrections.delta(entry['path'], entry['sha256'])
                entry['sha256'] = layout.corrections.project_hash(entry['path'], entry['sha256'])
        if before != expected or set(old) != set(new) or {p for p in old if old[p] != new[p]} != {physical}:
            raise ValueError('legacy maintenance changed owner, roots or unapproved file')
        if len(row['files']) != 1:
            raise ValueError('legacy maintenance requires exact demand patch coverage')
        file = row['files'][0]
        if (not isinstance(file, dict)
                or set(file) != {'before', 'after', 'before_sha256', 'after_sha256', 'replacements'}
                or file['before'] != physical or file['after'] != physical
                or file['before_sha256'] != old[physical] or file['after_sha256'] != new[physical]):
            raise ValueError('legacy maintenance patch path/hash differs')
        self.file = file
        self.validate_catalog(catalog, prospective)
        data, hashes = {}, {}
        total = 0
        for base in roots:
            target = routing.safe(layout.root, base)
            if not target.is_dir():
                raise ValueError('missing legacy maintenance root')
            for directory, children, names in os.walk(target, followlinks=False):
                for name in children + names:
                    path = (Path(directory) / name).relative_to(layout.root).as_posix()
                    current = routing.safe(layout.root, path)
                    if current.is_file():
                        with current.open('rb') as stream:
                            value = stream.read(routing.MAX_LIVE_BYTES - total + 1)
                        total += len(value)
                        if total > routing.MAX_LIVE_BYTES or len(data) >= routing.MAX_FILES:
                            raise ValueError('legacy maintenance material bound exceeded')
                        data[path] = value
                        hashes[path] = routing.digest(value)
                    elif not current.is_dir():
                        raise ValueError('legacy maintenance special file')
        maintenance._bytes_match(data, hashes, row['after'], new)
        data[physical] = self.read(LOGICAL + '/' + FILE, data[physical])
        hashes[physical] = routing.digest(data[physical])
        maintenance._bytes_match(data, hashes, row['before'], old)

    def validate_catalog(self, catalog, prospective=False):
        registered = catalog.get(KEY, [])
        expected = self.row['before'] if prospective and registered == [] else self.row['after']
        if (registered != list(RECEIPTS) and not (prospective and registered == [])
                or maintenance.state(catalog, OWNER) != expected):
            raise ValueError('legacy maintenance registration/current owner differs')

    def validate_refresh(self, previous, current):
        self.validate_catalog(previous, prospective=True)
        self.validate_catalog(current)

    def read(self, path, data):
        if path != LOGICAL + '/' + FILE:
            return data
        if routing.digest(data) != self.file['after_sha256']:
            raise ValueError('legacy maintenance live demand bytes differ')
        return routing.inverse_replacements(data, self.file)

    def project_hash(self, path, sha):
        if path == LOGICAL + '/' + FILE and sha == self.file['after_sha256']:
            return self.file['before_sha256']
        return sha


def load(layout, catalog, prospective=False):
    if not RECEIPTS or LOGICAL not in layout.rows:
        if catalog.get(KEY):
            raise ValueError('legacy maintenance registered without supported owner')
        return None
    if len(RECEIPTS) != 1:
        raise ValueError('legacy maintenance supports only the reviewed initial HTTP step')
    return Maintenance(layout, catalog, RECEIPTS[0], prospective)
