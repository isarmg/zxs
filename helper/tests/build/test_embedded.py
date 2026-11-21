"""Inert ELF header fixtures only, never invoke the build recipe or a product."""
import runpy
import struct
from pathlib import Path
import unittest

R = runpy.run_path(str(Path(__file__).resolve().parents[2] / "build_embedded.py"))


def image(rows, size=4096):
    b = bytearray(size)
    b[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHIQQ", b, 16, 3, 62, 1, 0x450, 64)
    struct.pack_into("<HHH", b, 52, 64, 56, len(rows))
    for i, row in enumerate(rows):
        struct.pack_into("<IIQQQQQQ", b, 64 + 56 * i, *row)
    return b


class EmbeddedProof(unittest.TestCase):
    def test_whole_static_pie_not_old_dependencies_or_a_loader(self):
        def with_tags(tags, extra=()):
            rows = [(1, 4, 0, 0, 0, 0x400, 0x400, 0x1000),
                    (1, 5, 0x400, 0x400, 0, 0x100, 0x100, 0x1000),
                    (2, 4, 0x240, 0x240, 0, 16 * len(tags), 16 * len(tags), 8),
                    (0x6474e551, 6, 0, 0, 0, 0, 0, 16), *extra]
            b = image(rows)
            for i, (tag, v) in enumerate(tags):
                struct.pack_into("<QQ", b, 0x240 + 16 * i, tag, v)
            return b
        pie = (0x6ffffffb, 0x08000000)
        R["qualified_zxs"](with_tags([pie, (0, 0)]))
        for b in [with_tags([pie, (1, 42), (0, 0)]),
                  with_tags([pie, (0, 0), (29, 42)]),
                  with_tags([pie, (0, 0)], [(3, 4, 0, 0, 0, 1, 1, 1)]),
                  with_tags([(0, 0)])]:
            with self.assertRaises(ValueError):
                R["qualified_zxs"](b)

    def test_exactly_one_whole_payload_in_the_actual_readonly_load_range(self):
        raw = b"synthetic proof bytes only!"
        b = image([(1, 4, 0, 0, 0, 0x400, 0x400, 0x1000),
                   (1, 5, 0x400, 0x400, 0, 0x100, 0x100, 0x1000)])
        b[0x200:0x200 + len(raw)] = raw
        self.assertEqual(R["verify_embedding"](b, raw), 0x200)
        doubled = bytearray(b)
        doubled[0x480:0x480 + len(raw)] = raw
        changed = bytearray(b)
        changed[0x201] ^= 1
        wrong_region = bytearray(b)
        wrong_region[0x200:0x200 + len(raw)] = bytes(len(raw))
        wrong_region[0x480:0x480 + len(raw)] = raw
        shared = image([(1, 4, 0, 0, 0, 0x400, 0x400, 0x1000),
                        (1, 5, 0x400, 0x400, 0, 0x100, 0x100, 0x1000),
                        (1, 6, 0x201, 0x801, 0, 8, 8, 0x1000)])
        shared[0x200:0x200 + len(raw)] = raw
        for implausible in [doubled, changed, wrong_region, shared]:
            with self.assertRaises(ValueError):
                R["verify_embedding"](implausible, raw)


if __name__ == "__main__":
    unittest.main()
