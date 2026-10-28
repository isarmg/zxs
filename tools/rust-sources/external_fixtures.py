"""Explicit imported data fixtures, separate from package and local-test origins.

This is an exact, append-only registry of reviewed source receipts, not a general
permission to adopt arbitrary test files. Original fixture bytes and their legal
material stay pinned; a different dataset needs its own reviewed receipt.
"""
import hashlib
import json


RECEIPTS = {
    'test/naming/data/normalization-test-16.0.0.txt': (
        'compliance/fixtures/unicode-ucd-16.0.0-normalization.json',
        '765684aaae2e070d29f1fd5d4639147b8d99592acbac8b026ff381854a69bd25',
    ),
}


def claims(entry):
    """Do not let a registered external fixture masquerade as a local test."""
    return (entry.get('path') in RECEIPTS
            or entry.get('source_kind') == 'upstream-fixture'
            or 'source_receipt' in entry)


def permits(root, owner, entry, origin):
    from external_tests import safe
    registration = RECEIPTS.get(entry.get('path'))
    if root is None or registration is None:
        return False
    receipt_path, receipt_digest = registration
    if entry.get('source_receipt') != receipt_path:
        return False
    content = safe(root, receipt_path).read_bytes()
    if hashlib.sha256(content).hexdigest() != receipt_digest:
        raise ValueError(f'imported fixture receipt changed: {receipt_path}')
    receipt = json.loads(content)
    expected = {
        'path': receipt['path'],
        'source': receipt['source'],
        'source_kind': 'upstream-fixture',
        'source_receipt': receipt_path,
        'source_sha256_before': receipt['sha256'],
        'sha256': receipt['sha256'],
    }
    if (entry != expected
            or owner['directory'] != receipt['directory']
            or origin != receipt['owner_origin']
            or not any(entry['path'].startswith(p + '/') for p in owner['roots'])):
        return False
    license_path = receipt['license_path']
    license_bytes = safe(root, license_path).read_bytes()
    if hashlib.sha256(license_bytes).hexdigest() != receipt['license_sha256']:
        raise ValueError(f'imported fixture license changed: {license_path}')
    return True
