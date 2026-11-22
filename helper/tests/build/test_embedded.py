"""Inert ELF header fixtures only, never invoke the build recipe or a product."""
import runpy
import struct
from pathlib import Path
import unittest

R = runpy.run_path(str(Path(__file__).resolve().parents[2] / "build_embedded.py"))


def image(rows, size=4096, entry=0x450):
    b = bytearray(size)
    b[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHIQQ", b, 16, 3, 62, 1, entry, 64)
    struct.pack_into("<HHH", b, 52, 64, 56, len(rows))
    for i, row in enumerate(rows):
        struct.pack_into("<IIQQQQQQ", b, 64 + 56 * i, *row)
    return b


PAYLOAD = b"synthetic proof bytes only!"
OFFSET = 0x1200


def embedded_image(*extra):
    b = image([(1, 4, 0, 0, 0, 0x1400, 0x1400, 0x1000),
               (1, 5, 0x2000, 0x3000, 0, 0x100, 0x100, 0x1000), *extra],
              size=0x6000, entry=0x3050)
    b[OFFSET:OFFSET + len(PAYLOAD)] = PAYLOAD
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
        b = embedded_image()
        self.assertEqual(R["verify_embedding"](b, PAYLOAD), OFFSET)
        doubled = bytearray(b)
        doubled[0x2080:0x2080 + len(PAYLOAD)] = PAYLOAD
        changed = bytearray(b)
        changed[OFFSET + 1] ^= 1
        wrong_region = bytearray(b)
        wrong_region[OFFSET:OFFSET + len(PAYLOAD)] = bytes(len(PAYLOAD))
        wrong_region[0x2080:0x2080 + len(PAYLOAD)] = PAYLOAD
        shared = embedded_image((1, 6, OFFSET + 1, 0x4201, 0, 8, 8, 0x1000))
        for implausible in [doubled, changed, wrong_region, shared]:
            with self.assertRaises(ValueError):
                R["verify_embedding"](implausible, PAYLOAD)

    def test_different_file_offsets_must_not_remap_the_payload_virtual_pages(self):
        for row in [(1, 6, 0x4200, OFFSET, 0, 8, 8, 0x1000),
                    (1, 6, 0x4100, 0x1100, 0, 8, 8, 0x1000),
                    (1, 6, 0x4800, 0x1800, 0, 8, 8, 0x1000),
                    (1, 6, 0, 0x1800, 0, 0, 8, 0x1000)]:
            with self.subTest(row=row), self.assertRaisesRegex(ValueError, "touches a virtual page"):
                R["verify_embedding"](embedded_image(row), PAYLOAD)
        # A genuinely adjacent page and a zero-memory load are not aliases.
        separate = embedded_image((1, 6, 0x4000, 0x2000, 0, 8, 8, 0x1000),
                                  (1, 6, 0, OFFSET, 0, 0, 0, 0x1000))
        self.assertEqual(R["verify_embedding"](separate, PAYLOAD), OFFSET)

    def test_file_backed_loads_have_to_agree_with_the_linux_page_offset(self):
        b = embedded_image((1, 6, 0x4100, 0x4200, 0, 8, 8, 0x1000))
        with self.assertRaisesRegex(ValueError, "disagree within a page"):
            R["verify_embedding"](b, PAYLOAD)


if __name__ == "__main__":
    unittest.main()
