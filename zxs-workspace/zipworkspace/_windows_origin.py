"""Narrow Windows container-origin evidence, never an attachment verdict or network input."""
import struct

from .model import Rejected


DATA = "::$DATA"
ZONE = ":Zone.Identifier:$DATA"
NAMES = {name.encode("utf-16-le"): name for name in (DATA, ZONE)}
HEADER = struct.Struct("<IIqq")
# Accept only the documented, unambiguous two-line Internet/Restricted form. A URL or
# other unapproved field may be private: refuse the entire export, never strip, log or rewrite it.
MARKS = frozenset(b"[ZoneTransfer]" + nl + b"ZoneId=" + digit + end
                  for nl in (b"\r\n", b"\n") for digit in (b"3", b"4") for end in (b"", nl))
LENGTHS = frozenset(map(len, MARKS))
LIMIT = 512  # FileStreamInformation extent, not a data/marker read limit.


def stream_lengths(buffer):
    """Parse only the bytes native IO_STATUS_BLOCK.Information says it returned."""
    if not HEADER.size <= len(buffer) <= LIMIT:
        raise Rejected("missing or oversized NTFS stream inventory")
    found, pos = {}, 0
    while pos + HEADER.size <= len(buffer):
        nxt, count, size, allocation = HEADER.unpack_from(buffer, pos)
        start = pos + HEADER.size
        end = start + count
        if end > len(buffer) or size < 0 or allocation < 0:
            raise Rejected("incomplete NTFS stream inventory")
        # WDK also permits an empty stream-name field for a sole unnamed data stream.
        name = DATA if not count and not pos and not nxt else NAMES.get(buffer[start:end])
        if name is None or name in found:
            raise Rejected("unexpected or duplicate NTFS data stream")
        found[name] = size
        aligned = (end + 7) & ~7
        if not nxt:
            if not end <= len(buffer) <= aligned or DATA not in found:
                raise Rejected("incomplete NTFS stream terminator")
            return found
        if nxt != aligned - pos or aligned + HEADER.size > len(buffer):
            raise Rejected("invalid NTFS stream boundary")
        pos = aligned
    raise Rejected("unterminated NTFS stream inventory")


def read_mark(api, primary, stream):
    volume, item = api.info(primary)
    other_volume, alternate = api.info(stream)  # Includes the actual stream's EOF and delete-pending flag.
    inventory = api.streams(primary)
    if (item.directory or alternate.directory or other_volume != volume or alternate.file_id != item.file_id
            or alternate.size not in LENGTHS or inventory != {DATA:item.size, ZONE:alternate.size}):
        raise Rejected("cannot establish the original container mark on this file")
    # FileId belongs to the file, NOT to an individual stream. The caller acquired this fixed
    # stream relative to that very primary handle. Both concrete contents and the live name matter.
    api.rewind(stream)
    raw = api.read(stream, alternate.size)
    if len(raw) != alternate.size or api.read(stream, 1) or raw not in MARKS:
        raise Rejected("missing or unsupported Windows container-origin evidence")
    return raw


def check_mark(api, primary, held, original):
    if read_mark(api, primary, held) != original:
        raise Rejected("the held container mark changed")
    # A stage mark must share DELETE with our primary rename handle. Not sharing WRITE does
    # not lock the stream name; prove its current bytes by a fresh, existing-only relative open.
    with api.zone_witness(primary) as named:
        if read_mark(api, primary, named) != original:
            raise Rejected("the current container mark does not match the original")
