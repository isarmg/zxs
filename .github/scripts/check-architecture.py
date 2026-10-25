#!/usr/bin/env python3
"""Check declared first-party Cargo boundaries, without Cargo or network access.

All target/optional declarations are checked, not just the current host feature
set. tools/rust-sources/architecture.py indexes Foundation consumer declarations;
source and layout validation belongs to tools/rust-sources/manage.py. This gate
validates paths from first-party packages but does not traverse Foundation
internals or enforce their layering. Policies are reviewed inputs, never inferred
approvals.
"""
import argparse
import json
from pathlib import Path
import sys
import tomllib


class PolicyError(ValueError):
    """An actionable manifest or policy violation."""


def require(condition, message):
    if not condition:
        raise PolicyError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate policy key: {key}")
        result[key] = value
    return result


def local_path(root, base, value, context):
    require(isinstance(value, str) and bool(value), f"{context}: path must be a nonempty string")
    require(not Path(value).is_absolute(), f"{context}: absolute path is forbidden: {value}")
    path = (base / value).resolve()
    require(path.is_relative_to(root), f"{context}: path escapes repository: {value}")
    return path


def manifest(root, directory):
    path = directory / "Cargo.toml"
    require(path.resolve().is_relative_to(root), f"{path}: manifest escapes repository")
    require(path.is_file(), f"{path}: missing Cargo.toml")
    with path.open('rb') as stream:
        data = tomllib.load(stream)
    name = data.get('package', {}).get('name')
    require(isinstance(name, str) and bool(name), f"{path}: missing package.name")
    return name, data


def scoped_tables(data, location):
    for key, scope in [('dependencies', 'runtime'), ('build-dependencies', 'build'),
                       ('dev-dependencies', 'dev')]:
        table = data.get(key, {})
        require(isinstance(table, dict), f"{location}: invalid {key} table")
        yield scope, table, f"{location} [{key}]"


def dependency_tables(data, location):
    yield from scoped_tables(data, location)
    targets = data.get('target', {})
    require(isinstance(targets, dict), f"{location}: invalid target table")
    for target, tables in targets.items():
        require(isinstance(tables, dict), f"{location}: invalid target {target}")
        yield from scoped_tables(tables, f"{location} target.{target}")


def validate(root, policy_path=None):
    root = Path(root).resolve()
    policy_path = policy_path or root / '.github/architecture-policy.json'
    policy = json.loads(Path(policy_path).read_text(), object_pairs_hook=unique_object)
    require(isinstance(policy, dict) and set(policy) == {'version', 'owners'}, 'policy requires only version and owners')
    require(type(policy['version']) is int and policy['version'] == 1, 'unsupported policy version')
    owners = policy['owners']
    require(isinstance(owners, dict) and bool(owners), 'owners must be a nonempty object')
    foundation = root / 'src/foundation'
    paths = {}
    for name, rule in owners.items():
        require(isinstance(rule, dict) and set(rule) == {'path', 'runtime', 'build', 'dev'},
                f"{name}: rule requires path, runtime, build and dev scopes")
        path = local_path(root, root, rule['path'], f"policy {name}")
        require(not path.is_relative_to(foundation), f"{name}: foundation is outside first-party scope")
        require(path not in paths, f"duplicate owner path: {rule['path']}")
        paths[path] = name
        actual, _ = manifest(root, path)
        require(actual == name, f"policy {name}: path contains package {actual}")
        for scope in ('runtime', 'build', 'dev'):
            edges = rule[scope]
            require(isinstance(edges, list) and all(isinstance(edge, str) for edge in edges),
                    f"{name}.{scope}: expected list of package names")
            require(len(edges) == len(set(edges)), f"{name}.{scope}: duplicate edge")
            for edge in edges:
                require(edge in owners, f"{name}.{scope}: unknown rule owner {edge}")
                require(edge != name or (name == "storage-engine" and scope == "dev"),
                        f"{name}.{scope}: self dependency is forbidden")
    with (root / 'Cargo.toml').open('rb') as stream:
        root_manifest = tomllib.load(stream)
    require('package' not in root_manifest, 'root Cargo.toml must remain a virtual workspace without [package]')
    workspace = root_manifest['workspace']
    require(isinstance(workspace, dict), 'workspace must be a table')
    members = workspace.get('members', [])
    require(isinstance(members, list), 'workspace.members must be a list')
    discovered = {}
    for member in members:
        path = local_path(root, root, member, 'workspace member')
        # Explicit members make changes to the architectural inventory reviewable.
        require(not any(character in member for character in '*?['), 'workspace members must use explicit paths, not globs')
        if path.is_relative_to(foundation):
            continue
        name, data = manifest(root, path)
        require(path in paths and paths[path] == name, f"undeclared first-party member: {name} ({member})")
        require(name not in discovered, f"duplicate workspace member: {name}")
        discovered[name] = (path, data)
    require(set(discovered) == set(owners), f"stale rule owners absent from workspace: {sorted(set(owners) - set(discovered))}")
    inherited = workspace.get('dependencies', {})
    require(isinstance(inherited, dict), 'workspace.dependencies must be a table')
    graph = {name: set() for name in owners}
    count = 0
    for name, (directory, data) in discovered.items():
        for scope, table, context in dependency_tables(data, str(directory.relative_to(root) / 'Cargo.toml')):
            for alias, declaration in table.items():
                require(isinstance(declaration, (str, dict)), f"{context} {alias}: invalid dependency")
                spec = {'version': declaration} if isinstance(declaration, str) else declaration
                base = directory
                if 'workspace' in spec:
                    require(spec['workspace'] is True and alias in inherited, f"{context} {alias}: invalid workspace inheritance")
                    require(not set(spec).intersection({'path', 'package', 'git', 'version', 'registry'}),
                            f"{context} {alias}: inherited source cannot be overridden")
                    spec = inherited[alias]
                    spec = {'version': spec} if isinstance(spec, str) else spec
                    require(isinstance(spec, dict) and 'workspace' not in spec, f"{context} {alias}: invalid workspace dependency")
                    base = root
                package = spec.get('package', alias)
                require(isinstance(package, str) and bool(package), f"{context} {alias}: invalid package name")
                if 'path' not in spec:
                    require(package not in owners, f"{context} {alias}: first-party {package} requires its approved local path")
                    continue
                require('git' not in spec, f"{context} {alias}: dependency cannot have both path and git sources")
                target = local_path(root, base, spec['path'], f"{context} {alias}")
                actual, _ = manifest(root, target)
                require(actual == package, f"{context} {alias}: declared package {package}, path contains {actual}")
                if target.is_relative_to(foundation):
                    require(actual not in owners, f"{context} {alias}: first-party package cannot use foundation source")
                    continue
                require(target in paths and paths[target] == actual, f"{context} {alias}: undeclared first-party source {target}")
                require(actual in owners[name][scope], f"{context} {alias}: forbidden {scope} edge {name} -> {actual}")
                count += 1
                if scope != 'dev':
                    graph[name].add(actual)
    # Dev-only backedges (e.g. proc-macro integration tests) do not create a
    # production cycle. Build dependencies do, even under a target cfg/feature.
    visited, active = set(), []
    def visit(name):
        require(name not in active, 'runtime/build dependency cycle: ' + ' -> '.join(active + [name]))
        if name in visited:
            return
        active.append(name)
        for target in sorted(graph[name]):
            visit(target)
        active.pop()
        visited.add(name)
    for name in sorted(graph):
        visit(name)
    return len(owners), count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument('--policy', type=Path)
    args = parser.parse_args()
    try:
        owners, edges = validate(args.root, args.policy)
    except (PolicyError, OSError, ValueError, KeyError, TypeError) as error:
        print(f'architecture: {error}', file=sys.stderr)
        return 1
    print(f'architecture: {owners} first-party owners, {edges} declared edges checked')
    return 0


if __name__ == '__main__':
    sys.exit(main())
