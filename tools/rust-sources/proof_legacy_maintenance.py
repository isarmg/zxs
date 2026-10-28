"""Pinned same-owner HTTP source maintenance before the exact layout inverse."""
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
    def __init__(self, layout, catalog, prospective):
        self.layout = layout
        self.receipt_ids = tuple(RECEIPTS)
        self.steps = []
        installation = layout.rows[LOGICAL]
        terminal = catalog.get('consolidations', {}).get(TERMINAL)
        if (not isinstance(terminal, dict)
                or source_consolidation.receipt_id(terminal) != TERMINAL
                or catalog.get('consolidation_records_sha256', {}).get(TERMINAL) != TERMINAL
                or terminal.get('destination_directory') != LOGICAL
                or installation['destination_directory'] != OWNER):
            raise ValueError('legacy maintenance terminal/layout binding differs')
        for sha in self.receipt_ids:
            row = maintenance._read(layout.root, sha)
            self._append(sha, row, catalog, installation)
        # Consumers that need the current owner use the tip; receipt material
        # consumers must preserve receipt_ids, not just this last receipt.
        self.sha, self.row = self.steps[-1][:2]
        self.validate_catalog(catalog, prospective)
        self._verify_live()

    def _installation_state(self, installation):
        expected = dict(package=deepcopy(installation['package']),
                        merges=deepcopy(installation['merged_packages']),
                        external=deepcopy(installation['destination_external_tests']))
        for package in [expected['package'], *expected['merges']]:
            package['directory'] = OWNER
            if package.get('merged_into') == LOGICAL:
                package['merged_into'] = OWNER
            package['files_sha256'] = deepcopy(installation['files_sha256'])
        return expected

    def _append(self, sha, row, catalog, installation):
        parent = self.steps[-1][0] if self.steps else None
        if (row['terminal'] != TERMINAL or row['parent'] != parent
                or row['layout'] != {key: catalog.get(key) for key in
                                    (source_layout.KEY, source_layout_corrections.KEY)}
                or row['layout'][source_layout.KEY] != self.layout.receipt_id):
            raise ValueError('legacy maintenance ancestry/terminal/layout binding differs')
        old, new, roots = maintenance._transition(self.layout.root, row['before'], row['after'])
        if (set(old) != set(new) or row['before']['external'] != row['after']['external']):
            raise ValueError('legacy maintenance requires unchanged file sets and external tests')
        patches = maintenance._patches(row, old, new)
        if any(not path.startswith(OWNER + '/src/') or not path.endswith('.rs') for path in patches):
            raise ValueError('legacy maintenance only supports existing owner Rust source content')
        before = row['before']
        if self.steps:
            if before != self.steps[-1][1]['after']:
                raise ValueError('legacy maintenance full owner ancestry differs')
        else:
            before = deepcopy(before)
            if self.layout.corrections is not None and before['external'] is not None:
                for entry in before['external']['files']:
                    before['external']['bytes'] -= self.layout.corrections.delta(entry['path'], entry['sha256'])
                    entry['sha256'] = self.layout.corrections.project_hash(entry['path'], entry['sha256'])
            if (before != self._installation_state(installation)
                    or set(patches) != {OWNER + '/' + FILE}):
                raise ValueError('legacy maintenance changed initial owner or demand proof')
        self.steps.append((sha, row, old, new, roots, patches))

    def _verify_live(self):
        _, tip, _, inventory, roots, _ = self.steps[-1]
        data, hashes = {}, {}
        total = 0
        for base in roots:
            target = routing.safe(self.layout.root, base)
            if not target.is_dir():
                raise ValueError('missing legacy maintenance root')
            for directory, children, names in os.walk(target, followlinks=False):
                for name in children + names:
                    path = (Path(directory) / name).relative_to(self.layout.root).as_posix()
                    current = routing.safe(self.layout.root, path)
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
        maintenance._bytes_match(data, hashes, tip['after'], inventory)
        for _, row, old, _, _, patches in reversed(self.steps):
            for path, file in patches.items():
                data[path] = routing.inverse_replacements(data[path], file)
                hashes[path] = file['before_sha256']
            maintenance._bytes_match(data, hashes, row['before'], old)

    def validate_catalog(self, catalog, prospective=False):
        registered = catalog.get(KEY, [])
        complete = list(self.receipt_ids)
        if (not isinstance(registered, list)
                or (registered != complete and not (prospective and registered == complete[:-1]))):
            raise ValueError('legacy maintenance registration must be complete or one immediate predecessor')
        expected = self.steps[len(registered) - 1][1]['after'] if registered else self.steps[0][1]['before']
        if maintenance.state(catalog, OWNER) != expected:
            raise ValueError('legacy maintenance registration/current owner differs')

    def validate_refresh(self, previous, current):
        self.validate_catalog(previous, prospective=True)
        self.validate_catalog(current)

    def read(self, path, data):
        if not path.startswith(LOGICAL + '/'):
            return data
        physical = OWNER + path[len(LOGICAL):]
        if routing.digest(data) != self.steps[-1][3].get(physical):
            raise ValueError('legacy maintenance live owner bytes differ')
        for _, _, _, _, _, patches in reversed(self.steps):
            if physical in patches:
                data = routing.inverse_replacements(data, patches[physical])
        return data

    def project_hash(self, path, sha):
        if not path.startswith(LOGICAL + '/'):
            return sha
        physical = OWNER + path[len(LOGICAL):]
        for _, _, _, _, _, patches in reversed(self.steps):
            file = patches.get(physical)
            if file is not None and sha == file['after_sha256']:
                sha = file['before_sha256']
        return sha


def load(layout, catalog, prospective=False):
    if not RECEIPTS or LOGICAL not in layout.rows:
        if catalog.get(KEY):
            raise ValueError('legacy maintenance registered without supported owner')
        return None
    return Maintenance(layout, catalog, prospective)
