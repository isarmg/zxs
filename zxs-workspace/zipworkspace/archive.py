"""Budget the central directory before ZipFile allocates it; verify stored data."""
from __future__ import annotations
import hashlib
import os
from pathlib import Path
import stat
import struct
import zipfile
from .model import CHUNK, MAX_CD, MAX_MEMBERS, MAX_TOTAL, Commit, Manifest, Rejected

EOCD = struct.Struct("<4s4H2IH")
LOC = struct.Struct("<4sIQI")
EOCD64 = struct.Struct("<4sQ2H2I4Q")


def bounded_directory(f):
    end = f.seek(0, os.SEEK_END)
    if end < EOCD.size or end > MAX_TOTAL + 2 * MAX_CD + (1 << 20):
        raise Rejected("archive length outside budget")
    f.seek(max(0, end - 65535 - EOCD.size))
    tail_start = f.tell()
    tail = f.read()
    i = tail.rfind(b"PK\x05\x06")
    if i < 0 or len(tail) - i < EOCD.size:
        raise Rejected("missing end of central directory")
    _, disk, cd_disk, local_count, count, cd_size, cd_offset, length = EOCD.unpack_from(tail, i)
    if len(tail) != i + EOCD.size + length or disk or cd_disk or local_count != count:
        raise Rejected("not a single complete ZIP")
    pos = tail_start + i
    if pos >= LOC.size:
        f.seek(pos - LOC.size)
        loc = LOC.unpack(f.read(LOC.size))
        if loc[0] == b"PK\x06\x07":
            if loc[1] or loc[3] != 1 or loc[2] + EOCD64.size != pos - LOC.size:
                raise Rejected("unsupported or inconsistent ZIP64 layout")
            f.seek(loc[2])
            z = EOCD64.unpack(f.read(EOCD64.size))
            if z[0] != b"PK\x06\x06" or z[1] != 44 or z[4:6] != (0, 0) or z[6] != z[7]:
                raise Rejected("invalid ZIP64 directory")
            for narrow, full, sentinel in ((count, z[7], 0xffff), (cd_size, z[8], 0xffffffff), (cd_offset, z[9], 0xffffffff)):
                if narrow not in (sentinel, full):
                    raise Rejected("ZIP64 fields disagree")
            count, cd_size, cd_offset, pos = z[7], z[8], z[9], loc[2]
    if count > MAX_MEMBERS or cd_size > MAX_CD or cd_offset + cd_size != pos:
        raise Rejected("invalid or oversized central directory")
    if count * 46 > cd_size:
        raise Rejected("invalid declared member count")
    # Do not let a lying EOCD count cause ZipFile to materialize an unbounded
    # number of objects from the central directory. Check each fixed record.
    f.seek(cd_offset)
    for _ in range(count):
        header = f.read(46)
        if len(header) != 46 or header[:4] != b"PK\x01\x02":
            raise Rejected("malformed central directory header")
        name_len, extra_len, comment_len, volume = struct.unpack_from("<4H", header, 28)
        if not name_len or volume or f.seek(name_len + extra_len + comment_len, os.SEEK_CUR) > pos:
            raise Rejected("invalid central directory lengths or volume")
    if f.tell() != pos: raise Rejected("extra central directory entries")
    return count, cd_offset


LOCAL = struct.Struct("<4s5H3I2H")
ZIP64_LIMIT = 0xffffffff


def _zip64_extra(extra):
    zi = None
    i = 0
    while i < len(extra):
        if len(extra)-i < 4: raise Rejected("truncated ZIP extra field")
        field, length = struct.unpack_from("<HH", extra, i)
        i += 4
        if i + length > len(extra): raise Rejected("invalid ZIP extra length")
        if field == 1:
            if zi is not None: raise Rejected("duplicate local ZIP64 extra field")
            zi = extra[i:i+length]
        if field == 0x9901: raise Rejected("encrypted ZIP metadata is not accepted")
        i += length
    return zi


def _local_sizes(extra, uncompressed, compressed):
    required = (uncompressed == ZIP64_LIMIT, compressed == ZIP64_LIMIT)
    zi = _zip64_extra(extra)
    values = [uncompressed,compressed]
    pos = 0
    for n, needed in enumerate(required):
        if needed:
            if zi is None or len(zi)-pos < 8: raise Rejected("missing local ZIP64 entry size")
            values[n] = struct.unpack_from("<Q", zi, pos)[0]; pos += 8
    return values, any(required)


def _local_header(f, item, central_at):
    # zipfile verifies the payload CRC and local filename when opening. Check
    # the format declarations too: many ordinary tools consult the local header.
    f.seek(item.header_offset)
    raw = f.read(LOCAL.size)
    if len(raw) != LOCAL.size: raise Rejected("missing local member header")
    sig, version, flags, method, _, __, crc, compressed, size, nlen, xlen = LOCAL.unpack(raw)
    if sig != b"PK\x03\x04" or flags != item.flag_bits or method != zipfile.ZIP_STORED:
        raise Rejected("local and central Store/encryption declarations disagree")
    end = item.header_offset + LOCAL.size + nlen + xlen + item.compress_size
    if nlen > 4096 or end > central_at: raise Rejected("local member outside archive budget")
    name = f.read(nlen)
    if name.decode("utf-8" if flags & 0x800 else "cp437") != item.orig_filename:
        raise Rejected("local and central member names disagree")
    sizes, big = _local_sizes(f.read(xlen), size, compressed)
    targets = (item.file_size,item.compress_size)
    if not flags & 8:
        if crc != item.CRC or tuple(sizes) != targets:
            raise Rejected("local sizes or CRC disagree with central record")
        return
    if crc not in (0,item.CRC) or any(a not in (0,b) for a,b in zip(sizes,targets)):
        raise Rejected("streamed local member disagrees with central record")
    if item.file_size >= ZIP64_LIMIT or item.compress_size >= ZIP64_LIMIT: big = True
    descriptor = struct.pack("<IQQ" if big else "<III", item.CRC, item.compress_size, item.file_size)
    f.seek(end)
    remaining = min(len(descriptor)+4, central_at-end)
    after = f.read(remaining)
    if not (after.startswith(descriptor) or after.startswith(b"PK\x07\x08"+descriptor)):
        raise Rejected("streamed ZIP descriptor does not describe its actual member")


def stamp(f):
    m = os.fstat(f.fileno())
    return m.st_dev, m.st_ino, m.st_size, m.st_mtime_ns, m.st_ctime_ns


def read_archive(path, *, managed=True, sink: Path | None = None, internal=False):
    """Sink is a new app-private scratch directory, never a user-selected existing tree."""
    path = Path(path)
    if not internal and path.suffix.lower() != (".zxs" if managed else ".zip"):
        raise Rejected("use .zxs for a repository, or the separate .zip import command")
    with path.open("rb") as f:
        before = stamp(f)
        expected_count, central_at = bounded_directory(f)
        with zipfile.ZipFile(f, "r") as z:
            if len(z.filelist) != expected_count:
                raise Rejected("central directory member count disagrees with its footer")
            marker = Commit.decode(z.comment) if managed else None
            manifest = Manifest()
            items = []
            for item in z.infolist():
                d = item.is_dir()
                if item.orig_filename != item.filename or item.compress_type != zipfile.ZIP_STORED or item.flag_bits & ~0x808:
                    raise Rejected("only ordinary unencrypted Store entries are supported")
                if item.compress_size != item.file_size or d and (item.file_size or item.CRC):
                    raise Rejected("inconsistent Store sizes")
                kind = stat.S_IFMT(item.external_attr >> 16) if item.create_system == 3 else 0
                if kind not in (0, stat.S_IFDIR if d else stat.S_IFREG) or item.external_attr & 0x400:
                    raise Rejected("links, devices and reparse entries are not accepted")
                _zip64_extra(item.extra)
                _local_header(f, item, central_at)
                name = item.filename[:-1] if d else item.filename
                manifest.add(name, d, item.file_size, None)
                items.append((item, name, d))
            if sink is not None:
                # Only private freshly created sinks are passed by our callers.
                for name, (is_dir, _, _) in sorted(manifest.rows.items()):
                    if is_dir:
                        (sink / name).mkdir(mode=0o700, exist_ok=False)
            for info, name, directory in items:
                h = hashlib.sha256()
                written = 0
                with z.open(info) as source:
                    target = (sink / name).open("xb") if sink is not None and not directory else None
                    try:
                        while block := source.read(CHUNK):
                            written += len(block)
                            if written > info.file_size:
                                raise Rejected("data exceeds declared size")
                            h.update(block)
                            if target: target.write(block)
                        if target:
                            target.flush()
                            os.fsync(target.fileno())
                    finally:
                        if target: target.close()
                if written != info.file_size:
                    raise Rejected("incomplete member")
                if not directory: manifest.rows[name] = (False, written, h.hexdigest())
            if sink is not None:
                # Child data/name durability precedes publishing a visible checkout.
                dirs = [sink/name for name,(directory,_,__) in manifest.rows.items() if directory]
                for directory in sorted(dirs, key=lambda p:len(p.parts), reverse=True) + [sink]:
                    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                    try: os.fsync(fd)
                    finally: os.close(fd)
            if marker and manifest.digest() != marker.tree:
                raise Rejected("repository marker disagrees with the complete directory")
        if before != stamp(f):
            raise Rejected("archive changed while reading")
    return marker, manifest
