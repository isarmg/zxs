"""Pure bounded native inventory shape; no filesystem or operating system is touched."""
import struct
import unittest

from zipworkspace._windows_origin import DATA, ZONE, stream_lengths
from zipworkspace.model import Rejected


def row(name, length, *, another=False):
    name = name.encode("utf-16-le")
    extent = 24 + len(name)
    advance = (extent + 7) & ~7
    return struct.pack("<IIqq", advance if another else 0, len(name), length, 0) + name + (
        bytes(advance - extent) if another else b"")


class StreamInventory(unittest.TestCase):
    def test_actual_extent_order_terminal_padding_and_default_only(self):
        for a, b in (((DATA, 3), (ZONE, 24)), ((ZONE, 24), (DATA, 3))):
            self.assertEqual(stream_lengths(row(*a, another=True) + row(*b)), {DATA:3, ZONE:24})
        self.assertEqual(stream_lengths(row("", 0)), {DATA:0})
        self.assertEqual(stream_lengths(row(DATA, 0) + b"\x00\x00"), {DATA:0})

    def test_broken_chain_extra_duplicate_and_incomplete_terminal_never_mean_no_mark(self):
        full = row(DATA, 1, another=True) + row(ZONE, 24)
        for wire in (b"", bytes(513), full[:-2], full + bytes(9), row(DATA, 3, another=True),
                     row(DATA, 1, another=True) + row(DATA, 1), row(ZONE, 1),
                     row(DATA, 1, another=True) + row(":other:$DATA", 1),
                     row("", 1, another=True) + row(ZONE, 24),
                     struct.pack("<IIqq", 1, 0, 0, 0) + bytes(24)):
            with self.subTest(length=len(wire)), self.assertRaises(Rejected): stream_lengths(wire)
