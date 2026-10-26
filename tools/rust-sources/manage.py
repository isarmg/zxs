#!/usr/bin/env python3
"""Verify the single-workspace source tree and immutable upstream provenance."""
import argparse
from collections import deque
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import stat
import sys
import tempfile
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent))
import external_tests
import source_naming
import source_moves
import proof_routing
import proof_maintenance
import proof_legacy_maintenance
import source_consolidation
import external_transfers
import source_relocation
import benchmark_relocation
import dependency_edges
import source_trimming
import source_layout
import source_layout_corrections
import source_imports

ROOT = Path(__file__).resolve().parents[2]
CATALOG = "compliance/rust-sources.json"
ORIGINS = "compliance/provenance/origins.json"
DOC = "docs/THIRD_PARTY_RUST_SOURCES.md"
POLICY = "tools/rust-sources/workspaces.json"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
NATIVE_TARGET = "x86_64-unknown-linux-gnu"
WEB_TARGET = "wasm32-unknown-unknown"
HEX = re.compile(r"[0-9a-f]{64}")
DESCRIPTION_PATHS = (
    ("tools/zxs-bindgen/PROVENANCE.json", "compliance/provenance/packages/zxs-bindgen-0.2.129/PROVENANCE.json"),
    ("tools/zxs-bindgen", "tools/web-bindings"),
    ("crates/zxs-naming/README.md", "docs/components/naming.md"),
    ("crates/zxs-naming", "src/naming"),
    ("vendor/web/zxs-web-core", "src/interface"),
    ("vendor/web/zxs-web-macro", "src/macros"),
    ("vendor/web/dependencies/patches.json", "compliance/provenance/shared/vendor/web/dependencies/patches.json"),
    ("vendor/web/README.md", "docs/components/interface.md"),
    ("web/runtime", "src/interface"),
    ("web/dist", "build/web"),
    ("web/src/", "src/web/src/"),
)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def atomic_write(path, data):
    """Replace one receipt only after its complete bytes have been written."""
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix='.' + path.name + '.', delete=False) as stream:
        temporary = Path(stream.name)
        try:
            os.fchmod(stream.fileno(), stat.S_IMODE(path.stat().st_mode))
            stream.write(data)
            stream.close()
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def files(directory):
    """Inventory regular files only; never follow symlinks or read special files."""
    if directory.is_symlink():
        raise ValueError(f"source directory symlink: {directory}")
    if not directory.is_dir():
        raise ValueError(f"missing source directory: {directory}")
    result = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"source symlink: {path}")
        if path.is_file():
            result[path.relative_to(directory).as_posix()] = digest(path.read_bytes())
        elif not path.is_dir():
            raise ValueError(f"source special file: {path}")
    return result


def safe_path(root, relative):
    if (not isinstance(relative, str) or not relative or "\\" in relative
            or any(part in ("", ".", "..") or ":" in part for part in relative.split("/"))):
        raise ValueError(f"unsafe inventory path: {relative}")
    path = Path(relative)
    if path.is_absolute():
        raise ValueError(f"unsafe inventory path: {relative}")
    target = root / path
    if not target.resolve().is_relative_to(root.resolve()):
        raise ValueError(f"inventory escapes repository: {relative}")
    current = root
    for part in path.parts:
        current /= part
        if current.is_symlink():
            raise ValueError(f"inventory path symlink: {relative}")
    return target


def project_manifests(root):
    result = {"Cargo.toml"}
    for base in ("src", "tools", "examples", "test"):
        for path in (root / base).rglob("Cargo.toml"):
            result.add(path.relative_to(root).as_posix())
    return result


def manifests(root):
    return {path for path in project_manifests(root)
            if "workspace" in tomllib.loads(safe_path(root, path).read_text(encoding="utf-8"))}


def structure(root, policy):
    """No nested Git roots, Cargo workspaces, registry edges, or package locks."""
    if [workspace["manifest"] for workspace in policy["workspaces"]] != ["Cargo.toml"]:
        raise ValueError("exactly one root Cargo workspace is required")
    if (root / "test").is_symlink():
        raise ValueError("test source root symlink")
    verify_target_policy(policy)
    for profile in policy.get("profiles", []):
        profile_features(root, profile)
    if any(path.startswith("test/") for path in project_manifests(root)):
        raise ValueError("Cargo package manifests are forbidden beneath test/")
    if manifests(root) != {"Cargo.toml"}:
        raise ValueError("nested/independent Cargo workspace")
    skipped = {".git", "target", "build", "runtime", "zxs-data", "xcs-data", ".agents", ".aws", ".codex"}
    for directory, children, names in os.walk(root, followlinks=False):
        relative = Path(directory).relative_to(root)
        if relative == Path("."):
            children[:] = [name for name in children if name not in skipped]
        if relative != Path(".") and "Cargo.toml" in names:
            # Existing generated example distributions and Cargo caches are
            # outside the maintained source boundary. A cc/src/target source
            # directory is not a package root and remains fully checked.
            children[:] = [name for name in children if name not in {"target", "dist", ".dist-staging", ".dist-backup"}]
        if relative != Path(".") and (".git" in children or ".git" in names):
            raise ValueError(f"nested Git repository: {relative}")
        if "Cargo.lock" in names and relative != Path("."):
            raise ValueError(f"nested Cargo.lock: {relative}")
        if relative.parts and relative.parts[0] == "test":
            for name in children + names:
                if (root / relative / name).is_symlink():
                    raise ValueError(f"test source symlink: {relative / name}")
        if relative.parts and relative.parts[0] in ("src", "tools", "test", "tests", "deploy", "examples"):
            for name in names:
                if Path(name).suffix.lower() in (".md", ".rst", ".adoc"):
                    raise ValueError(f"scattered documentation outside docs/: {relative / name}")
                if re.fullmatch(r"(?:LICEN[CS]E|COPYING|COPYRIGHT|NOTICES?|AUTHORS)(?:[._-].*)?", name, re.IGNORECASE):
                    raise ValueError(f"scattered legal material outside compliance/: {relative / name}")
        if (root / relative / "Cargo.toml").is_file() and relative != Path("."):
            scaffold = set(names) & {".gitignore", ".gitattributes", ".editorconfig", ".rustfmt.toml", "rustfmt.toml", "clippy.toml", ".clippy.toml", "Makefile.toml", "justfile", "flake.nix", "flake.lock", ".pre-commit-config.yaml", "deny.toml"}
            scaffold.update(set(children) & {".github", ".gitlab", ".vscode", ".idea", ".cargo", "LICENSES"})
            if scaffold:
                raise ValueError(f"nested package development/legal scaffolding: {relative} {sorted(scaffold)}")
    if (root / ".gitmodules").exists():
        raise ValueError("Git submodules are not permitted in the monorepo")
    config = tomllib.loads((root / ".cargo/config.toml").read_text(encoding="utf-8"))
    if config.get("net", {}).get("offline") is not True:
        raise ValueError("Cargo must default to offline operation")
    sources = config.get("source", {})
    replacement = sources.get("crates-io", {}).get("replace-with")
    directory = sources.get(replacement, {}).get("directory")
    if not directory:
        raise ValueError("missing empty registry fallback")
    registry = safe_path(root, directory)
    if not registry.is_dir() or any(path.is_dir() or path.is_symlink() for path in registry.iterdir()):
        raise ValueError("registry fallback must contain no source packages or links")
    root_manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    workspace = root_manifest["workspace"]
    verification_members = policy.get("verification_members", [])
    if ((verification_members and not workspace.get("default-members"))
            or len(set(verification_members)) != len(verification_members)
            or set(verification_members) != {
                path for path in workspace.get("members", []) if path.startswith("src/foundation/")}
            or any(not path.startswith("src/foundation/") or path not in workspace.get("members", [])
                   or path in workspace.get("default-members", []) for path in verification_members)):
        raise ValueError("foundation verification members must be explicit non-default root members")
    for relative in project_manifests(root):
        manifest_path = safe_path(root, relative)
        manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        sections = [manifest, *manifest.get("target", {}).values()]
        if relative == "Cargo.toml":
            sections.append(manifest["workspace"])
        for section in sections:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for name, dependency in section.get(kind, {}).items():
                    dependency, base = dependency_edges.resolve(root, relative, manifest, workspace, name, dependency)
                    if (not isinstance(dependency, dict) or "path" not in dependency
                            or any(key in dependency for key in ("git", "registry", "branch", "tag", "rev"))):
                        raise ValueError(f"non-path dependency: {relative} {name}")
                    path = base / dependency["path"]
                    if not path.resolve().is_relative_to(root.resolve()):
                        raise ValueError(f"external dependency path: {relative} {name}")
                    normalized = Path(os.path.normpath(path))
                    target = safe_path(root, normalized.relative_to(root).as_posix())
                    if not (target / "Cargo.toml").is_file():
                        raise ValueError(f"missing local dependency: {relative} {name}")
    verify_feature_edges(root)


def verify_target_policy(policy):
    """The maintained native platform is Linux amd64; WASM is Web-only."""
    workspace_targets = policy["workspaces"][0]["targets"]
    if (not workspace_targets or len(set(workspace_targets)) != len(workspace_targets)
            or set(workspace_targets) - {NATIVE_TARGET, WEB_TARGET}):
        raise ValueError("unsupported workspace target: only Linux amd64 and Web WASM are maintained")
    selected = set()
    for profile in policy.get("profiles", []):
        expected = WEB_TARGET if profile["id"] == "web" else NATIVE_TARGET
        if profile["targets"] != [expected]:
            raise ValueError(f"unsupported profile target: {profile['id']} requires {expected}")
        selected.add(expected)
    if policy.get("profiles") and selected != set(workspace_targets):
        raise ValueError("workspace/profile target coverage drift")


def verify_feature_edges(root):
    """Validate dormant feature edges too, not just Cargo's selected features."""
    documents = {relative: tomllib.loads(safe_path(root, relative).read_text(encoding="utf-8"))
                 for relative in project_manifests(root)}
    workspace = documents["Cargo.toml"]["workspace"]
    dependencies = {}
    for relative, manifest in documents.items():
        edges = {}
        sections = [manifest, *manifest.get("target", {}).values()]
        if relative == "Cargo.toml":
            sections.append(manifest["workspace"])
        for section in sections:
            for kind in ("dependencies", "build-dependencies", "dev-dependencies"):
                for alias, dependency in section.get(kind, {}).items():
                    dependency, base = dependency_edges.resolve(root, relative, manifest, workspace, alias, dependency)
                    target = (base / dependency["path"] / "Cargo.toml").resolve().relative_to(root.resolve()).as_posix()
                    edges.setdefault(alias, []).append((dependency, target))
        dependencies[relative] = edges
    features = {}
    for relative, manifest in documents.items():
        declared = manifest.get("features", {})
        hidden = {value[4:] for values in declared.values() for value in values if value.startswith("dep:")}
        features[relative] = set(declared) | {alias for alias, edges in dependencies[relative].items()
                                            if alias not in hidden and any(edge.get("optional") for edge, _ in edges)}
    for relative, manifest in documents.items():
        edges = dependencies[relative]
        for name, values in manifest.get("features", {}).items():
            for value in values:
                if value.startswith("dep:"):
                    alias = value[4:]
                    valid = alias in edges and any(edge.get("optional") for edge, _ in edges[alias])
                elif "/" in value:
                    alias, feature = value.split("/", 1)
                    weak = alias.endswith("?")
                    alias = alias.removesuffix("?")
                    valid = (alias in edges and all(feature in features[target] for _, target in edges[alias])
                             and (not weak or any(edge.get("optional") for edge, _ in edges[alias])))
                else:
                    valid = value in features[relative]
                if not valid:
                    raise ValueError(f"dangling feature edge: {relative} {name} -> {value}")
        for alias, targets in edges.items():
            for dependency, target in targets:
                for feature in dependency.get("features", []):
                    if feature not in features[target]:
                        raise ValueError(f"dangling dependency feature: {relative} {alias}/{feature}")


def profile_features(root, profile):
    """Only explicit, tested capabilities may extend a validation closure.

    Feature selection is qualified by a selected workspace member, never by an
    arbitrary dependency or Cargo's all-features switch. Production profiles
    continue to resolve with their existing default workspace feature graph.
    """
    values = profile.get("features", [])
    if (not isinstance(values, list)
            or any(not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_-]+/[A-Za-z0-9_-]+", value)
                   for value in values)
            or len(values) != len(set(values))):
        raise ValueError(f"invalid explicit profile features: {profile['id']}")
    if not values:
        return ()
    if profile.get("production") is not False or profile.get("role") != "compile":
        raise ValueError("explicit capability features require a non-production compile profile")
    paths = profile.get("manifests", [])
    if (not isinstance(paths, list) or not paths or any(not isinstance(path, str) for path in paths)
            or len(paths) != len(set(paths)) or profile.get("packages")):
        raise ValueError("explicit capability features require selected workspace manifests")
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    declared = {}
    for relative in paths:
        path = safe_path(root, relative)
        if path.name != "Cargo.toml" or Path(relative).parent.as_posix() not in workspace["members"]:
            raise ValueError(f"capability root is not an explicit workspace member: {relative}")
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
        declared[manifest["package"]["name"]] = manifest.get("features", {})
    for value in values:
        package, feature = value.split("/")
        if feature not in declared.get(package, {}):
            raise ValueError(f"capability feature is not declared by a selected root: {value}")
    return tuple(sorted(values))


def cargo_metadata(root, manifest="Cargo.toml", target=None, cargo_home=None, features=()):
    if manifest != "Cargo.toml":
        raise ValueError("metadata must resolve the single root workspace")
    command = ["cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--manifest-path", str(root / manifest)]
    if target:
        command += ["--filter-platform", target]
    if features:
        command += ["--features", ",".join(features)]
    environment = os.environ.copy()
    if cargo_home:
        environment["CARGO_HOME"] = str(cargo_home)
    result = subprocess.run(command, cwd=root, env=environment, capture_output=True, text=True, encoding="utf-8")
    if result.returncode:
        raise ValueError(f"offline metadata failed: {manifest} {target or 'all targets'}\n{result.stderr}")
    return json.loads(result.stdout)


def closure(metadata, initial_role="runtime", roots=None):
    """Traverse a conservative workspace-feature graph; skip direct dev edges.

    Cargo unifies workspace features before roots are selected below. Another
    member or dev dependency can therefore activate normal/build edges here.
    Roles describe this superset, not an isolated consumer or linked artifact.
    """
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    if initial_role not in ("runtime", "compile"):
        raise ValueError(f"invalid closure role: {initial_role}")
    queue = deque((package, initial_role) for package in (roots if roots is not None else metadata["workspace_members"]))
    roles = {}
    while queue:
        key, role = queue.popleft()
        if any("proc-macro" in target["kind"] for target in packages[key]["targets"]):
            role = "compile"
        if role in roles.setdefault(key, set()):
            continue
        roles[key].add(role)
        for dependency in nodes[key]["deps"]:
            for kind in dependency["dep_kinds"]:
                if kind["kind"] != "dev":
                    queue.append((dependency["pkg"], "compile" if role == "compile" or kind["kind"] == "build" else "runtime"))
    return {key: dict(roles=sorted(value), features=sorted(nodes[key]["features"])) for key, value in roles.items()}


def workspace_role(manifest):
    return "compile" if manifest.startswith("tools/") else "runtime"


def entries(policy):
    profiles = policy.get("profiles")
    if not profiles:
        profiles = [dict(id="workspace", targets=policy["workspaces"][0]["targets"], role="runtime")]
    names = set()
    for profile in profiles:
        if profile["id"] in names:
            raise ValueError("duplicate source closure profile")
        names.add(profile["id"])
        for target in profile["targets"]:
            yield dict(profile, manifest="Cargo.toml", target=target)


def selected_sources(root, metadata, initial_role="runtime", extracted_sources=(), profile=None):
    roots = None
    if profile and (profile.get("packages") or profile.get("manifests")):
        names, paths = set(profile.get("packages", [])), set(profile.get("manifests", []))
        roots = [package["id"] for package in metadata["packages"]
                 if package["id"] in metadata["workspace_members"]
                 and (package["name"] in names or Path(package["manifest_path"]).relative_to(root).as_posix() in paths)]
        found_names = {package["name"] for package in metadata["packages"] if package["id"] in roots}
        found_paths = {Path(package["manifest_path"]).relative_to(root).as_posix() for package in metadata["packages"] if package["id"] in roots}
        if names - found_names or paths - found_paths:
            raise ValueError(f"profile roots missing from root workspace: {profile['id']}")
    used = closure(metadata, initial_role, roots)
    selected = {}
    for package in metadata["packages"]:
        relative = Path(package["manifest_path"]).parent.relative_to(root).as_posix()
        if package["id"] in used and (relative.startswith("src/foundation/") or relative in extracted_sources):
            selected[relative] = used[package["id"]]
    return dict(sorted(selected.items()))


def verify_source_paths(root, metadata):
    for package in metadata["packages"]:
        manifest = Path(package["manifest_path"])
        if not manifest.resolve().is_relative_to(root.resolve()):
            raise ValueError(f"external Rust source: {package['id']} {manifest}")
        safe_path(root, manifest.relative_to(root).as_posix())
        if package.get("source") is not None:
            raise ValueError(f"registry/Git Rust source is forbidden: {package['id']}")


def source_directories(root, policy):
    result = set(policy.get("integrated_sources", ["src/macros", "tools/web-bindings"]))
    foundation = root / "src/foundation"
    layout = source_layout.load(root)
    if foundation.exists():
        categories = set(layout.receipt['categories']) if layout else None
        children = list(foundation.iterdir())
        if categories is not None and {p.name for p in children} != categories:
            raise ValueError('foundation category inventory drift')
        for entry in children:
            if entry.is_symlink() or not entry.is_dir():
                raise ValueError(f"invalid foundation source directory: {entry}")
            packages = list(entry.iterdir()) if categories is not None else [entry]
            for path in packages:
                if path.is_symlink() or not path.is_dir() or not (path / "Cargo.toml").is_file():
                    raise ValueError(f"invalid foundation source directory: {path}")
                result.add(path.relative_to(root).as_posix())
        if layout and not {p for p in result if p.startswith('src/foundation/')} <= set(layout.reverse) | set(layout.imported_owners):
            raise ValueError('foundation grouped source inventory drift')
    return result


def upstream_identity(upstream):
    name, version, checksum = (upstream[key] for key in ("name", "version", "archive_sha256"))
    if (not re.fullmatch(r"[A-Za-z0-9_-]+", name)
            or not re.fullmatch(r"[0-9][0-9A-Za-z.+-]*", version) or not HEX.fullmatch(checksum)
            or upstream["archive_url"] != f"https://static.crates.io/crates/{name}/{name}-{version}.crate"):
        raise ValueError(f"invalid upstream archive identity: {name}@{version}")
    return name, version, checksum


def origins(root):
    records = read_json(root / ORIGINS)["packages"]
    result = {}
    for record in records:
        key = record["origin_directory"]
        if key in result:
            raise ValueError(f"duplicate historical origin: {key}")
        upstream_identity(record["upstream"])
        if not record.get("original_files_sha256") or any(not HEX.fullmatch(value) for value in record["original_files_sha256"].values()):
            raise ValueError(f"missing/invalid original file receipts: {key}")
        for material in (*record.get("central_license_materials", []), *record.get("central_original_metadata", []), *record.get("central_original_documentation", [])):
            if not safe_path(root, material).is_file():
                raise ValueError(f"missing centralized origin material: {material}")
        result[key] = record
    return result


def origin_hashes(records):
    return {key: digest(json.dumps(record, sort_keys=True, ensure_ascii=False).encode()) for key, record in sorted(records.items())}


def match_origin(package, records):
    key = package.get("origin_directory")
    if key is not None:
        record = records.get(key)
        if record is None or record["upstream"] != package["upstream"]:
            raise ValueError(f"upstream identity drift: {key}")
        return record
    identity = upstream_identity(package["upstream"])
    matches = [record for record in records.values() if upstream_identity(record["upstream"]) == identity]
    if len(matches) != 1:
        raise ValueError(f"ambiguous/missing historical origin: {package['directory']}")
    return matches[0]


def provenance(root):
    bases = ["compliance/provenance", "compliance/licenses"]
    if (root / "docs/upstream").exists():
        bases.append("docs/upstream")
    result = {f"{base}/{path}": value for base in bases for path, value in files(root / base).items()}
    migration = root / "compliance/provenance/document-migration.json"
    if migration.exists():
        for record in read_json(migration)["files"]:
            central = record["central_path"]
            if central.startswith(("compliance/", "docs/upstream/")):
                if digest(safe_path(root, central).read_bytes()) != record["sha256"]:
                    raise ValueError(f"original archive material checksum drift: {central}")
    return result


def current_description(text):
    """Migrate path tokens once, without matching inside an already moved path."""
    # Repair descriptions produced by the old unbounded replacement. This is
    # descriptive metadata only, never the immutable upstream file inventory.
    text = re.sub(r"(?:compliance/provenance/shared/){2,}(?=vendor/web/dependencies/patches\.json)",
                  "compliance/provenance/shared/", text)
    for before, after in DESCRIPTION_PATHS:
        text = re.sub(description_path_pattern(before), lambda _: after, text)
    return text


def description_path_pattern(path):
    end = "" if path.endswith("/") else r"(?![A-Za-z0-9_-]|\.[A-Za-z0-9])"
    return rf"(?<![A-Za-z0-9_./-]){re.escape(path)}{end}"


def verify_descriptions(root, package):
    for key in ("changes", "merged_into"):
        text = package.get(key, "")
        if current_description(text) != text:
            raise ValueError(f"outdated source description: {package['directory']} {key}")
        # Build output need not exist in a clean checkout. Migrated references
        # to retained source, documentation and provenance must still resolve.
        for target in {after for _, after in DESCRIPTION_PATHS if after != "build/web"}:
            if re.search(description_path_pattern(target), text):
                if not safe_path(root, target.rstrip("/")).exists():
                    raise ValueError(f"missing source description path: {target}")


def source_owners(packages, merges):
    """Keep the primary package's test/source ownership for overlapping merges.

    A merged identity shares the destination tree's full current receipt, but
    must not replace that tree's primary upstream attribution. Merge-only trees
    still provide their own owner. Advisory and origin coverage continue to use
    every merge separately rather than this directory-keyed accounting view.
    """
    return dict({merge["directory"]: merge for merge in merges}, **packages)


def resolutions(root, policy, merged_sources=()):
    result, metadata = [], {}
    with tempfile.TemporaryDirectory(prefix="zxs-empty-cargo-") as home:
        for entry in entries(policy):
            target = entry["target"]
            features = profile_features(root, entry)
            key = (target, features)
            if key not in metadata:
                metadata[key] = cargo_metadata(root, "Cargo.toml", target, home, features=features)
                verify_source_paths(root, metadata[key])
            selected = selected_sources(root, metadata[key], entry.get("role", "runtime"), source_directories(root, policy) | set(merged_sources), entry)
            resolution = dict(manifest="Cargo.toml", profile=entry["id"], target=target, packages=selected)
            if features:
                resolution["requested_features"] = list(features)
            result.append(resolution)
    return result


def retired_sources(root, catalog, records):
    """Retired implementations keep their last receipt, never an active path."""
    renames = source_naming.validate_history(root, catalog, check_manifests=False)
    retired = catalog.get("retired", {})
    if not isinstance(retired, dict):
        raise ValueError("invalid retired source inventory")
    for relative, package in retired.items():
        if (relative != package["directory"] or not relative.startswith("src/foundation/")
                or relative in catalog["packages"]):
            raise ValueError(f"invalid retired source identity: {relative}")
        path = safe_path(root, relative)
        if path.exists():
            raise ValueError(f"retired source has returned: {relative}")
        match_origin(package, records)
        retirement = package.get("retirement", {})
        if (not retirement.get("reason") or not retirement.get("category")
                or not retirement.get("targets") or not package.get("files_sha256")
                or retirement.get("file_count") != len(package["files_sha256"])
                or not isinstance(retirement.get("bytes"), int) or retirement["bytes"] < 0
                or any(not HEX.fullmatch(value) for value in package["files_sha256"].values())):
            raise ValueError(f"incomplete retired source receipt: {relative}")
        if "external_tests" in retirement:
            external_tests.validate_archived(root, retirement["external_tests"], package, removed=True, renames=renames)
    expected = catalog.get("retired_records_sha256", {})
    actual = origin_hashes(retired)
    if actual != expected:
        raise ValueError("immutable retired source receipt drift")
    return retired


def retirement_plan(root, previous, plan, records, policy):
    """Only an explicit, reviewed removal plan may retire registered sources."""
    retired = dict(retired_sources(root, previous, records))
    packages = dict(previous["packages"])
    if plan is None:
        return packages, retired
    if not isinstance(plan, dict) or plan.get("schema") != 1 or not isinstance(plan.get("packages"), list):
        raise ValueError("invalid explicit source retirement plan")
    targets = sorted({entry["target"] for entry in entries(policy)})
    seen = set()
    for item in plan["packages"]:
        relative = item["directory"]
        if relative in seen or relative in retired:
            raise ValueError(f"duplicate/already retired source: {relative}")
        seen.add(relative)
        before = packages.get(relative)
        if before is None or item.get("package") != before:
            raise ValueError(f"retirement plan does not match last source receipt: {relative}")
        if not relative.startswith("src/foundation/") or safe_path(root, relative).exists():
            raise ValueError(f"retirement requires a removed foundation source: {relative}")
        match_origin(before, records)
        package = dict(before)
        package["retirement"] = dict(reason=item["reason"], category=item["category"],
                                     bytes=item["bytes"], file_count=item["file_count"], targets=targets)
        external = external_tests.archive_removal(root, previous, item, before,
            source_naming.validate_history(root, previous, check_manifests=False))
        if external is not None:
            package["retirement"]["external_tests"] = external
        retired[relative] = package
        del packages[relative]
    return packages, retired


def upgrade_source_prefixes(root, item):
    prefixes = item.get("source_prefixes")
    if (not isinstance(prefixes, list) or not prefixes
            or any(not isinstance(prefix, str) or not prefix.endswith("/") for prefix in prefixes)
            or len(set(prefixes)) != len(prefixes)):
        raise ValueError("source upgrade requires explicit nonempty source_prefixes")
    for prefix in prefixes:
        safe_path(root, prefix[:-1])
        if any(prefix != other and prefix.startswith(other) for other in prefixes):
            raise ValueError("overlapping source upgrade prefixes")
    return tuple(prefixes)


@source_layout.historical_catalog
def superseded_sources(root, catalog, records, check_manifests=True):
    """Preserve prior source receipts and immutable same-path upgrade chains."""
    superseded = catalog.get("superseded", {})
    if (not isinstance(superseded, dict)
            or (superseded and catalog.get("schema", 0) < 4)
            or (catalog.get("schema", 0) >= 4
                and not {"superseded", "superseded_records_sha256"} <= catalog.keys())):
        raise ValueError("invalid superseded source inventory")
    if origin_hashes(superseded) != catalog.get("superseded_records_sha256", {}):
        raise ValueError("immutable superseded source receipt drift")
    successors = set()
    for origin, receipt in superseded.items():
        package = receipt["package"]
        relative = package["directory"]
        successor = receipt["successor_origin_directory"]
        if (origin != package["origin_directory"] or not relative.startswith("src/foundation/")
                or not isinstance(receipt.get("reason"), str) or not receipt["reason"].strip()
                or not package.get("files_sha256")
                or any(not HEX.fullmatch(value) for value in package["files_sha256"].values())):
            raise ValueError(f"incomplete superseded source receipt: {origin}")
        safe_path(root, relative)
        upgrade_source_prefixes(root, receipt)
        if "external_tests" in receipt:
            external_tests.validate_archived(root, receipt["external_tests"], package,
                renames=source_naming.validate_history(root, catalog, check_manifests=False))
        match_origin(package, records)
        record = records.get(successor)
        if (record is None or successor == origin or successor in successors
                or record["upstream"]["name"] != package["upstream"]["name"]
                or record["upstream"]["version"] == package["upstream"]["version"]):
            raise ValueError(f"invalid superseded source successor: {origin}")
        successors.add(successor)
        # Direct links never change on later upgrades. Follow the chain to the
        # current receipt, including an explicit eventual retirement.
        seen = {origin}
        while successor in superseded:
            if successor in seen:
                raise ValueError(f"cyclic superseded source history: {origin}")
            seen.add(successor)
            next_receipt = superseded[successor]
            if next_receipt["package"]["directory"] != relative:
                raise ValueError(f"superseded source path drift: {origin}")
            successor = next_receipt["successor_origin_directory"]
        layout = source_layout.load(root)
        lookup = layout.historical_path(relative) if layout else relative
        current = catalog["packages"].get(lookup) or catalog.get("retired", {}).get(relative)
        if current is None or current["origin_directory"] != successor:
            raise ValueError(f"superseded source has no current successor: {origin}")
        match_origin(current, records)
        if current["version"] != current["upstream"]["version"]:
            raise ValueError(f"upgraded source version drift: {relative}")
        if check_manifests and lookup in catalog["packages"]:
            manifest = tomllib.loads(source_layout.read(root, relative + "/Cargo.toml").decode("utf-8"))["package"]
            if manifest["name"] != current["name"] or manifest["version"] != current["version"]:
                raise ValueError(f"upgraded source manifest identity drift: {relative}")
    return superseded


def upgrade_plan(root, previous, plan, records):
    """Only an exact reviewed plan may change an active upstream identity."""
    packages = dict(previous["packages"])
    superseded = dict(superseded_sources(root, previous, records, check_manifests=False))
    if plan is None:
        return packages, superseded
    if not isinstance(plan, dict) or plan.get("schema") != 1 or not isinstance(plan.get("packages"), list):
        raise ValueError("invalid explicit source upgrade plan")
    seen = set()
    for item in plan["packages"]:
        relative = item["directory"]
        if relative in seen:
            raise ValueError(f"duplicate source upgrade: {relative}")
        seen.add(relative)
        before = packages.get(relative)
        if before is None or item.get("package") != before:
            raise ValueError(f"upgrade plan does not match last source receipt: {relative}")
        prior_origin = match_origin(before, records)["origin_directory"]
        successor = item["successor_origin_directory"]
        record = records.get(successor)
        if (not relative.startswith("src/foundation/") or prior_origin in superseded
                or record is None or successor in previous.get("origin_records_sha256", {})
                or record["upstream"]["name"] != before["upstream"]["name"]
                or record["upstream"]["version"] == before["upstream"]["version"]):
            raise ValueError(f"upgrade requires a new same-package origin: {relative}")
        if not isinstance(item.get("reason"), str) or not item["reason"].strip():
            raise ValueError(f"source upgrade requires a reason: {relative}")
        manifest = tomllib.loads(source_layout.read(root, relative + "/Cargo.toml").decode("utf-8"))["package"]
        if manifest["name"] != before["name"] or manifest["version"] != record["upstream"]["version"]:
            raise ValueError(f"upgrade manifest does not match successor identity: {relative}")
        prefixes = upgrade_source_prefixes(root, item)
        expected = {path: value for path, value in record["original_files_sha256"].items()
                    if path.startswith(prefixes)}
        actual = {path: value for path, value in files(safe_path(root, relative)).items()
                  if path.startswith(prefixes)}
        if (any(not any(path.startswith(prefix) for path in expected) for prefix in prefixes)
                or actual != expected):
            raise ValueError(f"upgraded source differs from successor release: {relative}")
        superseded[prior_origin] = dict(package=before, successor_origin_directory=successor,
                                       reason=item["reason"], source_prefixes=list(prefixes))
        external = external_tests.archive_removal(root, previous, item, before,
            source_naming.validate_history(root, previous, check_manifests=False))
        if external is not None:
            superseded[prior_origin]["external_tests"] = external
        package = dict(before, origin_directory=successor, upstream=record["upstream"])
        for key in ("vcs", "license", "repository"):
            package.pop(key, None)
            if key in record:
                package[key] = record[key]
        package["changes"] = before.get("changes", "") + "；上游发布包升级：" + item["reason"]
        packages[relative] = package
    return packages, superseded


def refresh(root, retire_plan=None, upgrade_plan_data=None, rename_plan_data=None, move_plan_data=None, consolidate_plan_data=None, relocate_benchmarks=False, trim_plan_data=None, layout_plan_data=None, external_path_plan_data=None, import_plan_id=None):
    original_origins = (root / ORIGINS).read_bytes() if import_plan_id is not None else None
    original_catalog = (root / CATALOG).read_bytes() if import_plan_id is not None else None
    try:
        previous_raw = (root / CATALOG).read_bytes()
        previous = json.loads(previous_raw)
        if import_plan_id is not None:
            if any(p is not None for p in (retire_plan, upgrade_plan_data, rename_plan_data, move_plan_data, consolidate_plan_data, trim_plan_data, layout_plan_data, external_path_plan_data)) or relocate_benchmarks:
                raise ValueError('binding text import requires a separate reviewed transition')
            previous = source_imports.prepare(root, previous, import_plan_id, previous_raw)
        if external_path_plan_data is not None:
            if any(p is not None for p in (retire_plan, upgrade_plan_data, rename_plan_data, move_plan_data, consolidate_plan_data, trim_plan_data, layout_plan_data)) or relocate_benchmarks:
                raise ValueError('external path correction requires a separate reviewed transition')
            layout, previous = source_layout_corrections.prepare(root, previous, external_path_plan_data, digest(previous_raw))
        elif layout_plan_data is not None:
            if any(p is not None for p in (retire_plan, upgrade_plan_data, rename_plan_data, move_plan_data, consolidate_plan_data, trim_plan_data)) or relocate_benchmarks:
                raise ValueError('layout requires a separate reviewed transition')
            layout, previous = source_layout.prepare(root, previous, layout_plan_data, digest(previous_raw))
        else:
            layout = source_layout.load(root, previous, pending_upgrade=upgrade_plan_data, prospective_maintenance=True)
        if layout is not None and layout_plan_data is None:
            layout.require_supported_plan(previous, retirement=retire_plan, moves=move_plan_data,
                                          rename=rename_plan_data, upgrade=upgrade_plan_data,
                                          consolidation=consolidate_plan_data, trim=trim_plan_data)
        with source_layout.activate(layout):
            return _refresh(root, retire_plan, upgrade_plan_data, rename_plan_data, move_plan_data,
                            consolidate_plan_data, relocate_benchmarks, trim_plan_data, previous)
    finally:
        # Failed import leaves the previous catalog and origins as one consistent
        # pair. Once the catalog advances, never roll back its committed origins.
        if original_origins is not None and (root / CATALOG).read_bytes() == original_catalog:
            original_failure = sys.exception()
            try:
                if (root / ORIGINS).read_bytes() != original_origins:
                    atomic_write(root / ORIGINS, original_origins)
            except OSError as recovery:
                raise OSError(
                    f"import failed ({original_failure}); restoring {ORIGINS} also failed: {recovery}; "
                    f"predecessor {CATALOG} is unchanged, but origins recovery is incomplete; "
                    "retain both records and inspect them before retrying"
                ) from recovery


def _refresh(root, retire_plan=None, upgrade_plan_data=None, rename_plan_data=None, move_plan_data=None, consolidate_plan_data=None, relocate_benchmarks=False, trim_plan_data=None, previous=None):
    policy = read_json(root / POLICY)
    structure(root, policy)
    previous = previous if previous is not None else read_json(root / CATALOG)
    if previous.get("initial_test_relocation_sha256") not in (None, source_relocation.RECEIPT_SHA256):
        raise ValueError("initial test relocation history identity drift")
    if relocate_benchmarks and any(p is not None for p in (retire_plan, upgrade_plan_data, rename_plan_data, move_plan_data, consolidate_plan_data, trim_plan_data)):
        raise ValueError('benchmark relocation requires a separate reviewed transition')
    trim_records, trim_grants = source_trimming.prepare(root, previous, trim_plan_data)
    benchmark_receipt, benchmark_grants = benchmark_relocation.prepare(root, previous, relocate_benchmarks, files)
    records = origins(root)
    hashes = origin_hashes(records)
    for key, expected in previous.get("origin_records_sha256", {}).items():
        if hashes.get(key) != expected:
            raise ValueError(f"immutable historical origin drift: {key}")
    material = provenance(root)
    for path, expected in previous.get("provenance_sha256", {}).items():
        # New imports may append original identities and document migration
        # entries; original per-package materials must retain their bytes.
        if path in (ORIGINS, "compliance/provenance/document-migration.json"):
            continue
        if material.get(path) != expected:
            raise ValueError(f"immutable original material drift: {path}")
    def plan_paths(plan):
        if not isinstance(plan, dict) or not isinstance(plan.get("packages"), list):
            return set()
        return {item.get("directory") for item in plan["packages"] if isinstance(item, dict)}
    trim_paths = plan_paths(trim_plan_data)
    lifecycle_paths = set().union(*(plan_paths(p) for p in (retire_plan, upgrade_plan_data, rename_plan_data, move_plan_data)))
    lifecycle_paths |= {row.get('destination_directory') for row in (rename_plan_data or {}).get('packages', [])}
    for row in (consolidate_plan_data or {}).get('packages', []):
        lifecycle_paths.update([row.get('primary_directory'), row.get('destination_directory')])
        lifecycle_paths.update(item.get('package', {}).get('directory') for item in row.get('inputs', []))
    if (trim_paths | source_trimming.owners(previous)) & lifecycle_paths:
        raise ValueError('trimmed owner lifecycle requires a separate chain-aware adapter')
    upgrade_paths = plan_paths(upgrade_plan_data)
    retirement_paths = plan_paths(retire_plan)
    move_paths = plan_paths(move_plan_data)
    if upgrade_paths & retirement_paths:
        raise ValueError("upgrade and retirement of one source require separate reviewed refreshes")
    consolidated, merged_inputs, consolidation_history, consolidated_now = source_consolidation.prepare(
        root, previous, consolidate_plan_data, files, validate_external=external_transfers.validate,
        authorize_pinned=proof_routing.authorize_consolidation)
    source_consolidation.validate_origins(consolidation_history, records)
    consolidation_paths = {path for row in consolidated_now.values()
                           for path in [row['destination_directory'], *[item['package']['directory'] for item in row['inputs']]]}
    rename_requested = plan_paths(rename_plan_data) | {row.get('destination_directory') for row in (rename_plan_data or {}).get('packages', [])}
    if consolidation_paths & (upgrade_paths | retirement_paths | move_paths | rename_requested):
        raise ValueError('consolidation and other lifecycle operations overlap; use separate reviewed transitions')
    historical_consolidated = {row['destination_directory'] for row in previous.get('consolidations', {}).values()}
    if historical_consolidated & (upgrade_paths | retirement_paths | move_paths | rename_requested):
        raise ValueError('consolidated owner lifecycle requires an explicit chain-aware adapter')
    consolidation_previous = dict(previous, packages=consolidated, merges=merged_inputs,
        schema=max(previous.get('schema', 0), 8 if consolidation_history else 0),
        consolidations=consolidation_history, consolidation_records_sha256=source_consolidation.hashes(consolidation_history),
        # Consumed rename/move proofs revalidate this transitional catalog.
        # Project only the exact external transitions authenticated by prepare;
        # the on-disk mapping is independently checked against them below.
        external_tests=source_consolidation.remap_previous_external(previous.get('external_tests', {}), consolidated_now))
    renamed, renames = source_naming.prepare(root, consolidation_previous, rename_plan_data, files)
    rename_paths = {p for old, row in renames.items() if old in previous['packages']
                    for p in (old, row['destination_directory'])}
    if rename_paths & (upgrade_paths | retirement_paths):
        raise ValueError('rename and retirement/upgrade require separate reviewed refreshes')
    if upgrade_paths & {row['destination_directory'] for row in renames.values()}:
        raise ValueError('upgrading renamed source requires chain-aware migration')
    if move_paths & (rename_paths | upgrade_paths | retirement_paths):
        raise ValueError('internal moves and rename/retirement/upgrade require separate reviewed refreshes')
    moved, move_history, moved_now = source_moves.prepare(root, dict(consolidation_previous, packages=renamed), move_plan_data, files)
    source_moves.validate_origins(move_history, records)
    transition = dict(consolidation_previous, schema=max(consolidation_previous.get("schema", 0), 6 if move_history else 5 if renames else 0), packages=moved, renames=renames,
                      rename_records_sha256=source_naming.hashes(renames))
    active, superseded = upgrade_plan(root, transition, upgrade_plan_data, records)
    active, retired = retirement_plan(root, dict(transition, packages=active), retire_plan, records, policy)
    actual = source_directories(root, policy)
    if actual != set(active):
        raise ValueError(f"unregistered source directories: {sorted(actual ^ set(active))}")
    packages = {}
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"].get("package", {})
    for relative, before in sorted(active.items()):
        directory = safe_path(root, relative)
        record = match_origin(before, records)
        package = dict(before)
        manifest = tomllib.loads((directory / "Cargo.toml").read_text(encoding="utf-8"))["package"]
        version = manifest["version"]
        if isinstance(version, dict) and version.get("workspace") is True:
            version = workspace["version"]
        package.update(name=manifest["name"], version=version, directory=relative,
                       origin_directory=record["origin_directory"], license_materials=record["central_license_materials"],
                       files_sha256=files(directory))
        for key in ("changes", "merged_into"):
            if key in package:
                package[key] = current_description(package[key])
        packages[relative] = package
    merges = []
    for before in merged_inputs:
        record = match_origin(before, records)
        merged = dict(before)
        merged.update(origin_directory=record["origin_directory"],
                      license_materials=record["central_license_materials"],
                      files_sha256=files(safe_path(root, merged["directory"])))
        for key in ("changes", "merged_into"):
            if key in merged:
                merged[key] = current_description(merged[key])
        merges.append(merged)
    external, external_receipts = external_tests.load(root, source_owners(packages, merges), renames, benchmark_relocation.combine_grants(external_transfers.grants(consolidation_history), benchmark_grants))
    benchmark_relocation.check_current_external(external, benchmark_receipt, relocate_benchmarks)
    source_naming.check_current_external(previous, external, renames)
    source_moves.check_current_external(external, moved_now)
    source_consolidation.check_current_external(external, consolidated_now)
    if external:
        if not external_tests.has_history(previous):
            baseline_packages = [*previous["packages"].values(), *previous.get("merges", [])]
            baseline = {item["directory"] + "/" + path: value
                        for item in baseline_packages for path, value in item["files_sha256"].items()}
            source_relocation.verify(root, external, safe_path, baseline)
        else:
            source_relocation.read_receipt(root, safe_path)
    elif external_tests.has_history(previous):
        source_relocation.read_receipt(root, safe_path)
    retained_external = external_tests.transition_previous(source_naming.remap_previous(source_consolidation.remap_previous_external(previous.get("external_tests", {}), consolidated_now), renames), retired, superseded, external)
    previous_owners = benchmark_relocation.remap_previous(source_owners(previous["packages"], previous.get("merges", [])), benchmark_receipt, relocate_benchmarks)
    external_tests.check_previous(
        retained_external, external, root,
        source_owners(packages, merges),
        source_moves.remap_previous(source_naming.remap_previous(source_consolidation.remap_previous_packages(previous_owners, consolidated_now), renames), moved_now))
    catalog = dict(source_trims=trim_records, source_trim_records_sha256=source_trimming.hashes(trim_records), external_tests=external, external_test_mappings_sha256=external_receipts, schema=max(previous.get("schema", 6), 8 if consolidation_history else 7), proof_routing_receipts=list(proof_routing.RECEIPTS), consolidations=consolidation_history, consolidation_records_sha256=source_consolidation.hashes(consolidation_history), source_moves=move_history, renames=renames, rename_records_sha256=source_naming.hashes(renames), policy_sha256=digest((root / POLICY).read_bytes()),
                   config_sha256=digest((root / ".cargo/config.toml").read_bytes()),
                   manifests_sha256={path: digest(safe_path(root, path).read_bytes()) for path in sorted(project_manifests(root))},
                   lockfiles_sha256={"Cargo.lock": digest((root / "Cargo.lock").read_bytes())},
                   origins_sha256=digest((root / ORIGINS).read_bytes()), origin_records_sha256=hashes,
                   provenance_sha256=material, packages=packages, resolutions=resolutions(root, policy, [merge["directory"] for merge in merges]),
                   upstream_identities=[record["upstream"] for record in records.values()], merges=merges,
                   retired=retired, retired_records_sha256=origin_hashes(retired),
                   superseded=superseded, superseded_records_sha256=origin_hashes(superseded))
    layout = source_layout.load(root)
    if layout is not None and layout.legacy_maintenance is not None:
        catalog[proof_legacy_maintenance.KEY] = list(proof_legacy_maintenance.RECEIPTS)
    if proof_maintenance.RECEIPTS:
        catalog[proof_maintenance.KEY] = list(proof_maintenance.RECEIPTS)
    if source_imports.KEY in previous:
        catalog[source_imports.KEY] = previous[source_imports.KEY]
    if source_layout.KEY in previous:
        catalog[source_layout.KEY] = previous[source_layout.KEY]
    if source_layout_corrections.KEY in previous:
        catalog[source_layout_corrections.KEY] = previous[source_layout_corrections.KEY]
    if external or external_tests.has_history(previous):
        catalog["initial_test_relocation_sha256"] = source_relocation.RECEIPT_SHA256
    if benchmark_receipt is not None:
        catalog[benchmark_relocation.KEY] = benchmark_relocation.RECEIPT
    if source_layout.KEY in catalog:
        source_layout.load(root).check_current_identity(catalog)
    source_trimming.check_append_only(previous, catalog)
    source_trimming.validate(root, catalog)
    benchmark_relocation.validate(root, catalog, files)
    if layout is not None and layout.legacy_maintenance is not None:
        layout.legacy_maintenance.validate_refresh(previous, catalog)
    proof_routing.validate_refresh(root, previous, catalog)
    source_consolidation.validate_origins(source_consolidation.validate_history(root, catalog, validate_external=external_transfers.validate), records)
    source_naming.validate_history(root, catalog)
    source_moves.validate_origins(source_moves.validate_history(root, catalog), records)
    retired_sources(root, catalog, records)
    superseded_sources(root, catalog, records)
    verify_origin_coverage(catalog, records)
    external_tests.check_removed(source_moves.remap_previous(source_naming.remap_previous(source_consolidation.remap_previous_packages(previous_owners, consolidated_now), renames), moved_now), source_owners(packages, merges), external, renames, trim_grants, root=root)
    verify_retained_closure(catalog, policy)
    catalog_bytes = (json.dumps(catalog, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8")
    atomic_write(root / CATALOG, catalog_bytes)
    source_layout.written_catalog(root, catalog_bytes)
    (root / DOC).write_text(document(catalog, policy), encoding="utf-8", newline="\n")
    print(f"Recorded {len(packages)} active source trees, {len(retired)} retired and {len(superseded)} superseded sources and {len(catalog['resolutions'])} root-workspace closure profiles.")


def cell(value):
    return str(value).replace("|", "\\|").replace("\n", " ")


def document(catalog, policy):
    lines = ["# Rust 第三方源码来源与使用清单", "",
             "本仓库只有根 Git 仓库、根 Cargo workspace 与根 Cargo.lock。活动 Rust 依赖全部通过仓库内 path 连接；基础实现位于 src/foundation，界面内核与命名规范化实现分别整合进 src/interface 与 src/naming。没有独立第三方工作区或独立锁文件。", "",
             "基础库按职责分类，目录边界见 [基础库分组](architecture/foundation-groups.md)。精确来源及原始逐文件摘要见 [原始来源清单](../compliance/provenance/origins.json)；当前源码逐文件摘要、单锁文件及各目标闭包见 [来源账本](../compliance/rust-sources.json)；法律材料统一见 [NOTICE](../compliance/NOTICE.md) 和 compliance/licenses。集中管理不改变各上游许可，也不把改造代码全部重新许可为根项目许可。", "",
             "本仓库中的第三方实现是经过本地定制、裁剪或合并的维护版本，并非未经修改的上游发布包。逐项修改说明保留在来源账本的 packages / merges 条目，当前文件摘要与原始摘要分别登记；以下仅列当前来源身份、位置、许可和使用边界，不重复维护过程记录。", "",
             "runtime 是统一工作区特性图中 normal 边可达的运行候选；compile 包括 build/proc-macro 子图及维护工具。Cargo 先统一整个 workspace 的 features，再从各登记入口遍历；直接 dev 边虽被跳过，其他成员或开发依赖启用的特性仍可能间接扩大 normal/build 可达集合。这是保守可达超集，不是精确单入口运行时闭包或最终链接证明，不能单独据此判定源码可删除。元数据也不替代实际浏览器/SMB 验收或安全审计。每个活动源码身份必须至少进入一个受支持入口/目标的保守超集，移除身份仍须显式退役并保留来源证据。工具链及既有 Samba/SQLite/系统 SDK 管理边界保持独立。", "",
             "修改后显式执行 `python3 tools/rust-sources/manage.py refresh`，审查源码、来源清单与文档差异，再执行 `python3 tools/rust-sources/manage.py verify --metadata`；verify 不接受新哈希。删除依赖必须另提供经复核的 `refresh --retire-plan PLAN.json`，保存已移除实现的最后源码摘要及原因；普通 refresh 拒绝意外丢失目录或改写退役记录。原始身份与法律材料继续保存，不因退役而删除。原始发布包重建工具只向仓库外独立复核目录输出，不覆盖活动源码。", "",
             "同目录上游升级必须追加新的原始发布包及集中材料，并使用经复核的 `refresh --upgrade-plan PLAN.json`。计划 schema 为 1，packages 每项包含 directory、与上次活动记录完全相同的 package、successor_origin_directory、reason 及非空 source_prefixes（例如 `[\"src/\"]`）。升级时逐文件校验这些完整前缀下的源码与新发布包完全一致，不能仅提升版本号；路径清单、缺失、新增和内容差异均拒绝。schema 4 的 superseded 以旧 origin_directory 为键，保留原 package、后继来源、原因和已校验前缀，superseded_records_sha256 固定其摘要。连续升级保留不可改写的直接后继链，最终指向同目录的当前身份或显式退役记录。升级后有意的本地源码修改仍需常规 refresh，不改写上游原始收据。", "",
             "## 单工作区入口与解析目标", "", "| 入口闭包 | 用途 | 目标 |", "| --- | --- | --- |"]
    if policy.get("verification_members"):
        lines[2] += (" 基础库验证成员 " + "、".join(policy["verification_members"])
                     + " 显式登记在同一个根 workspace，不属于 default-members；这是上游回归对象，不是新业务组件。"
                     + " 追加测试材料的精确来源单独保存于 compliance/provenance/supplemental，不改写原始发布包记录。")
    for profile in policy.get("profiles", policy["workspaces"]):
        purpose = profile.get("purpose", "根工作区")
        if profile.get("features"):
            purpose += "；非产品默认，显式验证特性：" + ", ".join(profile["features"])
        lines.append("| " + " | ".join(map(cell, [profile.get("id", "workspace"), purpose, ", ".join(profile["targets"])])) + " |")
    lines += ["", "## 当前源码来源与许可", "", "| 原始身份 | 当前组件 / 位置 | 许可材料 | 用途与闭包 |", "| --- | --- | --- | --- |"]
    for key, package in sorted(catalog["packages"].items()):
        upstream = package["upstream"]
        roles, consumers = set(), set()
        for resolution in catalog["resolutions"]:
            if key in resolution["packages"]:
                roles.update(resolution["packages"][key]["roles"])
                consumers.add(f"{resolution.get('profile', 'workspace')}@{resolution['target']}")
        use = "+".join(sorted(roles)) or "保留超集"
        legal = ", ".join(package.get("license_materials", [])) or "原始发布包未提供独立许可资料；查中央来源记录的待复核状态"
        origin = f"{upstream['name']}@{upstream['version']}；{upstream['archive_url']}；SHA256 {upstream['archive_sha256']}"
        lines.append("| " + " | ".join(map(cell, [origin, f"{package['name']}@{package['version']}；{key}", f"{package.get('license', '见原始声明')}；{legal}", f"{use}；{', '.join(sorted(consumers))}"])) + " |")
    lines += ["", "## 已合并的上游实现", "", "这些实现保留来源身份与当前文件摘要，已经没有独立 Cargo 包或独立工作区。", ""]
    for merged in catalog.get("merges", []):
        upstream = merged["upstream"]
        usage = [f"{item.get('profile', 'workspace')}@{item['target']} ({'+'.join(item['packages'][merged['directory']]['roles'])})"
                 for item in catalog["resolutions"] if merged["directory"] in item["packages"]]
        lines.append(f"- `{upstream['name']}@{upstream['version']}` → `{merged['directory']}`；实际闭包：{', '.join(usage) or '未选中'}；法律材料：{', '.join(merged.get('license_materials', []))}。")
    external = catalog.get("external_tests", {})
    external_count = sum(item["file_count"] for item in external.values())
    external_bytes = sum(item["bytes"] for item in external.values())
    # Several original identities may share one consolidated physical tree.
    production_paths = {item["directory"] + "/" + path
                        for item in (*catalog["packages"].values(), *catalog.get("merges", []))
                        for path in item["files_sha256"]}
    production_count = len(production_paths)
    lines += ["", "## 根测试目录的来源核算", "",
              f"包内当前文件 {production_count} 个，另有 {len(external)} 个归属目录登记的外置测试文件 {external_count} 个、{external_bytes} 字节；当前来源文件总计 {production_count + external_count} 个。外置测试同时包含迁移保留的上游测试和明确标记为 project-authored 的本地新写回归；目录分离不表示删除、裁剪或空间优化，材料仍完整计入来源覆盖、许可及安全审计。", "",
              "JSON external_tests 按源码归属登记精确根目录、初始路径、当前路径及逐文件摘要；项目新作记录首次本地路径与字节，不冒充上游抽取。external_test_mappings_sha256 固定当前映射文档。refresh 先验证外置清单，不能静默吞入新增文件或因目录迁移缩减来源核算。独立说明见 docs/EXTERNAL_TEST_SOURCES.md。", ""]
    lines += ["", "## 完整来源证据", "",
              "已替换或移除的来源不属于当前实现清单，也不参与当前依赖公告扫描；原始身份、法律材料和最后文件摘要仍必须保留。来源账本的 superseded / retired 保存这些证据，renames / source_moves / consolidations 保存名称、路径及合并的可核验关系。它们是完整性门禁的输入，不是当前功能或行为等价证明。当前活动及合并实现仍全部扫描，不设公告豁免；重新引入来源必须显式登记。", ""]
    if catalog.get(benchmark_relocation.KEY):
        lines += ["## 外置保留基准源码", "",
                  "保留基准源码位于 test，来源身份及文件摘要受独立凭据校验；存在源码不表示已启用或已运行基准目标。详见 [基准来源与维护边界](BENCHMARK_SOURCE_RELOCATION.md)。", ""]
    lines += ["", "## 完整性、安全与验收", "",
              f"共 {len(catalog['packages'])} 个活动源码树、{len(catalog.get('retired', {}))} 个已移除来源、{len(catalog.get('superseded', {}))} 个已升级替换来源，{len(catalog['resolutions'])} 个入口/目标解析组合。原始 archive SHA-256 和原始逐文件摘要独立保留；当前修改、裁剪及合并后的文件另存摘要，不再声称其逐字节等同上游发布包。合并记录见 JSON merges，法律复核状态见 compliance/NOTICE.md。", "",
              "`.cargo/config.toml` 默认离线，并用空 registry 兜底拒绝遗漏的 registry 边。CI 检查所有活动 manifest 的本地路径、全部特性转发、单锁文件、工作区/嵌套 Git 边界、中央原始材料及空 Cargo home 的目标闭包。保留源码必须至少进入一个登记入口的保守可达超集；该记录不证明每个入口实际编译或链接了这些源码。安全门禁扫描全部保留和合并实现的原始身份；已移除实现单列历史记录，本地路径或改名不形成豁免。安全扫描、构建、真实浏览器、真实 Samba SDK 和物理盘故障验证分别提供证据。", ""]
    return "\n".join(lines)


def verify_origin_coverage(catalog, records):
    covered = [package["origin_directory"] for package in catalog["packages"].values()]
    covered.extend(merge["origin_directory"] for merge in catalog.get("merges", []))
    covered.extend(package["origin_directory"] for package in catalog.get("retired", {}).values())
    covered.extend(receipt["package"]["origin_directory"] for receipt in catalog.get("superseded", {}).values())
    if len(set(covered)) != len(covered) or set(covered) != set(records):
        raise ValueError("historical active/merged/retired/superseded source coverage drift")


def verify_retained_closure(catalog, policy):
    if policy.get("require_reachable_sources"):
        reachable = {path for resolution in catalog["resolutions"] for path in resolution["packages"]}
        retained = set(catalog["packages"]) | {merge["directory"] for merge in catalog.get("merges", [])}
        if retained - reachable:
            raise ValueError(f"retained source outside supported entry closures: {sorted(retained - reachable)}")


@source_layout.operation
def verify(root, metadata=False):
    catalog, policy = read_json(root / CATALOG), read_json(root / POLICY)
    if catalog.get("initial_test_relocation_sha256") not in (None, source_relocation.RECEIPT_SHA256):
        raise ValueError("initial test relocation history identity drift")
    structure(root, policy)
    source_trimming.validate(root, catalog)
    proof_routing.load(root, catalog)
    renames = source_naming.validate_history(root, catalog)
    for key, relative in (("policy_sha256", POLICY), ("config_sha256", ".cargo/config.toml"), ("origins_sha256", ORIGINS)):
        if catalog[key] != digest(safe_path(root, relative).read_bytes()):
            raise ValueError(f"policy/config/origin content drift: {relative}")
    records = origins(root)
    benchmark_receipt, benchmark_grants = benchmark_relocation.validate(root, catalog, files)
    consolidation_history = source_consolidation.validate_history(root, catalog, validate_external=external_transfers.validate)
    source_consolidation.validate_origins(consolidation_history, records)
    source_moves.validate_origins(source_moves.validate_history(root, catalog), records)
    if origin_hashes(records) != catalog["origin_records_sha256"] or provenance(root) != catalog["provenance_sha256"]:
        raise ValueError("centralized original provenance/license inventory drift")
    external, external_receipts = external_tests.load(root, source_owners(catalog["packages"], catalog.get("merges", [])), renames, benchmark_relocation.combine_grants(external_transfers.grants(consolidation_history), benchmark_grants))
    if external or external_tests.has_history(catalog):
        source_relocation.read_receipt(root, safe_path)
    if external != catalog.get("external_tests", {}) or external_receipts != catalog.get("external_test_mappings_sha256", {}):
        raise ValueError("external test source accounting drift")
    current_manifests = project_manifests(root)
    if current_manifests != set(catalog["manifests_sha256"]):
        raise ValueError("manifest inventory drift")
    for relative, expected in catalog["manifests_sha256"].items():
        if digest(safe_path(root, relative).read_bytes()) != expected:
            raise ValueError(f"manifest content drift: {relative}")
    if set(catalog["lockfiles_sha256"]) != {"Cargo.lock"}:
        raise ValueError("exactly one root lockfile receipt is required")
    if digest((root / "Cargo.lock").read_bytes()) != catalog["lockfiles_sha256"]["Cargo.lock"]:
        raise ValueError("root lockfile drift")
    for package in tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))["package"]:
        if package.get("source") is not None or package.get("checksum") is not None:
            raise ValueError(f"root lock contains non-path Rust source: {package['name']}")
    if source_directories(root, policy) != set(catalog["packages"]):
        raise ValueError("source directory inventory drift")
    for relative, package in catalog["packages"].items():
        if relative != package["directory"]:
            raise ValueError(f"source directory identity mismatch: {relative}")
        match_origin(package, records)
        verify_descriptions(root, package)
        if files(safe_path(root, relative)) != package["files_sha256"]:
            raise ValueError(f"source content drift: {relative}")
    for merge in catalog.get("merges", []):
        match_origin(merge, records)
        verify_descriptions(root, merge)
        if files(safe_path(root, merge["directory"])) != merge["files_sha256"]:
            raise ValueError(f"merged source content drift: {merge['directory']}")
    retired_sources(root, catalog, records)
    superseded_sources(root, catalog, records)
    verify_origin_coverage(catalog, records)
    verify_retained_closure(catalog, policy)
    expected = {(entry["manifest"], entry["id"], entry["target"], profile_features(root, entry)) for entry in entries(policy)}
    actual = {(item["manifest"], item.get("profile", "workspace"), item["target"], tuple(item.get("requested_features", [])))
              for item in catalog["resolutions"]}
    if expected != actual or len(actual) != len(catalog["resolutions"]):
        raise ValueError("resolution inventory drift: root profile/target/requested-feature coverage")
    identities = sorted(upstream_identity(record["upstream"]) for record in records.values())
    if sorted(upstream_identity(record) for record in catalog["upstream_identities"]) != identities:
        raise ValueError("upstream advisory identity inventory drift")
    if (root / DOC).read_text(encoding="utf-8") != document(catalog, policy):
        raise ValueError("third-party documentation out of sync")
    if metadata and resolutions(root, policy, [merge["directory"] for merge in catalog.get("merges", [])]) != catalog["resolutions"]:
        raise ValueError("closure resolution/features drift")
    print(f"Verified {len(catalog['packages'])} active source trees, {len(catalog.get('retired', {}))} retired and {len(catalog.get('superseded', {}))} superseded sources, immutable upstream provenance and one root workspace/lock.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("refresh", "verify"))
    parser.add_argument("--metadata", action="store_true")
    parser.add_argument("--import-plan", help="pinned binding-test text import receipt ID; refresh only")
    parser.add_argument("--external-path-plan", type=Path, help="explicit owner-resolved external test path correction; refresh only")
    parser.add_argument("--layout-plan", type=Path, help="explicit identity-preserving foundation root layout; refresh only")
    parser.add_argument("--trim-plan", type=Path, help="explicit reviewed per-file production source removal; refresh only")
    parser.add_argument("--retire-plan", type=Path, help="explicit reviewed removal plan; refresh only")
    parser.add_argument("--upgrade-plan", type=Path, help="explicit reviewed upstream replacement plan; refresh only")
    parser.add_argument("--rename-plan", type=Path, help="explicit reviewed lossless source naming plan; refresh only")
    parser.add_argument("--move-plan", type=Path, help="explicit reviewed identity-preserving internal file move plan; refresh only")
    parser.add_argument("--consolidate-plan", type=Path, help="explicit reviewed multi-origin consolidation plan; refresh only")
    parser.add_argument("--relocate-benchmarks", action="store_true", help="install the pinned 32-file post-consolidation benchmark relocation; refresh only")
    arguments = parser.parse_args()
    try:
        if (arguments.import_plan or arguments.external_path_plan or arguments.layout_plan or arguments.trim_plan or arguments.retire_plan or arguments.upgrade_plan or arguments.rename_plan or arguments.move_plan or arguments.consolidate_plan or arguments.relocate_benchmarks) and arguments.command != "refresh":
            raise ValueError("--trim-plan, --retire-plan, --upgrade-plan, --rename-plan, --move-plan, --consolidate-plan and --relocate-benchmarks are only accepted by refresh")
        (refresh(ROOT, read_json(arguments.retire_plan) if arguments.retire_plan else None,
                 read_json(arguments.upgrade_plan) if arguments.upgrade_plan else None,
                 read_json(arguments.rename_plan) if arguments.rename_plan else None,
                 read_json(arguments.move_plan) if arguments.move_plan else None,
                 read_json(arguments.consolidate_plan) if arguments.consolidate_plan else None,
                 trim_plan_data=read_json(arguments.trim_plan) if arguments.trim_plan else None,
                 layout_plan_data=read_json(arguments.layout_plan) if arguments.layout_plan else None,
                 external_path_plan_data=read_json(arguments.external_path_plan) if arguments.external_path_plan else None,
                 import_plan_id=arguments.import_plan,
                 **({'relocate_benchmarks': True} if arguments.relocate_benchmarks else {}))
         if arguments.command == "refresh" else verify(ROOT, arguments.metadata))
    except (ValueError, OSError, KeyError, TypeError) as error:
        print(f"Rust source check failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
