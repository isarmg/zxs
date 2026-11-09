"""One reviewed libsqlite3-sys replacement; no generic dependency updater."""
from copy import deepcopy
import json
import re
import shutil
import tomllib

from .archive import IDENTITY, NAME, VERSION, URL, SHA256, relative, sha256

DIRECTORY = "src/foundation/data-storage/libsqlite3-sys"
OLD_ORIGIN = "vendor/rust/libsqlite3-sys-0.37.0"
NEW_ORIGIN = "vendor/rust/" + IDENTITY
TEST_ROOT = "test/foundation/libsqlite3-sys"
PROVENANCE = "compliance/provenance/packages/" + IDENTITY
PREFIXES = ("src/", "sqlite3/", "bindgen-bindings/")
REASON = "RUSTSEC-2026-0459: authenticated libsqlite3-sys 0.39.1 / SQLite 3.54.1; preserve local path dependencies and complete predecessor history"


def write(root, path, data):
    path = relative(path)
    target = root / path
    for parent in (target, *target.parents):
        if parent == root:
            break
        if parent.is_symlink():
            raise ValueError("source destination symlink: " + path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode()


def replace_once(text, before, after):
    if text.count(before) != 1:
        raise ValueError("unreviewed source shape: " + before[:100])
    return text.replace(before, after, 1)


def baseline(root, manage, release, evidence):
    previous = manage.read_json(root / manage.CATALOG)
    package = previous["packages"][DIRECTORY]
    if (package["name"] != NAME or package["version"] != "0.37.0"
            or package["origin_directory"] != OLD_ORIGIN
            or package["upstream"]["version"] != "0.37.0"):
        raise ValueError("this temporary job only replaces the original 0.37.0 owner")
    records = manage.origins(root)
    before = records[OLD_ORIGIN]
    if NEW_ORIGIN in records:
        raise ValueError("successor already registered; do not repeat or overwrite")
    import source_layout
    layout = source_layout.load(root, previous)
    if layout is None:
        raise ValueError("the reviewed grouped source layout is required")
    layout.require_standalone(previous, layout.reverse[DIRECTORY])
    owner = previous.get("external_tests", {}).get(DIRECTORY)
    # The existing one-module extraction is the only approved external transition.
    # Unknown/local additions need a separately reviewed adapter, not deletion.
    if (not isinstance(owner, dict) or len(owner["files"]) != 1
            or owner["roots"] != ["test/foundation/libsqlite3-sys"]
            or owner["files"][0].get("source_kind") is not None
            or owner["files"][0].get("origin_directory", OLD_ORIGIN) != OLD_ORIGIN):
        raise ValueError("unreviewed SQLite external-test owner; retain it unchanged")
    row = owner["files"][0]
    if (row["path"] != "test/foundation/libsqlite3-sys/unit/src/error/test_160.rs"
            or not row["source"].endswith("/src/error.rs")):
        raise ValueError("unreviewed SQLite test extraction path")
    error = release.files["src/error.rs"].decode()
    marker = "#[cfg(test)]\nmod test {"
    if error.count(marker) != 1 or not error.rstrip().endswith("}"):
        raise ValueError("unknown successor unit-test shape")
    head, body = error.split(marker)
    module = marker + body
    old_test = (root / row["path"]).read_bytes()
    if (sha256(old_test) != row["sha256"]
            or old_test.decode().strip() != module.strip()):
        raise ValueError("successor tests differ from retained tests; explicit review required")
    # Only the known local manifest/build/test wiring may differ from 0.37.0.
    # Do not erase later local changes just because the old origin label matches.
    customized = {
        "Cargo.toml": "6653457e59bfff7102c81f4e7fc333923576acb0",
        "build.rs": "f66501d2223787b880885688266fb424e8b5ee82",
        "src/lib.rs": "6dfdf2c144c8726e344c89b5061544680324379b",
        "src/error.rs": "0ea4b1a6e3a7b88456d71a16ac23f87fb72314f3",
    }
    import hashlib
    for path, expected in package["files_sha256"].items():
        data = (root / DIRECTORY / relative(path)).read_bytes()
        if sha256(data) != expected:
            raise ValueError("predecessor source changed during admission")
        if path in customized:
            blob = hashlib.sha1(b"blob " + str(len(data)).encode() + bytes([0]) + data).hexdigest()
            if blob != customized[path]:
                raise ValueError("unreviewed local SQLite customization: " + path)
        elif before["original_files_sha256"].get(path) != expected:
            raise ValueError("unreviewed local SQLite source difference: " + path)
    evidence["predecessor"] = {"package": package, "origin": before, "external_tests": owner}
    evidence["successor_original_files_sha256"] = release.inventory
    return previous, owner, head, module


def materials(root, release, before):
    inventory, licenses, metadata, docs = release.inventory, [], [], []
    disposition = {}
    # Every archive file has an explicit destination. Original non-code bytes are
    # preserved centrally, including shell scripts, but are never executed.
    for path, data in sorted(release.files.items()):
        leaf = path.rsplit("/", 1)[-1]
        if re.fullmatch(r"(?:LICEN[CS]E|COPYING|COPYRIGHT|NOTICES?|AUTHORS)(?:[._-].*)?", leaf, re.I):
            destination = "compliance/licenses/upstream/" + IDENTITY + "/" + path
            licenses.append(destination)
        elif leaf.lower().endswith((".md", ".rst", ".adoc")):
            destination = "docs/upstream/" + IDENTITY + "/" + path
            docs.append(destination)
        elif path.startswith(PREFIXES) or path in ("build.rs", "wrapper.h", "wrapper_ext.h"):
            destination = DIRECTORY + "/" + path
        else:
            destination = PROVENANCE + "/original/" + path + ".txt"
            metadata.append(destination)
        write(root, destination, data)
        disposition[path] = {"path": destination, "original_sha256": inventory[path]}
    if not licenses or PROVENANCE + "/original/Cargo.toml.txt" not in metadata:
        raise ValueError("incomplete centralized license/manifest set")
    record = {
        "origin_directory": NEW_ORIGIN,
        "upstream": {"name": NAME, "version": VERSION, "archive_url": URL, "archive_sha256": SHA256},
        "license": "MIT", "repository": "https://github.com/rusqlite/rusqlite",
        "vcs": release.vcs, "original_files_sha256": inventory,
        "central_license_materials": licenses, "central_original_metadata": metadata,
        "central_original_documentation": docs,
    }
    origins = deepcopy(before)
    origins["packages"].append(record)
    write(root, "compliance/provenance/origins.json", json_bytes(origins))
    write(root, PROVENANCE + "/source-disposition.json", json_bytes(disposition))
    return record, disposition


def manifest(root, release, original_manifest):
    # Retain the project's exact dependency aliases/paths and supported features.
    # Never restore registry, vcpkg, openssl-sys or sqlcipher-src dependencies.
    text = replace_once(original_manifest, 'edition = "2021"', 'edition = "2024"\nrust-version = "1.88.0"')
    text = replace_once(text, 'version = "0.37.0"', 'version = "0.39.1"')
    text = replace_once(text, "docs/upstream/libsqlite3-sys-0.37.0/", "docs/upstream/libsqlite3-sys-0.39.1/")
    text = text.replace("min_sqlite_version_3_34_1", "min_sqlite_version_3_45_3")
    parsed = tomllib.loads(text)
    old = tomllib.loads(original_manifest)
    if (parsed["build-dependencies"] != old["build-dependencies"]
            or set(parsed["build-dependencies"]) != {"bindgen", "cc", "pkg-config", "rust-source-format", "quote", "rust-syntax"}
            or parsed["build-dependencies"]["bindgen"]["version"] != "0.72"
            or parsed["features"]["default"] != ["min_sqlite_version_3_45_3"]):
        raise ValueError("local SQLite manifest adapters changed")
    # The 0.72 local bindgen adapter is retained, not claimed to be upstream 0.73.
    # Compilation/optional feature acceptance remains a later GitHub check.
    write(root, DIRECTORY + "/Cargo.toml", text.encode())
    access = "src/foundation/data-storage/sqlite-access/Cargo.toml"
    text = (root / access).read_text()
    text = replace_once(text, '[dependencies.libsqlite3-sys]\nversion = ">=0.30.1, <0.38.0"',
                        '[dependencies.libsqlite3-sys]\nversion = "=0.39.1"')
    write(root, access, text.encode())
    platform = "src/platform/Cargo.toml"
    text = (root / platform).read_text()
    text = replace_once(text, 'libsqlite3-sys = { version = "=0.37.0",', 'libsqlite3-sys = { version = "=0.39.1",')
    write(root, platform, text.encode())


def build_adapter(text):
    text = replace_once(text, '    #[cfg(feature = "vcpkg")]\n    extern crate vcpkg;\n\n', "")
    text = replace_once(text, '        if cfg!(feature = "vcpkg") && is_compiler("msvc") {\n'
                        '            println!("cargo:rerun-if-env-changed=VCPKGRS_DYNAMIC");\n        }\n', "")
    text = replace_once(text, '        if let Some(header) = try_vcpkg() {\n            return header;\n        }\n\n', "")
    begin = text.index("    fn try_vcpkg() -> Option<HeaderLocation> {")
    end = text.index('\n}\n\n#[cfg(not(feature = "buildtime_bindgen"))]', begin)
    block = text[begin:end]
    if block.count("fn ") != 1 or text.count("    fn try_vcpkg()") != 1:
        raise ValueError("unreviewed vcpkg function boundary")
    text = text[:begin] + text[end:]
    text = replace_once(text, "use super::{HeaderLocation, bindings, env_prefix, is_compiler, lib_name, win_target};",
                        "use super::{HeaderLocation, bindings, env_prefix, lib_name, win_target};")
    text = replace_once(text, "                let dir = sqlcipher_src::source_dir();\n"
                        "                (dir.to_owned(), dir.join(sqlcipher_src::SOURCE_FILE))",
                        '                panic!("Bundled SQLCipher is not maintained by this SQLite-only source upgrade")')
    text = text.replace("prettyplease::", "rust_source_format::").replace("syn::", "rust_syntax::")
    if re.search(r"\b(?:vcpkg|prettyplease|syn|sqlcipher_src)::", text):
        raise ValueError("unadapted upstream dependency reference")
    return ('#[cfg(feature = "bundled-sqlcipher")]\n'
            'compile_error!("Bundled SQLCipher requires a separate authenticated source upgrade");\n\n' + text)


def remove_old_tests(root, owner):
    mapping_path = None
    for path in (root / "compliance/external-tests").glob("*.json"):
        mapping = json.loads(path.read_bytes())
        found = [item for item in mapping["owners"] if item["directory"] == DIRECTORY]
        if not found:
            continue
        if mapping_path is not None or len(found) != 1:
            raise ValueError("ambiguous external-test mapping")
        mapping_path = path.relative_to(root).as_posix()
        mapping["owners"] = [item for item in mapping["owners"] if item["directory"] != DIRECTORY]
        write(root, mapping_path, json_bytes(mapping))
    if mapping_path is None:
        raise ValueError("missing predecessor external-test mapping")
    retained = []
    for row in owner["files"]:
        data = (root / row["path"]).read_bytes()
        if sha256(data) != row["sha256"]:
            raise ValueError("retained test changed during preparation")
        central = PROVENANCE + "/predecessor-tests/" + row["path"] + ".txt"
        write(root, central, data)
        retained.append({"path": row["path"], "sha256": row["sha256"], "retained_at": central})
    write(root, PROVENANCE + "/predecessor-tests.json", json_bytes({"owner": owner, "bytes": retained}))
    # Only runner-checkout paths whose exact complete inventory passed manage.verify.
    for path in owner["roots"]:
        shutil.rmtree(root / relative(path))
    return mapping_path


def successor_tests(root, release, head, module):
    path = TEST_ROOT + "/unit/src/error/test_160.rs"
    write(root, path, module.encode())
    hook = '#[cfg(test)]\ninclude!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../' + path + '"));\n'
    write(root, DIRECTORY + "/src/error.rs", (head + hook).encode())
    lib = release.files["src/lib.rs"].decode()
    lib = replace_once(lib, '// force linking to openssl\n#[cfg(feature = "bundled-sqlcipher-vendored-openssl")]\n'
                       'extern crate openssl_sys;\n', "")
    write(root, DIRECTORY + "/src/lib.rs", lib.encode())
    row = {"source": DIRECTORY + "/src/error.rs",
           "source_sha256_before": release.inventory["src/error.rs"],
           "path": path, "sha256": sha256(module.encode())}
    mapping = {"schema_version": 1, "owners": [{"directory": DIRECTORY, "roots": [TEST_ROOT], "files": [row]}]}
    target = "compliance/external-tests/sqlite-0.39.1.json"
    if (root / target).exists():
        raise ValueError("successor external-test mapping already exists")
    write(root, target, json_bytes(mapping))


def upgrade(root, release, manage, run, evidence):
    previous, owner, head, module = baseline(root, manage, release, evidence)
    origin_document = manage.read_json(root / manage.ORIGINS)
    old_manifest = (root / DIRECTORY / "Cargo.toml").read_text()
    old_files = manage.files(root / DIRECTORY)
    evidence["old_source_files_sha256"] = old_files
    # Keep every old source receipt and all predecessor legal materials. Only this
    # exact active owner is replaced; no unrelated source or historical record.
    shutil.rmtree(root / DIRECTORY)
    record, disposition = materials(root, release, origin_document)
    manifest(root, release, old_manifest)
    write(root, DIRECTORY + "/build.rs", build_adapter(release.files["build.rs"].decode()).encode())
    mapping_path = remove_old_tests(root, owner)
    plan = {"schema": 1, "packages": [{
        "directory": DIRECTORY, "package": previous["packages"][DIRECTORY],
        "successor_origin_directory": NEW_ORIGIN, "reason": REASON,
        "source_prefixes": list(PREFIXES), "external_action": "remove", "external_tests": owner,
    }]}
    evidence["upgrade_plan"] = plan
    old_lock = tomllib.loads((root / "Cargo.lock").read_text())
    run(["cargo", "generate-lockfile", "--offline"])
    current_lock = tomllib.loads((root / "Cargo.lock").read_text())
    expected_lock = deepcopy(old_lock)
    matches = [p for p in expected_lock["package"] if p["name"] == NAME]
    if len(matches) != 1 or matches[0]["version"] != "0.37.0":
        raise ValueError("unexpected baseline SQLite lock identity")
    matches[0]["version"] = VERSION
    if current_lock != expected_lock:
        raise ValueError("lock generation changed more than the one SQLite version")
    # First transition authenticates COMPLETE original source prefixes, including
    # successor tests in their original bytes. No patch is emitted at this stage.
    manage.refresh(root, upgrade_plan_data=plan)
    # The second transition separates the authenticated successor unit module
    # back beneath root test/. Its new origin is explicit; old labels stay archived.
    successor_tests(root, release, head, module)
    notice = root / "compliance/NOTICE.md"
    text = notice.read_text()
    if "libsqlite3-sys@0.39.1" in text:
        raise ValueError("successor notice already exists")
    old_notice = [line for line in text.splitlines() if line.startswith("| `libsqlite3-sys@0.37.0` |")]
    if len(old_notice) != 1:
        raise ValueError("missing exact predecessor notice")
    new_notice = "| `libsqlite3-sys@0.39.1` | MIT | [LICENSE](licenses/upstream/" + IDENTITY + "/LICENSE) |"
    text = replace_once(text, old_notice[0] + "\n", old_notice[0] + "\n" + new_notice + "\n")
    write(root, "compliance/NOTICE.md", text.encode())
    manage.refresh(root)
    manage.verify(root, metadata=True)
    run(["python3", "tools/rust-sources/architecture.py", "refresh"])
    run(["python3", "tools/rust-sources/architecture.py", "verify"])
    current = manage.read_json(root / manage.CATALOG)
    if (current["packages"][DIRECTORY]["upstream"] != record["upstream"]
            or current["packages"][DIRECTORY]["version"] != VERSION
            or current["superseded"][OLD_ORIGIN]["package"] != previous["packages"][DIRECTORY]
            or current["superseded"][OLD_ORIGIN]["external_tests"] != owner):
        raise ValueError("incomplete source replacement chain")
    if any(p["upstream"]["name"] == NAME and p["upstream"]["version"] != VERSION
           for p in [*current["packages"].values(), *current.get("merges", [])]):
        raise ValueError("an affected active SQLite origin remains")
    for path, expected in previous["provenance_sha256"].items():
        if path not in (manage.ORIGINS, "compliance/provenance/document-migration.json"):
            if sha256((root / path).read_bytes()) != expected:
                raise ValueError("old provenance or license material changed: " + path)
    for prefix in ("sqlite3/", "bindgen-bindings/"):
        actual = {p: value for p, value in manage.files(root / DIRECTORY).items() if p.startswith(prefix)}
        wanted = {p: value for p, value in release.inventory.items() if p.startswith(prefix)}
        if actual != wanted:
            raise ValueError("authenticated native source/bindings changed after refresh")
    evidence["source_disposition"] = disposition
    evidence["new_source_files_sha256"] = manage.files(root / DIRECTORY)
    evidence["adaptations"] = [
        "Retained local path dependencies and rust-source-format/rust-syntax names; bindgen 0.72 retained as a local adapter, not upstream 0.73.",
        "Bundled SQLCipher explicitly fails: its separate source is not fetched or accepted by this SQLite-only job.",
        "Replaced complete SQLite 3.54.1 C/header/prebuilt bindings; no local WAL backport or advisory exception.",
        "Preserved predecessor test bytes and receipt centrally; identical successor module remains wired from root test/ under its new source identity.",
    ]
    return mapping_path
