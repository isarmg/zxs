#!/usr/bin/env python3
"""Chain-aware import of one upstream fix into a customized, consolidated owner.

plan -> reviewed resolutions -> apply -> reviewed RECEIPTS append -> refresh -> verify.

Nothing here contacts a network: the upstream clone is a local input. Upstream
paths are replayed from catalog receipts, crate names come from a reviewed table
that must name a registered consolidation or rename, and the only byte
transformation is a deterministic line-based three-way merge. Conflicts stop the
plan until the plan carries a reviewed resolution bound to the exact conflict.
The result is a schema 2 row of the HTTP maintenance chain, verified by the same
live-byte inverse as schema 1 plus a full replay of the recorded merge.
"""
import argparse
import base64
from copy import deepcopy
import difflib
import gzip
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import shlex
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import traceback
from typing import NamedTuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

CATALOG = 'compliance/rust-sources.json'
ORIGINS = 'compliance/provenance/origins.json'
RECEIPTS_DIR = 'compliance/proof-maintenance'
MAPPINGS = 'compliance/external-tests'
EXTENSION = ('upstream', 'mapping', 'replay', 'advisory', 'license', 'consumer_tests')
MAX_UPSTREAM_FILES = 64
MAX_UPSTREAM_BYTES = 2 * 1024 * 1024
MAX_IDENTIFIERS = 64
MAX_RESOLUTIONS = 512
MAX_COMMANDS = 32
GZIP_ABOVE = 64 * 1024
EXIT_CONFLICT = 3
DISPOSITIONS = ('backported-fix', 'hardening')
ADVISORY_ID = re.compile(r'GHSA(?:-[23456789cfghjmpqrvwx]{4}){3}|RUSTSEC-\d{4}-\d{4}')
SHA1 = re.compile(r'[0-9a-f]{40}')
SHA256 = re.compile(r'[0-9a-f]{64}')
IDENT = re.compile(r'[A-Za-z_][A-Za-z0-9_]*')
OUTPUTS = {'plan.json', 'review.diff', 'conflict-report.json', 'CONFLICTS.md', 'attempts', 'receipt-id.txt'}
REQUEST_KEYS = {'owner', 'upstream_clone', 'upstream', 'identifier_map', 'reason', 'advisory', 'consumer_tests'}


class BackportError(ValueError):
    pass


class Conflict(BackportError):
    def __init__(self, report):
        count = sum(not c['resolved'] for item in report for c in item['conflicts'])
        super().__init__('three-way merge left %d unresolved conflict(s)' % count)
        self.report = report


def digest(data):
    return hashlib.sha256(data).hexdigest()


def b64(data):
    return base64.b64encode(data).decode()


def unb64(text):
    if not isinstance(text, str):
        raise BackportError('expected base64 text')
    return base64.b64decode(text, validate=True)


def split_lines(data):
    """Split on LF only and keep every terminator byte; a lone CR stays in its line."""
    parts = bytes(data).split(b'\n')
    lines = [part + b'\n' for part in parts[:-1]]
    if parts[-1]:
        lines.append(parts[-1])
    return lines


def pack(data):
    if len(data) > MAX_UPSTREAM_BYTES:
        raise BackportError('upstream material exceeds %d bytes' % MAX_UPSTREAM_BYTES)
    if len(data) > GZIP_ABOVE:
        return dict(encoding='gzip', base64=b64(gzip.compress(data, mtime=0)))
    return dict(encoding='identity', base64=b64(data))


def unpack(value, label):
    if (not isinstance(value, dict) or set(value) != {'encoding', 'base64'}
            or value['encoding'] not in ('identity', 'gzip')):
        raise BackportError('invalid encoded upstream material: ' + label)
    data = unb64(value['base64'])
    if value['encoding'] == 'gzip':
        with gzip.GzipFile(fileobj=io.BytesIO(data)) as stream:
            data = stream.read(MAX_UPSTREAM_BYTES + 1)
    if len(data) > MAX_UPSTREAM_BYTES:
        raise BackportError('upstream material exceeds %d bytes: %s' % (MAX_UPSTREAM_BYTES, label))
    return data


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False).encode()


# ---------------------------------------------------------------- merge3 ----

def _sync_regions(base, ours, theirs):
    """Line ranges where all three sides agree, ending with an empty sentinel."""
    ours_blocks = difflib.SequenceMatcher(None, base, ours, autojunk=False).get_matching_blocks()
    theirs_blocks = difflib.SequenceMatcher(None, base, theirs, autojunk=False).get_matching_blocks()
    regions, i, j = [], 0, 0
    while i < len(ours_blocks) and j < len(theirs_blocks):
        base_o, at_o, size_o = ours_blocks[i]
        base_t, at_t, size_t = theirs_blocks[j]
        start, end = max(base_o, base_t), min(base_o + size_o, base_t + size_t)
        if start < end:
            regions.append((start, end, at_o + start - base_o, at_t + start - base_t))
        if base_o + size_o < base_t + size_t:
            i += 1
        else:
            j += 1
    regions.append((len(base), len(base), len(ours), len(theirs)))
    return regions


def merge3(base, ours, theirs):
    """Standard diff3 on lines: one-sided changes merge, identical changes merge once.

    Returns (merged_lines, conflicts). Every unequal two-sided change becomes a
    conflict: its `<<<<<<< ours / ||||||| base / ======= / >>>>>>> theirs`
    block stays in merged_lines (offset/length index it) and the three texts are
    recorded with 0-based line ranges. Nothing is guessed.
    """
    base_l, ours_l, theirs_l = split_lines(base), split_lines(ours), split_lines(theirs)
    merged, conflicts = [], []
    b = o = t = 0
    for start, end, at_o, at_t in _sync_regions(base_l, ours_l, theirs_l):
        mine, other, old = ours_l[o:at_o], theirs_l[t:at_t], base_l[b:start]
        if mine or other or old:
            if mine == other or other == old:
                merged.extend(mine)
            elif mine == old:
                merged.extend(other)
            else:
                texts = [b''.join(part) for part in (old, mine, other)]
                block = [b'<<<<<<< ours\n', *mine, b'||||||| base\n', *old, b'=======\n', *other, b'>>>>>>> theirs\n']
                conflicts.append(dict(
                    index=len(conflicts), sha256=digest(canonical([b64(text) for text in texts])),
                    base=dict(start=b, end=start, text=b64(texts[0])),
                    ours=dict(start=o, end=at_o, text=b64(texts[1])),
                    theirs=dict(start=t, end=at_t, text=b64(texts[2])),
                    offset=len(merged), length=len(block)))
                merged.extend(block)
        merged.extend(base_l[start:end])
        b, o, t = end, at_o + end - start, at_t + end - start
    return merged, conflicts


def resolve(merged, conflicts, chosen):
    """Splice reviewed resolution bytes over every conflict block; all must be chosen."""
    if set(chosen) != {c['index'] for c in conflicts}:
        raise BackportError('resolutions must cover exactly the conflicts')
    out, cursor = [], 0
    for conflict in conflicts:
        out.extend(merged[cursor:conflict['offset']])
        out.append(chosen[conflict['index']])
        cursor = conflict['offset'] + conflict['length']
    out.extend(merged[cursor:])
    return b''.join(out)


# ------------------------------------------------------ identifier mapping --

RAW_STRING = re.compile(rb'(?<![A-Za-z0-9_])[bc]?r(#*)"')
STRING = re.compile(rb'(?<![A-Za-z0-9_])[bc]?"')
CHAR = re.compile(rb"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|[^\n])|[\xc0-\xf7][\x80-\xbf]{1,3}|[^'\\\n])'")


class _RustToken(NamedTuple):
    kind: str
    text: bytes
    start: int
    end: int


def _unicode_character(data, index):
    first = data[index]
    if 0xc2 <= first <= 0xdf:
        width = 2
    elif 0xe0 <= first <= 0xef:
        width = 3
    elif 0xf0 <= first <= 0xf4:
        width = 4
    else:
        width = 1
    try:
        return data[index:index + width].decode('utf-8'), width
    except UnicodeDecodeError as error:
        raise BackportError('Rust identifier is not UTF-8 at byte %d' % index) from error


def _identifier_width(data, index):
    """Consume complete XID continuation characters, never ASCII substrings."""
    byte = data[index]
    if byte < 128:
        return int(byte == 95 or 48 <= byte <= 57 or 65 <= byte <= 90 or 97 <= byte <= 122)
    character, width = _unicode_character(data, index)
    return width if ('a' + character).isidentifier() else 0


def _space_width(data, index):
    if data[index] in b' \t\r\n\x0b\x0c':
        return 1
    # Non-ASCII Pattern_White_Space, including Rust's LTR/RTL marks.
    for space in (b'\xc2\x85', b'\xe2\x80\x8e', b'\xe2\x80\x8f',
                  b'\xe2\x80\xa8', b'\xe2\x80\xa9'):
        if data.startswith(space, index):
            return len(space)
    return 0


def _rust_tokens(data, prose=False):
    """Token boundaries needed for path roots, plus independently scanned comments.

    This is not a Rust expression parser. Identifiers use Python's Unicode XID
    support; a source token outside that supported set is rejected explicitly.
    Comment prose keeps its historical path rewriting without carrying import
    state into code or treating prose quotes as Rust literals.
    """
    tokens, comments, index, size = [], [], 0, len(data)
    while index < size:
        start = index
        space = _space_width(data, index)
        if space:
            index += space
            continue
        if not prose and data.startswith(b'//', index):
            end = data.find(b'\n', index)
            index = size if end < 0 else end
            comments.append((start, index))
            continue
        if not prose and data.startswith(b'/*', index):
            depth, index = 1, index + 2
            while index < size and depth:
                if data.startswith(b'/*', index):
                    depth, index = depth + 1, index + 2
                elif data.startswith(b'*/', index):
                    depth, index = depth - 1, index + 2
                else:
                    index += 1
            if depth:
                raise BackportError('unterminated Rust block comment')
            comments.append((start, index))
            continue
        raw = None if prose else RAW_STRING.match(data, index)
        if raw:
            close = data.find(b'"' + raw[1], raw.end())
            if close < 0:
                raise BackportError('unterminated Rust raw string')
            index = close + 1 + len(raw[1])
            tokens.append(_RustToken('literal', b'', start, index))
            continue
        string = None if prose else STRING.match(data, index)
        if string:
            end = string.end()
            while end < size and data[end] != 0x22:
                end += 2 if data[end] == 0x5c else 1
            if end >= size:
                raise BackportError('unterminated Rust string')
            index = end + 1
            tokens.append(_RustToken('literal', b'', start, index))
            continue
        if not prose and (data[index] == 0x27 or data.startswith(b"b'", index)):
            char = CHAR.match(data, index + (data[index] == 0x62))
            if char:
                index = char.end()
                tokens.append(_RustToken('literal', b'', start, index))
                continue
        raw_ident = data.startswith(b'r#', index)
        at = index + 2 if raw_ident else index
        width = _identifier_width(data, at) if at < size else 0
        if width:
            index = at + width
            while index < size:
                width = _identifier_width(data, index)
                if not width:
                    break
                index += width
            name = data[at:index]
            kind = 'number' if 48 <= name[0] <= 57 else 'identifier'
            if kind == 'identifier' and not prose:
                try:
                    supported = name.decode('utf-8').isidentifier()
                except UnicodeDecodeError as error:
                    raise BackportError('Rust identifier is not UTF-8 at byte %d' % at) from error
                if not supported:
                    raise BackportError('unsupported Rust identifier token at byte %d' % at)
            tokens.append(_RustToken(kind, data[start:index], start, index))
            continue
        if data.startswith(b'::', index):
            index += 2
        elif data[index] >= 128:
            _, width = _unicode_character(data, index)
            if not prose:
                raise BackportError('unsupported Rust identifier token at byte %d' % index)
            index += width
        else:
            index += 1
        tokens.append(_RustToken('punctuation', data[start:index], start, index))
    return tokens, comments


def _use_roots(tokens, strict):
    """Parse use trees, carrying the enclosing path prefix through each group."""
    roots, covered, index = set(), set(), 0
    while index < len(tokens):
        if tokens[index].text != b'use':
            index += 1
            continue
        begin, index = index, index + 1
        local, groups, root, state = set(), [], True, 'tree'
        while index < len(tokens):
            token = tokens[index]
            text = token.text
            if state in ('tree', 'group', 'segment'):
                if text == b'}' and state == 'group':
                    groups.pop()
                    state = 'done'
                elif text == b'::' and state != 'segment':
                    root, state = True, 'absolute'
                elif text == b'{':
                    groups.append(root)
                    state = 'group'
                elif token.kind == 'identifier' or text == b'*':
                    if root and token.kind == 'identifier':
                        local.add(index)
                    root, state = False, 'path' if text != b'*' else 'done'
                else:
                    break
            elif state == 'absolute':
                if text == b'{':
                    groups.append(root)
                    state = 'group'
                elif token.kind == 'identifier':
                    local.add(index)
                    root, state = False, 'path'
                else:
                    break
            elif state == 'path':
                if text == b'::':
                    state = 'segment'
                elif text == b'as':
                    state = 'alias'
                else:
                    state = 'done'
                    continue
            elif state == 'alias':
                if token.kind != 'identifier':
                    break
                state = 'done'
            else:
                if text == b',' and groups:
                    root, state = groups[-1], 'group'
                elif text == b'}' and groups:
                    groups.pop()
                elif text == b';' and not groups:
                    roots.update(local)
                    covered.update(range(begin, index + 1))
                    index += 1
                    break
                else:
                    break
            index += 1
        else:
            # An import in prose can be an incomplete example. It cannot
            # authorize rewriting tokens by the expression-path fallback.
            index = len(tokens)
        if index and tokens[index - 1].text == b';' and not groups:
            continue
        if strict:
            raise BackportError('unsupported Rust use tree at byte %d' % tokens[begin].start)
        if index == len(tokens) and not groups and state in ('path', 'done'):
            roots.update(local)
            covered.update(range(begin, index))
            continue
        # A prose phrase such as "we use crate::Type in examples" is not a
        # use item. Leave its ordinary path roots available to the fallback.
        while index < len(tokens) and tokens[index].text != b';':
            index += 1
        index += 1
    return roots, covered


def _crate_root(tokens, index, prose=False):
    """Expression/type path roots and extern crate names outside parsed use trees."""
    before = tokens[index - 1] if index else None
    if before is not None and (before.text == b'.' or (not prose and before.text == b"'")):
        return False
    if before is not None and before.text == b'::' and index >= 2:
        left = tokens[index - 2]
        if left.kind in ('identifier', 'literal', 'number') or left.text in (b'>', b')', b']'):
            return False
    return ((index + 1 < len(tokens) and tokens[index + 1].text == b'::')
            or (index >= 2 and tokens[index - 2].text == b'extern' and before.text == b'crate'))


def map_identifiers(data, identifier_map):
    """Rewrite crate-name path roots outside literals; return bytes and every replacement."""
    if not identifier_map:
        return data, []
    targets = {entry['from'].encode(): entry['to'] for entry in identifier_map}
    tokens, comments = _rust_tokens(data)
    streams = [(tokens, 0, True)]
    streams += [(_rust_tokens(data[start:end], prose=True)[0], start, False) for start, end in comments]
    changes = []
    for tokens, offset, strict in streams:
        roots, imports = _use_roots(tokens, strict)
        for index, token in enumerate(tokens):
            raw = token.text.startswith(b'r#')
            name = token.text[2:] if raw else token.text
            if token.kind != 'identifier' or name not in targets:
                continue
            root = index in roots if index in imports else _crate_root(tokens, index, prose=not strict)
            if root:
                changes.append((offset + token.start + (2 if raw else 0), offset + token.end, name.decode()))
    out, last, records = [], 0, []
    for start, end, name in sorted(changes):
        out += [data[last:start], targets[name.encode()].encode()]
        last = end
        records.append({'line': data.count(b'\n', 0, start) + 1,
                        'column': start - (data.rfind(b'\n', 0, start) + 1) + 1,
                        'from': name, 'to': targets[name.encode()]})
    out.append(data[last:])
    return b''.join(out), records


def validate_identifier_map(catalog, identifier_map):
    """Each entry must be a registered consolidation input -> destination, or a rename."""
    import source_consolidation
    if not isinstance(identifier_map, list) or len(identifier_map) > MAX_IDENTIFIERS:
        raise BackportError('identifier_map must be a list of at most %d entries' % MAX_IDENTIFIERS)
    seen = set()
    for entry in identifier_map:
        if not isinstance(entry, dict) or set(entry) != {'from', 'to', 'basis'}:
            raise BackportError('identifier_map entries need exactly from/to/basis')
        if any(not isinstance(entry[k], str) or not IDENT.fullmatch(entry[k]) for k in ('from', 'to')):
            raise BackportError('identifier_map from/to must be Rust identifiers')
        if entry['from'] == entry['to'] or entry['from'] in seen:
            raise BackportError('identifier_map entry is redundant or duplicated: ' + entry['from'])
        seen.add(entry['from'])
        kind, _, key = entry['basis'].partition(':') if isinstance(entry['basis'], str) else ('', '', '')
        if kind == 'consolidation':
            record = catalog.get('consolidations', {}).get(key)
            if (not isinstance(record, dict) or source_consolidation.receipt_id(record) != key
                    or catalog.get('consolidation_records_sha256', {}).get(key) != key):
                raise BackportError('identifier basis names an unregistered consolidation: ' + key)
            inputs = {item['package']['name'].replace('-', '_') for item in record['inputs']}
            if entry['from'] not in inputs or entry['to'] != record['destination_name'].replace('-', '_'):
                raise BackportError('identifier %s -> %s is not consolidation %s (inputs %s -> %s)'
                                    % (entry['from'], entry['to'], key, sorted(inputs), record['destination_name']))
        elif kind == 'rename':
            record = catalog.get('renames', {}).get(key)
            if (not isinstance(record, dict) or record['package']['name'].replace('-', '_') != entry['from']
                    or record['destination_name'].replace('-', '_') != entry['to']):
                raise BackportError('identifier %s -> %s is not registered rename %s' % (entry['from'], entry['to'], key))
        else:
            raise BackportError('identifier basis must be consolidation:<receipt> or rename:<directory>')


# ------------------------------------------------------------ path mapping --

def consolidation_chain(catalog, logical):
    """Consolidation receipts that produced the logical owner, oldest first."""
    records = catalog.get('consolidations', {})
    consumed = {link.get('receipt_id') for record in records.values()
                for link in record.get('input_consolidations', {}).values()}
    terminal = [key for key, record in records.items()
                if record['destination_directory'] == logical and key not in consumed]
    if len(terminal) != 1:
        raise BackportError('owner needs exactly one terminal consolidation receipt: ' + logical)
    chain, key = [], terminal[0]
    while key is not None:
        if key not in records or len(chain) >= 64:
            raise BackportError('broken or unbounded consolidation chain at ' + key)
        record = records[key]
        chain.append((key, record))
        key = record.get('input_consolidations', {}).get(record['primary_directory'], {}).get('receipt_id')
    chain.reverse()
    return chain


def derive_paths(catalog, origins, layout, owner, upstream_paths):
    """upstream path -> current production path, replayed only from receipts.

    origin inventory -> each consolidation's primary file_routes -> layout
    current path -> source_moves file_moves. Every historical owner path is kept
    because external test `source` labels use extraction-time paths.
    """
    import source_layout
    import source_layout_corrections
    package = catalog['packages'].get(owner)
    if package is None:
        raise BackportError('unknown owner: ' + owner)
    origin = next((r for r in origins if r['origin_directory'] == package['origin_directory']), None)
    if origin is None:
        raise BackportError('owner origin is not registered: ' + package['origin_directory'])
    logical = layout.historical_path(owner)
    chain = consolidation_chain(catalog, logical)
    first = chain[0][1]
    primary = next(item['package'] for item in first['inputs'] if item['package']['directory'] == first['primary_directory'])
    if primary['origin_directory'] != package['origin_directory']:
        raise BackportError('owner origin is not the primary input of its first consolidation')
    evidence = [key for key, _ in chain]
    result = {}
    for upstream_path in upstream_paths:
        if upstream_path not in origin['original_files_sha256']:
            raise BackportError('upstream path is not in the origin inventory: ' + upstream_path)
        relative, directory, history = upstream_path, first['primary_directory'], []
        for key, record in chain:
            history.append(directory + '/' + relative)
            route = record['file_routes'].get(directory, {}).get(relative)
            if not isinstance(route, dict) or set(route) != {'destination'}:
                raise BackportError('upstream file has no production route in consolidation %s: %s' % (key, relative))
            relative, directory = route['destination'], record['destination_directory']
        history.append(directory + '/' + relative)
        current = layout.current_path(directory + '/' + relative)
        for key, move in catalog.get('source_moves', {}).items():
            if move['directory'] == owner and relative in move['file_moves']:
                relative = move['file_moves'][relative]
                current = owner + '/' + relative
                if key not in evidence:
                    evidence.append(key)
        if current != owner + '/' + relative or relative not in package['files_sha256']:
            raise BackportError('replayed path is not a current production file: ' + current)
        result[upstream_path] = dict(local_path=current, history=list(dict.fromkeys(history)),
                                     origin_sha256=origin['original_files_sha256'][upstream_path])
    evidence += [catalog[key] for key in (source_layout.KEY, source_layout_corrections.KEY) if catalog.get(key)]
    return result, evidence


def _external_entries(catalog, owner, derived, upstream_path):
    owner_row = catalog.get('external_tests', {}).get(owner) or {}
    item = derived[upstream_path]
    return [entry for entry in owner_row.get('files', [])
            if 'source_kind' not in entry and entry['source'] in item['history']]


def test_regions(layout, catalog, owner, derived, upstream_path, base, mapped_base, historical=None):
    """External test files that are verbatim, line-aligned slices of the upstream base.

    The slice is matched against the file's pre-maintenance (historical) bytes,
    so a later backport still finds the extraction-time region.
    """
    item, found = derived[upstream_path], []
    for entry in _external_entries(catalog, owner, derived, upstream_path):
        if entry['source_sha256_before'] != item['origin_sha256']:
            raise BackportError('external test attribution differs from the origin bytes: ' + entry['path'])
        content = layout.read(entry['path']) if historical is None else historical[entry['path']]
        candidates = [text for text in (base, mapped_base) if text.count(content) == 1]
        if not content or not candidates:
            raise BackportError('external test is not a unique verbatim slice of the upstream base: ' + entry['path'])
        text = candidates[0]
        start = text.find(content)
        end = start + len(content)
        if (start and text[start - 1:start] != b'\n') or (end != len(text) and not content.endswith(b'\n')):
            raise BackportError('external test slice is not line-aligned: ' + entry['path'])
        line = text.count(b'\n', 0, start)
        found.append(dict(path=entry['path'], line_start=line, line_end=line + len(split_lines(content))))
    found.sort(key=lambda region: region['line_start'])
    for left, right in zip(found, found[1:]):
        if left['line_end'] > right['line_start']:
            raise BackportError('overlapping extracted test regions in ' + upstream_path)
    return found


def route_changes(base, fixed, regions, label):
    """Split base -> fixed into the production part and one part per extracted test region.

    Returns {key: (base_part, theirs_part, change_count)}. The production part is
    the whole base with only production changes applied; a region part is its
    base slice with only its own changes. A change across a boundary fails.
    """
    base_l, fixed_l = split_lines(base), split_lines(fixed)
    spans = [(r['line_start'], r['line_end'], r['path']) for r in regions]
    routed = []
    for tag, b0, b1, f0, f1 in difflib.SequenceMatcher(None, base_l, fixed_l, autojunk=False).get_opcodes():
        if tag == 'equal':
            continue
        key = 'production'
        for lo, hi, path in spans:
            if (lo < b0 < hi) if b0 == b1 else (lo <= b0 and b1 <= hi):
                key = path
            elif b0 < hi and b1 > lo:
                raise BackportError('upstream change at base lines %d-%d of %s straddles extracted test %s (%d-%d)'
                                    % (b0 + 1, b1, label, path, lo + 1, hi))
        routed.append((key, b0, b1, f0, f1))
    parts = {}
    for key, lo, hi in [('production', 0, len(base_l))] + [(path, lo, hi) for lo, hi, path in spans]:
        out, cursor, count = [], lo, 0
        for owner, b0, b1, f0, f1 in routed:
            if owner == key:
                out += base_l[cursor:b0] + fixed_l[f0:f1]
                cursor, count = b1, count + 1
        out += base_l[cursor:hi]
        parts[key] = (b''.join(base_l[lo:hi]), b''.join(out), count)
    return parts


# ---------------------------------------------------------------- upstream --

HUNK = re.compile(rb'@@ -(\d+),(\d+) \+(\d+),(\d+) @@\n')
NO_NEWLINE = b'\\ No newline at end of file\n'


def unified_patch(base, fixed, path):
    """Unified diff with explicit counts; a final line without LF is marked, never merged."""
    old, new = split_lines(base), split_lines(fixed)
    out = [b'--- a/' + path.encode() + b'\n', b'+++ b/' + path.encode() + b'\n']

    def emit(marker, line):
        out.append(marker + line)
        if not line.endswith(b'\n'):
            out.extend([b'\n', NO_NEWLINE])
    for group in difflib.SequenceMatcher(None, old, new, autojunk=False).get_grouped_opcodes(3):
        i1, i2, j1, j2 = group[0][1], group[-1][2], group[0][3], group[-1][4]
        out.append(b'@@ -%d,%d +%d,%d @@\n' % (i1 + (i2 > i1), i2 - i1, j1 + (j2 > j1), j2 - j1))
        for tag, a0, a1, c0, c1 in group:
            for line in old[a0:a1]:
                emit(b' ' if tag == 'equal' else b'-', line)
            if tag != 'equal':
                for line in new[c0:c1]:
                    emit(b'+', line)
    return b''.join(out)


def apply_unified(base, patch):
    """Exact application of unified_patch output: no fuzz, counts and context must match."""
    old, lines = split_lines(base), split_lines(patch)
    if len(lines) < 2 or not lines[0].startswith(b'--- ') or not lines[1].startswith(b'+++ '):
        raise BackportError('malformed upstream patch header')
    out, cursor, index = [], 0, 2
    while index < len(lines):
        match = HUNK.fullmatch(lines[index])
        if not match:
            raise BackportError('malformed upstream patch hunk header')
        start, old_count, new_count = int(match[1]), int(match[2]), int(match[4])
        begin = start - 1 if old_count else start
        if begin < cursor or begin > len(old):
            raise BackportError('upstream patch hunks overlap or exceed the base')
        out += old[cursor:begin]
        cursor, index, seen_old, seen_new = begin, index + 1, 0, 0
        while index < len(lines) and not lines[index].startswith(b'@@ '):
            marker, text = lines[index][:1], lines[index][1:]
            index += 1
            if index < len(lines) and lines[index] == NO_NEWLINE:
                if not text.endswith(b'\n'):
                    raise BackportError('malformed upstream patch end-of-file marker')
                text, index = text[:-1], index + 1
            if marker in (b' ', b'-'):
                if cursor >= len(old) or old[cursor] != text:
                    raise BackportError('upstream patch context mismatch at base line %d' % (cursor + 1))
                cursor, seen_old = cursor + 1, seen_old + 1
            if marker in (b' ', b'+'):
                out.append(text)
                seen_new += 1
            elif marker != b'-':
                raise BackportError('malformed upstream patch line')
        if (seen_old, seen_new) != (old_count, new_count):
            raise BackportError('upstream patch hunk counts do not match its lines')
    return b''.join(out + old[cursor:])


def _git(clone, *args):
    try:
        return subprocess.run(['git', '-C', str(clone), *args], check=True, capture_output=True, timeout=600).stdout
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        stderr = getattr(error, 'stderr', b'') or b''
        raise BackportError('git %s failed: %s' % (' '.join(args), stderr.decode(errors='replace').strip()))


def git_commit(clone, ref):
    return _git(clone, 'rev-parse', '--verify', ref + '^{commit}').decode().strip()


def upstream_material(upstream):
    """Decode recorded upstream bytes; base, patch and fixed hashes must all agree."""
    if (not isinstance(upstream, dict) or set(upstream) != {'name', 'repository', 'base', 'fix', 'files', 'excluded'}
            or not isinstance(upstream['base'], dict) or set(upstream['base']) != {'ref', 'sha1'}
            or not isinstance(upstream['fix'], dict) or set(upstream['fix']) != {'ref', 'sha1', 'parent', 'subject'}
            or any(not SHA1.fullmatch(str(upstream[k]['sha1'])) for k in ('base', 'fix'))
            or not SHA1.fullmatch(str(upstream['fix']['parent']))
            or not isinstance(upstream['excluded'], list)
            or not isinstance(upstream['files'], list) or not 1 <= len(upstream['files']) <= MAX_UPSTREAM_FILES):
        raise BackportError('invalid upstream record')
    material = {}
    for entry in upstream['files']:
        if (not isinstance(entry, dict)
                or set(entry) != {'upstream_path', 'base_sha256', 'fixed_sha256', 'patch_sha256', 'base', 'patch'}):
            raise BackportError('invalid upstream file record')
        path = entry['upstream_path']
        if (not isinstance(path, str) or path in material or path.startswith('/')
                or any(part in ('', '.', '..') for part in path.split('/'))):
            raise BackportError('invalid or duplicate upstream path')
        base, patch = unpack(entry['base'], path), unpack(entry['patch'], path)
        if digest(base) != entry['base_sha256']:
            raise BackportError('upstream base bytes drift: ' + path)
        if digest(patch) != entry['patch_sha256']:
            raise BackportError('upstream patch bytes drift: ' + path)
        fixed = apply_unified(base, patch)
        if digest(fixed) != entry['fixed_sha256']:
            raise BackportError('upstream fixed bytes drift: ' + path)
        if base == fixed:
            raise BackportError('upstream file is unchanged by the fix: ' + path)
        material[path] = (base, fixed)
    return material


# ------------------------------------------------------------------ merge ----

def _resolution_index(resolutions):
    if not isinstance(resolutions, list) or len(resolutions) > MAX_RESOLUTIONS:
        raise BackportError('resolutions must be a list of at most %d entries' % MAX_RESOLUTIONS)
    index = {}
    for item in resolutions:
        if (not isinstance(item, dict) or set(item) != {'file', 'hunk_index', 'conflict_sha256', 'resolution_base64'}
                or not isinstance(item['file'], str) or type(item['hunk_index']) is not int
                or not SHA256.fullmatch(str(item['conflict_sha256']))):
            raise BackportError('resolution entries need exactly file/hunk_index/conflict_sha256/resolution_base64')
        key = (item['file'], item['hunk_index'])
        if key in index:
            raise BackportError('duplicate resolution for %s hunk %d' % key)
        index[key] = (item, unb64(item['resolution_base64']))
    return index


def merge_files(material, mapping, ours, resolutions):
    """Deterministic core shared by plan, apply and verify.

    Returns (entries, results, replacements, report): one entry per local file
    that receives upstream changes, its merged bytes, every identifier
    replacement, and per-file conflict reports (resolved or not).
    """
    chosen, used, targets = _resolution_index(resolutions), set(), set()
    entries, results, replacements, report = [], {}, [], []
    for upstream_path, (base, fixed) in material.items():
        mapped_base, base_records = map_identifiers(base, mapping['identifiers'])
        mapped_fixed, fixed_records = map_identifiers(fixed, mapping['identifiers'])
        if len(split_lines(mapped_base)) != len(split_lines(base)):
            raise BackportError('identifier mapping changed the line structure of ' + upstream_path)
        replacements += [dict(file=upstream_path, side='base', **r) for r in base_records]
        replacements += [dict(file=upstream_path, side='fixed', **r) for r in fixed_records]
        regions = mapping['test_regions'].get(upstream_path, [])
        for key, (base_part, theirs, count) in route_changes(mapped_base, mapped_fixed, regions, upstream_path).items():
            if not count:
                continue
            local = mapping['paths'][upstream_path]['production'] if key == 'production' else key
            if local in targets or local not in ours:
                raise BackportError('replay target is duplicated or missing: ' + local)
            targets.add(local)
            current = ours[local]
            merged, conflicts = merge3(base_part, current, theirs)
            picks, applied = {}, []
            for conflict in conflicts:
                item = chosen.get((local, conflict['index']))
                conflict['resolved'] = item is not None
                if item is None:
                    continue
                if item[0]['conflict_sha256'] != conflict['sha256']:
                    raise BackportError('resolution drift: %s hunk %d no longer matches its reviewed conflict'
                                        % (local, conflict['index']))
                picks[conflict['index']] = item[1]
                applied.append(item[0])
                used.add((local, conflict['index']))
            if conflicts:
                report.append(dict(file=local, upstream_path=upstream_path, region=key,
                                   base_sha256=digest(base_part), ours_sha256=digest(current),
                                   theirs_sha256=digest(theirs), conflicts=conflicts))
            if len(picks) == len(conflicts):
                results[local] = resolve(merged, conflicts, picks) if conflicts else b''.join(merged)
            entries.append(dict(local_path=local, upstream_path=upstream_path, region=key,
                                base_sha256=digest(base_part), ours_sha256=digest(current), theirs_sha256=digest(theirs),
                                result_sha256=digest(results[local]) if local in results else None,
                                hunks_applied=count, hunks_conflicted=len(conflicts), resolutions=applied))
    if set(chosen) - used:
        raise BackportError('resolutions name no current conflict: %s' % sorted(set(chosen) - used))
    return entries, results, replacements, report


# ----------------------------------------------------- receipt validation --

def check_advisory(advisory, keys=('ids', 'disposition', 'basis')):
    if (not isinstance(advisory, dict) or set(advisory) != set(keys)
            or not isinstance(advisory['ids'], list) or len(set(advisory['ids'])) != len(advisory['ids'])
            or any(not isinstance(i, str) or not ADVISORY_ID.fullmatch(i) for i in advisory['ids'])
            or advisory['disposition'] not in DISPOSITIONS
            or not isinstance(advisory['basis'], str) or not advisory['basis'].strip()
            or advisory.get('scanner_identity_unchanged', True) is not True):
        raise BackportError('advisory needs GHSA/RUSTSEC ids (or none), a disposition in %s and a basis'
                            % (DISPOSITIONS,))


def check_consumer_tests(tests, keys=('gates', 'commands')):
    if (not isinstance(tests, dict) or set(tests) != set(keys)
            or any(not isinstance(tests[k], list) or len(tests[k]) > MAX_COMMANDS
                   or any(not isinstance(v, str) or not v.strip() for v in tests[k]) for k in ('gates', 'commands'))
            or not isinstance(tests.get('executed', {}), dict)):
        raise BackportError('consumer_tests needs bounded gates/commands string lists')


def _check_static(row):
    package = row['before']['package']
    check_advisory(row['advisory'], ('ids', 'disposition', 'basis', 'scanner_identity_unchanged'))
    check_consumer_tests(row['consumer_tests'], ('gates', 'commands', 'executed'))
    license_ = row['license']
    if (not isinstance(license_, dict) or set(license_) != {'materials', 'changed'} or license_['changed'] is not False
            or license_['materials'] != package.get('license_materials')):
        raise BackportError('backport license record differs from the unchanged owner license materials')
    upstream = row['upstream']
    vcs = package.get('vcs', {}).get('git', {}).get('sha1')
    if (upstream['name'] != package['upstream']['name'] or upstream['repository'] != package.get('repository')
            or (vcs is not None and upstream['base']['sha1'] != vcs)):
        raise BackportError('backport upstream identity differs from the owner origin')
    mapping, replay = row['mapping'], row['replay']
    if (not isinstance(mapping, dict) or set(mapping) != {'paths', 'test_regions', 'identifiers', 'evidence'}
            or not isinstance(mapping['paths'], dict) or not isinstance(mapping['test_regions'], dict)
            or not isinstance(replay, dict) or set(replay) != {'merged', 'identifier_replacements'}
            or not isinstance(replay['merged'], list) or not replay['merged']):
        raise BackportError('invalid backport mapping/replay record')
    for path in upstream_paths(row):
        paths = mapping['paths'].get(path)
        if not isinstance(paths, dict) or set(paths) != {'production', 'tests'} or not isinstance(paths['tests'], list):
            raise BackportError('invalid backport path mapping for ' + path)
    if set(mapping['paths']) != set(upstream_paths(row)) or not set(mapping['test_regions']) <= set(mapping['paths']):
        raise BackportError('backport path mapping does not cover exactly the upstream files')


def upstream_paths(row):
    return [entry['upstream_path'] for entry in row['upstream']['files']]


def check_replay(row, before, after, catalog):
    """Receipt-internal proof: before + upstream + identifiers + resolutions == after.

    `before`/`after` hold the owner's bytes on each side of this step. Any drift
    names its input; the receipt's changed files must be exactly the replay's.
    """
    material = upstream_material(row['upstream'])
    _check_static(row)
    validate_identifier_map(catalog, row['mapping']['identifiers'])
    recorded = row['replay']['merged']
    ours, resolutions = {}, []
    for item in recorded:
        if not isinstance(item, dict) or item.get('local_path') not in before or not isinstance(item.get('resolutions'), list):
            raise BackportError('replay entry names a file outside the owner snapshot')
        ours[item['local_path']] = before[item['local_path']]
        resolutions += item['resolutions']
    entries, results, replacements, report = merge_files(material, row['mapping'], ours, resolutions)
    unresolved = sorted({item['file'] for item in report for c in item['conflicts'] if not c['resolved']})
    if unresolved:
        raise BackportError('receipt replay has unresolved conflicts in %s' % ', '.join(unresolved))
    if replacements != row['replay']['identifier_replacements']:
        raise BackportError('identifier mapping drift: replayed replacements differ from the record')
    if [e['local_path'] for e in entries] != [item['local_path'] for item in recorded]:
        raise BackportError('replay entry set drift')
    for entry, item in zip(entries, recorded):
        path = entry['local_path']
        for field, message in (('upstream_path', 'replay entry upstream path drift'), ('region', 'test region drift'),
                               ('base_sha256', 'upstream base slice drift'), ('ours_sha256', 'local before bytes drift'),
                               ('theirs_sha256', 'upstream fix or identifier mapping drift'),
                               ('hunks_applied', 'merge hunk accounting drift'),
                               ('hunks_conflicted', 'merge conflict accounting drift'),
                               ('resolutions', 'resolution drift'), ('result_sha256', 'merge result drift')):
            if entry[field] != item.get(field):
                raise BackportError('%s: %s' % (message, path))
        if set(item) != set(entry):
            raise BackportError('invalid replay entry fields: ' + path)
        if after.get(path) != results[path]:
            raise BackportError('replayed merge differs from recorded after bytes: ' + path)
    changed = {path for path, data in results.items() if data != ours[path]}
    if {file['after'] for file in row['files']} != changed:
        raise BackportError('receipt changed files differ from the replayed change set')
    return results


def check_mapping(layout, catalog, row):
    """Re-derive production paths from receipts; return {external path: upstream path} allowed to change."""
    origins = json.loads((Path(layout.root) / ORIGINS).read_bytes())['packages']
    owner = row['before']['package']['directory']
    derived, evidence = derive_paths(catalog, origins, layout, owner, upstream_paths(row))
    if row['mapping']['evidence'] != evidence:
        raise BackportError('backport mapping evidence drift')
    external = {entry['path']: entry for entry in (row['after']['external'] or {}).get('files', [])}
    for entry in row['upstream']['files']:
        if entry['base_sha256'] != derived[entry['upstream_path']]['origin_sha256']:
            raise BackportError('upstream base is not the registered origin bytes: ' + entry['upstream_path'])
    material, allowed = upstream_material(row['upstream']), {}
    for entry in row['upstream']['files']:
        path = entry['upstream_path']
        if row['mapping']['paths'][path]['production'] != derived[path]['local_path']:
            raise BackportError('path mapping drift: ' + path)
        regions = row['mapping']['test_regions'].get(path, [])
        lines = len(split_lines(material[path][0]))
        if [r.get('path') if isinstance(r, dict) else None for r in regions] != row['mapping']['paths'][path]['tests']:
            raise BackportError('test region list drift: ' + path)
        end = 0
        for region in regions:
            test = external.get(region['path'])
            if (set(region) != {'path', 'line_start', 'line_end'}
                    or any(type(region[k]) is not int for k in ('line_start', 'line_end'))
                    or not end <= region['line_start'] < region['line_end'] <= lines
                    or test is None or 'source_kind' in test or test['source'] not in derived[path]['history']
                    or test['source_sha256_before'] != derived[path]['origin_sha256']):
                raise BackportError('test region is not an extracted slice of %s: %s' % (path, region.get('path')))
            end = region['line_end']
            allowed[region['path']] = path
    return allowed


def check_regions(layout, catalog, row, historical=None):
    """Re-derive exact regions, optionally from an already authenticated historical byte view.

    Maintenance construction passes its fully inverted bytes instead of reading
    through the layout adapter that is still being constructed.
    """
    origins = json.loads((Path(layout.root) / ORIGINS).read_bytes())['packages']
    owner = row['before']['package']['directory']
    derived, _ = derive_paths(catalog, origins, layout, owner, upstream_paths(row))
    material = upstream_material(row['upstream'])
    for path, (base, _) in material.items():
        mapped, _ = map_identifiers(base, row['mapping']['identifiers'])
        regions = test_regions(layout, catalog, owner, derived, path, base, mapped, historical)
        if regions != row['mapping']['test_regions'].get(path, []):
            raise BackportError('test region drift: ' + path)


# ------------------------------------------------------------- workspace ----

def load_json(path):
    return json.loads(Path(path).read_bytes())


def existing_file(root, relative):
    """Repository input/write target: contained, no symlink, existing regular file."""
    import proof_routing
    path = proof_routing.safe(root, relative)
    if not stat.S_ISREG(path.stat().st_mode):
        raise BackportError('backport target is not a regular file: ' + relative)
    return path


def workspace(root, out_dir):
    """An output directory outside the repository holding only this tool's outputs."""
    out = Path(out_dir).resolve()
    if out.is_relative_to(Path(root).resolve()):
        raise BackportError('output directory must be outside the repository')
    if out.exists():
        unknown = {child.name for child in out.iterdir()} - OUTPUTS
        if unknown:
            raise BackportError('output directory holds unrelated files: ' + ', '.join(sorted(unknown)))
    out.mkdir(parents=True, exist_ok=True)
    return out


def retain(out, kind, documents):
    """失败保留: each failed attempt gets its own directory; the tool never prunes attempts/."""
    tag = digest(canonical({k: v if isinstance(v, str) else canonical(v).decode() for k, v in documents.items()}))[:12]
    stamp = time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())
    attempts = out / 'attempts'
    attempts.mkdir(exist_ok=True)
    for serial in range(1000):
        attempt = attempts / ('%s-%s-%s%s' % (stamp, kind, tag, '-%d' % serial if serial else ''))
        try:
            attempt.mkdir()
            break
        except FileExistsError:
            continue
    else:
        raise BackportError('cannot allocate a retained attempt directory')
    for name, document in documents.items():
        (attempt / name).write_text(document if isinstance(document, str)
                                    else json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + '\n')
    return attempt


def write_document(path, document):
    path.write_text(json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + '\n')


def conflict_summary(report):
    lines = ['# Backport conflicts', '']
    for item in report:
        lines.append('## %s (%s, %s)' % (item['file'], item['upstream_path'], item['region']))
        for conflict in item['conflicts']:
            lines += ['', '### hunk %d (%s): base lines %d-%d, ours %d-%d, theirs %d-%d; conflict_sha256 %s'
                      % (conflict['index'], 'resolved' if conflict['resolved'] else 'UNRESOLVED',
                         conflict['base']['start'] + 1, conflict['base']['end'],
                         conflict['ours']['start'] + 1, conflict['ours']['end'],
                         conflict['theirs']['start'] + 1, conflict['theirs']['end'], conflict['sha256'])]
            for side in ('base', 'ours', 'theirs'):
                lines += ['', '%s:' % side, '', '```rust', unb64(conflict[side]['text']).decode('utf-8', 'replace').rstrip('\n'), '```']
        lines.append('')
    lines.append('Add reviewed `resolutions: [{file, hunk_index, conflict_sha256, resolution_base64}]` '
                 'to the request and run plan again; each resolution is bound to its conflict_sha256.')
    return '\n'.join(lines) + '\n'


# ------------------------------------------------------------------ plan ----

def parse_request(request):
    if not isinstance(request, dict) or not REQUEST_KEYS <= set(request) <= REQUEST_KEYS | {'resolutions'}:
        raise BackportError('request needs exactly %s (+ optional resolutions)' % sorted(REQUEST_KEYS))
    upstream = request['upstream']
    if (not isinstance(upstream, dict) or set(upstream) != {'base', 'fix', 'files'}
            or not all(isinstance(upstream[k], str) and upstream[k] for k in ('base', 'fix'))
            or not isinstance(upstream['files'], list) or not 1 <= len(upstream['files']) <= MAX_UPSTREAM_FILES
            or len(set(upstream['files'])) != len(upstream['files'])):
        raise BackportError('request upstream needs base, fix and a unique bounded files list')
    if not isinstance(request['reason'], str) or not request['reason'].strip():
        raise BackportError('request needs a reviewed reason')
    check_advisory(request['advisory'])
    check_consumer_tests(request['consumer_tests'])
    return request


class FlatLayout:
    """Path view of a repository without a foundation layout receipt."""
    def __init__(self, root):
        self.root = Path(root)
        self.corrections = None

    def historical_path(self, path):
        return path

    def current_path(self, path):
        return path

    def read(self, path):
        return existing_file(self.root, path).read_bytes()


def build_document(root, catalog, layout, owner, reason, upstream, material, identifier_map,
                   resolutions, advisory, consumer_tests):
    """Plan document for decoded upstream material; returns (document, conflict report, review diff)."""
    origins = load_json(Path(root) / ORIGINS)['packages']
    package = catalog['packages'][owner]
    paths = list(material)
    derived, evidence = derive_paths(catalog, origins, layout, owner, paths)
    files, regions = [], {}
    for path, (base, fixed) in material.items():
        if digest(base) != derived[path]['origin_sha256']:
            raise BackportError('upstream base bytes differ from the origin inventory: ' + path)
        if base == fixed:
            raise BackportError('upstream file is unchanged by the fix: ' + path)
        patch = unified_patch(base, fixed, path)
        if apply_unified(base, patch) != fixed:
            raise BackportError('upstream patch does not reproduce the fixed bytes: ' + path)
        files.append(dict(upstream_path=path, base_sha256=digest(base), fixed_sha256=digest(fixed),
                          patch_sha256=digest(patch), base=pack(base), patch=pack(patch)))
        regions[path] = test_regions(layout, catalog, owner, derived, path, base, map_identifiers(base, identifier_map)[0])
    mapping = dict(paths={p: dict(production=derived[p]['local_path'], tests=[r['path'] for r in regions[p]]) for p in paths},
                   test_regions={p: regions[p] for p in paths if regions[p]},
                   identifiers=identifier_map, evidence=evidence)
    ours = {target: existing_file(Path(root), target).read_bytes()
            for p in paths for target in [mapping['paths'][p]['production'], *mapping['paths'][p]['tests']]}
    entries, results, replacements, report = merge_files(material, mapping, ours, resolutions)
    document = dict(schema=1, owner=owner, reason=reason, catalog_sha256=digest(canonical(catalog)),
                    upstream=dict(upstream, name=package['upstream']['name'], repository=package.get('repository'), files=files),
                    mapping=mapping, replay=dict(merged=entries, identifier_replacements=replacements),
                    advisory=advisory, consumer_tests=consumer_tests)
    review = [b''.join(difflib.diff_bytes(difflib.unified_diff, split_lines(ours[path]), split_lines(data),
                                          fromfile=b'a/' + path.encode(), tofile=b'b/' + path.encode()))
              for path, data in results.items()]
    return document, report, b''.join(review)


def _plan(root, request):
    import source_layout
    request = parse_request(request)
    catalog = load_json(root / CATALOG)
    owner = request['owner']
    if owner not in catalog['packages']:
        raise BackportError('unknown owner: ' + owner)
    validate_identifier_map(catalog, request['identifier_map'])
    for gate in request['consumer_tests']['gates']:
        if not (root / gate).is_file():
            raise BackportError('consumer gate does not exist: ' + gate)
    clone = Path(request['upstream_clone'])
    base_sha, fix_sha = git_commit(clone, request['upstream']['base']), git_commit(clone, request['upstream']['fix'])
    if subprocess.run(['git', '-C', str(clone), 'merge-base', '--is-ancestor', base_sha, fix_sha],
                      capture_output=True, timeout=600).returncode:
        raise BackportError('upstream base %s is not an ancestor of the fix %s' % (base_sha, fix_sha))
    vcs = catalog['packages'][owner].get('vcs', {}).get('git', {}).get('sha1')
    if vcs is not None and vcs != base_sha:
        raise BackportError('upstream base %s is not the registered vcs sha %s' % (base_sha, vcs))
    paths = list(request['upstream']['files'])
    changed = set(_git(clone, 'diff', '--name-only', base_sha, fix_sha).decode().splitlines())
    if not set(paths) <= changed:
        raise BackportError('requested upstream files are not changed by the fix: %s' % sorted(set(paths) - changed))
    material = {path: (_git(clone, 'show', '%s:%s' % (base_sha, path)), _git(clone, 'show', '%s:%s' % (fix_sha, path)))
                for path in paths}
    upstream = dict(base=dict(ref=request['upstream']['base'], sha1=base_sha),
                    fix=dict(ref=request['upstream']['fix'], sha1=fix_sha, parent=git_commit(clone, fix_sha + '^'),
                             subject=_git(clone, 'log', '-1', '--format=%s', fix_sha).decode().strip()),
                    excluded=sorted(changed - set(paths)))
    layout = source_layout.load(root, catalog) or FlatLayout(root)
    return build_document(root, catalog, layout, owner, request['reason'], upstream, material,
                          request['identifier_map'], request.get('resolutions', []),
                          request['advisory'], request['consumer_tests'])


def plan(root, request, out_dir):
    """Compute the merge and write plan.json; any unresolved conflict exits 3. Never touches the repo."""
    root = Path(root).resolve()
    out = workspace(root, out_dir)
    for stale in ('plan.json', 'review.diff', 'conflict-report.json', 'CONFLICTS.md'):
        # Derived working copies only: every failed run is retained under attempts/.
        if (out / stale).exists():
            (out / stale).unlink()
    try:
        document, report, review = _plan(root, request)
        if report:
            write_document(out / 'conflict-report.json', report)
            (out / 'CONFLICTS.md').write_text(conflict_summary(report))
        if any(not c['resolved'] for item in report for c in item['conflicts']):
            raise Conflict(report)
    except Conflict as conflict:
        retain(out, 'plan-conflict', {'request.json': request, 'conflict-report.json': conflict.report,
                                      'CONFLICTS.md': conflict_summary(conflict.report)})
        raise
    except (ValueError, OSError) as error:
        retain(out, 'plan-failed', {'request.json': request, 'error.txt': '%s\n' % error})
        raise
    write_document(out / 'plan.json', document)
    (out / 'review.diff').write_bytes(review)
    return document


# ----------------------------------------------------------------- apply ----

def reversible_replacements(before, after):
    """Grouped line replacements that proof_routing.inverse_replacements accepts."""
    import proof_routing
    old_lines, new_lines = split_lines(before), split_lines(after)
    matcher = difflib.SequenceMatcher(None, old_lines, new_lines, autojunk=False)
    for context in (3, 6, 12, 24, 48):
        replacements = [dict(old=b64(b''.join(old_lines[g[0][1]:g[-1][2]])), new=b64(b''.join(new_lines[g[0][3]:g[-1][4]])))
                        for g in matcher.get_grouped_opcodes(context)]
        try:
            proof_routing.inverse_replacements(after, dict(replacements=replacements, before_sha256=digest(before)))
        except ValueError:
            continue
        return replacements
    raise BackportError('cannot derive unique reversible replacements')


def after_catalog(catalog, owner, changes):
    result = deepcopy(catalog)
    external = result.get('external_tests', {}).get(owner)
    for path, (old, new) in changes.items():
        if path.startswith(owner + '/'):
            relative = path[len(owner) + 1:]
            for package in [result['packages'][owner], *[m for m in result.get('merges', []) if m['directory'] == owner]]:
                package['files_sha256'][relative] = digest(new)
        else:
            entry = next(e for e in external['files'] if e['path'] == path)
            entry['sha256'] = digest(new)
            external['bytes'] += len(new) - len(old)
    return result


def build_receipt(document, catalog, changes, parent):
    """The schema 2 maintenance row: schema 1 fields plus the replayable backport record."""
    import proof_legacy_maintenance as legacy
    import proof_maintenance
    import source_layout
    import source_layout_corrections
    owner = document['owner']
    if owner != legacy.OWNER:
        raise BackportError('only the pinned HTTP owner has a backport-capable maintenance chain')
    files = [dict(before=path, after=path, before_sha256=digest(old), after_sha256=digest(new),
                  replacements=reversible_replacements(old, new)) for path, (old, new) in sorted(changes.items())]
    if not files:
        raise BackportError('backport changed no local file')
    return dict(schema=2, reason=document['reason'], terminal=legacy.TERMINAL, parent=parent,
                layout={key: catalog.get(key) for key in (source_layout.KEY, source_layout_corrections.KEY)},
                before=proof_maintenance.state(deepcopy(catalog), owner),
                after=proof_maintenance.state(after_catalog(catalog, owner, changes), owner), files=files,
                upstream=document['upstream'], mapping=document['mapping'], replay=document['replay'],
                advisory=dict(document['advisory'], scanner_identity_unchanged=True),
                license=dict(materials=list(catalog['packages'][owner]['license_materials']), changed=False),
                consumer_tests=dict(gates=list(document['consumer_tests']['gates']),
                                    commands=list(document['consumer_tests']['commands']), executed={}))


def receipt_bytes(row):
    return gzip.compress(json.dumps(row, sort_keys=True).encode(), mtime=0)


def update_mapping(root, owner, changes):
    """Record new external test hashes in the owner's mapping document, byte-stable otherwise."""
    pending = {path for path in changes if not path.startswith(owner + '/')}
    writes = {}
    for file in sorted((root / MAPPINGS).glob('*.json')):
        file = existing_file(root, file.relative_to(root).as_posix())
        raw = file.read_bytes()
        data = json.loads(raw)
        if all(entry_owner['directory'] != owner for entry_owner in data['owners']):
            continue
        if (json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + '\n').encode() != raw:
            raise BackportError('external test mapping is not in canonical form: ' + str(file))
        for entry_owner in data['owners']:
            if entry_owner['directory'] != owner:
                continue
            for entry in entry_owner['files']:
                if entry['path'] in pending:
                    entry['sha256'] = digest(changes[entry['path']][1])
                    pending.discard(entry['path'])
                    writes[file] = data
    if pending:
        raise BackportError('external test files are not in the owner mapping: %s' % sorted(pending))
    return {file: (json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + '\n').encode()
            for file, data in writes.items()}


def run_commands(root, commands):
    executed = {}
    for command in commands:
        result = subprocess.run(shlex.split(command), cwd=root)
        executed[command] = 'pass' if result.returncode == 0 else 'fail(%d)' % result.returncode
        print('%s -> %s' % (command, executed[command]), flush=True)
    return executed


def _apply(root, document, out, run_gates):
    import proof_legacy_maintenance as legacy
    import proof_maintenance
    import proof_routing
    import source_layout
    catalog = load_json(root / CATALOG)
    if document.get('schema') != 1 or document.get('catalog_sha256') != digest(canonical(catalog)):
        raise BackportError('catalog changed since plan (or plan schema differs); run plan again')
    if catalog.get(legacy.KEY, []) != list(legacy.RECEIPTS):
        raise BackportError('register and refresh the previous maintenance receipt before another backport')
    if document['owner'] != legacy.OWNER:
        raise BackportError('only the pinned HTTP owner has a backport-capable maintenance chain')
    resolutions = [r for entry in document['replay']['merged'] for r in entry['resolutions']]
    if (out / 'conflict-report.json').exists():
        covered = {(r['file'], r['hunk_index'], r['conflict_sha256']) for r in resolutions}
        for item in load_json(out / 'conflict-report.json'):
            for conflict in item['conflicts']:
                if (item['file'], conflict['index'], conflict['sha256']) not in covered:
                    raise BackportError('conflict report has an unresolved conflict: %s hunk %d' % (item['file'], conflict['index']))
    material = upstream_material(document['upstream'])
    validate_identifier_map(catalog, document['mapping']['identifiers'])
    layout = source_layout.load(root, catalog) or FlatLayout(root)
    owner = proof_maintenance.state(catalog, document['owner'])
    route = dict(before=owner, after=owner, upstream=document['upstream'], mapping=document['mapping'])
    allowed = set(check_mapping(layout, catalog, route))
    allowed.update(item['production'] for item in document['mapping']['paths'].values())
    ours = {}
    for entry in document['replay']['merged']:
        if entry['local_path'] not in allowed:
            raise BackportError('replay target is outside authenticated routes: ' + entry['local_path'])
        ours[entry['local_path']] = existing_file(root, entry['local_path']).read_bytes()
        if digest(ours[entry['local_path']]) != entry['ours_sha256']:
            raise BackportError('local file changed since plan: ' + entry['local_path'])
    check_regions(layout, catalog, route)
    entries, results, replacements, _ = merge_files(material, document['mapping'], ours, resolutions)
    if entries != document['replay']['merged'] or replacements != document['replay']['identifier_replacements']:
        raise BackportError('plan does not replay byte-for-byte; run plan again')
    changes = {path: (ours[path], data) for path, data in results.items() if data != ours[path]}
    corrected = layout.corrections.rows if layout.corrections is not None else {}
    for path in changes:
        if (not path.endswith('.rs') or path in corrected
                or (path.startswith(document['owner'] + '/') and not path.startswith(document['owner'] + '/src/'))):
            raise BackportError('backport change is outside existing owner Rust sources and uncorrected routed tests: ' + path)
    row = build_receipt(document, catalog, changes, legacy.RECEIPTS[-1] if legacy.RECEIPTS else None)
    check_replay(row, ours, results, catalog)
    writes = {existing_file(root, path): new for path, (_, new) in changes.items()}
    writes.update(update_mapping(root, document['owner'], changes))
    originals = {path: path.read_bytes() for path in writes}
    proof_routing.safe(root, RECEIPTS_DIR)
    receipt = None
    try:
        for path, data in writes.items():
            path.write_bytes(data)
        if run_gates:
            row['consumer_tests']['executed'] = run_commands(root, row['consumer_tests']['commands'])
            if any(status != 'pass' for status in row['consumer_tests']['executed'].values()):
                raise BackportError('consumer gate failed: %s' % row['consumer_tests']['executed'])
        raw = receipt_bytes(row)
        sha = digest(raw)
        target = proof_routing.safe(root, RECEIPTS_DIR + '/' + sha + '.json.gz')
        target.parent.mkdir(exist_ok=True)
        with target.open('xb') as stream:
            receipt = target
            stream.write(raw)
    except BaseException as error:
        for path, data in originals.items():
            try:
                path.write_bytes(data)
            except OSError as rollback_error:
                error.add_note('could not restore %s: %s' % (path, rollback_error))
        if receipt is not None:
            try:
                receipt.unlink()
            except OSError as cleanup_error:
                error.add_note('could not remove incomplete receipt %s: %s' % (receipt, cleanup_error))
        raise
    return sha, row


def apply(root, plan_path, out_dir, run_gates=False):
    """Write merged bytes, external test hashes and the receipt; registration stays a reviewed manual step."""
    root = Path(root).resolve()
    out = workspace(root, out_dir)
    document = load_json(plan_path)
    try:
        sha, row = _apply(root, document, out, run_gates)
    except (ValueError, OSError, KeyError) as error:
        try:
            retain(out, 'apply-failed', {'plan.json': document,
                                       'error.txt': ''.join(traceback.format_exception_only(error))})
        except OSError as retain_error:
            error.add_note('could not retain failed apply evidence: %s' % retain_error)
        raise
    (out / 'receipt-id.txt').write_text(sha + '\n')
    print('backport applied; receipt %s' % sha)
    print("after review, append to RECEIPTS in tools/rust-sources/proof_legacy_maintenance.py:\n    '%s'" % sha)
    print('then: python3 tools/rust-sources/manage.py refresh && python3 tools/rust-sources/manage.py verify --metadata')
    return sha, row


# ---------------------------------------------------------------- verify ----

def chain_step(root, sha):
    """The authenticated adapter step for a registered receipt and its before/after bytes."""
    import source_layout
    catalog = load_json(Path(root) / CATALOG)
    layout = source_layout.load(Path(root), catalog)
    adapter = layout.legacy_maintenance if layout is not None else None
    if adapter is None or sha not in adapter.receipt_ids:
        raise BackportError('receipt is not a registered step of the HTTP maintenance chain: ' + sha)
    row = next(step[1] for step in adapter.steps if step[0] == sha)
    if row['schema'] != 2:
        raise BackportError('receipt is not a schema 2 backport row')
    before, after = adapter.step_bytes[sha]
    return catalog, layout, row, before, after


def check_clone(row, clone):
    """A supplied clone must still contain exactly the recorded upstream material."""
    upstream = row['upstream']
    if (git_commit(clone, upstream['base']['sha1']) != upstream['base']['sha1']
            or git_commit(clone, upstream['fix']['sha1']) != upstream['fix']['sha1']
            or git_commit(clone, upstream['fix']['sha1'] + '^') != upstream['fix']['parent']):
        raise BackportError('upstream clone commit graph differs from the receipt')
    for path, (base, fixed) in upstream_material(upstream).items():
        if _git(clone, 'show', '%s:%s' % (upstream['base']['sha1'], path)) != base:
            raise BackportError('upstream clone base differs from the recorded base: ' + path)
        if _git(clone, 'show', '%s:%s' % (upstream['fix']['sha1'], path)) != fixed:
            raise BackportError('upstream clone fix differs from the recorded fix: ' + path)


def check_external_accounting(sha, row, catalog, before, after):
    """Changed external tests carry new hashes and an updated byte total in after and the catalog."""
    owner = row['after']['package']['directory']
    changed = {file['after'] for file in row['files'] if not file['after'].startswith(owner + '/')}
    if not changed:
        return
    prior, current = row['before']['external'], row['after']['external']
    rows = {entry['path']: entry for entry in current['files']}
    for path in changed:
        if rows[path]['sha256'] != digest(after[path]):
            raise BackportError('external test hash not updated in after snapshot: ' + path)
    if (current['bytes'] - prior['bytes'] != sum(len(after[p]) - len(before[p]) for p in changed)
            or current['file_count'] != prior['file_count'] or current['file_count'] != len(current['files'])):
        raise BackportError('external byte/file accounting drift in after snapshot')
    live = catalog.get('external_tests', {}).get(owner)
    if catalog.get('legacy_proof_maintenance_receipts', [None])[-1] == sha and live != current:
        raise BackportError('catalog external accounting differs from the backport tip')


def verify(root, sha, upstream_clone=None, run_gates=False):
    root = Path(root).resolve()
    catalog, layout, row, before, after = chain_step(root, sha)
    results = check_replay(row, before, after, catalog)
    check_mapping(layout, catalog, row)
    check_regions(layout, catalog, row)
    check_external_accounting(sha, row, catalog, before, after)
    if upstream_clone is not None:
        check_clone(row, upstream_clone)
    print('backport receipt %s verified: %d file(s) replayed byte-for-byte from %s %s..%s%s'
          % (sha, len(results), row['upstream']['name'], row['upstream']['base']['sha1'][:12],
             row['upstream']['fix']['sha1'][:12], '; upstream clone matches' if upstream_clone else ''))
    if run_gates:
        executed = run_commands(root, row['consumer_tests']['commands'])
        if any(status != 'pass' for status in executed.values()):
            raise BackportError('consumer gate failed: %s' % executed)
    return results


def verify_clean_copy(root, sha, upstream_clone=None):
    """Verify committed HEAD extracted into a fresh directory: no working-tree state is used."""
    root = Path(root).resolve()
    head = _git(root, 'rev-parse', 'HEAD').decode().strip()
    dirty = _git(root, 'status', '--porcelain', '--untracked-files=no').decode().strip()
    with tempfile.TemporaryDirectory(prefix='backport-clean-copy-') as temporary:
        copy = Path(temporary) / 'repository'
        copy.mkdir()
        archive = _git(root, 'archive', '--format=tar', head)
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(copy, filter='data')
        commands = [[sys.executable, str(copy / 'tools/rust-sources/source_backport.py'), 'verify', '--receipt', sha,
                     '--root', str(copy), *(['--upstream-clone', str(upstream_clone)] if upstream_clone else [])],
                    [sys.executable, str(copy / 'tools/rust-sources/manage.py'), 'verify']]
        for command in commands:
            result = subprocess.run(command, cwd=copy, capture_output=True, text=True)
            sys.stdout.write(result.stdout)
            if result.returncode:
                raise BackportError('clean-copy command failed: %s\n%s' % (' '.join(command[1:3]), result.stderr))
    print('clean copy of HEAD %s verified receipt %s%s' % (
        head, sha, ' (uncommitted tracked changes were not part of the copy)' if dirty else ''))


# ----------------------------------------------------------------- report ---

def report(root, sha, output, upstream_clone=None):
    """Write the full reconstruction to an empty directory outside the repository."""
    root = Path(root).resolve()
    out = Path(output).resolve()
    if out.is_relative_to(root):
        raise BackportError('report output must be outside the repository')
    if out.exists() and any(out.iterdir()):
        raise BackportError('report output directory must be empty')
    catalog, _, row, before, after = chain_step(root, sha)
    merged = check_replay(row, before, after, catalog)
    if upstream_clone is not None:
        check_clone(row, upstream_clone)
    material = upstream_material(row['upstream'])

    def write(folder, name, data):
        target = out / folder / PurePosixPath(name)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    for path in merged:
        write('before', path, before[path])
        write('after', path, after[path])
        write('merged', path, merged[path])
    for entry in row['upstream']['files']:
        base, fixed = material[entry['upstream_path']]
        write('upstream-base', entry['upstream_path'], base)
        write('upstream-fixed', entry['upstream_path'], fixed)
        write('upstream-patch', entry['upstream_path'] + '.patch', unpack(entry['patch'], entry['upstream_path']))
    write_document(out / 'receipt.json', row)
    upstream = row['upstream']
    lines = ['# Backport replay %s' % sha, '',
             '- upstream: %s %s (%s) -> %s (%s, parent %s): %s' % (
                 upstream['name'], upstream['base']['sha1'], upstream['base']['ref'], upstream['fix']['sha1'],
                 upstream['fix']['ref'], upstream['fix']['parent'], upstream['fix']['subject']),
             '- upstream changes outside this backport: %s' % (', '.join(upstream['excluded']) or 'none'),
             '- parent receipt: %s; terminal consolidation: %s' % (row['parent'], row['terminal']),
             '- advisory: %s' % json.dumps(row['advisory'], sort_keys=True),
             '- identifier map: %s' % json.dumps(row['mapping']['identifiers'], sort_keys=True),
             '- evidence receipts: %s' % ', '.join(row['mapping']['evidence']), '', '## Files', '']
    for item in row['replay']['merged']:
        lines.append('- `%s` <- `%s` (%s): base %s, ours %s, theirs %s -> result %s; hunks %d, conflicts %d, resolutions %d'
                     % (item['local_path'], item['upstream_path'], item['region'], item['base_sha256'][:12],
                        item['ours_sha256'][:12], item['theirs_sha256'][:12], item['result_sha256'][:12],
                        item['hunks_applied'], item['hunks_conflicted'], len(item['resolutions'])))
    lines += ['', '## Directories', '',
              '- `before/`: owner bytes before this step, inverted from live files through every later step',
              '- `after/`: owner bytes after this step', '- `merged/`: replayed merge output; equal to `after/`',
              '- `upstream-base/`, `upstream-fixed/`, `upstream-patch/`: upstream material decoded from the receipt', '',
              '## Reproduce', '', '```sh',
              'python3 tools/rust-sources/source_backport.py verify --receipt %s' % sha,
              'python3 tools/rust-sources/source_backport.py verify --receipt %s --clean-copy' % sha, '```', '',
              '## Consumer gates recorded in the receipt (informational)', '', '```json',
              json.dumps(row['consumer_tests'], indent=2, sort_keys=True), '```', '']
    (out / 'REPLAY.md').write_text('\n'.join(lines))
    print('report written to %s' % out)


# ------------------------------------------------------------------- CLI ----

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('plan')
    p.add_argument('--request', required=True)
    p.add_argument('--out', required=True)
    a = sub.add_parser('apply')
    a.add_argument('--plan', required=True)
    a.add_argument('--out', required=True)
    a.add_argument('--run-gates', action='store_true')
    v = sub.add_parser('verify')
    v.add_argument('--receipt', required=True)
    v.add_argument('--upstream-clone')
    v.add_argument('--clean-copy', action='store_true')
    v.add_argument('--run-gates', action='store_true')
    r = sub.add_parser('report')
    r.add_argument('--receipt', required=True)
    r.add_argument('--output', required=True)
    r.add_argument('--upstream-clone')
    for item in (p, a, v, r):
        item.add_argument('--root', default=str(Path(__file__).resolve().parents[2]))
    args = parser.parse_args(argv)
    try:
        if args.command == 'plan':
            plan(args.root, load_json(args.request), args.out)
            print('plan written to %s/plan.json (review review.diff before apply)' % args.out)
        elif args.command == 'apply':
            apply(args.root, args.plan, args.out, args.run_gates)
        elif args.command == 'verify' and args.clean_copy:
            if args.run_gates:
                raise BackportError('--run-gates is not combined with --clean-copy')
            verify_clean_copy(args.root, args.receipt, args.upstream_clone)
        elif args.command == 'verify':
            verify(args.root, args.receipt, args.upstream_clone, args.run_gates)
        else:
            report(args.root, args.receipt, args.output, args.upstream_clone)
    except Conflict as error:
        print('backport plan: %s; see conflict-report.json and CONFLICTS.md in %s' % (error, args.out), file=sys.stderr)
        return EXIT_CONFLICT
    except (ValueError, OSError) as error:
        print('backport failed: %s' % ''.join(traceback.format_exception_only(error)).rstrip(), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
