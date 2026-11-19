"""Internal, one-shot export of ONE verified regular member. No GUI/CLI writer is enabled."""
from contextlib import contextmanager, ExitStack
import hashlib
import os
import zipfile

from .archive import read_managed_document, stamp
from .document import archive_sha
from .model import CHUNK, Rejected, check_id, member_name
from ._windows_access import OutputProtection
from ._windows_output import PinnedParent, leaf, _NTError
from ._windows_origin import check_mark, read_mark


class _Member:
    def __init__(self, src, dst, staging, protection, doc, h, source_zone, snapshot, name, output, operation):
        self.src, self.dst, self.staging, self.doc, self.h = src, dst, staging, doc, h
        self.protection = protection
        self.snapshot, self.name, self.output = snapshot, name, output
        # A literal DOS 8.3 name avoids generating a second short spelling. The staging
        # parent is physically separate: even an old NTFS tunnel cannot expose the final name.
        key = check_id(operation).upper()
        self.stage = key[:8] + "." + key[8:11]  # 44-bit lookup only; FILE_CREATE collision always stops.
        self.live, self.used, self.started, self.renaming = True, False, False, False
        self.marker = snapshot.browser.commit
        self.source_zone = source_zone
        self.original = self.source_object()
        self.origin = read_mark(src.api, h, source_zone)
        record = next((row for path,row in snapshot.browser.verified_entries() if path == name), None)
        if record is None or record.kind != "file": raise Rejected("select one original regular member, not a directory")
        self.size, self.sha = record.size_bytes, record.sha256
        marker, tree = read_managed_document(doc, display_name=snapshot.path.name)
        if marker != self.marker or tree.rows.get(name) != (False, self.size, self.sha):
            raise Rejected("the selected node or member no longer matches the original snapshot")
        self.raw = archive_sha(doc)  # Bind a loose selection to these bytes at action preparation.
        node = snapshot.exact
        if node is not None and (node.path != snapshot.path or node.observed != snapshot.observed or
                                 node.marker != marker or node.sha256 != self.raw):
            raise Rejected("not the originally listed archive bytes")
        self.check_source()
        staging.check()
        dst.check()
        self.protection.check()
        dst.api.absent(dst.handle, output)  # Advisory; the atomic rename never replaces an occupant.

    def source_object(self):
        self.src.check()
        api, original = self.src.api, self.snapshot
        pathname = self.src.name(original.path.name)
        row = api.ordinary(self.h, self.src.volume, pathname)
        with api.witness(self.src.handle, original.path.name) as named:
            if (api.ordinary(named, self.src.volume, pathname) != row or
                    stamp(self.doc) != original.observed or row != getattr(self, "original", row)):
                raise Rejected("not the same selected archive instance and generation")
        return row

    def check_source(self):
        self.source_object()
        check_mark(self.src.api, self.h, self.source_zone, self.origin)

    def _copy(self, output):
        # The source has been fully admitted; its held handle denies external data writers/deletion.
        with zipfile.ZipFile(self.doc, "r") as z:
            info = z.getinfo(self.name)
            if (z.comment != self.marker.encode() or info.orig_filename != self.name or info.is_dir() or
                    info.compress_type != zipfile.ZIP_STORED or info.file_size != self.size):
                raise Rejected("original stored member changed")
            digest, left = hashlib.sha256(), self.size
            with z.open(info) as member:
                while left:
                    data = member.read(min(CHUNK, left))
                    if not data or len(data) > min(CHUNK, left): raise Rejected("short or oversized member read")
                    digest.update(data)
                    self.dst.api.write(output, data)
                    left -= len(data)
                if member.read(1) or digest.hexdigest() != self.sha:
                    raise Rejected("the copied member differs from its verified size or SHA-256")

    def _name_matches(self, h, parent, leafname, first_id, size, zone=None):
        api, exact = self.dst.api, parent.name(leafname)
        held = api.ordinary(h, parent.volume, exact)
        if held.file_id != first_id or held.size != size:
            raise Rejected("the selected new output changed identity or size")
        with api.witness(parent.handle, leafname) as witness:
            if api.ordinary(witness, parent.volume, exact) != held:
                raise Rejected("the relative output name no longer identifies the held new file")
        if zone is None: api.only_default_stream(h, held.size)
        else: check_mark(api, h, zone, self.origin)

    def publish(self):
        if not self.live or self.used: raise Rejected("this exact member export was closed or already attempted")
        self.used = True
        dst, staging, api = self.dst, self.staging, self.dst.api
        self.check_source()
        staging.check()
        dst.check()
        api.absent(dst.handle, self.output)
        self.protection.check()
        # Establish both original directories admit normal local sync before creating a file.
        api.flush(staging.handle)
        api.flush(dst.handle)
        staging.check()
        dst.check()
        self.protection.check()
        self.started = True
        try:
            with ExitStack() as new:
                h = new.enter_context(api.new_file(staging.handle, self.stage))
                stage_id = api.ordinary(h, staging.volume, staging.name(self.stage)).file_id
                self._name_matches(h, staging, self.stage, stage_id, 0)
                self.protection.check(h)  # Check the actual new file before writing even its zone.
                api.absent(dst.handle, self.output)
                zone = new.enter_context(api.new_zone(h))
                api.write(zone, self.origin)
                api.flush(zone)
                self._name_matches(h, staging, self.stage, stage_id, 0, zone)
                self.check_source()  # No member payload is written before both original marks pass.
                self.protection.check(h)
                self._copy(h)
                api.flush(h)
                api.verify_bytes(h, self.size, self.sha)  # Read back the held physical file, not the ZIP.
                self._name_matches(h, staging, self.stage, stage_id, self.size, zone)
                self.check_source()
                staging.check()
                dst.check()
                self.protection.check(h)
                self.renaming = True
                try:
                    api.rename_new(h, dst.handle, self.output)
                except _NTError as err:
                    if err.code == 0xc0000035:  # STATUS_OBJECT_NAME_COLLISION, not a generic failure.
                        self._name_matches(h, staging, self.stage, stage_id, self.size, zone)
                        self.renaming = False
                        raise FileExistsError("another file already occupies the output name") from err
                    raise
                api.flush(dst.handle)
                api.flush(staging.handle)
                self._name_matches(h, dst, self.output, stage_id, self.size, zone)
                api.absent(staging.handle, self.stage)
                self.check_source()
                staging.check()
                dst.check()
                self.protection.check(h)  # After both real directory barriers; never repair by name.
            return {"node": self.marker.operation, "archive_sha256": self.raw, "member": self.name,
                    "bytes": self.size, "sha256": self.sha, "new_file": dst.name(self.output)}
        except BaseException as err:
            self.note(err)
            raise

    def note(self, err):
        if not self.started: return
        if self.renaming:
            msg = f"Final name {self.output!r} may already exist; the exact result is unconfirmed."
        else:
            msg = f"No completed publication; a partial or complete file {self.stage!r} may remain in the selected staging directory."
        msg += " Do not automatically retry, remove or adopt an object by name."
        if msg not in getattr(err, "__notes__", ()): err.add_note(msg)


@contextmanager
def selected_export(snapshot, member, destination_parent, filename, operation, *, staging_parent):
    """Acquire the actual original paths at selection, hold through the decision, never infer a HEAD.

    The caller must explicitly grant BOTH existing directories (including retained staging files)
    and keep them across consent. No writes at preparation. No implicit temp location or copy
    fallback. Only a matching narrow owner/DACL/default-integrity profile is admitted and the
    new file itself must have the expected inherited grants. Existing holders, privileged or
    same-user writers are not excluded by DACL observations; synthetic directories must remain
    exclusive and unsynchronized. No account settings or ACL edits. Not a public writer.
    """
    if os.name != "nt": raise Rejected("this internal exporter requires a fixed local Windows NTFS volume")
    import msvcrt
    member_name(member)
    leaf(filename)
    check_id(operation)
    plan = None
    try:
        with ExitStack() as stack:
            # Neither a read-only folder bookmark nor a later re-open of a destination is used.
            dst = stack.enter_context(PinnedParent(destination_parent, output=True))
            staging = stack.enter_context(PinnedParent(staging_parent, output=True))
            if (staging.guid != dst.guid or staging.volume != dst.volume or
                    staging.chain[-1][1] == dst.chain[-1][1]):
                raise Rejected("select separate staging and output directories on the same fixed local NTFS volume")
            protection = OutputProtection(staging.handle, dst.handle)
            src = stack.enter_context(PinnedParent(snapshot.path.parent))
            own = src.api.input_file(src.handle, snapshot.path.name)
            h = stack.enter_context(own)
            # Successful conversion transfers the actual HANDLE to the CRT. One owner closes it.
            fd = msvcrt.open_osfhandle(h, os.O_RDONLY)
            own.value = None
            stack.callback(os.close, fd)
            msvcrt.setmode(fd, os.O_BINARY)  # A CRT descriptor mode, not a system or file setting.
            doc = stack.enter_context(os.fdopen(fd, "rb", closefd=False))
            # Opening this existing stream does not create, synthesize or infer a zone.
            source_zone = stack.enter_context(src.api.source_zone(h))
            plan = _Member(src, dst, staging, protection, doc, h, source_zone, snapshot, member, filename, operation)
            try: yield plan
            finally: plan.live = False
    except BaseException as err:
        if plan is not None: plan.note(err)
        raise
