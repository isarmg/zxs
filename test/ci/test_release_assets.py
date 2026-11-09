"""Publication checks the whole set, not just whichever hashes were supplied."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import tarfile
import unittest
from unittest.mock import patch
from web_dist_fixture import write_release_assets, elf_fixture, add_runtime_fields, bundle_member

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release_assets", ROOT / ".github/scripts/verify-release-assets.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ReleaseAssets(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.assets = self.root / "release-assets"
        self.assets.mkdir()
        self.commit = write_release_assets(self.assets)['commit']
        self.source_bytes = (self.assets / 'zxs-source-0.1.0.tar.gz').read_bytes()
        self.checksums()

    def checksums(self):
        lines = [f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
                 for path in sorted(self.assets.iterdir()) if path.name != "SHA256SUMS"]
        (self.assets / "SHA256SUMS").write_text("".join(lines))

    def verify(self, stage=None):
        return MODULE.verify(self.assets, "0.1.0", self.commit, stage)

    def test_complete_source_matched_snapshot_is_independent_of_later_edits(self):
        stage = self.root / "snapshot"
        self.assertEqual(self.verify(stage), 4)
        (self.assets / "zxs-source-0.1.0.tar.gz").write_text("later modification")
        self.assertEqual(MODULE.verify(stage, "0.1.0", self.commit), 4)

    def test_missing_checksum_entry_is_not_proof_of_a_complete_release(self):
        lines = (self.assets / "SHA256SUMS").read_text().splitlines(True)
        (self.assets / "SHA256SUMS").write_text("".join(lines[1:]))
        with self.assertRaisesRegex(ValueError, "checksum inventory"):
            self.verify()

    def test_extra_visible_hidden_and_directory_assets_are_rejected(self):
        for name in ("extra.bin", ".hidden", "directory"):
            path = self.assets / name
            if name == "directory":
                path.mkdir()
            else:
                path.write_text("unlisted")
            with self.assertRaisesRegex(ValueError, "complete expected asset set"):
                self.verify()
            path.rmdir() if path.is_dir() else path.unlink()

    def test_hash_manifest_cannot_reference_paths_or_repeat_files(self):
        original = (self.assets / "SHA256SUMS").read_text()
        for entry in ("../outside", "/etc/passwd", "nested/file", "zxs-0.1.0-debian13-amd64"):
            (self.assets / "SHA256SUMS").write_text(original + f"{'a' * 64}  {entry}\n")
            with self.assertRaises(ValueError):
                self.verify()

    def test_valid_checksums_do_not_hide_stale_commit_or_version(self):
        for build in (f"version=0.1.0\ncommit={'b' * 40}\n", f"version=0.0.9\ncommit={self.commit}\n",
                      f"version=0.1.0\ncommit={self.commit}\ncommit={self.commit}\n"):
            (bundle_member(self.assets, "BUILD.txt")).write_text(build)
            self.checksums()
            with self.assertRaisesRegex(ValueError, "identity"):
                self.verify()

    def test_modified_asset_fails(self):
        (self.assets / "zxs-source-0.1.0.tar.gz").write_text("modified")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.verify()

    def test_target_and_archive_identity_cannot_be_relabelled_with_new_checksums(self):
        path = bundle_member(self.assets, "BUILD.txt")
        original = path.read_text()
        for old, new in (("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"),
                         ("native-profile=release", "native-profile=debug"),
                         ("source-archive-sha256=", "unrecognized-archive="),
                         ("native-zxs-sha256=", "unrecognized-binary=")):
            path.write_text(original.replace(old, new))
            self.checksums()
            with self.assertRaises(ValueError):
                self.verify()

    def test_vfs_identity_and_bytes_cannot_be_relabelled(self):
        path = bundle_member(self.assets, "VFS-BUILD.json")
        original = json.loads(path.read_bytes())
        for key, value in (("commit", "c" * 40), ("tree", "c" * 40),
                           ("release", "12"), ("vfs_interface", 51),
                           ("package_version", "2:4.23.0"),
                           ("module_sha256", "0" * 64)):
            with self.subTest(key=key):
                path.write_text(json.dumps(dict(original, **{key: value})))
                self.checksums()
                with self.assertRaises(ValueError):
                    self.verify()

    def test_native_must_bind_embedded_web_to_same_source(self):
        path = bundle_member(self.assets, "BUILD.txt")
        original = path.read_text()
        for old, new in (("native-baseline=debian13-amd64", "native-baseline=ubuntu24"),
                         ("native-web-embedded=true", "native-web-embedded=false"),
                         (f"native-web-commit={self.commit}", f"native-web-commit={'c' * 40}"),
                         ("native-web-manifest-sha256=", "missing-manifest=")):
            with self.subTest(old=old):
                path.write_text(original.replace(old, new))
                self.checksums()
                with self.assertRaises(ValueError):
                    self.verify()

    def test_helper_materials_require_exact_source_digest_and_bounded_inventory(self):
        path = bundle_member(self.assets, "BUILD.txt")
        fields = dict(line.split("=", 1) for line in path.read_text().splitlines())
        for key, value in (("native-helper-license-materials-embedded", "false"),
                           ("native-helper-license-commit", "c" * 40),
                           ("native-helper-license-tree", "c" * 40),
                           ("native-helper-license-materials-sha256", "missing"),
                           ("native-helper-license-manifest-sha256", "missing"),
                           ("native-helper-license-file-count", "0"),
                           ("native-helper-license-file-count", "8193")):
            with self.subTest(key=key, value=value):
                changed = dict(fields, **{key: value})
                path.write_text("".join(f"{name}={text}\n" for name, text in changed.items()))
                self.checksums()
                with self.assertRaises(ValueError):
                    self.verify()

    def test_helper_payload_binding_is_required_even_with_consistent_asset_checksums(self):
        build = bundle_member(self.assets, "BUILD.txt")
        fields = dict(line.split("=", 1) for line in build.read_text().splitlines())
        for key, value in (("native-helper-embedded-zxs", "false"),
                           ("native-helper-payload-sha256", "0" * 64),
                           ("native-helper-payload-sha256", "missing")):
            with self.subTest(key=key, value=value):
                changed = dict(fields, **{key: value})
                build.write_text("".join(f"{name}={text}\n" for name, text in changed.items()))
                self.checksums()
                with self.assertRaisesRegex(ValueError, "helper must embed the exact standalone ZXS build payload"):
                    self.verify()

    def test_elf_type_and_abi_are_required_even_with_consistent_hashes(self):
        binary = self.assets / "zxs-0.1.0-debian13-amd64"
        build = bundle_member(self.assets, "BUILD.txt")
        original = build.read_text()
        invalid = [b"#!/bin/sh\nexit 0\n", b"not an ELF archive"]
        for offset, byte in ((4, 1), (5, 2), (16, 1), (18, 183), (52, 0)):
            header = bytearray(elf_fixture())
            header[offset] = byte
            invalid.append(bytes(header))
        for data in invalid:
            binary.write_bytes(data)
            fields = dict(line.split("=", 1) for line in original.splitlines())
            fields["native-zxs-sha256"] = hashlib.sha256(data).hexdigest()
            # This case targets ELF validation, not the earlier helper-payload
            # identity gate. Keep both declared payload digests consistent.
            fields["native-helper-payload-sha256"] = fields["native-zxs-sha256"]
            add_runtime_fields(fields)
            build.write_text("".join(f"{key}={value}\n" for key, value in fields.items()))
            self.checksums()
            with self.assertRaisesRegex(ValueError, "ELF64"):
                self.verify()
        MODULE.elf_identity(elf_fixture())
        MODULE.elf_identity(elf_fixture(module=True))  # PIE executable allowed.
        MODULE.elf_identity(elf_fixture(module=True), module=True)
        with self.assertRaisesRegex(ValueError, "ELF64"):
            MODULE.elf_identity(elf_fixture(), module=True)

    def test_runtime_receipt_must_bind_actual_binary_and_debian_target(self):
        path = bundle_member(self.assets, "BUILD.txt")
        original = path.read_text()
        fields = dict(line.split("=", 1) for line in original.splitlines())
        runtime = json.loads(fields["native-runtime-dependencies"])
        for field, value in (("release", "12"), ("architecture", "arm64"), ("binaries", [])):
            changed = json.dumps(dict(runtime, **{field: value}), separators=(",", ":"))
            updated = dict(fields, **{"native-runtime-dependencies": changed,
                                     "native-runtime-dependencies-sha256": hashlib.sha256(changed.encode()).hexdigest()})
            path.write_text("".join(f"{key}={value}\n" for key, value in updated.items()))
            self.checksums()
            with self.assertRaises(ValueError):
                self.verify()
        path.write_text(original.replace('native-runtime-dependencies-sha256=', 'missing-runtime-sha='))
        self.checksums()
        with self.assertRaises(ValueError):
            self.verify()

    def test_missing_build_receipt_is_rejected(self):
        (bundle_member(self.assets, "BUILD.txt")).unlink()
        self.checksums()
        with self.assertRaisesRegex(ValueError, "source bundle lacks"):
            self.verify()

    def test_installation_text_cannot_be_changed_with_consistent_asset_hashes(self):
        bundle_member(self.assets, "INSTALL.md").write_text("unbound installation instructions")
        self.checksums()
        with self.assertRaisesRegex(ValueError, "instructions differ from the exact source tree"):
            self.verify()

    def test_omitted_license_cannot_be_hidden_by_rehashing_both_archives(self):
        repository = bundle_member(self.assets, "repository.tar.gz")
        modified = io.BytesIO()
        with tarfile.open(fileobj=io.BytesIO(repository.read_bytes())) as source:
            with tarfile.open(fileobj=modified, mode="w:gz", format=tarfile.PAX_FORMAT) as output:
                for entry in source:
                    if entry.name != "source/compliance/NOTICE.md":
                        output.addfile(entry, source.extractfile(entry))
        repository.write_bytes(modified.getvalue())
        build = bundle_member(self.assets, "BUILD.txt")
        fields = dict(line.split("=", 1) for line in build.read_text().splitlines())
        fields["source-archive-sha256"] = hashlib.sha256(modified.getvalue()).hexdigest()
        build.write_text("".join(f"{key}={value}\n" for key, value in fields.items()))
        self.checksums()
        with self.assertRaisesRegex(ValueError, "complete requested Git tree"):
            self.verify()

    def test_symlink_asset_directory_and_snapshot_are_rejected(self):
        original = self.assets / "zxs-source-0.1.0.tar.gz"
        outside = self.root / "outside"
        original.rename(outside)
        original.symlink_to(outside)
        with self.assertRaises(OSError):
            self.verify()
        original.unlink()
        outside.rename(original)
        linked = self.root / "linked"
        linked.symlink_to(self.assets, target_is_directory=True)
        with self.assertRaises(OSError):
            MODULE.verify(linked, "0.1.0", self.commit)
        with self.assertRaisesRegex(ValueError, "snapshot symlink"):
            self.verify(linked / "snapshot")
        self.assertFalse((self.assets / "snapshot").exists())

    def test_leaf_replaced_while_opening_cannot_follow_links_or_block_on_fifo(self):
        for replacement in ("symlink", "fifo"):
            with self.subTest(replacement=replacement):
                target = self.assets / "zxs-source-0.1.0.tar.gz"
                original = target.read_bytes()
                outside = self.root / "outside"
                outside.write_bytes(original)
                real_open = os.open
                replaced = False

                def replace_then_open(path, flags, *args, **kwargs):
                    nonlocal replaced
                    if path == "zxs-source-0.1.0.tar.gz" and not replaced:
                        replaced = True
                        target.unlink()
                        if replacement == "symlink":
                            target.symlink_to(outside)
                        else:
                            os.mkfifo(target)
                        self.assertTrue(flags & os.O_NOFOLLOW)
                        self.assertTrue(flags & os.O_NONBLOCK)
                        self.assertIn("dir_fd", kwargs)
                    return real_open(path, flags, *args, **kwargs)

                with patch.object(MODULE.os, "open", side_effect=replace_then_open):
                    with self.assertRaises((OSError, ValueError)):
                        self.verify(self.root / f"snapshot-{replacement}")
                self.assertTrue(replaced)
                target.unlink()
                target.write_bytes(original)
                self.assertEqual(outside.read_bytes(), original)

    def test_replaced_directory_cannot_redirect_remaining_asset_reads(self):
        original_read = MODULE.read_metadata
        moved = self.root / "original-assets"
        def replace_after_receipt(fd, name):
            data = original_read(fd, name)
            if name == "SHA256SUMS":
                self.assets.rename(moved)
                self.assets.mkdir()
                (self.assets / "zxs-source-0.1.0.tar.gz").write_text("redirected asset")
            return data
        with patch.object(MODULE, "read_metadata", side_effect=replace_after_receipt):
            self.assertEqual(self.verify(self.root / "snapshot"), 4)
        self.assertEqual((self.root / "snapshot/zxs-source-0.1.0.tar.gz").read_bytes(), self.source_bytes)


if __name__ == "__main__":
    unittest.main()

