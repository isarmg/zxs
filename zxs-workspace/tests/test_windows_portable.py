"""Inert PE resource fixtures and an actual Server-side packaging gate check."""
import importlib.util
import os
from pathlib import Path
import runpy
import struct
import subprocess
import sys
import sysconfig
import tempfile
import unittest

PACKAGING = Path(__file__).resolve().parents[1] / "packaging" / "windows-amd64"
recipe = runpy.run_path(str(PACKAGING / "build.py"))
MANIFEST = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
</assembly>"""


def resource_image(manifests, *, machine=0x8664, subsystem=2, resource_id=1, virtual_size=3584, resource_size=None):
    """A data-only PE with one .rsrc section, no entry point or executable code."""
    raw = bytearray(4096)
    def put(fmt, offset, *values):
        struct.pack_into("<" + fmt, raw, offset, *values)

    raw[:2] = b"MZ"
    put("I", 0x3C, 0x80)
    raw[0x80:0x84] = b"PE\0\0"
    put("HHIIIHH", 0x84, machine, 1, 0, 0, 0, 0xF0, 0x20)
    h = 0x98
    put("H", h, 0x20B)
    put("QII", h + 24, 0x140000000, 4096, 512)
    put("II", h + 56, 8192, 512)
    put("H", h + 68, subsystem)
    put("III", h + 108, 16, 0, 0)
    put("II", h + 112 + 2 * 8, 4096, virtual_size if resource_size is None else resource_size)
    section = h + 0xF0
    put("8sIIIIIIHHI", section, b".rsrc", virtual_size, 4096, 3584, 512, 0, 0, 0, 0, 0x40000040)
    base = 512
    put("HII", base + 14, 1, 24, 0x80000018)
    put("HII", base + 0x18 + 14, 1, resource_id, 0x80000030)
    put("H", base + 0x30 + 14, len(manifests))
    body = 0x100
    for i, (lang, manifest) in enumerate(manifests):
        data = 0x80 + 16 * i
        put("II", base + 0x40 + 8 * i, lang, data)
        put("IIII", base + data, 4096 + body, len(manifest), 65001, 0)
        raw[base + body:base + body + len(manifest)] = manifest
        body += len(manifest)
    return raw


class ManifestPolicy(unittest.TestCase):
    def test_exact_invoker_policy(self):
        require = recipe["require_invoker_manifest"]
        require(MANIFEST)
        for raw in (
            b"", b" " * 65537, b"\xff", MANIFEST[:-1],
            MANIFEST.replace(b"asm.v3", b"asm.v1"),
            MANIFEST.replace(b"asInvoker", b"highestAvailable"),
            MANIFEST.replace(b"false", b"true"),
            MANIFEST.replace(b' uiAccess="false"', b""),
            MANIFEST.replace(b"</requestedPrivileges>", b"<requestedExecutionLevel/></requestedPrivileges>"),
            b"<!DOCTYPE assembly>" + MANIFEST,
        ):
            with self.subTest(raw=raw[:80]), self.assertRaises(ValueError):
                require(raw)

    @unittest.skipUnless(importlib.util.find_spec("pefile"), "requires the pinned portable recipe's pefile")
    def test_normal_and_conflicting_actual_pe_resources(self):
        require = recipe["require_entry_image"]
        with tempfile.TemporaryDirectory(prefix="workspace-inert-pe-") as path:
            image = Path(path) / "data-only.pe"
            def probe(manifests, **options):
                image.write_bytes(resource_image(manifests, **options))
                require(image)

            probe([(0, MANIFEST)])
            probe([(0, MANIFEST), (1033, MANIFEST)])
            probe([(0, MANIFEST)], virtual_size=0x100 + len(MANIFEST))
            for entries, options in (
                ([(1033, MANIFEST)], {}),
                ([(0, MANIFEST), (1033, MANIFEST.replace(b"false", b"true"))], {}),
                ([(0, MANIFEST)], {"resource_id": 2}),
                ([(0, MANIFEST)], {"machine": 0xAA64}),
                ([(0, MANIFEST)], {"subsystem": 3}),
            ):
                with self.subTest(options=options, languages=[n for n, _ in entries]), self.assertRaises(ValueError):
                    probe(entries, **options)
            with self.assertRaisesRegex(ValueError, "section's declared raw and virtual extent"):
                probe([(0, MANIFEST)], virtual_size=0x100 + len(MANIFEST) - 1)
            with self.assertRaisesRegex(ValueError, "manifest outside"):
                probe([(0, MANIFEST)], resource_size=0x100 + len(MANIFEST) - 1)
            with self.assertRaisesRegex(ValueError, "Resource structure outside"):
                probe([(0, MANIFEST)], resource_size=0x50)


@unittest.skipUnless(sys.platform == "win32" and sys.getwindowsversion().product_type in (2, 3)
                     and sysconfig.get_platform() == "win-amd64", "requires a real Windows x64 Server")
class NativeServer(unittest.TestCase):
    def test_actual_x64_architecture_does_not_admit_a_server_build(self):
        self.assertTrue(recipe["native_amd64"]())
        with self.assertRaisesRegex(ValueError, "Windows 11"):
            recipe["profile"]()

    def test_inbox_powershell_parses_the_launch_qualifier_without_running_it(self):
        shell = Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
        test_path = str(PACKAGING / "qualify-launch.ps1").replace("'", "''")
        code = (
            "$tokens = $null; $errors = $null;"
            f"$null = [System.Management.Automation.Language.Parser]::ParseFile('{test_path}', [ref]$tokens, [ref]$errors);"
            "if ($errors.Count) { $errors | Out-String | Write-Error; exit 1 }"
        )
        subprocess.run([shell, "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", code],
                       check=True, timeout=30)
