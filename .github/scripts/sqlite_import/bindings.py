"""Restore one authenticated linked-SQLCipher binding, not its C implementation."""
from copy import deepcopy

from sqlite_upgrade.archive import IDENTITY, NAME, SHA256, URL, VERSION, sha256
from sqlite_upgrade.upgrade import DIRECTORY, PROVENANCE

IMPORT_COMMIT = "bb85413646c2481ff72cc1d52b3558e9caa496f1"
BINDING = "sqlcipher/bindgen_bundled_version.rs"
ARCHIVED = PROVENANCE + "/original/" + BINDING + ".txt"
DESTINATION = DIRECTORY + "/" + BINDING
BINDING_SHA256 = "4ce4a6519255f9d1c22c9f894b7f0452f1ae11d54dc9bf3205265ec9142f1da3"
BINDING_BLOB = "f9cb81254df6304c1061576ed61853735bebc761"
BINDING_BYTES = 127402
CONSUMERS = "docs/architecture/foundation-consumers.md"
DEPENDENCY_PAGE = "docs/dependencies/data-storage.md"


def prepare(api, head, root, manage, run, previous, evidence):
    comparison = api.request("compare/" + IMPORT_COMMIT + "..." + head + "?per_page=1")
    if (comparison["status"] not in ("ahead", "identical")
            or comparison["base_commit"]["sha"] != IMPORT_COMMIT
            or comparison["merge_base_commit"]["sha"] != IMPORT_COMMIT):
        raise ValueError("binding repair requires a descendant of the verified SQLite import")

    package = previous["packages"][DIRECTORY]
    origin = "vendor/rust/" + IDENTITY
    upstream = {"name": NAME, "version": VERSION, "archive_url": URL, "archive_sha256": SHA256}
    if (package["name"] != NAME or package["version"] != VERSION
            or package["origin_directory"] != origin or package["upstream"] != upstream):
        raise ValueError("binding repair requires the exact authenticated 0.39.1 owner")
    record = manage.origins(root)[origin]
    disposition = manage.read_json(root / PROVENANCE / "source-disposition.json")
    if (record["original_files_sha256"].get(BINDING) != BINDING_SHA256
            or ARCHIVED not in record["central_original_metadata"]
            or disposition.get(BINDING) != {"path": ARCHIVED, "original_sha256": BINDING_SHA256}):
        raise ValueError("the original SQLCipher binding receipt differs")

    original = manage.safe_path(root, ARCHIVED).read_bytes()
    if len(original) != BINDING_BYTES or sha256(original) != BINDING_SHA256:
        raise ValueError("the retained SQLCipher binding is not the authenticated original")
    before_files = package["files_sha256"]
    if any(path.startswith("sqlcipher/") for path in before_files):
        raise ValueError("SQLCipher source is already active; do not repeat or overwrite this repair")
    if manage.files(root / DIRECTORY) != before_files:
        raise ValueError("the active SQLite source changed after admission")

    evidence.update({
        "task": "restore-original-linked-sqlcipher-binding",
        "source_import_commit": IMPORT_COMMIT,
        "source_artifacts_downloaded": False,
        "source_archive_path": ARCHIVED,
        "source_binding_sha256": BINDING_SHA256,
        "source_binding_git_blob": BINDING_BLOB,
        "source_binding_bytes": BINDING_BYTES,
        "unchanged": [
            "Cargo.lock", "all manifests and feature dependencies",
            "SQLCipher C/header absence and bundled-SQLCipher rejection",
            "immutable source-disposition, origins, licenses and predecessor receipts",
        ],
    })
    evidence["stage"] = "activate-original-binding-without-overwrite"
    destination = manage.safe_path(root, DESTINATION)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("xb") as stream:
        stream.write(original)

    # The original disposition is historical evidence and cannot be rewritten.
    # The existing generator records this same-origin active copy in the current
    # package inventory, preserving every original and superseded receipt.
    evidence["stage"] = "refresh-current-source-inventory"
    run(["python3", "tools/rust-sources/manage.py", "refresh"])
    run(["python3", "tools/rust-sources/manage.py", "verify", "--metadata"])
    run(["python3", "tools/rust-sources/architecture.py", "refresh"])
    run(["python3", "tools/rust-sources/architecture.py", "verify"])
    evidence["stage"] = "refresh-current-dependency-documents"
    run(["python3", "tools/rust-sources/documentation.py", "refresh"])
    run(["python3", "tools/rust-sources/documentation.py", "verify"])

    expected = deepcopy(previous)
    expected["packages"][DIRECTORY]["files_sha256"][BINDING] = BINDING_SHA256
    current = manage.read_json(root / manage.CATALOG)
    if current != expected:
        raise ValueError("binding repair changed more than one current package file receipt")
    if manage.files(root / DIRECTORY) != expected["packages"][DIRECTORY]["files_sha256"]:
        raise ValueError("binding repair changed other active SQLite sources")
    if manage.safe_path(root, ARCHIVED).read_bytes() != original:
        raise ValueError("binding repair changed its immutable original")

    allowed = [DESTINATION, manage.CATALOG, manage.DOC, CONSUMERS, DEPENDENCY_PAGE]
    run(["git", "add", "--", *allowed])
    paths = sorted(path.decode("utf-8") for path in
                   run(["git", "diff", "--cached", "--name-only", "-z", "--"]).split(b"\0") if path)
    if not {DESTINATION, manage.CATALOG} <= set(paths) <= set(allowed):
        raise ValueError("binding repair staged an unexpected path set")
    if run(["git", "rev-parse", ":" + DESTINATION]).decode().strip() != BINDING_BLOB:
        raise ValueError("the restored active binding differs from the original Git blob")
    evidence["binding_repair_paths"] = paths
    return paths
