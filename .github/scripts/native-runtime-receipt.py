#!/usr/bin/env python3
"""Record actual Debian 13 ELF runtime dependencies without copying libraries."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def run(*args):
    return subprocess.run(args, check=True, text=True, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE).stdout


def sha256(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def needed(dynamic):
    return sorted(set(re.findall(r'\(NEEDED\).*Shared library: \[([^\]\n]+)\]', dynamic)))


def resolved_libraries(output):
    result = {}
    for line in output.splitlines():
        line = line.strip()
        if not line:
            continue
        if re.fullmatch(r'linux-vdso\.so\.\d+ \(0x[0-9a-fA-F]+\)', line):
            continue  # Kernel-provided, not a filesystem library.
        match = re.fullmatch(r'([^\s]+) => (/[^\s]+) \(0x[0-9a-fA-F]+\)', line)
        if match:
            name, path = match.groups()
        else:
            match = re.fullmatch(r'(/[^\s]+) \(0x[0-9a-fA-F]+\)', line)
            if not match:
                raise ValueError(f'unresolved or unsupported ldd entry: {line}')
            path = match[1]
            name = Path(path).name
        if name in result:
            raise ValueError(f'duplicate runtime library: {name}')
        result[name] = path
    if not result:
        raise ValueError('no dynamic runtime libraries resolved')
    return result


def package_owner(path):
    # Debian usrmerge may record either /lib/... or /usr/lib/... in dpkg.
    resolved = path.resolve(strict=True)
    candidates = [str(path), str(resolved)]
    if str(resolved).startswith('/usr/lib/'):
        candidates.append(str(resolved)[4:])
    owners = set()
    for candidate in dict.fromkeys(candidates):
        query = subprocess.run(('dpkg-query', '-S', candidate), text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if query.returncode == 1:
            continue  # This spelling is not in dpkg; try its actual usrmerge identity.
        if query.returncode:
            raise ValueError(f'dpkg ownership query failed for {candidate}: {query.stderr.strip()}')
        diversions = {}
        for line in query.stdout.splitlines():
            diversion = re.fullmatch(
                r'diversion by ([a-z0-9][a-z0-9+.-]*(?::[a-z0-9]+)?) (from|to): (/[^\n]+)', line)
            if diversion:
                package, direction, target = diversion.groups()
                if ('..' in Path(target).parts or str(Path(target)) != target
                        or direction in diversions.setdefault(package, {})):
                    raise ValueError(f'invalid package diversion: {line}')
                diversions[package][direction] = target
                continue  # A diversion is metadata, never proof of file ownership.
            package, separator, owned_path = line.partition(': ')
            if not separator or owned_path != candidate or not re.fullmatch(r'[a-z0-9][a-z0-9+.-]*(?::[a-z0-9]+)?', package):
                raise ValueError(f'ambiguous package ownership: {line}')
            owners.add(package)
        for package, targets in diversions.items():
            if (set(targets) != {'from', 'to'} or candidate not in targets.values()
                    or targets['from'] == targets['to']):
                raise ValueError(f'unrelated or incomplete package diversion: {package}: {targets}')
    if len(owners) != 1:
        raise ValueError(f'expected one Debian owner for {path}: {sorted(owners)}')
    package = owners.pop()
    version = run('dpkg-query', '-W', '-f=${Version}', package).strip()
    if not version:
        raise ValueError(f'empty Debian version for {package}')
    return {'path': str(path), 'resolved_path': str(resolved), 'sha256': sha256(resolved),
            'package': package, 'version': version}


def binary_receipt(name, path):
    path = path.resolve(strict=True)
    before = sha256(path)
    dynamic = needed(run('readelf', '-d', str(path)))
    libraries = resolved_libraries(run('ldd', str(path)))
    if not set(dynamic).issubset(libraries):
        raise ValueError(f'NEEDED libraries missing from ldd: {sorted(set(dynamic) - libraries.keys())}')
    dependencies = [dict(soname=soname, **package_owner(Path(library)))
                    for soname, library in sorted(libraries.items())]
    if before != sha256(path):
        raise ValueError(f'binary changed during runtime inspection: {path}')
    return {'name': name, 'sha256': before, 'needed': dynamic, 'libraries': dependencies}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', action='append', required=True, help='logical-name=/absolute/path')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.unlink(missing_ok=True)  # Failed invocation cannot retain an old successful receipt.
    os_release = dict(line.split('=', 1) for line in Path('/etc/os-release').read_text().splitlines()
                      if '=' in line)
    if os_release.get('ID', '').strip('"') != 'debian' or os_release.get('VERSION_ID', '').strip('"') != '13':
        raise ValueError('runtime receipt requires a Debian 13 build environment')
    if run('dpkg', '--print-architecture').strip() != 'amd64':
        raise ValueError('runtime receipt requires Debian amd64')
    binaries = []
    names = set()
    for value in args.binary:
        name, separator, raw_path = value.partition('=')
        if not separator or not re.fullmatch(r'[a-z][a-z0-9-]*', name) or name in names or not Path(raw_path).is_absolute():
            raise ValueError('binary must have a unique logical name and absolute path')
        names.add(name)
        binaries.append(binary_receipt(name, Path(raw_path)))
    data = {'schema_version': 1, 'distribution': 'debian', 'release': '13',
            'architecture': 'amd64', 'binaries': binaries}
    args.output.write_text(json.dumps(data, indent=2, sort_keys=True) + '\n')


if __name__ == '__main__':
    main()
