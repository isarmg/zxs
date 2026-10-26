"""Single-workspace, local-edge and immutable-origin policy regressions."""
from contextlib import redirect_stdout
from copy import deepcopy
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("source_manager", (Path(__file__).resolve().parents[3] / "tools/rust-sources" / "manage.py"))
manager = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manager)


class SourcePolicy(unittest.TestCase):
    def setUp(self):
        import proof_maintenance
        maintenance = patch.object(proof_maintenance, 'RECEIPTS', ())
        maintenance.start(); self.addCleanup(maintenance.stop)
        routing = patch.object(manager.proof_routing, "RECEIPTS", ())
        routing.start()
        self.addCleanup(routing.stop)
        temporary = tempfile.TemporaryDirectory(prefix="source-policy-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for directory in (".cargo", "docs", "tools/rust-sources", "src/app", "src/merged", "src/foundation/mini",
                          "compliance/empty-registry", "compliance/provenance/packages/mini", "compliance/licenses/upstream/mini"):
            (self.root / directory).mkdir(parents=True)
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = ["src/app"]\nexclude = ["src/foundation/mini"]\n')
        (self.root / "src/app/Cargo.toml").write_text(
            '[package]\nname = "app"\nversion = "1.0.0"\n[dependencies]\nmini = { path = "../foundation/mini" }\n')
        (self.root / ".cargo/config.toml").write_text(
            '[net]\noffline = true\n[source.crates-io]\nreplace-with = "empty"\n[source.empty]\ndirectory = "compliance/empty-registry"\n')
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "app"\nversion = "1.0.0"\n[[package]]\nname = "mini"\nversion = "1.0.0"\n')
        self.policy = dict(schema=2, workspaces=[dict(manifest="Cargo.toml", targets=["x86_64-unknown-linux-gnu"], purpose="fixture")],
                           profiles=[dict(id="native", manifests=["src/app/Cargo.toml"], targets=["x86_64-unknown-linux-gnu"], role="runtime", purpose="fixture")],
                           integrated_sources=[])
        self.write_json(manager.POLICY, self.policy)
        self.source = self.root / "src/foundation/mini"
        (self.source / "Cargo.toml").write_text('[package]\nname = "mini"\nversion = "1.0.0"\n')
        (self.source / "lib.rs").write_text("pub fn value() -> u8 { 1 }\n")
        (self.root / "src/merged/algorithm.rs").write_text("pub fn normalized() {}\n")
        (self.root / "compliance/provenance/packages/mini/Cargo.toml.orig.txt").write_text("original manifest bytes\n")
        (self.root / "compliance/licenses/upstream/mini/LICENSE").write_text("original copyright bytes\n")
        self.upstream = dict(name="mini", version="1.0.0", archive_url="https://static.crates.io/crates/mini/mini-1.0.0.crate", archive_sha256="a" * 64)
        self.merged_upstream = dict(name="normalization", version="2.0.0", archive_url="https://static.crates.io/crates/normalization/normalization-2.0.0.crate", archive_sha256="b" * 64)
        self.origin_records = [self.origin("old/mini", self.upstream), self.origin("old/normalization", self.merged_upstream)]
        self.write_json(manager.ORIGINS, dict(packages=self.origin_records))
        self.catalog = dict(packages={"src/foundation/mini": dict(name="mini", version="1.0.0", directory="src/foundation/mini",
                            repository="https://example.test/upstream", description="fixture", license="MIT", upstream=self.upstream,
                            origin_directory="old/mini", status="integrated", changes="path dependency", merged_into="foundation")},
                            merges=[dict(directory="src/merged", upstream=self.merged_upstream, origin_directory="old/normalization")])
        self.write_json(manager.CATALOG, self.catalog)
        self.metadata = dict(packages=[dict(id=name, name=name, manifest_path=str(path / "Cargo.toml"), source=None, targets=[{"kind": ["lib"]}])
                                       for name, path in (("app", self.root / "src/app"), ("mini", self.source))], workspace_members=["app"],
                             resolve={"nodes": [dict(id="app", features=[], deps=[dict(pkg="mini", dep_kinds=[{"kind": None}])]),
                                                dict(id="mini", features=["enabled"], deps=[])]})
        self.refresh()

    def origin(self, directory, upstream):
        return dict(origin_directory=directory, upstream=upstream,
                    original_files_sha256={"lib.rs": "c" * 64},
                    central_license_materials=["compliance/licenses/upstream/mini/LICENSE"],
                    central_original_metadata=["compliance/provenance/packages/mini/Cargo.toml.orig.txt"],
                    central_original_documentation=[])

    def write_json(self, relative, value):
        (self.root / relative).write_text(json.dumps(value, ensure_ascii=False))

    def save_catalog(self):
        self.write_json(manager.CATALOG, self.catalog)
        (self.root / manager.DOC).write_text(manager.document(self.catalog, self.policy))

    def refresh(self, **options):
        with patch.object(manager, "cargo_metadata", return_value=self.metadata), redirect_stdout(io.StringIO()):
            manager.refresh(self.root, **options)
        self.catalog = manager.read_json(self.root / manager.CATALOG)

    def verify(self, **options):
        with redirect_stdout(io.StringIO()):
            manager.verify(self.root, **options)

    def test_root_test_rejects_cargo_packages_and_workspaces(self):
        directory = self.root / "test/rogue"
        directory.mkdir(parents=True)
        for text in ('[package]\nname = "rogue"\nversion = "1.0.0"\n', '[workspace]\n'):
            (directory / "Cargo.toml").write_text(text)
            with self.assertRaisesRegex(ValueError, "forbidden beneath test"):
                manager.structure(self.root, self.policy)

    def test_root_test_rejects_scattered_documentation_and_legal_material(self):
        directory = self.root / "test/rogue"
        directory.mkdir(parents=True)
        for name in ('README.md', 'LICENSE', 'COPYRIGHT.txt'):
            path = directory / name
            path.write_text('scattered material')
            with self.assertRaisesRegex(ValueError, "scattered"):
                manager.structure(self.root, self.policy)
            path.unlink()

    def test_root_test_rejects_symlinked_subtrees(self):
        directory = self.root / "test"
        directory.mkdir()
        (directory / "linked").symlink_to(self.root / "src/app", target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            manager.structure(self.root, self.policy)

    def test_root_test_itself_cannot_be_a_symlink(self):
        with tempfile.TemporaryDirectory() as outside:
            (self.root / "test").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "test source root symlink"):
                manager.structure(self.root, self.policy)

    def test_complete_fixture_passes_without_registry_cache(self):
        self.verify()
        with patch.object(manager, "cargo_metadata", return_value=self.metadata):
            self.verify(metadata=True)

    def test_source_document_preserves_current_identity_and_customization_notice_without_history(self):
        self.catalog["packages"]["src/foundation/mini"]["changes"] = "2026-10-20 package review: docs/reports/old-review.md"
        self.catalog["merges"][0]["changes"] = "2026-10-21 merge implementation: docs/reports/old-merge.md"
        original = deepcopy(self.catalog)
        document = manager.document(self.catalog, self.policy)
        for required in ("mini@1.0.0", "normalization@2.0.0", "src/foundation/mini", "src/merged",
                         "SHA256 " + "a" * 64, "compliance/licenses/upstream/mini/LICENSE",
                         "本地定制", "../compliance/rust-sources.json", "packages / merges"):
            self.assertIn(required, document)
        self.assertNotIn("2026-10-20 package review", document)
        self.assertNotIn("2026-10-21 merge implementation", document)
        self.assertNotIn("docs/reports/", document)
        self.assertEqual(self.catalog, original, "documentation rendering must not rewrite provenance")

    def test_verification_member_is_explicit_and_non_default(self):
        self.policy["verification_members"] = ["src/foundation/mini"]
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["src/app", "src/foundation/mini"]\n'
            'default-members = ["src/app"]\n')
        manager.structure(self.root, self.policy)

    def test_rejects_unregistered_or_default_verification_member(self):
        self.policy["verification_members"] = ["src/foundation/mini"]
        for members, defaults in (
            ('["src/app"]', '["src/app"]'),
            ('["src/app", "src/foundation/mini"]', '["src/app", "src/foundation/mini"]'),
            ('["src/app", "src/foundation/mini"]', '[]'),
        ):
            with self.subTest(members=members, defaults=defaults):
                (self.root / "Cargo.toml").write_text(
                    f'[workspace]\nmembers = {members}\ndefault-members = {defaults}\n')
                with self.assertRaisesRegex(ValueError, 'explicit non-default'):
                    manager.structure(self.root, self.policy)

    def test_rejects_unrecorded_foundation_workspace_member(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["src/app", "src/foundation/mini"]\n'
            'default-members = ["src/app"]\n')
        with self.assertRaisesRegex(ValueError, 'explicit non-default'):
            manager.structure(self.root, self.policy)

    def test_native_target_policy_rejects_arm64_even_after_refresh(self):
        self.policy["workspaces"][0]["targets"].append("aarch64-unknown-linux-gnu")
        self.write_json(manager.POLICY, self.policy)
        with self.assertRaisesRegex(ValueError, "unsupported workspace target"):
            self.refresh()

    def test_native_profile_cannot_advertise_web_or_other_native_targets(self):
        for target in (manager.WEB_TARGET, "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-musl"):
            with self.subTest(target=target):
                self.policy["profiles"][0]["targets"] = [target]
                with self.assertRaisesRegex(ValueError, "unsupported profile target"):
                    manager.verify_target_policy(self.policy)

    def test_web_target_is_separate_from_native_platform_support(self):
        self.policy["workspaces"][0]["targets"].append(manager.WEB_TARGET)
        self.policy["profiles"].append(dict(id="web", targets=[manager.WEB_TARGET]))
        manager.verify_target_policy(self.policy)
        self.policy["profiles"][-1]["targets"] = [manager.NATIVE_TARGET]
        with self.assertRaisesRegex(ValueError, "unsupported profile target"):
            manager.verify_target_policy(self.policy)

    def test_target_coverage_cannot_advertise_unused_platforms(self):
        self.policy["workspaces"][0]["targets"].append(manager.WEB_TARGET)
        with self.assertRaisesRegex(ValueError, "workspace/profile target coverage drift"):
            manager.verify_target_policy(self.policy)

    def test_profile_target_cannot_be_duplicated(self):
        self.policy["profiles"][0]["targets"] *= 2
        with self.assertRaisesRegex(ValueError, "unsupported profile target"):
            manager.verify_target_policy(self.policy)

    def test_refresh_descriptions_is_idempotent_and_preserves_original_receipts(self):
        package = self.catalog["packages"]["src/foundation/mini"]
        package["changes"] = "See crates/zxs-naming/README.md."
        (self.root / "docs/components").mkdir()
        (self.root / "docs/components/naming.md").write_text("current documentation\n")
        self.save_catalog()
        original = self.catalog["origin_records_sha256"]
        with self.assertRaisesRegex(ValueError, "outdated source description"):
            self.verify()
        self.refresh()
        self.verify()
        first_catalog = (self.root / manager.CATALOG).read_bytes()
        first_doc = (self.root / manager.DOC).read_bytes()
        self.refresh()
        self.assertEqual((self.root / manager.CATALOG).read_bytes(), first_catalog)
        self.assertEqual((self.root / manager.DOC).read_bytes(), first_doc)
        self.assertEqual(self.catalog["origin_records_sha256"], original)

    def test_description_references_must_exist_even_if_generated_doc_matches(self):
        self.catalog["packages"]["src/foundation/mini"]["changes"] = "See docs/components/naming.md"
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "missing source description path"):
            self.verify()

    def test_repeated_provenance_prefix_is_rejected_even_with_matching_generated_doc(self):
        self.catalog["packages"]["src/foundation/mini"]["changes"] = (
            "See " + "compliance/provenance/shared/" * 16 + "vendor/web/dependencies/patches.json"
        )
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "outdated source description"):
            self.verify()

    def test_merged_descriptions_are_checked_too(self):
        self.catalog["merges"][0]["changes"] = "See crates/zxs-naming/README.md"
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "outdated source description"):
            self.verify()

    def test_changed_local_source_fails_until_explicit_refresh(self):
        (self.source / "lib.rs").write_text("pub fn value() -> u8 { 2 }\n")
        with self.assertRaisesRegex(ValueError, "source content drift"):
            self.verify()
        original = self.catalog["origin_records_sha256"]
        self.refresh()
        self.verify()
        self.assertEqual(self.catalog["origin_records_sha256"], original)

    def test_merged_source_is_receipted_without_independent_package(self):
        self.assertNotIn("src/merged", self.catalog["packages"])
        (self.root / "src/merged/algorithm.rs").write_text("changed merged algorithm\n")
        with self.assertRaisesRegex(ValueError, "merged source content drift"):
            self.verify()

    def test_same_directory_merge_preserves_primary_external_test_origin(self):
        before = self.install_external_tests()
        primary = "src/foundation/mini"
        self.catalog["merges"][0].update(directory=primary, files_sha256=manager.files(self.source))
        self.save_catalog()
        self.refresh()
        self.verify()
        self.assertEqual(self.catalog["external_tests"][primary], before)
        self.assertEqual(self.catalog["packages"][primary]["origin_directory"], "old/mini")
        self.assertEqual(self.catalog["merges"][0]["origin_directory"], "old/normalization")
        # Both identities authenticate the destination's whole current tree;
        # these equal receipts do not imply identical upstream ownership.
        self.assertEqual(self.catalog["merges"][0]["files_sha256"],
                         self.catalog["packages"][primary]["files_sha256"])
        self.assertEqual(manager.source_owners(self.catalog["packages"], self.catalog["merges"])[primary],
                         self.catalog["packages"][primary])
        self.refresh()
        self.verify()
        self.assertEqual(self.catalog["external_tests"][primary], before)

    def test_multiple_merged_origins_cannot_override_same_primary_owner(self):
        before = self.install_external_tests()
        primary = "src/foundation/mini"
        self.catalog["merges"][0].update(directory=primary, files_sha256=manager.files(self.source))
        other_upstream = dict(name="second-merged", version="3.0.0", archive_sha256="d" * 64,
                              archive_url="https://static.crates.io/crates/second-merged/second-merged-3.0.0.crate")
        self.origin_records.append(self.origin("old/second-merged", other_upstream))
        self.write_json(manager.ORIGINS, dict(packages=self.origin_records))
        self.catalog["merges"].append(dict(directory=primary, upstream=other_upstream,
                                          origin_directory="old/second-merged", files_sha256=manager.files(self.source)))
        self.save_catalog()
        self.refresh(); self.verify()
        self.assertEqual(self.catalog["external_tests"][primary], before)
        self.assertEqual({m["origin_directory"] for m in self.catalog["merges"]},
                         {"old/normalization", "old/second-merged"})
        for merged in self.catalog["merges"]:
            self.assertEqual(merged["files_sha256"], self.catalog["packages"][primary]["files_sha256"])
        self.catalog["merges"].reverse()
        self.save_catalog(); self.refresh(); self.verify()
        self.assertEqual(self.catalog["external_tests"][primary], before)

    def test_correcting_old_merge_owner_keeps_immutable_move_receipt_and_source_labels(self):
        # Reproduce the old last-merge-wins behavior, including a later internal
        # move receipt, before testing the current-owner correction.
        before = self.install_external_tests()
        primary = "src/foundation/mini"
        self.catalog["merges"][0].update(directory=primary, files_sha256=manager.files(self.source))
        self.save_catalog()
        legacy = lambda packages, merges: dict(packages, **{m["directory"]: m for m in merges})
        with patch.object(manager, "source_owners", side_effect=legacy):
            self.refresh()
            self.assertEqual(self.catalog["external_tests"][primary]["origin_directory"], "old/normalization")
            self.refresh(move_plan_data=self.internal_move_plan())
            self.verify()
        history = deepcopy(self.catalog["source_moves"])
        renames = deepcopy(self.catalog["renames"])
        originals = (self.root / manager.ORIGINS).read_bytes()
        self.refresh(); self.verify(); self.refresh(); self.verify()
        self.assertEqual(self.catalog["external_tests"][primary], before)
        self.assertEqual(self.catalog["source_moves"], history)
        self.assertEqual(self.catalog["renames"], renames)
        self.assertEqual((self.root / manager.ORIGINS).read_bytes(), originals)
        archived = next(iter(history.values()))["external_tests"]
        self.assertEqual(archived["origin_directory"], "old/normalization")
        self.assertEqual(archived["files"], before["files"])

    def test_duplicate_merged_origin_is_rejected_even_for_distinct_destination(self):
        duplicate = dict(self.catalog["merges"][0], directory="src/foundation/mini")
        self.catalog["merges"].append(duplicate)
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "historical active/merged/retired/superseded source coverage drift"):
            self.refresh()

    def test_merged_origin_cannot_also_be_retired(self):
        plan = self.retire_mini()
        self.catalog["merges"].append(dict(plan["packages"][0]["package"], directory="src/merged"))
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "historical active/merged/retired/superseded source coverage drift"):
            self.refresh(retire_plan=plan)

    def test_missing_source_manifest_fails(self):
        (self.source / "Cargo.toml").unlink()
        with self.assertRaises(ValueError):
            self.verify()

    def retire_mini(self):
        before = self.catalog["packages"]["src/foundation/mini"]
        size = sum(path.stat().st_size for path in self.source.rglob("*") if path.is_file())
        plan = dict(schema=1, packages=[dict(directory="src/foundation/mini", package=before,
                    reason="unsupported platform backend", category="platform", bytes=size,
                    file_count=len(before["files_sha256"]))])
        (self.root / "src/app/Cargo.toml").write_text('[package]\nname = "app"\nversion = "1.0.0"\n')
        (self.root / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "app"\nversion = "1.0.0"\n')
        self.metadata["packages"] = self.metadata["packages"][:1]
        self.metadata["resolve"]["nodes"] = [dict(id="app", features=[], deps=[])]
        shutil.rmtree(self.source)
        return plan

    def test_source_removal_requires_explicit_matching_retirement_plan(self):
        plan = self.retire_mini()
        with self.assertRaisesRegex(ValueError, "unregistered source directories"):
            self.refresh()
        with patch.object(manager, "cargo_metadata", return_value=self.metadata), redirect_stdout(io.StringIO()):
            manager.refresh(self.root, plan)
        self.catalog = manager.read_json(self.root / manager.CATALOG)
        self.verify()
        self.assertEqual(set(self.catalog["retired"]), {"src/foundation/mini"})
        self.assertEqual(len(self.catalog["upstream_identities"]), 2)
        self.assertEqual(self.catalog["origin_records_sha256"], manager.origin_hashes(manager.origins(self.root)))
        self.refresh()
        self.verify()

    def test_retirement_plan_rejects_altered_last_source_receipt(self):
        plan = self.retire_mini()
        plan["packages"][0]["package"] = dict(plan["packages"][0]["package"], name="another")
        with self.assertRaisesRegex(ValueError, "last source receipt"):
            manager.refresh(self.root, plan)

    def test_retired_source_cannot_silently_return_or_change_its_receipt(self):
        plan = self.retire_mini()
        with patch.object(manager, "cargo_metadata", return_value=self.metadata), redirect_stdout(io.StringIO()):
            manager.refresh(self.root, plan)
        self.catalog = manager.read_json(self.root / manager.CATALOG)
        self.source.mkdir()
        with self.assertRaisesRegex(ValueError, "retired source has returned"):
            self.refresh()
        self.source.rmdir()
        self.catalog["retired"]["src/foundation/mini"]["retirement"]["reason"] = "rewritten history"
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "retired source receipt drift"):
            self.refresh()

    def test_retirement_does_not_allow_remaining_local_dependency_edge(self):
        plan = self.retire_mini()
        (self.root / "src/app/Cargo.toml").write_text(
            '[package]\nname = "app"\nversion = "1.0.0"\n[dependencies]\nmini = { path = "../foundation/mini" }\n')
        with self.assertRaisesRegex(ValueError, "missing local dependency"):
            manager.refresh(self.root, plan)

    def install_external_tests(self):
        directory = "src/foundation/mini"
        self.external_root = self.root / "test/foundation/mini"
        self.external_root.mkdir(parents=True)
        data = b"assert!(true);\n"
        (self.external_root / "tests.rs").write_bytes(data)
        owner = dict(directory=directory, roots=["test/foundation/mini"], files=[dict(
            source=directory + "/lib.rs", path="test/foundation/mini/tests.rs",
            source_sha256_before=self.catalog["packages"][directory]["files_sha256"]["lib.rs"],
            sha256=manager.digest(data))])
        self.external_mapping = self.root / "compliance/external-tests/mini.json"
        self.external_mapping.parent.mkdir(parents=True)
        self.external_mapping.write_text(json.dumps(dict(schema_version=1, owners=[owner])))
        owners, receipts = manager.external_tests.load(self.root, self.catalog["packages"])
        self.catalog.update(external_tests=owners, external_test_mappings_sha256=receipts)
        self.save_catalog()
        # This is an already-verified synthetic last receipt, not the repository's
        # real bootstrap corpus. Only the unrelated initial-proof reader is stubbed.
        stub = patch.object(manager.source_relocation, "read_receipt", return_value={})
        stub.start(); self.addCleanup(stub.stop)
        self.verify()
        return deepcopy(owners[directory])

    def test_catalog_four_without_rename_history_remains_verifiable(self):
        self.catalog['schema'] = 4
        self.catalog.pop('renames', None); self.catalog.pop('rename_records_sha256', None)
        self.save_catalog(); self.verify(); self.refresh(); self.verify()
        self.assertEqual(self.catalog['schema'], 7)
        self.assertEqual(self.catalog['renames'], {})

    def rename_mini(self):
        owner = self.install_external_tests()
        old, new = 'src/foundation/mini', 'src/foundation/value-core'
        before = deepcopy(self.catalog['packages'][old])
        destination = self.root / new
        self.source.rename(destination)
        self.source = destination
        manifest = destination / 'Cargo.toml'
        manifest.write_text(manifest.read_text().replace('name = "mini"', 'name = "value-core"'))
        test_new = self.root / 'test/foundation/value-core'
        self.external_root.rename(test_new)
        self.external_root = test_new
        after_owner = deepcopy(owner)
        after_owner.update(directory=new, roots=['test/foundation/value-core'])
        after_owner['files'][0]['path'] = 'test/foundation/value-core/tests.rs'
        mapping_owner = {k: v for k, v in after_owner.items() if k not in ('origin_directory', 'bytes', 'file_count')}
        self.external_mapping.write_text(json.dumps(dict(schema_version=1, owners=[mapping_owner])))
        for path in ('Cargo.toml', 'src/app/Cargo.toml'):
            file = self.root / path
            file.write_text(file.read_text().replace('foundation/mini', 'foundation/value-core').replace('mini = {', 'value-core = {'))
        lock = self.root / 'Cargo.lock'
        lock.write_text(lock.read_text().replace('"mini"', '"value-core"'))
        self.metadata['packages'][1].update(name='value-core', manifest_path=str(destination / 'Cargo.toml'))
        item = dict(directory=old, package=before, destination_directory=new,
                    destination_name='value-core', reason='Functional responsibility',
                    files_sha256=manager.files(destination), external_tests=owner,
                    destination_external_tests=after_owner)
        return dict(schema=1, packages=[item])

    def test_explicit_rename_preserves_original_sources_and_refresh_history(self):
        plan = self.rename_mini()
        origins = (self.root / manager.ORIGINS).read_bytes()
        self.refresh(rename_plan_data=plan)
        self.verify()
        self.assertEqual(self.catalog['schema'], 7)
        self.assertEqual(self.catalog['renames']['src/foundation/mini']['package'], plan['packages'][0]['package'])
        owner = self.catalog['external_tests']['src/foundation/value-core']
        self.assertEqual(owner['files'][0]['source'], 'src/foundation/mini/lib.rs')
        self.assertEqual((self.root / manager.ORIGINS).read_bytes(), origins)
        history = deepcopy(self.catalog['renames'])
        self.refresh(); self.verify()
        self.assertEqual(self.catalog['renames'], history)

    def test_rename_requires_plan_and_exact_current_external_attribution(self):
        plan = self.rename_mini()
        with self.assertRaisesRegex(ValueError, 'unregistered source'):
            self.refresh()
        mapping = json.loads(self.external_mapping.read_text())
        mapping['owners'][0]['files'][0]['source'] = 'src/foundation/value-core/lib.rs'
        self.external_mapping.write_text(json.dumps(mapping))
        with self.assertRaisesRegex(ValueError, 'reviewed rename plan'):
            self.refresh(rename_plan_data=plan)

    def test_rename_history_tamper_or_name_drift_rejected(self):
        plan = self.rename_mini(); self.refresh(rename_plan_data=plan)
        self.catalog['renames']['src/foundation/mini']['reason'] = 'rewritten'
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, 'rename receipt drift'):
            self.verify()
        with self.assertRaisesRegex(ValueError, 'rename receipt drift'):
            self.refresh()

    def test_renamed_external_owner_can_be_explicitly_retired(self):
        plan = self.rename_mini(); self.refresh(rename_plan_data=plan)
        new = 'src/foundation/value-core'
        package = self.catalog['packages'][new]
        owner = self.catalog['external_tests'][new]
        retirement = dict(schema=1, packages=[dict(directory=new, package=package,
            reason='unsupported platform backend', category='platform',
            bytes=sum(p.stat().st_size for p in self.source.rglob('*') if p.is_file()),
            file_count=len(package['files_sha256']), external_action='remove', external_tests=owner)])
        (self.root / 'src/app/Cargo.toml').write_text('[package]\nname = "app"\nversion = "1.0.0"\n')
        (self.root / 'Cargo.lock').write_text('version = 4\n[[package]]\nname = "app"\nversion = "1.0.0"\n')
        self.metadata['packages'] = self.metadata['packages'][:1]
        self.metadata['resolve']['nodes'] = [dict(id='app', features=[], deps=[])]
        shutil.rmtree(self.source); self.remove_external_tests()
        self.refresh(retire_plan=retirement); self.verify(); self.refresh(); self.verify()
        self.assertEqual(self.catalog['retired'][new]['retirement']['external_tests'], owner)

    def test_rename_cannot_combine_retirement_or_upgrade(self):
        plan = self.rename_mini()
        for key in ('retire_plan', 'upgrade_plan_data'):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'separate reviewed'):
                self.refresh(rename_plan_data=plan, **{key: dict(packages=[dict(directory='src/foundation/value-core')])})

    def internal_move_plan(self, before_file='lib.rs', after_file='value.rs', move_external=False):
        directory = self.source.relative_to(self.root).as_posix()
        package = deepcopy(self.catalog['packages'][directory])
        (self.source / before_file).rename(self.source / after_file)
        owner = deepcopy(self.catalog.get('external_tests', {}).get(directory))
        after_owner = deepcopy(owner)
        external_moves = {}
        if move_external:
            old_path = owner['files'][0]['path']
            new_path = str(Path(old_path).with_name('value_test.rs'))
            (self.root / old_path).rename(self.root / new_path)
            after_owner['files'][0]['path'] = new_path
            external_moves[old_path] = new_path
            mapping = {k: v for k, v in after_owner.items() if k not in ('origin_directory', 'bytes', 'file_count')}
            self.external_mapping.write_text(json.dumps(dict(schema_version=1, owners=[mapping])))
        return dict(schema=1, packages=[dict(directory=directory, package=package,
            merged_packages=[deepcopy(p) for p in self.catalog.get('merges', []) if p['directory'] == directory],
            reason='Functional internal filenames', files_sha256=manager.files(self.source),
            file_moves={before_file: after_file}, external_tests=owner,
            destination_external_tests=after_owner, external_file_moves=external_moves)])

    def test_internal_move_refresh_verify_and_repeat_preserve_history(self):
        plan = self.internal_move_plan()
        original = (self.root / manager.ORIGINS).read_bytes()
        self.refresh(move_plan_data=plan); self.verify()
        self.assertEqual(self.catalog['schema'], 7)
        history = deepcopy(self.catalog['source_moves'])
        self.refresh(); self.verify()
        self.assertEqual(self.catalog['source_moves'], history)
        second = self.internal_move_plan('value.rs', 'codec.rs')
        self.refresh(move_plan_data=second); self.verify()
        self.assertEqual(len(self.catalog['source_moves']), 2)
        for key, value in history.items():
            self.assertEqual(self.catalog['source_moves'][key], value)
        self.assertEqual((self.root / manager.ORIGINS).read_bytes(), original)

    def test_internal_move_after_root_rename_keeps_rename_receipt(self):
        rename = self.rename_mini(); self.refresh(rename_plan_data=rename)
        frozen = deepcopy(self.catalog['renames'])
        plan = self.internal_move_plan(move_external=True)
        old_source = plan['packages'][0]['external_tests']['files'][0]['source']
        self.refresh(move_plan_data=plan); self.verify(); self.refresh(); self.verify()
        self.assertEqual(self.catalog['renames'], frozen)
        self.assertEqual(self.catalog['external_tests']['src/foundation/value-core']['files'][0]['source'], old_source)
        self.assertEqual(len(self.catalog['source_moves']), 1)

    def test_internal_move_without_plan_rejected(self):
        self.internal_move_plan()
        with self.assertRaisesRegex(ValueError, 'unaccounted source removal'):
            self.refresh()

    def test_internal_move_tamper_rejected(self):
        self.refresh(move_plan_data=self.internal_move_plan())
        next(iter(self.catalog['source_moves'].values()))['reason'] = 'changed history'
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, 'internal move receipt drift'):
            self.verify()
        with self.assertRaisesRegex(ValueError, 'internal move receipt drift'):
            self.refresh()

    def test_internal_move_cannot_combine_same_owner_retirement(self):
        plan = self.internal_move_plan()
        with self.assertRaisesRegex(ValueError, 'separate reviewed'):
            self.refresh(move_plan_data=plan, retire_plan=dict(schema=1, packages=[dict(directory='src/foundation/mini')]))

    def test_internal_move_integrated_source_and_merged_overlap(self):
        new = 'tools/helper'
        destination = self.root / new
        self.source.rename(destination); self.source = destination
        package = self.catalog['packages'].pop('src/foundation/mini')
        package['directory'] = new
        self.catalog['packages'][new] = package
        self.catalog['merges'][0].update(directory=new, files_sha256=deepcopy(package['files_sha256']))
        self.policy['integrated_sources'] = [new]
        self.write_json(manager.POLICY, self.policy)
        app = self.root / 'src/app/Cargo.toml'
        app.write_text(app.read_text().replace('../foundation/mini', '../../tools/helper'))
        root = self.root / 'Cargo.toml'
        root.write_text(root.read_text().replace('src/foundation/mini', new))
        self.metadata['packages'][1]['manifest_path'] = str(destination / 'Cargo.toml')
        self.save_catalog(); self.refresh(); self.verify()
        plan = self.internal_move_plan()
        self.assertEqual(len(plan['packages'][0]['merged_packages']), 1)
        self.refresh(move_plan_data=plan); self.verify(); self.refresh(); self.verify()
        self.assertEqual(self.catalog['merges'][0]['files_sha256'], self.catalog['packages'][new]['files_sha256'])
        receipt = next(iter(self.catalog['source_moves'].values()))
        self.assertEqual(receipt['merged_packages'], plan['packages'][0]['merged_packages'])

    def test_move_plan_cli_refresh_routing_and_verify_rejection(self):
        path = self.root / 'move-plan.json'
        self.write_json('move-plan.json', dict(schema=1, packages=[]))
        with patch.object(manager.sys, 'argv', ['manage.py', 'refresh', '--move-plan', str(path)]), patch.object(manager, 'refresh') as call:
            self.assertEqual(manager.main(), 0)
            self.assertEqual(call.call_args.args[-2], dict(schema=1, packages=[]))
        with patch.object(manager.sys, 'argv', ['manage.py', 'verify', '--move-plan', str(path)]), patch.object(manager.sys, 'stderr', io.StringIO()):
            self.assertEqual(manager.main(), 1)

    def remove_external_tests(self):
        shutil.rmtree(self.external_root)
        self.external_mapping.write_text(json.dumps(dict(schema_version=1, owners=[])))

    def test_external_retirement_archives_exact_last_material_and_is_idempotent(self):
        old = self.install_external_tests()
        plan = self.retire_mini()
        plan["packages"][0].update(external_action="remove", external_tests=old)
        self.remove_external_tests()
        self.refresh(retire_plan=plan)
        self.verify()
        archived = self.catalog["retired"]["src/foundation/mini"]["retirement"]["external_tests"]
        self.assertEqual(archived, old)
        self.assertEqual(archived["file_count"], 1)
        self.assertEqual(archived["bytes"], len(b"assert!(true);\n"))
        self.assertEqual(self.catalog["external_tests"], {})
        before = deepcopy(self.catalog["retired_records_sha256"])
        self.refresh(); self.verify()
        self.assertEqual(self.catalog["retired_records_sha256"], before)

    def test_external_retirement_requires_exact_explicit_disposition(self):
        old = self.install_external_tests()
        plan = self.retire_mini(); self.remove_external_tests()
        with self.assertRaisesRegex(ValueError, "explicit external test removal"):
            self.refresh(retire_plan=plan)
        plan["packages"][0].update(external_action="remove", external_tests=dict(old, bytes=0))
        with self.assertRaisesRegex(ValueError, "explicit external test removal"):
            self.refresh(retire_plan=plan)

    def test_external_retirement_rejects_leftover_material_or_mapping(self):
        old = self.install_external_tests(); plan = self.retire_mini()
        plan["packages"][0].update(external_action="remove", external_tests=old)
        with self.assertRaisesRegex(ValueError, "requires absent root"):
            self.refresh(retire_plan=plan)
        shutil.rmtree(self.external_root)
        with self.assertRaisesRegex(ValueError, "unknown/duplicate external test owner"):
            self.refresh(retire_plan=plan)

    def test_external_ordinary_refresh_still_rejects_owner_disappearance(self):
        self.install_external_tests(); self.remove_external_tests()
        with self.assertRaisesRegex(ValueError, "accounting shrank"):
            self.refresh()

    def test_external_retirement_rejects_history_tampering_and_reappearance(self):
        old = self.install_external_tests(); plan = self.retire_mini()
        plan["packages"][0].update(external_action="remove", external_tests=old)
        self.remove_external_tests(); self.refresh(retire_plan=plan)
        self.external_root.mkdir()
        with self.assertRaisesRegex(ValueError, "requires absent root"):
            self.verify()
        self.external_root.rmdir()
        self.catalog["retired"]["src/foundation/mini"]["retirement"]["external_tests"]["bytes"] += 1
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "immutable retired source receipt"):
            self.refresh()

    def test_external_upgrade_requires_removal_and_archives_prior_identity(self):
        old = self.install_external_tests(); plan = self.upgrade_mini()
        with self.assertRaisesRegex(ValueError, "explicit external test removal"):
            self.refresh(upgrade_plan_data=plan)
        plan["packages"][0].update(external_action="remove", external_tests=old)
        with self.assertRaisesRegex(ValueError, "requires absent root"):
            self.refresh(upgrade_plan_data=plan)
        self.remove_external_tests(); self.refresh(upgrade_plan_data=plan); self.verify()
        record = self.catalog["superseded"][old["origin_directory"]]
        self.assertEqual(record["external_tests"], old)
        self.assertNotEqual(record["external_tests"]["origin_directory"], self.catalog["packages"]["src/foundation/mini"]["origin_directory"])
        self.refresh(); self.verify()
        self.catalog["superseded"][old["origin_directory"]]["external_tests"]["files"][0]["sha256"] = "0" * 64
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "immutable superseded source receipt"):
            self.refresh()

    def test_successor_tests_can_be_separately_relocated_without_relabeling_history(self):
        old = self.install_external_tests(); plan = self.upgrade_mini()
        plan["packages"][0].update(external_action="remove", external_tests=old)
        self.remove_external_tests(); self.refresh(upgrade_plan_data=plan)
        historical = deepcopy(self.catalog["superseded"])
        directory = "src/foundation/mini"
        source = "src/lib.rs"
        data = (self.source / source).read_bytes()
        self.external_root.mkdir()
        (self.external_root / "successor.rs").write_bytes(data)
        owner = dict(directory=directory, roots=["test/foundation/mini"], files=[dict(
            source=directory + "/" + source, path="test/foundation/mini/successor.rs",
            source_sha256_before=manager.digest(data), sha256=manager.digest(data))])
        self.external_mapping.write_text(json.dumps(dict(schema_version=1, owners=[owner])))
        (self.source / source).unlink()
        self.refresh(); self.verify()
        self.assertEqual(self.catalog["superseded"], historical)
        self.assertEqual(self.catalog["external_tests"][directory]["origin_directory"], "releases/mini-1.1.0")
        self.assertEqual(historical[old["origin_directory"]]["external_tests"]["origin_directory"], old["origin_directory"])

    def test_explicit_removal_does_not_exempt_an_unrelated_external_owner(self):
        old = self.install_external_tests()
        other_root = self.root / "test/merged"
        other_root.mkdir()
        data = b"assert!(true);\n"; (other_root / "tests.rs").write_bytes(data)
        other = dict(directory="src/merged", roots=["test/merged"], files=[dict(
            source="src/merged/algorithm.rs", path="test/merged/tests.rs",
            source_sha256_before=self.catalog["merges"][0]["files_sha256"]["algorithm.rs"], sha256=manager.digest(data))])
        mapping = json.loads(self.external_mapping.read_text()); mapping["owners"].append(other)
        self.external_mapping.write_text(json.dumps(mapping))
        packages = dict(self.catalog["packages"], **{m["directory"]: m for m in self.catalog["merges"]})
        owners, hashes = manager.external_tests.load(self.root, packages)
        self.catalog.update(external_tests=owners, external_test_mappings_sha256=hashes); self.save_catalog()
        plan = self.retire_mini(); plan["packages"][0].update(external_action="remove", external_tests=old)
        self.remove_external_tests(); shutil.rmtree(other_root)
        with self.assertRaisesRegex(ValueError, "accounting shrank: src/merged"):
            self.refresh(retire_plan=plan)

    def test_external_removal_rejects_symlinked_former_root(self):
        old = self.install_external_tests(); plan = self.retire_mini()
        plan["packages"][0].update(external_action="remove", external_tests=old)
        self.remove_external_tests()
        with tempfile.TemporaryDirectory() as outside:
            self.external_root.symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlink"):
                self.refresh(retire_plan=plan)

    def test_full_file_restoration_preserves_source_and_bootstrap_history(self):
        self.install_external_tests()
        data = (self.source / "lib.rs").read_bytes(); digest = manager.digest(data)
        (self.external_root / "tests.rs").write_bytes(data)
        (self.source / "lib.rs").unlink()
        mapping = json.loads(self.external_mapping.read_text())
        mapping["owners"][0]["files"][0].update(sha256=digest, source_sha256_before=digest)
        self.external_mapping.write_text(json.dumps(mapping))
        self.catalog["packages"]["src/foundation/mini"]["files_sha256"].pop("lib.rs")
        owners, hashes = manager.external_tests.load(self.root, self.catalog["packages"])
        self.catalog.update(external_tests=owners, external_test_mappings_sha256=hashes)
        self.save_catalog(); self.verify()
        (self.external_root / "tests.rs").rename(self.source / "lib.rs")
        self.external_mapping.write_text(json.dumps(dict(schema_version=1, owners=[])))
        self.refresh(); self.verify()
        self.assertEqual(self.catalog["external_tests"], {})
        self.assertEqual(self.catalog["packages"]["src/foundation/mini"]["files_sha256"]["lib.rs"], digest)
        self.assertEqual(self.catalog["initial_test_relocation_sha256"], manager.source_relocation.RECEIPT_SHA256)
        with patch.object(manager.source_relocation, "read_receipt", side_effect=ValueError("proof removed")):
            with self.assertRaisesRegex(ValueError, "proof removed"):
                self.refresh()
        self.refresh(); self.verify()

    def upgrade_mini(self, version="1.1.0"):
        before = deepcopy(self.catalog["packages"]["src/foundation/mini"])
        upstream = dict(name="mini", version=version,
                        archive_url=f"https://static.crates.io/crates/mini/mini-{version}.crate",
                        archive_sha256=manager.digest(version.encode()))
        origin = self.origin(f"releases/mini-{version}", upstream)
        source = {"src/lib.rs": f'pub const VERSION: &str = "{version}";\n',
                  "src/helper.rs": "pub fn helper() {}\n"}
        origin["original_files_sha256"] = {path: manager.digest(data.encode()) for path, data in source.items()}
        origin["vcs"] = {"git": {"sha1": "new-release"}}
        self.origin_records.append(origin)
        self.write_json(manager.ORIGINS, dict(packages=self.origin_records))
        (self.source / "src").mkdir(exist_ok=True)
        (self.source / "lib.rs").unlink(missing_ok=True)
        for path, data in source.items():
            (self.source / path).write_text(data)
        (self.source / "Cargo.toml").write_text(f'[package]\nname = "mini"\nversion = "{version}"\n')
        lock = self.root / "Cargo.lock"
        lock.write_text(lock.read_text().replace(f'name = "mini"\nversion = "{before["version"]}"',
                                                f'name = "mini"\nversion = "{version}"'))
        return dict(schema=1, packages=[dict(directory="src/foundation/mini", package=before,
                    successor_origin_directory=origin["origin_directory"], reason="official security fix",
                    source_prefixes=["src/"])])

    def test_upgrade_preserves_exact_history_and_refresh_is_idempotent(self):
        plan = self.upgrade_mini()
        before = deepcopy(plan["packages"][0]["package"])
        prior_hashes = deepcopy(self.catalog["origin_records_sha256"])
        self.refresh(upgrade_plan_data=plan)
        self.verify()
        receipt = self.catalog["superseded"]["old/mini"]
        self.assertEqual(receipt["package"], before)
        self.assertEqual(receipt["source_prefixes"], ["src/"])
        current = self.catalog["packages"]["src/foundation/mini"]
        self.assertEqual(current["origin_directory"], "releases/mini-1.1.0")
        self.assertEqual(current["upstream"]["version"], "1.1.0")
        self.assertEqual(current["vcs"], {"git": {"sha1": "new-release"}})
        self.assertTrue(prior_hashes.items() <= self.catalog["origin_records_sha256"].items())
        document = (self.root / manager.DOC).read_text()
        self.assertNotIn("mini@1.0.0", document)
        self.assertIn("mini@1.1.0", document)
        first = (self.root / manager.CATALOG).read_bytes()
        self.refresh()
        self.verify()
        self.assertEqual((self.root / manager.CATALOG).read_bytes(), first)

    def test_new_origin_requires_explicit_upgrade_plan(self):
        self.upgrade_mini()
        with self.assertRaisesRegex(ValueError, "source coverage drift"):
            self.refresh()

    def test_upgrade_rejects_altered_prior_receipt_and_repeated_plan(self):
        plan = self.upgrade_mini()
        altered = deepcopy(plan)
        altered["packages"][0]["package"]["description"] = "rewritten history"
        with self.assertRaisesRegex(ValueError, "last source receipt"):
            self.refresh(upgrade_plan_data=altered)
        self.refresh(upgrade_plan_data=plan)
        with self.assertRaisesRegex(ValueError, "last source receipt"):
            self.refresh(upgrade_plan_data=plan)

    def test_upgrade_requires_new_same_package_origin_and_new_version(self):
        plan = self.upgrade_mini()
        item = plan["packages"][0]
        new_key = item["successor_origin_directory"]
        for successor in ("missing", "old/mini", "old/normalization"):
            with self.subTest(successor=successor):
                item["successor_origin_directory"] = successor
                with self.assertRaisesRegex(ValueError, "new same-package origin"):
                    self.refresh(upgrade_plan_data=plan)
        item["successor_origin_directory"] = new_key
        for upstream in (dict(self.upstream), dict(self.merged_upstream)):
            with self.subTest(upstream=upstream):
                self.origin_records[-1]["upstream"] = upstream
                self.write_json(manager.ORIGINS, dict(packages=self.origin_records))
                with self.assertRaisesRegex(ValueError, "new same-package origin"):
                    self.refresh(upgrade_plan_data=plan)

    def test_upgrade_rejects_duplicate_paths_and_empty_reason(self):
        plan = self.upgrade_mini()
        duplicate = deepcopy(plan)
        duplicate["packages"] *= 2
        with self.assertRaisesRegex(ValueError, "duplicate source upgrade"):
            self.refresh(upgrade_plan_data=duplicate)
        plan["packages"][0]["reason"] = "  "
        with self.assertRaisesRegex(ValueError, "requires a reason"):
            self.refresh(upgrade_plan_data=plan)

    def test_upgrade_requires_manifest_to_match_successor_version_and_local_name(self):
        plan = self.upgrade_mini()
        for name, version in (("mini", "1.0.0"), ("another", "1.1.0")):
            with self.subTest(name=name, version=version):
                (self.source / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "{version}"\n')
                with self.assertRaisesRegex(ValueError, "manifest does not match successor"):
                    self.refresh(upgrade_plan_data=plan)

    def test_upgrade_requires_safe_explicit_complete_source_prefixes(self):
        plan = self.upgrade_mini()
        for prefixes in (None, [], ["src"], ["../"], ["/src/"], ["src/", "src/"], ["src/", "src/nested/"]):
            with self.subTest(prefixes=prefixes):
                plan["packages"][0]["source_prefixes"] = prefixes
                with self.assertRaises(ValueError):
                    self.refresh(upgrade_plan_data=plan)
        plan["packages"][0]["source_prefixes"] = ["missing/"]
        with self.assertRaisesRegex(ValueError, "differs from successor release"):
            self.refresh(upgrade_plan_data=plan)

    def test_upgrade_rejects_missing_modified_and_extra_release_source(self):
        plan = self.upgrade_mini()
        path = self.source / "src/helper.rs"
        original = path.read_bytes()
        path.unlink()
        with self.assertRaisesRegex(ValueError, "differs from successor release"):
            self.refresh(upgrade_plan_data=plan)
        path.write_text("not the official fix\n")
        with self.assertRaisesRegex(ValueError, "differs from successor release"):
            self.refresh(upgrade_plan_data=plan)
        path.write_bytes(original)
        (self.source / "src/extra.rs").write_text("unreviewed code\n")
        with self.assertRaisesRegex(ValueError, "differs from successor release"):
            self.refresh(upgrade_plan_data=plan)

    def test_upgrade_does_not_allow_rebaselining_prior_origin_or_legal_material(self):
        plan = self.upgrade_mini()
        self.origin_records[0]["original_files_sha256"]["lib.rs"] = "d" * 64
        self.write_json(manager.ORIGINS, dict(packages=self.origin_records))
        with self.assertRaisesRegex(ValueError, "immutable historical origin"):
            self.refresh(upgrade_plan_data=plan)
        self.origin_records[0]["original_files_sha256"]["lib.rs"] = "c" * 64
        self.write_json(manager.ORIGINS, dict(packages=self.origin_records))
        (self.root / "compliance/licenses/upstream/mini/LICENSE").write_text("rewritten license\n")
        with self.assertRaisesRegex(ValueError, "immutable original material"):
            self.refresh(upgrade_plan_data=plan)

    def test_superseded_receipts_cannot_change_during_refresh(self):
        self.refresh(upgrade_plan_data=self.upgrade_mini())
        self.catalog["superseded"]["old/mini"]["reason"] = "rewritten history"
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "immutable superseded source receipt"):
            self.refresh()

    def test_upgraded_source_may_be_edited_but_cannot_be_silently_reversioned(self):
        self.refresh(upgrade_plan_data=self.upgrade_mini())
        before = deepcopy(self.catalog["superseded"])
        (self.source / "src/lib.rs").write_text("deliberate local patch\n")
        self.refresh()
        self.verify()
        self.assertEqual(self.catalog["superseded"], before)
        (self.source / "Cargo.toml").write_text('[package]\nname="mini"\nversion="1.0.0"\n')
        with self.assertRaisesRegex(ValueError, "upgraded source version drift"):
            self.refresh()

    def test_successive_upgrades_preserve_old_links_and_allow_eventual_retirement(self):
        self.refresh(upgrade_plan_data=self.upgrade_mini())
        first_receipt = deepcopy(self.catalog["superseded"]["old/mini"])
        first_hash = self.catalog["superseded_records_sha256"]["old/mini"]
        self.refresh(upgrade_plan_data=self.upgrade_mini("1.2.0"))
        self.verify()
        self.assertEqual(self.catalog["superseded"]["old/mini"], first_receipt)
        self.assertEqual(self.catalog["superseded_records_sha256"]["old/mini"], first_hash)
        self.assertEqual(set(self.catalog["superseded"]), {"old/mini", "releases/mini-1.1.0"})
        self.refresh(retire_plan=self.retire_mini())
        self.verify()
        self.refresh()
        self.verify()

    def test_superseded_successors_cannot_point_to_missing_wrong_path_or_cyclic_history(self):
        self.refresh(upgrade_plan_data=self.upgrade_mini())
        self.refresh(upgrade_plan_data=self.upgrade_mini("1.2.0"))
        original = deepcopy(self.catalog)
        for successor, expected in (("missing", "invalid superseded source successor|no current successor"),
                                    ("old/mini", "cyclic superseded source history")):
            with self.subTest(successor=successor):
                self.catalog = deepcopy(original)
                self.catalog["superseded"]["releases/mini-1.1.0"]["successor_origin_directory"] = successor
                self.catalog["superseded_records_sha256"] = manager.origin_hashes(self.catalog["superseded"])
                self.save_catalog()
                with self.assertRaisesRegex(ValueError, expected):
                    self.verify()
        self.catalog = deepcopy(original)
        self.catalog["superseded"]["releases/mini-1.1.0"]["package"]["directory"] = "src/foundation/other"
        self.catalog["superseded_records_sha256"] = manager.origin_hashes(self.catalog["superseded"])
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "superseded source path drift"):
            self.verify()

    def test_superseded_origins_must_be_unique_and_cover_all_history(self):
        self.refresh(upgrade_plan_data=self.upgrade_mini())
        original = deepcopy(self.catalog)
        self.catalog["superseded"] = {}
        self.catalog["superseded_records_sha256"] = {}
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "source coverage drift"):
            self.verify()
        self.catalog = original
        self.catalog["merges"].append(deepcopy(self.catalog["superseded"]["old/mini"]["package"]))
        with self.assertRaisesRegex(ValueError, "source coverage drift"):
            manager.verify_origin_coverage(self.catalog, manager.origins(self.root))

    def test_source_link_is_rejected_even_with_target_inside_repository(self):
        (self.source / "alias.rs").symlink_to("lib.rs")
        with self.assertRaisesRegex(ValueError, "source symlink"):
            self.verify()

    def test_special_file_is_rejected_without_reading_it(self):
        if not hasattr(os, "mkfifo"):
            self.skipTest("platform has no FIFO")
        os.mkfifo(self.source / "pipe.rs")
        with self.assertRaisesRegex(ValueError, "source special file"):
            self.verify()

    def test_inventory_paths_reject_escape_and_nonportable_spellings(self):
        for path in ("../outside", "/absolute/path", "a/../b", "a//b", "a\\b", "C:/file", "./file"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                manager.safe_path(self.root, path)
        (self.root / "alias").symlink_to(self.source, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            manager.safe_path(self.root, "alias/lib.rs")

    def test_added_independent_workspace_is_rejected(self):
        (self.root / "tools/unregistered").mkdir()
        (self.root / "tools/unregistered/Cargo.toml").write_text("[workspace]\n")
        with self.assertRaisesRegex(ValueError, "workspace"):
            self.verify()

    def test_nested_lockfile_and_nested_git_file_or_directory_fail(self):
        for name, directory in (("Cargo.lock", False), (".git", False), (".git", True)):
            path = self.source / name
            if directory:
                path.mkdir()
            else:
                path.write_text("forbidden nested root\n")
            with self.subTest(name=name, directory=directory), self.assertRaisesRegex(ValueError, "nested"):
                self.verify()
            path.rmdir() if directory else path.unlink()

    def test_scattered_docs_legal_texts_and_development_roots_are_rejected(self):
        for name, directory in (("README.md", False), ("LICENSE", False), (".github", True), ("LICENSES", True)):
            path = self.source / name
            path.mkdir() if directory else path.write_text("scattered metadata\n")
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "scattered|scaffolding"):
                self.verify()
            path.rmdir() if directory else path.unlink()

    def test_missing_manifest_receipt_fails(self):
        del self.catalog["manifests_sha256"]["Cargo.toml"]
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "manifest inventory drift"):
            self.verify()

    def test_first_party_manifest_drift_without_lock_change_is_rejected(self):
        with (self.root / "src/app/Cargo.toml").open("a") as manifest:
            manifest.write("\n[features]\nnew-business-behavior = []\n")
        with self.assertRaisesRegex(ValueError, "manifest.*drift"):
            self.verify()

    def test_registry_edges_are_rejected_even_when_package_is_optional(self):
        manifest = self.root / "src/app/Cargo.toml"
        manifest.write_text(manifest.read_text() + 'optional = { version = "1", optional = true }\n')
        with self.assertRaisesRegex(ValueError, "non-path dependency"):
            self.verify()

    def test_dormant_feature_forwarding_cannot_target_removed_backend(self):
        manifest = self.root / "src/app/Cargo.toml"
        manifest.write_text(manifest.read_text() + '\n[features]\nunused = ["mini/missing"]\n')
        with self.assertRaisesRegex(ValueError, "dangling feature edge"):
            self.refresh()

    def test_optional_implicit_features_and_target_variants_are_validated(self):
        manifest = self.root / "src/app/Cargo.toml"
        manifest.write_text('[package]\nname="app"\nversion="1.0.0"\n'
                            '[dependencies]\nmini={path="../foundation/mini",optional=true}\n'
                            '[target.\'cfg(unix)\'.dependencies]\nmini={path="../foundation/mini"}\n'
                            '[features]\ndefault=["mini"]\nexplicit=["dep:mini"]\n')
        # dep: hides the automatic feature, even with multiple target tables.
        with self.assertRaisesRegex(ValueError, "dangling feature edge"):
            self.refresh()
        manifest.write_text(manifest.read_text().replace('default=["mini"]', 'default=["explicit"]'))
        self.refresh()
        self.verify()

    def test_inherited_consumer_features_are_checked_in_every_dependency_context(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["src/app"]\n[workspace.package]\nedition="2024"\n'
            '[workspace.dependencies]\nrenamed={path="src/foundation/mini",package="mini",features=["base"]}\n')
        (self.source / "Cargo.toml").write_text(
            '[package]\nname="mini"\nversion="1.0.0"\n[features]\nbase=[]\nextra=[]\n')
        app = self.root / "src/app/Cargo.toml"
        for prefix in ("", "target.'cfg(unix)'."):
            for kind in ("dependencies", "build-dependencies", "dev-dependencies"):
                with self.subTest(section=prefix + kind):
                    text = ('[package]\nname="app"\nversion="1.0.0"\nedition.workspace=true\n'
                            f'[{prefix}{kind}]\nrenamed={{workspace=true,default-features=false,features=["extra"]}}\n')
                    app.write_text(text)
                    manager.structure(self.root, self.policy)
                    app.write_text(text.replace('features=["extra"]', 'features=["missing"]'))
                    with self.assertRaisesRegex(ValueError, "dangling dependency feature: src/app/Cargo.toml renamed/missing"):
                        manager.structure(self.root, self.policy)

    def test_inherited_optional_features_and_weak_edges_remain_validated(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["src/app"]\n'
            '[workspace.dependencies]\nmini={path="src/foundation/mini",default-features=false}\n')
        (self.source / "Cargo.toml").write_text(
            '[package]\nname="mini"\nversion="1.0.0"\n[features]\nextra=[]\n')
        app = self.root / "src/app/Cargo.toml"
        for prefix in ("", "target.'cfg(unix)'."):
            for kind in ("dependencies", "build-dependencies"):
                for values in ('["mini"]', '["dep:mini"]', '["mini?/extra"]', '["mini/extra"]'):
                    with self.subTest(section=prefix + kind, values=values):
                        text = ('[package]\nname="app"\nversion="1.0.0"\n'
                                f'[{prefix}{kind}]\nmini={{workspace=true,optional=true}}\n'
                                f'[features]\nenabled={values}\n')
                        app.write_text(text)
                        manager.structure(self.root, self.policy)
                        if values != '["mini/extra"]':
                            app.write_text(text.replace('optional=true', 'optional=false'))
                            with self.assertRaisesRegex(ValueError, "dangling feature edge"):
                                manager.structure(self.root, self.policy)

    def test_inherited_dep_syntax_still_hides_implicit_feature(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["src/app"]\n'
            '[workspace.dependencies]\nmini={path="src/foundation/mini"}\n')
        (self.root / "src/app/Cargo.toml").write_text(
            '[package]\nname="app"\nversion="1.0.0"\n'
            '[dependencies]\nmini={workspace=true,optional=true}\n'
            '[features]\nexplicit=["dep:mini"]\ndefault=["mini"]\n')
        with self.assertRaisesRegex(ValueError, "dangling feature edge: src/app/Cargo.toml default -> mini"):
            manager.structure(self.root, self.policy)

    def test_disabling_inherited_defaults_does_not_hide_dormant_invalid_edges(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["src/app"]\n'
            '[workspace.dependencies]\nmini={path="src/foundation/mini",default-features=true}\n')
        (self.root / "src/app/Cargo.toml").write_text(
            '[package]\nname="app"\nversion="1.0.0"\nedition="2024"\n'
            '[dependencies]\nmini={workspace=true,default-features=false}\n')
        (self.source / "Cargo.toml").write_text(
            '[package]\nname="mini"\nversion="1.0.0"\n[features]\ndefault=["removed"]\n')
        with self.assertRaisesRegex(ValueError, "dangling feature edge: src/foundation/mini/Cargo.toml default -> removed"):
            manager.structure(self.root, self.policy)

    def test_registry_or_git_source_in_root_lock_fails_after_receipt_update(self):
        with (self.root / "Cargo.lock").open("a") as lock:
            lock.write(f'source = "{manager.REGISTRY}"\nchecksum = "' + "a" * 64 + '"\n')
        self.catalog["lockfiles_sha256"]["Cargo.lock"] = manager.digest((self.root / "Cargo.lock").read_bytes())
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "non-path Rust source"):
            self.verify()

    def test_refresh_cannot_rebaseline_original_identity_or_legal_material(self):
        self.origin_records[0]["upstream"]["archive_sha256"] = "f" * 64
        self.write_json(manager.ORIGINS, dict(packages=self.origin_records))
        with self.assertRaisesRegex(ValueError, "immutable historical origin"):
            self.refresh()
        self.origin_records[0]["upstream"]["archive_sha256"] = "a" * 64
        self.write_json(manager.ORIGINS, dict(packages=self.origin_records))
        (self.root / "compliance/licenses/upstream/mini/LICENSE").write_text("changed copyright\n")
        with self.assertRaisesRegex(ValueError, "immutable original material"):
            self.refresh()

    def test_missing_centralized_notice_fails(self):
        (self.root / "compliance/licenses/upstream/mini/LICENSE").unlink()
        with self.assertRaisesRegex(ValueError, "centralized origin material"):
            self.verify()

    def test_metadata_source_outside_repository_or_registry_source_fails(self):
        for manifest, source in (("/external/cache/mini/Cargo.toml", None), (str(self.source / "Cargo.toml"), manager.REGISTRY)):
            metadata = {"packages": [dict(id="mini", manifest_path=manifest, source=source)]}
            with self.subTest(manifest=manifest, source=source), self.assertRaisesRegex(ValueError, "external Rust|registry/Git"):
                manager.verify_source_paths(self.root, metadata)

    def test_metadata_features_must_match_inventory(self):
        self.metadata["resolve"]["nodes"][1]["features"] = ["changed"]
        with patch.object(manager, "cargo_metadata", return_value=self.metadata):
            with self.assertRaisesRegex(ValueError, "closure resolution/features drift"):
                self.verify(metadata=True)

    def test_capability_features_are_explicit_and_owned_by_selected_members(self):
        manifest = self.root / "src/app/Cargo.toml"
        manifest.write_text(manifest.read_text() + '\n[features]\npool=[]\ntrace=[]\n')
        profile = dict(self.policy["profiles"][0], role="compile", production=False,
                       features=["app/trace", "app/pool"])
        self.assertEqual(manager.profile_features(self.root, profile), ("app/pool", "app/trace"))
        self.assertEqual(manager.profile_features(self.root, self.policy["profiles"][0]), ())
        for features in (None, "app/pool", [None], ["pool"], ["app/*"], ["app/pool,app/trace"],
                         ["app/pool/extra"], ["app/pool", "app/pool"]):
            with self.subTest(features=features), self.assertRaisesRegex(ValueError, "invalid explicit profile features"):
                manager.profile_features(self.root, dict(profile, features=features))
        for changes in ({"production": True}, {"production": None}, {"role": "runtime"}):
            with self.subTest(changes=changes), self.assertRaisesRegex(ValueError, "non-production compile"):
                manager.profile_features(self.root, dict(profile, **changes))
        for changes in ({"manifests": []}, {"manifests": "src/app/Cargo.toml"}, {"manifests": [None]},
                        {"manifests": ["src/app/Cargo.toml"] * 2}, {"packages": ["app"]}):
            with self.subTest(changes=changes), self.assertRaisesRegex(ValueError, "selected workspace manifests"):
                manager.profile_features(self.root, dict(profile, **changes))
        with self.assertRaisesRegex(ValueError, "not an explicit workspace member"):
            manager.profile_features(self.root, dict(profile, manifests=["src/foundation/mini/Cargo.toml"]))
        for features in (["app/missing"], ["mini/pool"]):
            with self.subTest(features=features), self.assertRaisesRegex(ValueError, "not declared by a selected root"):
                manager.profile_features(self.root, dict(profile, features=features))

    def test_capability_metadata_cache_cannot_contaminate_default_profiles(self):
        manifest = self.root / "src/app/Cargo.toml"
        manifest.write_text(manifest.read_text() + '\n[features]\npool=[]\ntrace=[]\n')
        default = self.policy["profiles"][0]
        optional = dict(default, id="pool", role="compile", production=False, features=["app/pool"])
        combined = dict(optional, id="pool-trace", features=["app/trace", "app/pool"])
        policy = dict(self.policy, profiles=[default, optional, dict(default, id="default-after-pool"), combined,
                                            dict(combined, id="same-features-reordered", features=["app/pool", "app/trace"])])
        parking = self.root / "src/foundation/parking"
        parking.mkdir()
        (parking / "Cargo.toml").write_text('[package]\nname="parking"\nversion="1.0.0"\n')
        enabled = deepcopy(self.metadata)
        enabled["packages"].append(dict(id="parking", name="parking", manifest_path=str(parking / "Cargo.toml"),
                                        source=None, targets=[{"kind": ["lib"]}]))
        enabled["resolve"]["nodes"][1]["deps"].append(dict(pkg="parking", dep_kinds=[{"kind": None}]))
        enabled["resolve"]["nodes"].append(dict(id="parking", features=[], deps=[]))
        def metadata(_root, _manifest, _target, _home, features=()):
            return enabled if "app/pool" in features else self.metadata
        with patch.object(manager, "cargo_metadata", side_effect=metadata) as execute:
            results = manager.resolutions(self.root, policy)
        self.assertEqual([call.kwargs["features"] for call in execute.call_args_list],
                         [(), ("app/pool",), ("app/pool", "app/trace")])
        baseline = dict(manifest="Cargo.toml", profile=default["id"], target=default["targets"][0],
                        packages={"src/foundation/mini": {"roles": ["runtime"], "features": ["enabled"]}})
        self.assertEqual(results[0], baseline)
        self.assertEqual(results[2], dict(baseline, profile="default-after-pool"))
        for index in (1, 3, 4):
            self.assertEqual(results[index]["packages"]["src/foundation/parking"]["roles"], ["compile"])
            self.assertEqual(results[index]["requested_features"], sorted(policy["profiles"][index]["features"]))
        self.assertIn("非产品默认，显式验证特性：app/pool", manager.document(self.catalog, policy))

    def test_requested_capability_features_are_bound_even_without_metadata_recheck(self):
        self.catalog["resolutions"][0]["requested_features"] = ["app/pool"]
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "resolution inventory drift"):
            self.verify()

    def test_omitting_requested_capability_features_is_rejected(self):
        manifest = self.root / "src/app/Cargo.toml"
        manifest.write_text(manifest.read_text() + '\n[features]\npool=[]\n')
        self.policy["profiles"][0].update(role="compile", production=False, features=["app/pool"])
        self.write_json(manager.POLICY, self.policy)
        self.refresh()
        self.catalog["resolutions"][0].pop("requested_features")
        self.save_catalog()
        with self.assertRaisesRegex(ValueError, "resolution inventory drift"):
            self.verify()

    def test_unused_retained_source_is_rejected_by_closed_scope_policy(self):
        self.policy["require_reachable_sources"] = True
        self.write_json(manager.POLICY, self.policy)
        self.metadata["resolve"]["nodes"][0]["deps"] = []
        with self.assertRaisesRegex(ValueError, "outside supported entry closures"):
            self.refresh()


class DescriptionPaths(unittest.TestCase):
    def test_each_migration_is_idempotent(self):
        for before, after in manager.DESCRIPTION_PATHS:
            with self.subTest(before=before):
                self.assertEqual(manager.current_description(before), after)
                self.assertEqual(manager.current_description(after), after)
                self.assertEqual(manager.current_description(f"路径 `{before}`；说明"), f"路径 `{after}`；说明")

    def test_repeated_shared_prefix_is_repaired_once(self):
        suffix = "vendor/web/dependencies/patches.json"
        canonical = "compliance/provenance/shared/" + suffix
        for count in (2, 16, 32):
            with self.subTest(count=count):
                repaired = manager.current_description("compliance/provenance/shared/" * count + suffix)
                self.assertEqual(repaired, canonical)
                self.assertEqual(manager.current_description(repaired), canonical)

    def test_embedded_paths_and_similarly_named_paths_are_not_rewritten(self):
        for text in ("docs/upstream/web/dist", "https://example.test/web/src/file.rs",
                     "web/dist-old", "tools/zxs-bindgen-other", "src/web/src/main.rs"):
            with self.subTest(text=text):
                self.assertEqual(manager.current_description(text), text)


class MetadataProcess(unittest.TestCase):
    def test_metadata_forwards_only_requested_capability_features(self):
        result = subprocess.CompletedProcess([], 0, stdout='{"packages": []}', stderr="")
        with patch.object(manager.subprocess, "run", return_value=result) as execute:
            manager.cargo_metadata(Path("/repository"), features=("app/pool", "app/trace"))
        command = execute.call_args.args[0]
        self.assertEqual(command[-2:], ["--features", "app/pool,app/trace"])
        self.assertNotIn("--all-features", command)
        self.assertNotIn("--no-default-features", command)

    def test_metadata_explicitly_uses_empty_home_offline_lock_and_target(self):
        metadata = {"packages": []}
        result = subprocess.CompletedProcess([], 0, stdout=json.dumps(metadata), stderr="")
        with tempfile.TemporaryDirectory() as directory:
            root, home = Path(directory), Path(directory) / "empty-home"
            home.mkdir()
            with patch.object(manager.subprocess, "run", return_value=result) as execute:
                self.assertEqual(manager.cargo_metadata(root, "Cargo.toml", "aarch64-unknown-linux-gnu", home), metadata)
            args, options = execute.call_args
            self.assertIn("--offline", args[0])
            self.assertIn("--locked", args[0])
            self.assertEqual(args[0][-2:], ["--filter-platform", "aarch64-unknown-linux-gnu"])
            self.assertEqual(options["env"]["CARGO_HOME"], str(home))
            self.assertEqual(options["cwd"], root)

    def test_failed_offline_resolution_is_an_error(self):
        result = subprocess.CompletedProcess([], 101, stdout="", stderr="missing local source")
        with patch.object(manager.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(ValueError, "offline metadata failed"):
                manager.cargo_metadata(Path("/repository"))


class ClosureRoles(unittest.TestCase):
    def setUp(self):
        self.metadata = dict(workspace_members=["app"],
            packages=[dict(id=name, targets=[{"kind": ["proc-macro" if name == "macro" else "lib"]}])
                      for name in ("app", "normal", "build", "macro", "shared", "dev")],
            resolve={"nodes": [dict(id="app", features=[], deps=[
                dict(pkg="normal", dep_kinds=[{"kind": None}]), dict(pkg="build", dep_kinds=[{"kind": "build"}]),
                dict(pkg="macro", dep_kinds=[{"kind": None}]), dict(pkg="dev", dep_kinds=[{"kind": "dev"}])]),
                dict(id="normal", features=[], deps=[dict(pkg="shared", dep_kinds=[{"kind": None}])]),
                dict(id="build", features=[], deps=[dict(pkg="shared", dep_kinds=[{"kind": None}])]),
                dict(id="macro", features=[], deps=[]), dict(id="shared", features=["z", "a"], deps=[]),
                dict(id="dev", features=[], deps=[])]})

    def test_business_normal_build_and_macro_roles_remain_distinct(self):
        roles = manager.closure(self.metadata)
        self.assertEqual(roles["normal"]["roles"], ["runtime"])
        self.assertEqual(roles["build"]["roles"], ["compile"])
        self.assertEqual(roles["macro"]["roles"], ["compile"])
        self.assertEqual(roles["shared"], {"roles": ["compile", "runtime"], "features": ["a", "z"]})
        self.assertNotIn("dev", roles)

    def test_profile_is_a_conservative_workspace_feature_superset(self):
        # Mirrors Cargo metadata when tool enables base/extra but app does not.
        # Selecting app only after metadata cannot undo workspace unification.
        root = Path("/repository")
        metadata = dict(workspace_members=["app", "tool"], packages=[
            dict(id=name, name=name, manifest_path=str(root / path / "Cargo.toml"),
                 targets=[{"kind": ["lib"]}])
            for name, path in (("app", "src/app"), ("tool", "tools/tool"),
                               ("base", "src/foundation/base"), ("extra", "src/foundation/extra"))
        ], resolve={"nodes": [
            dict(id="app", features=[], deps=[dict(pkg="base", dep_kinds=[{"kind": None}])]),
            dict(id="tool", features=[], deps=[dict(pkg="base", dep_kinds=[{"kind": None}])]),
            dict(id="base", features=["extra"], deps=[dict(pkg="extra", dep_kinds=[{"kind": None}])]),
            dict(id="extra", features=[], deps=[]),
        ]})
        selected = manager.selected_sources(root, metadata, profile={"id": "app", "packages": ["app"]})
        self.assertEqual(selected, {
            "src/foundation/base": {"roles": ["runtime"], "features": ["extra"]},
            "src/foundation/extra": {"roles": ["runtime"], "features": []},
        })
        self.assertIn("not an isolated consumer", manager.closure.__doc__)

    def test_build_helper_entire_dependency_graph_is_compile_only(self):
        roles = manager.closure(self.metadata, initial_role="compile")
        self.assertTrue(all(package["roles"] == ["compile"] for package in roles.values()))
        self.assertEqual(manager.workspace_role("tools/web-build/Cargo.toml"), "compile")
        self.assertEqual(manager.workspace_role("src/smb/Cargo.toml"), "runtime")


if __name__ == "__main__":
    unittest.main()
