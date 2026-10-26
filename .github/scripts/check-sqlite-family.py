#!/usr/bin/env python3
"""Run preserved SQLite tests and compiler boundaries without changing root inputs."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
FAMILY_TEST_ROOTS = ("test/foundation/sqlite-access", "test/platform/unit/sqlite/driver")
INTEGRATION_TESTS = {
    "hash_table_compatibility": "test/engine/integration/hash_table_compatibility.rs",
    "sqlite_foundation": "test/platform/integration/sqlite_foundation.rs",
}


def cargo_command(operation, manifest, target, *arguments):
    return ["cargo", operation, "--locked", "--offline", "--color", "never", "--manifest-path", str(manifest),
            "--target-dir", str(target), *arguments]


def execute(command, root, expected=None, minimum_tests=None, required_tests=()):
    result = subprocess.run(command, cwd=root, text=True, capture_output=True)
    if expected is not None:
        diagnostic = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", result.stderr)
        assert result.returncode and expected in diagnostic, (command, result.stdout, result.stderr)
        return
    assert result.returncode == 0, (command, result.stdout, result.stderr)
    print(result.stdout, end="", flush=True)
    print(result.stderr, end="", flush=True)
    if minimum_tests is not None:
        passed = sum(map(int, re.findall(r"test result: ok\. (\d+) passed;", result.stdout)))
        assert passed >= minimum_tests, f"expected at least {minimum_tests} real tests, got {passed}"

    for name in required_tests:
        assert re.search(r"test (?:\S+::)?" + re.escape(name) + r" \.\.\. ok", result.stdout), (name, result.stdout)


def lock(manifest, root):
    execute(["cargo", "generate-lockfile", "--offline", "--manifest-path", str(manifest)], root)


def prepare_package(root, work):
    """Only the maintained package/tests are copied; dependencies remain real providers."""
    original = root / "src/foundation/data-storage/sqlite-access"
    package = work / "src/foundation/data-storage/sqlite-access"
    shutil.copytree(original, package, ignore=shutil.ignore_patterns("Cargo.lock", "target", "__pycache__"))
    for tests in FAMILY_TEST_ROOTS:
        shutil.copytree(root / tests, work / tests)
    manifest = package / "Cargo.toml"
    text = manifest.read_text()
    parsed = tomllib.loads(text)
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        for entry in parsed.get(section, {}).values():
            if isinstance(entry, dict) and "path" in entry:
                old = entry["path"]
                text = text.replace(f'path = "{old}"', f'path = {json.dumps(str((original / old).resolve()))}')
    if "readme" in parsed["package"]:
        old = parsed["package"]["readme"]
        text = text.replace(f'readme = "{old}"', f'readme = {json.dumps(str((original / old).resolve()))}')
    assert "[workspace]" not in text
    manifest.write_text(text + "\n[workspace]\n")
    return manifest


def consumer_manifest(root, work, features):
    manifest = work / "Cargo.toml"
    (work / "src").mkdir(parents=True, exist_ok=True)
    text = ('[package]\nname="sqlite-family-consumer"\nversion="0.0.0"\nedition="2021"\n'
            '[workspace]\nresolver="2"\n[features]\nsqlite-pool=["sqlite-access/pool"]\n[dependencies]\n'
            f'sqlite-access={{path={json.dumps(str(root / "src/foundation/data-storage/sqlite-access"))},default-features=false,features={json.dumps(features)}}}\n'
            '[dev-dependencies]\n'
            f'tempfile={{path={json.dumps(str(root / "src/foundation/platform/tempfile"))}}}\n'
            f'log={{path={json.dumps(str(root / "src/foundation/diagnostics/log"))},default-features=false,features=["std"]}}\n'
            f'futures={{path={json.dumps(str(root / "src/foundation/concurrency/futures"))},default-features=false,features=["std","utilities"]}}\n'
            f'libsqlite3-sys={{path={json.dumps(str(root / "src/foundation/data-storage/libsqlite3-sys"))},default-features=false}}\n'
            f'tokio={{path={json.dumps(str(root / "src/foundation/concurrency/tokio"))},default-features=false,features=["macros","rt","time","sync"]}}\n'
            f'hash-table={{path={json.dumps(str(root / "src/foundation/collections/hash-table"))}}}\n'
            f'hashlink={{path={json.dumps(str(root / "src/foundation/collections/hashlink"))}}}\n')
    for name, path in INTEGRATION_TESTS.items():
        text += f'[[test]]\nname={json.dumps(name)}\npath={json.dumps(str(root / path))}\n'
    manifest.write_text(text)
    return manifest


DIRECT_EXCLUDED_PACKAGES = {
    "tokio", "concurrent-collections", "async-notification", "unicode-normalizer",
    "unicode-properties", "unicode-data", "utf8_iter",
}


def assert_direct_dependencies(output):
    packages = {}
    for line in output.splitlines():
        package, separator, features = line.partition("|")
        assert separator, line
        name = package.split()[0]
        packages.setdefault(name, set()).update(features.split(",") if features else ())
    assert "sqlite-access" in packages and "url-encoding" in packages, packages
    assert not DIRECT_EXCLUDED_PACKAGES.intersection(packages), packages
    assert not {"pool", "url"}.intersection(packages["sqlite-access"]), packages
    assert not {"parser", "domain"}.intersection(packages["url-encoding"]), packages
    print(f"Direct SQLite production closure: {len(packages) - 1} packages (excluding probe); no pool/runtime or URL/IDNA capabilities.")


def check_direct_dependencies(manifest, root):
    command = ["cargo", "tree", "--locked", "--offline", "--color", "never", "--manifest-path", str(manifest),
               "--prefix", "none", "--edges", "normal,build", "--format", "{p}|{f}", "--no-dedupe"]
    result = subprocess.run(command, cwd=root, text=True, capture_output=True)
    assert result.returncode == 0, (command, result.stdout, result.stderr)
    assert_direct_dependencies(result.stdout)


def check(root):
    root = root.resolve()
    root_lock = root / "Cargo.lock"
    original_lock = root_lock.read_bytes()
    maintained_source = root / "src/foundation/data-storage/sqlite-access"
    original_source = {p.relative_to(maintained_source).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                       for p in maintained_source.rglob("*") if p.is_file()}
    (root / "target").mkdir(exist_ok=True)
    target = root / "target/sqlite-family-check"
    try:
        with tempfile.TemporaryDirectory(prefix="sqlite-family-", dir=root / "target") as temporary:
            work = Path(temporary)
            package_manifest = prepare_package(root, work)
            lock(package_manifest, root)
            execute(cargo_command("test", package_manifest, target, "--doc", "--features", "sqlite-bundled",
                                  "types::Type"), root, minimum_tests=1)
            execute(cargo_command("test", package_manifest, target, "--lib", "--features", "sqlite-bundled"), root,
                    minimum_tests=10, required_tests=("ordinal_parameters_preserve_repetition_order_and_missing_values",
                                                     "invalid_ordinal_parameters_return_errors_without_poisoning_connection",
                                                     "direct_memory_transactions_savepoints_rollback_and_errors",
                                                     "filename_absolute_relative_unicode_percent_and_query_characters_are_literal",
                                                     "missing_nul_and_non_utf8_filenames_fail_without_creating_files",
                                                     "parsed_memory_options_share_only_the_selected_database",
                                                     "parsed_percent_filename_and_query_options_reopen_readonly",
                                                     "direct_parser_preserves_paths_percent_form_options_and_errors",
                                                     "prepared_queries_reject_nul_before_execution",
                                                     "nul_filenames_are_rejected_before_uri_encoding"))
            execute(cargo_command("test", package_manifest, target, "--lib", "--features", "sqlite-bundled,url"), root,
                    minimum_tests=12, required_tests=("typed_url_conversion_preserves_retained_sqlite_options",))
            execute(cargo_command("test", package_manifest, target, "--lib", "--all-features"), root,
                    minimum_tests=19, required_tests=("test_regexp_filters_correctly", "assert_pool_traits"))
            consumer = work / "consumer"
            manifest = consumer_manifest(root, consumer, ["sqlite-bundled"])
            consumer_source = consumer / "src/lib.rs"
            consumer_source.write_text("pub fn accepts(_: Option<sqlite_access::SqliteConnection>) {}\n")
            lock(manifest, root)
            check_direct_dependencies(manifest, root)
            execute(cargo_command("test", manifest, target, "--tests"), root, minimum_tests=7)
            execute(cargo_command("test", manifest, target, "--tests", "--features", "sqlite-pool"), root,
                    minimum_tests=8, required_tests=("pool_timeout_rollback_savepoint_and_statement_eviction",))
            for features in ([], ["sqlite-bundled"]):
                manifest = consumer_manifest(root, consumer, features)
                lock(manifest, root)
                execute(cargo_command("check", manifest, target, "--lib"), root)
                check_direct_dependencies(manifest, root)
            for text, diagnostic in (
                ("use sqlite_access::SqlitePool;", "error[E0432]"),
                ("use sqlite_access::Url;", "error[E0432]"),
                ("fn absent<T: sqlite_access::ConnectOptions>() { let _ = T::from_url; }", "error[E0599]"),
            ):
                consumer_source.write_text(text + "\n")
                execute(cargo_command("check", manifest, target, "--lib"), root, expected=diagnostic)
            consumer_source.write_text("use sqlite_access::{ConnectOptions, SqlitePool, Url};\n"
                              "pub fn retained(url: &Url, _: Option<SqlitePool>) { let _ = sqlite_access::sqlite::SqliteConnectOptions::from_url(url); }\n")
            execute(cargo_command("check", manifest, target, "--lib", "--features", "sqlite-access/pool,sqlite-access/url"), root)
            consumer_source.write_text("use sqlite_access::pool::connection;\n")
            execute(cargo_command("check", manifest, target, "--lib", "--features", "sqlite-access/pool"), root,
                    expected="error[E0603]")
            platform_consumer = work / "platform-consumer"
            (platform_consumer / "src").mkdir(parents=True)
            platform_manifest = platform_consumer / "Cargo.toml"
            platform_manifest.write_text('[package]\nname="sqlite-platform-direct"\nversion="0.0.0"\nedition="2024"\n'
                                         '[workspace]\nresolver="2"\n[dependencies]\n'
                                         f'platform-io={{path={json.dumps(str(root / "src/platform"))}}}\n')
            (platform_consumer / "src/lib.rs").write_text("pub use platform_io::sqlite::{open_index_connection, block_on_sqlite_connection};\n")
            lock(platform_manifest, root)
            check_direct_dependencies(platform_manifest, root)
            execute(cargo_command("check", platform_manifest, target, "--lib"), root)
            # Native deadline tests must not compete with each other for CPU.
            execute(cargo_command("test", root / "Cargo.toml", target, "-p", "platform-io", "--lib", "sqlite::", "--", "--test-threads=1"), root,
                    minimum_tests=13, required_tests=("candidate_reader_is_read_only_bounded_and_never_creates_a_database",
                                    "metadata_excess_is_detected_with_only_two_rows_materialized",
                                    "schema_budget_rejects_complete_excess_and_preserves_canonical_bytes",
                                    "direct_connection_without_an_async_runtime_closes_before_returning",
                                    "index_connection_applies_fixed_pragmas_and_defensive_mode_without_runtime"))
            execute(cargo_command("test", root / "Cargo.toml", target, "-p", "platform-io", "--lib", "--features", "sqlite-pool", "sqlite::", "--", "--test-threads=1"), root,
                    minimum_tests=27, required_tests=("explicit_creation_applies_pragmas_to_every_connection_and_reopens",
                                    "missing_existing_and_invalid_options_are_typed_and_do_not_create",
                                    "sqlx_adapter_verifies_metadata_shape_values_and_fingerprint",
                                    "foreign_key_check_reports_the_violating_rows",
                                    "pool_limits_apply_to_replacement_connections_and_invalid_creation_is_readonly",
                                    "foreign_key_evidence_stays_bounded_for_many_violations",
                                    "checkpoint_result_distinguishes_busy_and_incomplete",
                                    "live_reader_makes_truncate_checkpoint_report_busy",
                                    "explicit_private_run_directory_keeps_limits_and_cleans_temporary_files",
                                    "connection_limits_survive_pool_replacement_without_touching_source",
                                    "an_existing_capture_opens_the_same_generation_after_source_disappears",
                                    "reads_wal_only_commits_and_preserves_every_source_file",
                                    "active_writer_is_busy_instead_of_an_unlocked_snapshot",
                                    "unsafe_missing_oversized_and_invalid_sources_fail_without_source_changes"))
            (consumer / "src/lib.rs").write_text("use sqlite_access::query_engine;\n")
            execute(cargo_command("check", manifest, target, "--lib"), root, expected="error[E0603]")
            query = package_manifest.parent / "src/query_engine/mod.rs"
            before = query.read_bytes()
            try:
                query.write_bytes(before + b"\npub fn forbidden_probe() { unsafe {} }\n")
                execute(cargo_command("check", package_manifest, target, "--lib", "--all-features"), root,
                        expected="usage of an `unsafe` block")
            finally:
                query.write_bytes(before)
            execute(cargo_command("check", package_manifest, target, "--lib", "--all-features"), root)
    finally:
        assert root_lock.read_bytes() == original_lock, "SQLite gate changed the root lock"
        current = {p.relative_to(maintained_source).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                   for p in maintained_source.rglob("*") if p.is_file()}
        assert current == original_source, "SQLite gate changed maintained source files"
    print("SQLite family: direct, typed-URL and full-capability library tests, preserved pool integrations, platform pool regressions, minimal dependency closures and compiler boundaries verified; root lock and source unchanged.")


if __name__ == "__main__":
    check(ROOT)
