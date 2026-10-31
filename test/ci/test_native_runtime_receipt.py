"""Parser tests, not Debian runtime certification."""
import importlib.util
from pathlib import Path
import unittest
import subprocess
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('runtime_receipt', ROOT / '.github/scripts/native-runtime-receipt.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


class RuntimeReceipt(unittest.TestCase):
    def test_direct_and_transitive_paths_include_loader_not_kernel_vdso(self):
        self.assertEqual(M.resolved_libraries('''linux-vdso.so.1 (0x123)
libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x123)
/lib64/ld-linux-x86-64.so.2 (0x123)
'''), {'libc.so.6': '/lib/x86_64-linux-gnu/libc.so.6',
       'ld-linux-x86-64.so.2': '/lib64/ld-linux-x86-64.so.2'})

    def test_not_found_static_or_unrecognized_output_cannot_pass(self):
        for text in ('libx.so => not found', 'statically linked', '',
                     'libx.so => relative.so (0x123)', 'unexpected warning'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                M.resolved_libraries(text)

    def test_duplicate_library_is_rejected(self):
        with self.assertRaises(ValueError):
            M.resolved_libraries('libx.so => /a (0x1)\nlibx.so => /b (0x2)')

    def test_readelf_needed_only(self):
        self.assertEqual(M.needed(' 0x01 (NEEDED) Shared library: [libc.so.6]\n'
                                  ' 0x02 (RUNPATH) Library runpath: [/tmp/x]\n'), ['libc.so.6'])

    def test_usrmerge_package_owner_records_exact_version_and_bytes(self):
        path = Path('/lib/x86_64-linux-gnu/libc.so.6')
        resolved = Path('/usr/lib/x86_64-linux-gnu/libc.so.6')
        queries = [subprocess.CompletedProcess([], 1, '', ''),
                   subprocess.CompletedProcess([], 0, f'libc6:amd64: {resolved}\n', '')]
        with patch.object(Path, 'resolve', return_value=resolved),                 patch.object(M.subprocess, 'run', side_effect=queries),                 patch.object(M, 'run', return_value='2.41-fixture'),                 patch.object(M, 'sha256', return_value='a' * 64):
            result = M.package_owner(path)
        self.assertEqual(result['package'], 'libc6:amd64')
        self.assertEqual(result['version'], '2.41-fixture')
        self.assertEqual(result['resolved_path'], str(resolved))

    def test_unowned_library_and_query_failure_are_errors(self):
        path = Path('/library.so')
        for code in (1, 2):
            with patch.object(Path, 'resolve', return_value=path),                     patch.object(M.subprocess, 'run', return_value=subprocess.CompletedProcess([], code, '', 'error')):
                with self.assertRaises(ValueError):
                    M.package_owner(path)

    def test_missing_needed_library_refuses_receipt(self):
        with patch.object(Path, 'resolve', return_value=Path('/binary')), \
                patch.object(M, 'sha256', return_value='a' * 64), \
                patch.object(M, 'run', side_effect=['(NEEDED) Shared library: [missing.so]', '/ld.so (0x1)']):
            with self.assertRaisesRegex(ValueError, 'NEEDED'):
                M.binary_receipt('zxs', Path('/binary'))


if __name__ == '__main__':
    unittest.main()
