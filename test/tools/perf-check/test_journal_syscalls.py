import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("journal_syscalls", ROOT / "tools/perf-check/journal-syscalls.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class JournalSyscallsTests(unittest.TestCase):
    def test_held_descriptors_and_parent_directory_are_separated(self):
        lines = [
            '22 openat2(4</tmp/disk/metadata/application-log>, "journal.bin", {flags=O_RDWR}, 24) = 5</tmp/disk/metadata/application-log/journal.bin>',
            '22 write(5</tmp/disk/metadata/application-log/journal.bin>, "frame", 5) = 5',
            '22 fdatasync(5</tmp/disk/metadata/application-log/journal.bin>) = 0',
            '22 fsync(4</tmp/disk/metadata/application-log>) = 0',
            '22 statx(5</tmp/disk/metadata/application-log/journal.bin>, "", AT_EMPTY_PATH, STATX_ALL, {}) = 0',
            '22 write(7</tmp/disk/metadata/objects/test>, "object", 6) = 6',
        ]
        self.assertEqual(MODULE.syscall_counts(lines), {
            "directory.fsync": 1, "journal.fdatasync": 1,
            "journal.openat2": 1, "journal.statx": 1, "journal.write": 1,
        })

    def test_interrupted_syscall_is_counted_once(self):
        lines = [
            '22 read(5</tmp/disk/metadata/application-log/journal.bin>, <unfinished ...>',
            '22 <... read resumed>"frame", 8192) = 5',
        ]
        self.assertEqual(MODULE.syscall_counts(lines), {"journal.read": 1})


if __name__ == "__main__":
    unittest.main()
