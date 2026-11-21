"""Budget the central directory before ZipFile allocates it; verify stored data."""
from __future__ import annotations
from contextlib import contextmanager, nullcontext
import hashlib
import io
import os
from pathlib import Path
import stat
import struct
import zipfile
from .model import CHUNK, MAX_CD, MAX_MEMBERS, MAX_TOTAL, Commit, Manifest, Rejected

EOCD = struct.Struct("<4s4H2IH")
LOC = struct.Struct("<4sIQI")
EOCD64 = struct.Struct("<4sQ2H2I4Q")
LOCAL = struct.Struct("<4s5H3I2H")
ZIP64_LIMIT = 0xffffffff


def bounded_directory(f, end=None, *, check=None):
    count, start, _ = _directory_layout(f, end, check=check)
    return count, start


def _directory_layout(f, end=None, *, check=None):
    if check is not None: check()
    if end is None: end = f.seek(0, os.SEEK_END)
    if end < EOCD.size or end > MAX_TOTAL + 2 * MAX_CD + (1 << 20):
        raise Rejected("archive length outside budget")
    f.seek(max(0, end - 65535 - EOCD.size))
    tail_start = f.tell()
    tail = f.read(end - tail_start)
    if len(tail) != end - tail_start: raise Rejected("archive ended before its selected length")
    i = tail.rfind(b"PK\x05\x06")
    if i < 0 or len(tail) - i < EOCD.size:
        raise Rejected("missing end of central directory")
    _, disk, cd_disk, local_count, count, cd_size, cd_offset, length = EOCD.unpack_from(tail, i)
    if len(tail) != i + EOCD.size + length or disk or cd_disk or local_count != count:
        raise Rejected("not a single complete ZIP")
    pos = tail_start + i
    if pos >= LOC.size:
        f.seek(pos - LOC.size)
        raw = f.read(LOC.size)
        if len(raw) != LOC.size: raise Rejected("incomplete ZIP64 locator window")
        loc = LOC.unpack(raw)
        if loc[0] == b"PK\x06\x07":
            if loc[1] or loc[3] != 1 or loc[2] + EOCD64.size != pos - LOC.size:
                raise Rejected("unsupported or inconsistent ZIP64 layout")
            f.seek(loc[2])
            raw = f.read(EOCD64.size)
            if len(raw) != EOCD64.size: raise Rejected("incomplete ZIP64 directory end")
            z = EOCD64.unpack(raw)
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
    _check_records(f, count, cd_offset, pos, check=check)
    return count, cd_offset, pos


def _check_records(f, count, cd_offset, pos, *, check=None, forms=None):
    # Do not let a lying EOCD count cause ZipFile to materialize an unbounded
    # number of objects from the central directory. Check each fixed record.
    f.seek(cd_offset)
    for _ in range(count):
        if check is not None: check()
        header = f.read(46)
        if len(header) != 46 or header[:4] != b"PK\x01\x02":
            raise Rejected("malformed central directory header")
        name_len, extra_len, comment_len, volume = struct.unpack_from("<4H", header, 28)
        if not name_len or volume or f.seek(name_len + extra_len + comment_len, os.SEEK_CUR) > pos:
            raise Rejected("invalid central directory lengths or volume")
        if forms is not None:
            # Keep original size and offset sentinels before ZipInfo expands them.
            compressed, size = struct.unpack_from("<II", header, 20)
            offset, = struct.unpack_from("<I", header, 42)
            forms.append((compressed == ZIP64_LIMIT) | ((size == ZIP64_LIMIT) << 1)
                         | ((offset == ZIP64_LIMIT) << 2))
    if f.tell() != pos: raise Rejected("extra central directory entries")


class _IndexedInput:
    """Constrain zipfile's own, second footer/directory read to the selected EOF and budgets."""
    def __init__(self, file, end, count, central, *, central_end=None, check=None):
        self.file, self.end, self.count, self.central = file, end, count, central
        self.check, self.central_end, self.forms = check, central_end, None

    def tell(self): return self.file.tell()
    def seekable(self): return True
    def seek(self, n, whence=os.SEEK_SET):
        if whence == os.SEEK_END: return self.file.seek(self.end + n)
        return self.file.seek(n, whence)

    def read(self, n=-1):
        if self.check is not None: self.check()
        pos = self.tell()
        left = max(0, self.end - pos)
        if n == -1: n = left
        if not 0 <= n <= MAX_CD: raise Rejected("ZIP input read exceeds its allocation budget")
        n = min(n, left)
        # Member reads never exceed CHUNK. A larger read is only legal for the original
        # central table. Smaller buffers cannot contain MAX_MEMBERS fixed-size records.
        if n > CHUNK and pos != self.central: raise Rejected("central directory moved during admission")
        data = self.file.read(n)
        if len(data) != n: raise Rejected("selected ZIP changed length during admission")
        if self.central_end is not None and pos == self.central and n == self.central_end - pos:
            forms = []
            _check_records(io.BytesIO(data), self.count, 0, n, check=self.check, forms=forms)
            self.forms = tuple(forms)
        elif n > CHUNK:
            raise Rejected("unexpected central directory length during admission")
        if self.check is not None: self.check()
        return data


def _zip64_extra(extra):
    zi = None
    i = 0
    while i < len(extra):
        if len(extra)-i < 4: raise Rejected("truncated ZIP extra field")
        field, length = struct.unpack_from("<HH", extra, i)
        i += 4
        if i + length > len(extra): raise Rejected("invalid ZIP extra length")
        if field == 1:
            if zi is not None: raise Rejected("duplicate ZIP64 extra field")
            zi = extra[i:i+length]
        if field == 0x9901: raise Rejected("encrypted ZIP metadata is not accepted")
        i += length
    return zi


def _local_sizes(extra, uncompressed, compressed):
    zi = _zip64_extra(extra)
    wide = ZIP64_LIMIT in (uncompressed, compressed)
    if wide:
        if (uncompressed, compressed) != (ZIP64_LIMIT, ZIP64_LIMIT) or zi is None or len(zi) != 16:
            raise Rejected("local ZIP64 must provide both entry sizes")
        return struct.unpack("<QQ", zi), True
    if zi is not None: raise Rejected("unexpected local ZIP64 size block")
    return (uncompressed, compressed), False


def _descriptor(f, item, start, stop, wide, central_form):
    width = stop - start
    # APPNOTE 4.3.9: two sizes use ZIP64 width when they are widened. In
    # particular a large physical offset alone is not a large member size;
    # both standard wide and CPython's narrow streamed layout occur there.
    if wide or central_form & 3:
        allowed = (20, 24)
    elif central_form & 4:
        allowed = (12, 16, 20, 24)
    else:
        allowed = (12, 16)
    if width not in allowed:
        raise Rejected("unsupported streamed ZIP descriptor width or member boundary")
    f.seek(start)
    descriptor = struct.pack("<IQQ" if width >= 20 else "<III", item.CRC, item.compress_size, item.file_size)
    if width in (16, 24): descriptor = b"PK\x07\x08" + descriptor
    if f.read(width) != descriptor:
        raise Rejected("streamed ZIP descriptor does not describe its actual member")


def _local_header(f, item, stop, central_form):
    # zipfile verifies the payload CRC and local filename when opening. Check
    # the format declarations too: many ordinary tools consult the local header.
    f.seek(item.header_offset)
    raw = f.read(LOCAL.size)
    if len(raw) != LOCAL.size: raise Rejected("missing local member header")
    sig, version, flags, method, _, __, crc, compressed, size, nlen, xlen = LOCAL.unpack(raw)
    if sig != b"PK\x03\x04" or flags != item.flag_bits or method != zipfile.ZIP_STORED:
        raise Rejected("local and central Store/encryption declarations disagree")
    start = item.header_offset + LOCAL.size + nlen + xlen
    end = start + item.compress_size
    if not 0 < nlen <= 4096 or end > stop: raise Rejected("local member outside its physical boundary")
    name = f.read(nlen)
    if len(name) != nlen or name.decode("utf-8" if flags & 0x800 else "cp437") != item.orig_filename:
        raise Rejected("local and central member names disagree")
    extra = f.read(xlen)
    if len(extra) != xlen: raise Rejected("incomplete local ZIP extra field")
    sizes, big = _local_sizes(extra, size, compressed)
    if big and (version & 0xff) < 45: raise Rejected("local ZIP64 version is lower than 4.5")
    targets = (item.file_size,item.compress_size)
    if not flags & 8:
        if crc != item.CRC or sizes != targets:
            raise Rejected("local sizes or CRC disagree with central record")
        return start
    if crc not in (0,item.CRC) or any(a not in (0,b) for a,b in zip(sizes,targets)):
        raise Rejected("streamed local member disagrees with central record")
    if item.file_size >= ZIP64_LIMIT or item.compress_size >= ZIP64_LIMIT: big = True
    _descriptor(f, item, end, stop, big, central_form)
    return start


def _member_header(f, item, stop, central_form):
    directory = item.is_dir()
    if item.orig_filename != item.filename or item.compress_type != zipfile.ZIP_STORED or item.flag_bits & ~0x808:
        raise Rejected("only ordinary unencrypted Store entries are supported")
    if item.compress_size != item.file_size or directory and (item.file_size or item.CRC):
        raise Rejected("inconsistent Store sizes")
    kind = stat.S_IFMT(item.external_attr >> 16) if item.create_system == 3 else 0
    if kind not in (0, stat.S_IFDIR if directory else stat.S_IFREG) or item.external_attr & 0x400:
        raise Rejected("links, devices and reparse entries are not accepted")
    zi = _zip64_extra(item.extra)
    if (zi is not None) != bool(central_form) or (
            zi is not None and (len(zi) != central_form.bit_count()*8 or item.extract_version < 45)):
        raise Rejected("central ZIP64 fields or version do not match the original sentinels")
    return directory, _local_header(f, item, stop, central_form)


@contextmanager
def _selected_index(document, length, marker=None, *, check=None):
    count, central, cd_end = _directory_layout(document, length, check=check)
    reader = _IndexedInput(document, length, count, central, central_end=cd_end, check=check)
    with zipfile.ZipFile(reader, "r") as archive:
        if (len(archive.filelist) != count or archive.start_dir != central or reader.forms is None
                or (marker is not None and Commit.decode(archive.comment) != marker)):
            raise Rejected("the selected snapshot directory or marker changed")
        physical = sorted(zip(archive.infolist(), reader.forms, strict=True), key=lambda entry: entry[0].header_offset)
        lanes = []
        for i, (item, form) in enumerate(physical):
            if check is not None: check()
            stop = physical[i+1][0].header_offset if i+1 < count else central
            if not 0 <= item.header_offset <= stop - LOCAL.size or stop > central:
                raise Rejected("local ZIP member headers overlap or leave the data region")
            lanes.append((item, stop, form))
        yield archive, lanes


@contextmanager
def stored_member(document, length, marker, name, size, *, locate=False, check=None):
    """Bound one admitted Store member; optionally return the verified physical body start."""
    with _selected_index(document, length, marker, check=check) as (archive, lanes):
        matches = [lane for lane in lanes if lane[0].filename == name]
        if len(matches) != 1: raise Rejected("cannot identify the exact original member")
        (item, stop, form), = matches
        directory, start = _member_header(document, item, stop, form)
        if directory or item.file_size != size or (size == 0 and item.CRC):
            raise Rejected("the selected ordinary member changed")
        with archive.open(item) as source:
            yield (source, start) if locate else source


@contextmanager
def stored_subtree(document, length, marker, root, rows):
    """Bind one admitted subtree against a single index, before creating a writer."""
    prefix = root + "/"
    with _selected_index(document, length, marker) as (archive, lanes):
        files, explicit, accounted = [], set(), set()
        for item, stop, form in lanes:
            if item.filename == root: raise Rejected("selected directory is a file in this index")
            if not item.filename.startswith(prefix): continue
            directory, _ = _member_header(document, item, stop, form)
            relative = item.filename[len(prefix):]
            if directory and relative: relative = relative[:-1]
            if relative in explicit: raise Rejected("duplicated record in selected subtree")
            explicit.add(relative)
            if not relative:
                if not directory or item.filename != prefix: raise Rejected("invalid selected directory root")
                continue
            expected = rows.get(relative)
            if expected is None or expected[:2] != (directory, item.file_size) or not item.file_size and item.CRC:
                raise Rejected("a selected subtree record is not in the original directory")
            if not directory: files.append((relative, item))
            # The original manifest includes implicit directories. Account for
            # them only as ancestors of a real record; empty leaves need a record.
            name = relative
            while name and name not in accounted:
                accounted.add(name)
                name = name.rpartition("/")[0]
        if accounted != rows.keys() or (not rows and "" not in explicit):
            raise Rejected("the selected subtree is missing an original member or empty directory")
        yield archive, files


@contextmanager
def stored_plain(document, length, rows):
    """Reconcile every local declaration with a previous read of this selected ordinary ZIP."""
    with _selected_index(document, length) as (archive, lanes):
        listed, files = Manifest(), []
        for item, stop, form in lanes:
            directory, _ = _member_header(document, item, stop, form)
            name = item.filename[:-1] if directory else item.filename
            if not item.file_size and item.CRC:
                raise Rejected("empty ZIP entry cannot have a nonzero CRC")
            listed.add(name, directory, item.file_size, None)
            if not directory: files.append((name, item))
        if listed.rows != {path: (directory, size, None) for path, (directory, size, _) in rows.items()}:
            raise Rejected("the ordinary ZIP no longer matches the original approved directory")
        yield archive, files


def copy_body(source, size, target, *, leaves=None, observer=None):
    """Read one complete Store stream (CRC by ZipExtFile), hashing and optionally syncing it."""
    digest, written = hashlib.sha256(), 0
    while True:
        if observer is not None: observer.check()
        block = source.read(CHUNK)
        if observer is not None: observer.check()
        if not block: break
        if leaves is not None:
            if len(block) != min(CHUNK, size - written):
                raise Rejected("incomplete verification block in Store member")
            leaves.extend(hashlib.sha256(block).digest())
        written += len(block)
        if written > size: raise Rejected("data exceeds declared size")
        digest.update(block)
        if observer is not None: observer.advance(len(block))
        if target is not None and target.write(block) != len(block):
            raise Rejected("short private member write")
    if written != size: raise Rejected("incomplete member")
    if target is not None:
        if observer is not None: observer.check()
        target.flush()
        os.fsync(target.fileno())
        if observer is not None: observer.check()
    return digest.hexdigest()


def stamp(f):
    m = os.fstat(f.fileno())
    return m.st_dev, m.st_ino, m.st_size, m.st_mtime_ns, m.st_ctime_ns


def read_archive(path, *, managed=True, sink: Path | None = None, internal=False):
    """Sink is a new app-private scratch directory, never a user-selected existing tree."""
    path = Path(path)
    if not internal and path.suffix.lower() != (".zxs" if managed else ".zip"):
        raise Rejected("use .zxs for a repository, or the separate .zip import command")
    if not managed and sink is not None:
        raise Rejected("ordinary import requires a fresh held destination, not an existing output path")
    return _read_checked(path.open("rb"), managed=managed, sink=_PathSink(sink) if sink is not None else None)


def read_managed_document(document, *, display_name: str, observer=None):
    """Verify one selected open file without closing it, reopening its name, or extracting."""
    return _managed_document(document, display_name, None, observer=observer)


def read_indexed_document(document, *, display_name: str):
    """Same full admission, additionally retaining verified Store offsets and block hashes for pread."""
    blocks = {}
    marker, manifest = _managed_document(document, display_name, blocks)
    return marker, manifest, blocks  # Never expose the partial index of a rejected archive.


def _managed_document(document, display_name, blocks, *, observer=None):
    if not display_name or "/" in display_name or "\\" in display_name or Path(display_name).suffix.lower() != ".zxs":
        raise Rejected("select a managed .zxs document")
    if not document.seekable() or not stat.S_ISREG(os.fstat(document.fileno()).st_mode):
        raise Rejected("select a seekable regular file or a completed private copy")
    position = document.tell()
    try:
        return _read_checked(nullcontext(document), managed=True, sink=None, blocks=blocks, observer=observer)
    finally:
        document.seek(position)


class _PathSink:
    """Adapter for the existing managed checkout's private scratch path."""
    def __init__(self, path): self.path = path
    def mkdir(self, name): (self.path / name).mkdir(mode=0o700, exist_ok=False)
    def file(self, name): return (self.path / name).open("xb")
    def sync(self, rows, *, checkpoint=None):
        dirs = [self.path/name for name,(directory,_,__) in rows.items() if directory]
        for directory in sorted(dirs, key=lambda p:len(p.parts), reverse=True) + [self.path]:
            if checkpoint is not None: checkpoint()
            fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try: os.fsync(fd)
            finally: os.close(fd)


def read_plain_document(document, *, observer=None):
    """Verify all entries on a borrowed ordinary ZIP without extracting anything."""
    return _read_checked(nullcontext(document), managed=False, sink=None, observer=observer)[1]


def _read_checked(source, *, managed, sink, blocks=None, observer=None):
    if not managed and sink is not None:
        raise Rejected("ordinary ZIP extraction is not admitted")
    with source as f:
        check = observer.check if observer is not None else None
        if observer is not None: observer.begin("layout")
        before = stamp(f)
        with _selected_index(f, before[2], check=check) as (z, lanes):
            marker = Commit.decode(z.comment) if managed else None
            manifest = Manifest()
            items = []
            for item, stop, form in lanes:
                if check is not None: check()
                d, start = _member_header(f, item, stop, form)
                name = item.filename[:-1] if d else item.filename
                manifest.add(name, d, item.file_size, None)
                items.append((item, name, d))
                # Only the optional mount index holds offsets; ordinary browse/checkout keeps no extra table.
                if blocks is not None and not d: blocks[name] = start
            if sink is not None:
                # Only private freshly created sinks are passed by our callers.
                for name, (is_dir, _, _) in sorted(manifest.rows.items()):
                    if is_dir:
                        if check is not None: check()
                        sink.mkdir(name)
            if observer is not None: observer.begin("members", manifest.total)
            for info, name, directory in items:
                leaves = bytearray() if blocks is not None and not directory else None
                with z.open(info) as source:
                    output = sink.file(name) if sink is not None and not directory else nullcontext()
                    with output as target:
                        sha = copy_body(source, info.file_size, target, leaves=leaves, observer=observer)
                if not directory: manifest.rows[name] = (False, info.file_size, sha)
                if leaves is not None: blocks[name] = (blocks[name], bytes(leaves))
            if sink is not None:
                # Child data/name durability precedes publishing a visible checkout.
                if check is None: sink.sync(manifest.rows)
                else:
                    observer.begin("sync")
                    sink.sync(manifest.rows, checkpoint=check)
            if observer is not None: observer.begin("identity")
            if marker and manifest.digest() != marker.tree:
                raise Rejected("repository marker disagrees with the complete directory")
        if before != stamp(f):
            raise Rejected("archive changed while reading")
        if check is not None: check()
    return marker, manifest
