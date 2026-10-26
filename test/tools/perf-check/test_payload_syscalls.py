import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("payload_syscalls", ROOT / "tools/perf-check/payload-syscalls.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PayloadSyscallsTests(unittest.TestCase):
    def test_only_scoped_successful_body_bytes_count(self):
        lines = [
            '22 read(4</tmp/state/workspaces/ws/entry-1.full>, "body", 4) = 4',
            '22 write(2</tmp/log>, "ZXS_PAYLOAD_MEASURE_BEGIN fresh_prepare\\n", 40) = 40',
            '22 pread64(4</tmp/state/workspaces/ws/entry-1.full>, "body", 4, 0) = 4',
            '22 read(6</tmp/primary/staging/op/content/payload-measurement-body.bin>, "body", 4) = 4',
            '22 read(7</tmp/primary/histories/id/file-containers/file/body/versions/v000001_x.bin>, "body", 4) = 4',
            '22 read(8</tmp/primary/histories/id/versions/ver/checksums.bin>, "metadata", 8) = 8',
            '22 read(7</tmp/primary/histories/id/file-containers/file/body/versions/v000001_x.bin>, "", 4) = 0',
            '22 read(4</tmp/state/workspaces/ws/entry-1.full>, 0x1234, 4) = -1 EIO (Input/output error)',
            '22 write(9</tmp/primary/staging/op/content/.zxs-full-req_123>, "body", 4) = 4',
            '22 fsync(9</tmp/primary/staging/op/content/.zxs-full-req_123>) = 0',
            '22 write(2</tmp/log>, "ZXS_PAYLOAD_MEASURE_END fresh_prepare\\n", 38) = 38',
            '22 read(4</tmp/state/workspaces/ws/entry-1.full>, "body", 4) = 4',
        ]
        result = MODULE.syscall_sample(lines, "fresh_prepare")
        self.assertEqual(result["payload_read_bytes"], 12)
        self.assertEqual(result["payload_write_bytes"], 4)
        self.assertEqual(result["syscalls"], {"fsync": 1, "pread64": 1, "read": 5, "write": 1})

    def test_unfinished_reads_join_once_and_keep_path(self):
        lines = [
            '22 write(2, "ZXS_PAYLOAD_MEASURE_BEGIN export\\n", 34) = 34',
            '22 pread64(5</tmp/x/file-containers/f/body/versions/v000001_x.bin>, <unfinished ...>',
            '21 read(4</proc/self/status>, "outside", 7) = 7',
            '22 <... pread64 resumed>"payload", 7, 0) = 7',
            '22 write(2, "ZXS_PAYLOAD_MEASURE_END export\\n", 32) = 32',
        ]
        result = MODULE.syscall_sample(lines, "export")
        self.assertEqual(result["payload_read_bytes"], 7)
        self.assertEqual(result["syscalls"], {"pread64": 1, "read": 1})

    def test_missing_markers_fail_instead_of_claiming_zero(self):
        with self.assertRaises(ValueError):
            MODULE.syscall_sample([], "export")
        with self.assertRaises(ValueError):
            MODULE.syscall_sample(['22 write(2, "ZXS_PAYLOAD_MEASURE_BEGIN export\\n", 34) = 34'], "export")

    def test_incomplete_syscall_fails(self):
        with self.assertRaises(ValueError):
            list(MODULE.completed_lines(['22 read(4</tmp/body.full>, <unfinished ...>']))


if __name__ == "__main__":
    unittest.main()
