"""Read-only, implementation-independent oracle for the generated ZIP pair."""

import binascii
import hashlib
import sys
import zipfile
from pathlib import Path

left, right = [Path(arg).read_bytes() for arg in sys.argv[1:]]
assert left == right, "equivalent ordinary trees must produce identical ZIPs"
assert left[-22:-18] == b"PK\x05\x06", "expected ordinary ZIP32 end record"
expected = {
    ".zxsignore": b"custom.tmp\n",
    "a.txt": b"",
    "empty/": b"",
    "nested/": b"",
    "nested/中.txt": b"exact synthetic\x00binary\n",
    "z.txt": b"deflate deflate deflate\n",
}
with zipfile.ZipFile(sys.argv[1]) as archive:
    assert archive.namelist() == list(expected)
    assert archive.comment == b""
    assert archive.testzip() is None
    for info in archive.infolist():
        payload = expected[info.filename]
        is_dir = info.filename.endswith("/")
        assert info.is_dir() == is_dir
        assert info.compress_type == (zipfile.ZIP_STORED if is_dir else zipfile.ZIP_DEFLATED)
        assert info.flag_bits == 0x800
        assert info.create_system == 3
        assert info.date_time == (1980, 1, 1, 0, 0, 0)
        assert (info.external_attr >> 16) == (0o040755 if is_dir else 0o100644)
        assert not info.extra and not info.comment
        assert info.file_size == len(payload)
        assert info.CRC == binascii.crc32(payload)
        assert archive.read(info) == payload

print(f"ZIP32/Deflate independent readback: {len(left)} bytes sha256={hashlib.sha256(left).hexdigest()}", flush=True)
