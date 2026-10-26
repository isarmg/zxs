"""Repository-level regressions for the maintained Linux amd64/Web boundary."""
import importlib.util
from pathlib import Path
import re
import tomllib
import unittest

SPEC = importlib.util.spec_from_file_location("platform_source_manager", (Path(__file__).resolve().parents[3] / "tools/rust-sources" / "manage.py"))
manager = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manager)
ROOT = manager.ROOT


class PlatformScope(unittest.TestCase):
    def test_product_profiles_have_only_the_supported_targets(self):
        policy = manager.read_json(ROOT / manager.POLICY)
        manager.verify_target_policy(policy)
        self.assertEqual(
            {(entry["id"], entry["target"]) for entry in manager.entries(policy)},
            {("root-native", manager.NATIVE_TARGET), ("web", manager.WEB_TARGET),
             ("smb", manager.NATIVE_TARGET), ("build-tools", manager.NATIVE_TARGET),
             ("validation-tools", manager.NATIVE_TARGET), ("sqlite-pool-validation", manager.NATIVE_TARGET),
             ("schema-async-validation", manager.NATIVE_TARGET),
             ("optional-macro-validation", manager.NATIVE_TARGET),
             ("binding-test-tools-validation", manager.NATIVE_TARGET)},
        )

    def test_binding_text_tools_have_a_nonproduction_validation_root(self):
        policy = manager.read_json(ROOT / manager.POLICY)
        profile = next(p for p in policy['profiles'] if p['id'] == 'binding-test-tools-validation')
        self.assertEqual(profile['manifests'], [
            'src/foundation/web-runtime/wasm-text-parser/Cargo.toml',
            'src/foundation/web-runtime/wasm-text-printer/Cargo.toml',
        ])
        self.assertEqual(profile['role'], 'compile')
        self.assertIs(profile['production'], False)
        self.assertEqual(profile['targets'], [manager.NATIVE_TARGET])

    def test_sqlite_pool_validation_matches_the_real_retained_capability(self):
        policy = manager.read_json(ROOT / manager.POLICY)
        optional = next(profile for profile in policy["profiles"] if profile["id"] == "sqlite-pool-validation")
        self.assertEqual(optional["manifests"], ["src/platform/Cargo.toml"])
        self.assertEqual(optional["role"], "compile")
        self.assertIs(optional["production"], False)
        self.assertEqual(manager.profile_features(ROOT, optional), ("platform-io/sqlite-pool",))
        self.assertTrue(all(not profile.get("features") for profile in policy["profiles"] if profile.get("production")))
        platform = tomllib.loads((ROOT / "src/platform/Cargo.toml").read_text())
        driver = tomllib.loads((ROOT / "src/foundation/data-storage/sqlite-access/Cargo.toml").read_text())
        notification = tomllib.loads((ROOT / "src/foundation/concurrency/async-notification/Cargo.toml").read_text())
        self.assertIn("sqlite-access/pool", platform["features"]["sqlite-pool"])
        self.assertIn("dep:async-notification", driver["features"]["pool"])
        self.assertTrue(driver["dependencies"]["async-notification"]["optional"])
        self.assertTrue(driver["dependencies"]["async-notification"]["default-features"])
        self.assertIn("std", notification["features"]["default"])
        self.assertIn("dep:parking", notification["features"]["std"])
        parking = notification["target"]['cfg(not(target_family = "wasm"))']["dependencies"]["parking"]
        self.assertEqual(parking["path"], "../parking")
        self.assertTrue(parking["optional"])

    def test_schema_async_validation_is_explicit_and_not_a_web_dependency(self):
        policy = manager.read_json(ROOT / manager.POLICY)
        optional = next(profile for profile in policy["profiles"] if profile["id"] == "schema-async-validation")
        self.assertEqual(optional["manifests"], ["tools/contract-check/Cargo.toml"])
        self.assertEqual(optional["role"], "compile")
        self.assertIs(optional["production"], False)
        self.assertEqual(manager.profile_features(ROOT, optional), ("contract-check/schema-async",))
        tool = tomllib.loads((ROOT / "tools/contract-check/Cargo.toml").read_text())
        self.assertEqual(tool["features"]["schema-async"], ["schema-validation/resolve-async"])
        self.assertNotIn("schema-async", tool["features"].get("default", []))
        renderer = tomllib.loads((ROOT / "src/foundation/web-runtime/view-renderer/Cargo.toml").read_text())
        self.assertNotIn("async-trait", renderer.get("dependencies", {}))
        gate = (ROOT / ".github/scripts/check-schema-validation.py").read_text()
        self.assertIn("resolve-async,jsonb-testkit,arbitrary-precision", gate)

    def test_optional_macro_validation_is_not_a_default_product_member(self):
        policy = manager.read_json(ROOT / manager.POLICY)
        optional = next(profile for profile in policy["profiles"]
                        if profile["id"] == "optional-macro-validation")
        members = ["src/foundation/diagnostics/tracing-attributes",
                   "src/foundation/networking/axum-macros"]
        self.assertEqual(optional["manifests"], [path + "/Cargo.toml" for path in members])
        self.assertEqual(optional["targets"], [manager.NATIVE_TARGET])
        self.assertEqual(optional["role"], "compile")
        self.assertIs(optional["production"], False)
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        for member in members:
            self.assertIn(member, policy["verification_members"])
            self.assertIn(member, workspace["members"])
            self.assertNotIn(member, workspace["default-members"])

    def test_linux_uapi_preserves_every_amd64_feature_and_binding(self):
        directory = ROOT / "src/foundation/platform/linux-raw-sys"
        manifest = tomllib.loads((directory / "Cargo.toml").read_text())
        source = (directory / "src/lib.rs").read_text()
        modules = re.findall(r'#\[cfg\(feature = "([^"]+)"\)\]\n(?:#\[cfg\([^\n]+\)\]\n)?#\[path = "([^"]+)"\]\npub mod ([^;]+);', source)
        self.assertEqual(len(modules), 23)
        self.assertEqual({path.split("/")[0] for _, path, _ in modules}, {"x86_64"})
        self.assertEqual(
            {feature for feature, _, _ in modules} | {"elf"},
            set(manifest["features"]) - {"default", "std", "no_std", "rustc-dep-of-std"},
        )
        self.assertEqual({path.name for path in (directory / "src").iterdir() if path.is_dir()}, {"x86_64"})
        # The local dispatch changes, but retained generated ABI definitions do not.
        catalog = manager.read_json(ROOT / manager.CATALOG)
        package = catalog["packages"]["src/foundation/platform/linux-raw-sys"]
        origin = manager.origins(ROOT)[package["origin_directory"]]
        for _, relative, _ in modules:
            original = "src/" + relative
            self.assertEqual(manager.digest((directory / original).read_bytes()),
                             origin["original_files_sha256"][original])
        self.assertEqual(manager.digest((directory / "src/elf.rs").read_bytes()),
                         origin["original_files_sha256"]["src/elf.rs"])

    def test_msvc_discovery_is_removed_without_dropping_provenance(self):
        self.assertFalse((ROOT / "src/foundation/vcpkg").exists())
        for relative in ("src/foundation/data-storage/libsqlite3-sys/Cargo.toml", "src/foundation/data-storage/sqlite-access/Cargo.toml", "src/foundation/data-storage/libsqlite3-sys/build.rs"):
            self.assertNotIn("vcpkg", (ROOT / relative).read_text().lower())
        records = manager.origins(ROOT)
        retired = [item for item in records.values() if item["upstream"]["name"] == "vcpkg"]
        self.assertEqual(len(retired), 1)
        self.assertTrue(retired[0]["original_files_sha256"])
        self.assertTrue(retired[0]["central_license_materials"])
        for relative in retired[0]["central_license_materials"]:
            self.assertTrue((ROOT / relative).is_file())


if __name__ == "__main__":
    unittest.main()
