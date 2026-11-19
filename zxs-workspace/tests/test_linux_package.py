"""Recipe contract only. These tests neither create Debian archives nor require Qt/FUSE."""
import importlib.util
import sys
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import TestCase, mock, skipUnless

RECIPE = Path(__file__).resolve().parents[1] / "packaging/ubuntu-26.10/package.py"
spec = importlib.util.spec_from_file_location("workspace_deb_recipe", RECIPE)
pkg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pkg)


class UbuntuProfile(TestCase):
    @skipUnless(sys.platform == "linux", "native Linux package shape; no POSIX UID on Windows")
    def test_components_are_disjoint_and_correspond_to_the_real_entry_points(self):
        with TemporaryDirectory() as t:
            out = Path(t).resolve() / "new"
            release = {"ID": "ubuntu", "VERSION_ID": "26.10", "VERSION_CODENAME": "stonking"}
            def simulated_archive(argv, **kw):
                self.assertEqual(argv[:2], ["/usr/bin/dpkg-deb", "--root-owner-group"])
                # No tool/executable is run in this contract test.
                Path(argv[-1]).write_bytes(b"package tool fixture")
            with (mock.patch.object(pkg.os, "getuid", return_value=1000),
                  mock.patch.object(pkg.platform, "freedesktop_os_release", return_value=release),
                  mock.patch.object(pkg.sys, "version_info", (3, 14)),
                  mock.patch.object(pkg.subprocess, "check_output", return_value=b"amd64\n"),
                  mock.patch.object(pkg.subprocess, "run", side_effect=simulated_archive) as native):
                pkg.build(out, "Test Packager <fixture@example.invalid>")
                with self.assertRaises(FileExistsError):
                    pkg.build(out, "Test Packager <fixture@example.invalid>")
            self.assertEqual(native.call_count, 3)
            self.assertEqual(len((out / "SHA256SUMS").read_text().splitlines()), 3)
            installed = set()
            for name in pkg.ENTRY:
                stage = out / "staging" / name
                files = {p.relative_to(stage).as_posix() for p in stage.rglob("*") if p.is_file()}
                self.assertIn("DEBIAN/control", files)
                self.assertFalse(any(p.startswith("DEBIAN/") and p != "DEBIAN/control" for p in files))
                files.remove("DEBIAN/control")
                self.assertTrue(installed.isdisjoint(files))
                installed.update(files)
                self.assertEqual({p for p in files if p.startswith("usr/bin/")}, {"usr/bin/" + name})
                self.assertIn(f"from {pkg.ENTRY[name].split(':')[0]} import main", (stage / "usr/bin" / name).read_text())
                self.assertEqual(pkg.ROOT + "zipworkspace/_fuse3.py" in files, name == pkg.MOUNT)
                self.assertEqual("usr/share/applications/zip-workspace.desktop" in files, name == pkg.GUI)
                self.assertEqual(pkg.ROOT + "zipworkspace/_mount_child.py" in files, name == pkg.BASE)
                self.assertEqual("pyfuse3" in pkg.DEPENDS[name], name == pkg.MOUNT)
                self.assertEqual("pyside6" in pkg.DEPENDS[name], name == pkg.GUI)

    def test_bad_checksum_or_injected_path_never_becomes_a_package_input(self):
        with TemporaryDirectory() as t:
            base = Path(t).resolve()
            (base / "pyproject.toml").write_bytes(b"changed")
            for path in ("pyproject.toml", "../notes", "zipworkspace/__pycache__/private.pyc"):
                with self.subTest(path=path), self.assertRaises(ValueError):
                    pkg.read_inputs(base, ["0" * 64 + "  " + path])
        with self.assertRaises(ValueError):
            pkg.maintainer("Bad <fixture@example.invalid>\nDepends: something")
