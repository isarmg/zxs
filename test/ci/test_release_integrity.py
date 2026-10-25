"""Reject stale, incomplete, escaped or modified release inputs before archiving."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tarfile
import unittest

from web_dist_fixture import prepare, record, ROOT, SPEC

MODULE = importlib.util.spec_from_file_location("verify_web_dist", ROOT / ".github/scripts/verify-web-dist.py")
validator = importlib.util.module_from_spec(MODULE)
MODULE.loader.exec_module(validator)


class ReleaseIntegrity(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="zxs-release-integrity-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "root"
        self.root.mkdir()
        self.receipt = prepare(self.root)
        self.dist = self.root / "build/web"

    def save_manifest(self, value):
        (self.dist / "build-manifest.json").write_text(json.dumps(value))

    def package(self, version="0.1.0"):
        return subprocess.run(["bash", str(self.root / ".github/scripts/release-package.sh")],
                              env=os.environ | {"RELEASE_VERSION": version},
                              capture_output=True, text=True)

    def test_complete_distribution_can_be_verified_and_repeatedly_packaged(self):
        self.assertEqual(validator.verify(self.root), (len(self.receipt["files"]), len(self.receipt["inputs"])))
        for _ in range(2):
            result = self.package()
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(list((self.root / "release-assets").glob("*.tar.gz"))), 2)
        for path in (self.root / "release-assets").glob("*.tar.gz"):
            with tarfile.open(path) as archive:
                for template in ("deploy/systemd/zxs.service", "deploy/samba/smb.conf.example"):
                    self.assertEqual(archive.extractfile(template).read(), (self.root / template).read_bytes())

    def test_missing_samba_template_rejects_incomplete_installation_material(self):
        (self.root / "deploy/samba/smb.conf.example").unlink()
        result = self.package()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "release-assets").exists())

    def test_missing_systemd_template_rejects_incomplete_installation_material(self):
        (self.root / "deploy/systemd/zxs.service").unlink()
        result = self.package()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "release-assets").exists())

    def test_missing_rescue_binary_rejects_partial_native_release(self):
        for name in ("zxs-rescue",):
            path = self.root / "target/native/release" / name
            saved = path.read_bytes()
            path.unlink()
            result = self.package()
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((self.root / "release-assets").exists())
            path.write_bytes(saved)
            path.chmod(0o755)

    def test_modified_output_is_rejected_before_creating_assets(self):
        (self.dist / "pkg/zxs_bg.wasm").write_bytes(b"damaged WASM")
        result = self.package()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Web output checksum mismatch", result.stderr)
        self.assertFalse((self.root / "release-assets").exists())

    def test_missing_and_unregistered_output_files_fail(self):
        (self.dist / "unexpected.js").write_text("unexpected executable bytes")
        with self.assertRaisesRegex(ValueError, "output inventory"):
            validator.verify(self.root)
        (self.dist / "unexpected.js").unlink()
        (self.dist / "pkg/zxs.js").unlink()
        with self.assertRaisesRegex(ValueError, "output inventory"):
            validator.verify(self.root)

    def test_missing_required_asset_cannot_be_hidden_by_editing_manifest(self):
        (self.dist / "pkg/zxs.js").unlink()
        del self.receipt["files"]["pkg/zxs.js"]
        self.save_manifest(self.receipt)
        with self.assertRaisesRegex(ValueError, "required generated assets"):
            validator.verify(self.root)

    def test_input_hashes_must_match_current_source_and_complete_range(self):
        source = self.root / "src/web/src/lib.rs"
        source.write_text("source that was not part of the build")
        with self.assertRaisesRegex(ValueError, "source input inventory/content"):
            validator.verify(self.root)
        self.receipt = record(self.root)
        source.write_text("source changed after the build")
        with self.assertRaisesRegex(ValueError, "source input inventory/content"):
            validator.verify(self.root)
        source.unlink()
        self.receipt = record(self.root)
        del self.receipt["inputs"]["Cargo.lock"]
        self.save_manifest(self.receipt)
        with self.assertRaisesRegex(ValueError, "source input inventory/content"):
            validator.verify(self.root)

    def test_self_reported_input_range_cannot_hide_foundation_changes(self):
        path = self.root / "src/foundation/local/src/lib.rs"
        path.parent.mkdir(parents=True)
        path.write_text("unregistered foundation input")
        self.receipt["inputs"] = {"Cargo.lock": self.receipt["inputs"]["Cargo.lock"]}
        self.save_manifest(self.receipt)
        with self.assertRaisesRegex(ValueError, "source input inventory/content"):
            validator.verify(self.root)

    def test_manifest_rejects_duplicate_fields_unknown_fields_and_invalid_identity(self):
        original = (self.dist / "build-manifest.json").read_text()
        (self.dist / "build-manifest.json").write_text(original[:-1] + ', "files": {}}')
        with self.assertRaisesRegex(ValueError, "duplicate JSON field"):
            validator.verify(self.root)
        for key, value in (("runtime", "other"), ("target", "native"), ("wasm_bindgen", "other"),
                           ("unknown", True), ("files", []), ("inputs", {})):
            with self.subTest(key=key):
                receipt = dict(self.receipt, **{key: value})
                self.save_manifest(receipt)
                with self.assertRaises(ValueError):
                    validator.verify(self.root)

    def test_manifest_paths_and_hashes_are_not_shell_or_filesystem_instructions(self):
        for path in ("../outside", "/absolute", "pkg/../outside", "pkg//file", "pkg\\file", "-C /tmp", "x\nfile"):
            with self.subTest(path=path):
                receipt = copy.deepcopy(self.receipt)
                receipt["files"][path] = "a" * 64
                self.save_manifest(receipt)
                with self.assertRaises(ValueError):
                    validator.verify(self.root)
        receipt = copy.deepcopy(self.receipt)
        receipt["files"]["pkg/zxs.js"] = True
        self.save_manifest(receipt)
        with self.assertRaisesRegex(ValueError, "invalid output hash"):
            validator.verify(self.root)

    def test_input_specification_rejects_escape_duplicates_overlap_and_unknown_types(self):
        original = json.loads((self.root / SPEC).read_text())
        variants = [dict(original, unknown=[]), dict(original, files="Cargo.lock"),
                    dict(original, files=original["files"] + ["../outside"]),
                    dict(original, files=original["files"] + ["Cargo.lock"]),
                    dict(original, directories=original["directories"] + ["src/web/src/nested"]),
                    dict(original, files=original["files"] + [42]),
                    dict(original, files=[p for p in original["files"] if p != SPEC])]
        for spec in variants:
            with self.subTest(spec=spec):
                (self.root / SPEC).write_text(json.dumps(spec))
                with self.assertRaises(ValueError):
                    validator.verify(self.root)
        (self.root / SPEC).write_text('{"files": [], "files": [], "directories": []}')
        with self.assertRaisesRegex(ValueError, "duplicate JSON field"):
            validator.verify(self.root)

    def test_input_specification_is_itself_part_of_the_build_identity(self):
        with (self.root / SPEC).open("a") as file:
            file.write("\n")
        with self.assertRaisesRegex(ValueError, "source input inventory/content"):
            validator.verify(self.root)

    def test_symlinks_in_outputs_source_ancestors_materials_and_binary_fail(self):
        for relative in ("build/web/pkg/zxs.js", "Cargo.lock", "README.md", "deploy/systemd/zxs.service", "deploy/samba/smb.conf.example", "target/native/release/zxs"):
            path = self.root / relative
            data, mode = path.read_bytes(), path.stat().st_mode
            path.unlink()
            path.symlink_to("/tmp/untrusted-release-file")
            with self.subTest(relative=relative), self.assertRaisesRegex(ValueError, "not a regular"):
                validator.verify(self.root)
            path.unlink()
            path.write_bytes(data)
            path.chmod(mode)
        source = self.root / "src/web/src"
        source.rename(source.with_name("original-src"))
        source.symlink_to("original-src", target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "not a regular"):
            validator.verify(self.root)

    def test_special_output_file_fails_without_being_read(self):
        os.mkfifo(self.dist / "pipe")
        with self.assertRaisesRegex(ValueError, "not a regular"):
            validator.verify(self.root)

    def test_snapshot_preserves_verified_bytes_after_original_files_change(self):
        stage = self.root.parent / "snapshot"
        validator.verify(self.root, stage)
        wasm = stage / "web/pkg/zxs_bg.wasm"
        before = wasm.read_bytes()
        (self.dist / "pkg/zxs_bg.wasm").write_bytes(b"changed after verification")
        self.assertEqual(wasm.read_bytes(), before)
        self.assertEqual((stage / "native/zxs").read_bytes(), (self.root / "target/native/release/zxs").read_bytes())
        self.assertTrue((stage / "material/compliance/NOTICE.md").is_file())
        (self.dist / "pkg/zxs_bg.wasm").write_bytes(before)
        with self.assertRaises(FileExistsError):
            # Do not overwrite a preexisting staging directory.
            validator.verify(self.root, stage)

    def test_version_cannot_escape_release_directory(self):
        for version in ("../escape", "0.1.0/../../../escape", "0.1.0\nother", "$(touch bad)"):
            result = self.package(version)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("safe version filename", result.stderr)
            self.assertFalse((self.root / "release-assets").exists())

    def test_release_directory_cannot_be_a_symlink(self):
        outside = self.root.parent / "outside"
        outside.mkdir()
        (self.root / "release-assets").symlink_to(outside, target_is_directory=True)
        self.assertNotEqual(self.package().returncode, 0)
        self.assertEqual(list(outside.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
