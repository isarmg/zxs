"""Publication checks the whole set, not just whichever hashes were supplied."""
import hashlib
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release_assets", ROOT / ".github/scripts/verify-release-assets.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
COMMIT = "a" * 40


class ReleaseAssets(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.assets = self.root / "release-assets"
        self.assets.mkdir()
        for name in ("INSTALL.md", "zxs-0.1.0-x86_64-unknown-linux-gnu.tar.gz", "web-interface-0.1.0.tar.gz"):
            (self.assets / name).write_bytes(b"release asset bytes")
        fields = {"version": "0.1.0", "commit": COMMIT,
                  "native-target": "x86_64-unknown-linux-gnu", "native-profile": "release",
                  "native-rustc": "fixture compiler", "native-environment-sha256": "a" * 64,
                  "native-inputs-sha256": "b" * 64, "native-zxs-sha256": "c" * 64,
                  "native-zxs-rescue-sha256": "d" * 64,
                  "native-archive-sha256": hashlib.sha256(b"release asset bytes").hexdigest()}
        (self.assets / "BUILD.txt").write_text("".join(f"{key}={value}\n" for key, value in fields.items()))
        self.checksums()

    def checksums(self):
        lines = [f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
                 for path in sorted(self.assets.iterdir()) if path.name != "SHA256SUMS"]
        (self.assets / "SHA256SUMS").write_text("".join(lines))

    def verify(self, stage=None):
        return MODULE.verify(self.assets, "0.1.0", COMMIT, stage)

    def test_complete_source_matched_snapshot_is_independent_of_later_edits(self):
        stage = self.root / "snapshot"
        self.assertEqual(self.verify(stage), 5)
        (self.assets / "INSTALL.md").write_text("later modification")
        self.assertEqual(MODULE.verify(stage, "0.1.0", COMMIT), 5)

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
        for entry in ("../outside", "/etc/passwd", "nested/file", "BUILD.txt"):
            (self.assets / "SHA256SUMS").write_text(original + f"{'a' * 64}  {entry}\n")
            with self.assertRaises(ValueError):
                self.verify()

    def test_valid_checksums_do_not_hide_stale_commit_or_version(self):
        for build in (f"version=0.1.0\ncommit={'b' * 40}\n", f"version=0.0.9\ncommit={COMMIT}\n",
                      f"version=0.1.0\ncommit={COMMIT}\ncommit={COMMIT}\n"):
            (self.assets / "BUILD.txt").write_text(build)
            self.checksums()
            with self.assertRaisesRegex(ValueError, "identity"):
                self.verify()

    def test_modified_asset_fails(self):
        (self.assets / "INSTALL.md").write_text("modified")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.verify()

    def test_target_and_archive_identity_cannot_be_relabelled_with_new_checksums(self):
        path = self.assets / "BUILD.txt"
        original = path.read_text()
        for old, new in (("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"),
                         ("native-profile=release", "native-profile=debug"),
                         ("native-archive-sha256=", "unrecognized-archive="),
                         ("native-zxs-sha256=", "unrecognized-binary=")):
            path.write_text(original.replace(old, new))
            self.checksums()
            with self.assertRaises(ValueError):
                self.verify()

    def test_missing_build_receipt_is_rejected(self):
        (self.assets / "BUILD.txt").unlink()
        self.checksums()
        with self.assertRaisesRegex(ValueError, "complete expected asset set"):
            self.verify()

    def test_symlink_asset_directory_and_snapshot_are_rejected(self):
        original = self.assets / "INSTALL.md"
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
            MODULE.verify(linked, "0.1.0", COMMIT)
        with self.assertRaisesRegex(ValueError, "snapshot symlink"):
            self.verify(linked / "snapshot")
        self.assertFalse((self.assets / "snapshot").exists())

    def test_leaf_replaced_while_opening_cannot_follow_links_or_block_on_fifo(self):
        for replacement in ("symlink", "fifo"):
            with self.subTest(replacement=replacement):
                target = self.assets / "INSTALL.md"
                original = target.read_bytes()
                outside = self.root / "outside"
                outside.write_bytes(original)
                real_open = os.open
                replaced = False

                def replace_then_open(path, flags, *args, **kwargs):
                    nonlocal replaced
                    if path == "INSTALL.md" and not replaced:
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
                (self.assets / "INSTALL.md").write_text("redirected asset")
            return data
        with patch.object(MODULE, "read_metadata", side_effect=replace_after_receipt):
            self.assertEqual(self.verify(self.root / "snapshot"), 5)
        self.assertEqual((self.root / "snapshot/INSTALL.md").read_bytes(), b"release asset bytes")


if __name__ == "__main__":
    unittest.main()
