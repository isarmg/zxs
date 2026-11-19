"""Opt-in synthetic contract on a disposable ORIGINAL Windows fixed-local NTFS scratch directory.

Provide an operator-only scratch with equally protected descendants; no sync jobs, EFS or user data.
The same-volume rename does not itself re-inherit the destination ACL. Tests leave
fresh synthetic cases for their operator to inspect; they perform NO recursive/rebound-name cleanup.
Do not change privilege, sharing policy or OS settings to pass. No UI, external process, or foreign file.
"""
from dataclasses import replace
import hashlib
from contextlib import contextmanager
import os
from pathlib import Path
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace._windows_access import NativeSecurity, OutputProtection
from zipworkspace._windows_member import selected_export
from zipworkspace._windows_output import PinnedParent
from zipworkspace.desktop.selection import Selection
from zipworkspace.document import archive_sha, named_document
from zipworkspace.model import Commit, Manifest, Rejected, new_id
from zipworkspace.refs import Node


SCRATCH = os.environ.get("ZIPWORKSPACE_NTFS_EXPORT_SCRATCH")
PAYLOAD = b"the selected subdirectory's ordinary member\0\r\n\x1a\xff\xfe" * 137
RECEIPT = b"Harmless synthetic Windows attachment qualification document.\r\n"
# Separate opt-in from SCRATCH. Never call real Attachment Services for an ordinary native fixture.
ALLOW_SERVICES = "ALLOW_SYNTHETIC_CONTENT_PATHS_AND_ARCHIVE_TO_CONFIGURED_SERVICES"
MARK = b"[ZoneTransfer]\r\nZoneId=3\r\n"
RESTRICTED = b"[ZoneTransfer]\nZoneId=4"


@unittest.skipUnless(os.name == "nt" and SCRATCH, "explicit disposable Windows fixed-local NTFS scratch required")
class OriginalMemberExport(unittest.TestCase):
    def setUp(self):
        # Only an operator-owned, quiescent, disposable location appointed for this qualification.
        scratch = PinnedParent(SCRATCH, output=True)
        self.addCleanup(scratch.close)  # Keep every approved ancestor from setup to teardown.
        # Do not change the operator's ACL or create synthetic bytes in an unqualified scratch.
        proof = NativeSecurity()
        try: proof.read(scratch.handle, proof.caller(), directory=True)
        except Rejected as err: self.skipTest(f"existing scratch/profile not applicable: {err}")
        # Windows mkdtemp/mkdir(0700) can install a special Owner Rights ACL instead of
        # inheriting this approved scratch. One new ordinary directory; never retry a collision.
        name = "zw-test-" + new_id()
        native = Path(scratch.chain[-1][2]) / name
        native.mkdir()  # Default create security: no explicit descriptor or ACL mutation.
        self.root = Path(SCRATCH) / name
        case = PinnedParent(self.root, output=True)
        self.addCleanup(case.close)
        if case.chain[-1][2] != str(native): raise Rejected("the synthetic case changed scope")
        proof.read(case.handle, proof.caller(), directory=True)
        outer = self.root / "outer"
        outer.mkdir()
        self.dst, self.staging = outer / "destination", self.root / "staging"
        self.dst.mkdir()
        self.staging.mkdir()
        # Verify the real pair BEFORE constructing even the synthetic source payload. Do
        # not retain these setup witnesses: the product must pin the selections for itself.
        with PinnedParent(self.dst, output=True) as dst, PinnedParent(self.staging, output=True) as stage:
            OutputProtection(stage.handle, dst.handle)
        self.archive = self.root / "loose.zxs"
        files = {"first/same.bin":b"not the chosen file", "second/same.bin":PAYLOAD, "zero":b"",
                 "reports/receipt.txt":RECEIPT, "reports/receipt .txt":RECEIPT}
        manifest = Manifest()
        for name, body in files.items():
            manifest.add(name, False, len(body), hashlib.sha256(body).hexdigest())
        marker = Commit(new_id(), new_id(), 1, new_id(), None, manifest.digest())
        with zipfile.ZipFile(self.archive, "x", compression=zipfile.ZIP_STORED) as z:
            for name, body in files.items(): z.writestr(name, body)
            z.comment = marker.encode()
        with open(str(self.archive) + ":Zone.Identifier", "xb") as evidence:
            evidence.write(MARK)
        self.picked = Selection.open(self.archive)
        self.op = new_id()

    def choice(self, name="second/same.bin", *, selected=None, output="result.bin", staging=None):
        return selected_export(selected or self.picked, name, self.dst, output, self.op,
                               staging_parent=staging or self.staging)

    def test_original_member_and_zero_bytes_no_write_until_publish_and_no_second_submission(self):
        for path, body, output in (("second/same.bin", PAYLOAD, "result.bin"), ("zero", b"", "zero.bin")):
            with self.choice(path, output=output) as plan:
                self.assertFalse((self.dst / output).exists())
                self.assertFalse((self.staging / plan.stage).exists())
                result = plan.publish()
                self.assertEqual(result["node"], self.picked.browser.commit.operation)
                self.assertEqual((result["member"], result["sha256"]), (path, hashlib.sha256(body).hexdigest()))
                with self.assertRaises(Rejected): plan.publish()
            self.assertEqual((self.dst / output).read_bytes(), body)
            with open(str(self.dst / output) + ":Zone.Identifier", "rb") as evidence:
                self.assertEqual(evidence.read(), MARK)  # Exact CRLF bytes; no regenerated zone.
            self.assertFalse((self.staging / plan.stage).exists())
            self.op = new_id()
        with self.choice(output="cancelled.bin") as discarded: pass
        with self.assertRaises(Rejected): discarded.publish()
        self.assertFalse((self.dst / "cancelled.bin").exists())

    def test_wrong_original_history_bytes_and_alternate_source_evidence_refuse_before_staging(self):
        with named_document(self.archive) as f: sha = archive_sha(f)
        bad = ("1" if sha[0] != "1" else "0") + sha[1:]
        foreign = Node(marker=self.picked.browser.commit, path=self.archive, sha256=bad, observed=self.picked.observed)
        with self.assertRaises(Rejected), self.choice(selected=replace(self.picked, exact=foreign)): pass
        # Never discard an unrelated source stream to make the archive acceptable.
        with open(str(self.archive) + ":other", "xb") as ads: ads.write(b"unrecognized synthetic metadata")
        with self.assertRaises(Rejected), self.choice(selected=Selection.open(self.archive)): pass
        self.assertEqual(list(self.dst.iterdir()), [])
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_unmarked_ambiguous_unapproved_and_oversized_evidence_is_never_inferred_or_stripped(self):
        # All files below are newly created synthetic copies; never alter an existing mark.
        for i, mark in enumerate((None, b"", b"[ZoneTransfer]\nZoneId=9", MARK + b"ZoneId=4\r\n",
                                  MARK + b"HostUrl=https://example.invalid/private\r\n", b"x" * 512)):
            path = self.root / f"badmark{i}.zxs"
            with path.open("xb") as f: f.write(self.archive.read_bytes())
            if mark is not None:
                with open(str(path) + ":Zone.Identifier", "xb") as ads: ads.write(mark)
            with self.assertRaises(Rejected), self.choice(selected=Selection.open(path)): pass
        self.assertEqual(list(self.dst.iterdir()), [])
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_restricted_mark_is_preserved_byte_for_byte_and_its_held_stream_is_flushed_before_copy(self):
        path = self.root / "restricted.zxs"
        with path.open("xb") as f: f.write(self.archive.read_bytes())
        with open(str(path) + ":Zone.Identifier", "xb") as zone: zone.write(RESTRICTED)
        with self.choice(selected=Selection.open(path)) as plan:
            original, events = plan.dst.api.flush, []
            def flush(h):
                outcome = original(h)
                events.append(h)
                return outcome
            copy = plan._copy
            def member(h):
                self.assertEqual(len(events), 3)  # Two real parent preflights, then the ADS flush.
                self.assertNotIn(events[-1], (plan.staging.handle, plan.dst.handle, h))
                return copy(h)
            with patch.object(plan.dst.api, "flush", side_effect=flush), patch.object(plan, "_copy", side_effect=member):
                plan.publish()
        with open(str(self.dst / "result.bin") + ":Zone.Identifier", "rb") as zone:
            self.assertEqual(zone.read(), RESTRICTED)  # Exact LF and lack of final newline also preserved.

    def test_origin_flush_failure_never_enters_copy_or_creates_unmarked_final(self):
        with self.choice() as plan:
            original = plan.dst.api.flush
            def barrier(h):
                if h not in (plan.staging.handle, plan.dst.handle):
                    raise OSError("injected refusal to flush the original new origin stream")
                original(h)
            with patch.object(plan.dst.api, "flush", side_effect=barrier), patch.object(plan, "_copy") as copy:
                with self.assertRaises(OSError): plan.publish()
                copy.assert_not_called()
        self.assertEqual((self.staging / plan.stage).stat().st_size, 0)
        self.assertEqual(list(self.dst.iterdir()), [])

    def test_final_origin_cannot_be_rechecked_reports_uncertain_and_never_repairs_by_name(self):
        with self.choice() as plan:
            api, renamed = plan.dst.api, False
            rename, witness = api.rename_new, api.zone_witness
            def native_rename(*args):
                nonlocal renamed
                rename(*args)
                renamed = True
            def verify(primary):
                if renamed: raise OSError("injected final stream name verification failure")
                return witness(primary)
            with patch.object(api, "new_zone", wraps=api.new_zone) as create, \
                    patch.object(api, "rename_new", side_effect=native_rename), \
                    patch.object(api, "zone_witness", side_effect=verify):
                with self.assertRaises(OSError) as error: plan.publish()
                self.assertTrue(any("Final name" in note for note in error.exception.__notes__))
                create.assert_called_once()  # Never reattach the mark to a path to obtain "success".
                with self.assertRaises(Rejected): plan.publish()
        self.assertEqual((self.dst / "result.bin").read_bytes(), PAYLOAD)
        with open(str(self.dst / "result.bin") + ":Zone.Identifier", "rb") as evidence:
            self.assertEqual(evidence.read(), MARK)

    def test_missing_or_directory_member_and_occupied_leaf_or_stage_do_not_write_an_existing_file(self):
        for nonfile in ("same.bin", "first", "First/same.bin"):
            with self.assertRaises(Rejected), self.choice(nonfile): pass
        occupied = self.dst / "RESULT.BIN"
        occupied.write_bytes(b"other")
        with self.assertRaises(FileExistsError), self.choice(): pass
        self.assertEqual(occupied.read_bytes(), b"other")
        with self.choice(output="different.bin") as plan:
            stage = self.staging / plan.stage
            stage.write_bytes(b"not mine")
            with self.assertRaises((OSError, Rejected)): plan.publish()
        self.assertEqual(stage.read_bytes(), b"not mine")
        self.assertEqual(occupied.read_bytes(), b"other")
        self.assertFalse((self.dst / "different.bin").exists())

    def test_real_stage_already_is_dos_83_and_can_never_supply_the_requested_short_final_during_copy(self):
        short = self.op[:8].upper() + "." + self.op[8:11].upper()
        with self.assertRaises(Rejected), self.choice(staging=self.dst): pass
        self.assertEqual(list(self.dst.iterdir()), [])
        self.assertEqual(list(self.staging.iterdir()), [])
        # Even exactly the same spelling as the stage is safe as the intended final:
        # the two original directories are different on the same qualified volume.
        final = short
        with self.choice(output=final) as plan:
            self.assertRegex(plan.stage, r"^[0-9A-F]{8}\.[0-9A-F]{3}\Z")
            copy = plan._copy
            def actual_file(h):
                self.assertEqual(plan.dst.api.guid_name(h), plan.staging.name(short))
                plan.dst.api.absent(plan.dst.handle, final)
                # This is the original staged primary handle, not a path that might have changed.
                with plan.dst.api.zone_witness(h) as ads:
                    from zipworkspace._windows_origin import read_mark
                    self.assertEqual(read_mark(plan.dst.api, h, ads), MARK)
                copy(h)
                plan.dst.api.absent(plan.dst.handle, final)
            with patch.object(plan, "_copy", side_effect=actual_file): plan.publish()
        self.assertEqual((self.dst / final).read_bytes(), PAYLOAD)

    def test_file_permissions_are_read_before_zone_and_final_profile_is_checked_after_real_rename(self):
        with self.choice() as plan:
            check = plan.protection.api.read
            def refuse_file(handle, actor, *, directory):
                actual = check(handle, actor, directory=directory)
                if not directory: raise Rejected("injected refusal reading actual new file ACL")
                return actual
            with patch.object(plan.protection.api, "read", side_effect=refuse_file), \
                    patch.object(plan.dst.api, "new_zone") as mark, patch.object(plan, "_copy") as copy:
                with self.assertRaises(Rejected): plan.publish()
                mark.assert_not_called()
                copy.assert_not_called()
        stage = self.staging / plan.stage
        self.assertEqual(stage.stat().st_size, 0)
        self.assertEqual(list(self.dst.iterdir()), [])

        self.op = new_id()
        with self.choice() as plan:
            api, published = plan.dst.api, False
            rename, read = api.rename_new, plan.protection.api.read
            def move(*args):
                nonlocal published
                rename(*args)
                published = True
            def changed(handle, actor, *, directory):
                actual = read(handle, actor, directory=directory)
                if published and not directory: raise Rejected("injected changed final file permissions")
                return actual
            with patch.object(api, "rename_new", side_effect=move), \
                    patch.object(plan.protection.api, "read", side_effect=changed):
                with self.assertRaises(Rejected) as error: plan.publish()
                self.assertTrue(any("Final name" in n for n in error.exception.__notes__))
                with self.assertRaises(Rejected): plan.publish()
        self.assertEqual((self.dst / "result.bin").read_bytes(), PAYLOAD)

    def test_changed_original_directory_permissions_stop_before_creating_a_file(self):
        with self.choice() as plan:
            read = plan.protection.api.read
            def changed(handle, actor, *, directory):
                actual = read(handle, actor, directory=directory)
                if handle == plan.dst.handle: raise Rejected("injected loss of approved directory profile")
                return actual
            with patch.object(plan.protection.api, "read", side_effect=changed), \
                    patch.object(plan.dst.api, "new_file") as create:
                with self.assertRaises(Rejected): plan.publish()
                create.assert_not_called()
        self.assertEqual(list(self.staging.iterdir()), [])
        self.assertEqual(list(self.dst.iterdir()), [])

    def test_efs_observation_on_original_staging_directory_refuses_before_native_create(self):
        # Exercise the decision without changing the operator's encryption or access settings.
        with self.choice() as plan:
            info = plan.staging.api.info
            def encrypted(h):
                vol, row = info(h)
                return (vol, replace(row, attributes=row.attributes | 0x4000)) if h == plan.staging.handle else (vol, row)
            with patch.object(plan.staging.api, "info", side_effect=encrypted), self.assertRaises(Rejected):
                plan.publish()
        self.assertEqual(list(self.staging.iterdir()), [])
        self.assertEqual(list(self.dst.iterdir()), [])

    def test_rebinding_ancestor_is_refused_and_atomic_collision_retains_the_newcomer_and_original(self):
        with self.choice() as plan:
            with self.assertRaises(OSError): self.dst.parent.rename(self.root / "other")
            rename = plan.dst.api.rename_new
            def collision(h, parent, new):
                self.assertNotEqual(new.upper(), new)
                with (self.dst / new.upper()).open("xb") as occupant: occupant.write(b"winner")
                return rename(h, parent, new)
            with patch.object(plan.dst.api, "rename_new", side_effect=collision):
                with self.assertRaises((OSError, Rejected)): plan.publish()
        self.assertEqual([p.name for p in self.dst.iterdir()], ["RESULT.BIN"])
        self.assertEqual((self.dst / "RESULT.BIN").read_bytes(), b"winner")
        self.assertEqual((self.staging / plan.stage).read_bytes(), PAYLOAD)

    def test_incomplete_native_payload_is_not_renamed_and_unrelated_opens_cannot_take_the_stage(self):
        with self.choice() as plan:
            write = plan.dst.api.write
            def incomplete(h, data):
                if data == MARK: return write(h, data)  # Only truncate the payload, not the origin.
                with self.assertRaises(OSError): (self.staging / plan.stage).open("r+b").close()
                write(h, data[:-1])
            with patch.object(plan.dst.api, "write", side_effect=incomplete), self.assertRaises(Rejected):
                plan.publish()
        self.assertTrue((self.staging / plan.stage).exists())
        self.assertFalse((self.dst / "result.bin").exists())

    def test_two_actual_parent_barriers_before_create_vs_after_rename_and_no_second_submission(self):
        for location in ("staging", "dst"):
            for occurrence in (1, 2):
                final = f"{location}{occurrence}.bin"
                with self.choice(output=final) as plan:
                    flush, refused = plan.dst.api.flush, getattr(plan, location).handle
                    observed = []
                    def barrier(h):
                        observed.append(h)
                        if h == refused and observed.count(h) == occurrence:
                            raise OSError("injected original parent barrier failure")
                        flush(h)
                    with patch.object(plan.dst.api, "flush", side_effect=barrier), self.assertRaises(OSError) as error:
                        plan.publish()
                    self.assertEqual(observed.count(refused), occurrence)
                    self.assertEqual(any("Final name" in n for n in getattr(error.exception, "__notes__", ())),
                                     occurrence == 2)
                    with self.assertRaises(Rejected): plan.publish()
                self.assertFalse((self.staging / plan.stage).exists())
                if occurrence == 1: self.assertFalse((self.dst / final).exists())
                else: self.assertEqual((self.dst / final).read_bytes(), PAYLOAD)
                self.op = new_id()

    def test_private_assessment_refuses_missing_consent_or_changed_filename_before_new_file_or_com(self):
        for name, permission in (("receipt.txt", False), ("changed.txt", True), ("receipt.TXT", True)):
            with self.choice("reports/receipt.txt", output=name) as plan, \
                    patch("zipworkspace._windows_member.attachment_services") as activate, \
                    patch.object(plan.dst.api, "new_file") as create:
                with self.assertRaises(Rejected):
                    plan.assess_private_attachment(allow_configured_windows_services=permission)
                activate.assert_not_called()
                create.assert_not_called()
                with self.assertRaises(Rejected): plan.publish()  # No fallback to offline publication.
        self.assertEqual(list(self.dst.iterdir()), [])
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_real_original_reopens_only_after_services_release_and_never_moves_to_final(self):
        with self.choice("reports/receipt.txt", output="receipt.txt") as plan:
            private, proposed, referring = map(str, (self.staging / "receipt.txt",
                                                      self.dst / "receipt.txt", self.archive))
            calls = []
            @contextmanager
            def synthetic_services(**_):
                class Services:
                    def check_proposed(inner, name, path, source):
                        calls.append(("proposed", name, path, source))
                        self.assertFalse(Path(private).exists())
                    def save_private(inner, name, path, source):
                        calls.append(("private", name, path, source))
                        # A normally writable data handle proves the old exclusive writer is gone.
                        with Path(path).open("r+b") as f: self.assertEqual(f.read(), RECEIPT)
                        with open(path+":Zone.Identifier", "r+b") as f: self.assertEqual(f.read(), MARK)
                yield Services()
            with patch("zipworkspace._windows_member.attachment_services", synthetic_services), \
                    patch.object(plan.dst.api, "rename_new") as rename:
                result = plan.assess_private_attachment(allow_configured_windows_services=True)
                self.assertFalse(result["published"])
                self.assertEqual(calls, [("proposed","receipt.txt",proposed,referring),
                                         ("private","receipt.txt",private,referring)])
                rename.assert_not_called()
                with self.assertRaises(Rejected): plan.publish()
        self.assertEqual(list(self.dst.iterdir()), [])

    def test_service_quarantine_changed_bytes_or_original_replacement_cannot_publish_or_recreate(self):
        # Separate newly made cases: every input below is synthetic; never clean an unknown object.
        for mode in ("quarantine", "rewrite", "same-bytes-replacement", "block", "changed-origin"):
            parent = self.staging / mode
            parent.mkdir()
            with self.choice("reports/receipt.txt", output="receipt.txt", staging=parent) as plan:
                @contextmanager
                def synthetic_services(**_):
                    class Services:
                        def check_proposed(inner, *args): pass
                        def save_private(inner, name, path, source):
                            p = Path(path)
                            if mode == "block": raise Rejected("policy refused synthetic attachment")
                            if mode == "rewrite":
                                with p.open("r+b") as f: f.write(b"X")
                            elif mode == "changed-origin":
                                with open(path+":Zone.Identifier", "wb") as f: f.write(RESTRICTED)
                            else:
                                p.rename(parent / "held-aside.txt")
                                if mode == "same-bytes-replacement":
                                    with p.open("xb") as f:
                                        with open(path+":Zone.Identifier", "xb") as mark: mark.write(MARK)
                                        f.write(RECEIPT)
                    yield Services()
                with patch("zipworkspace._windows_member.attachment_services", synthetic_services), \
                        patch.object(plan.dst.api, "rename_new") as rename:
                    with self.assertRaises((Rejected, OSError)):
                        plan.assess_private_attachment(allow_configured_windows_services=True)
                    with self.assertRaises(Rejected): plan.publish()
                    rename.assert_not_called()
                self.assertEqual(list(self.dst.iterdir()), [])
                if mode == "quarantine": self.assertFalse((parent/"receipt.txt").exists())

    @unittest.skipUnless(os.environ.get("ZIPWORKSPACE_WINDOWS_ATTACHMENT_CONSENT") == ALLOW_SERVICES,
                         "requires separate agreement to OS security services handling synthetic contents and local paths")
    def test_explicit_real_windows_attachment_services_on_synthetic_private_document_only(self):
        # These services may legitimately reject/quarantine this sample or contact remote security
        # providers. No execution, no settings changes, no publication and no retry on their decision.
        with self.choice("reports/receipt.txt", output="receipt.txt") as plan:
            try:
                result = plan.assess_private_attachment(allow_configured_windows_services=True)
            finally:
                self.assertFalse((self.dst/"receipt.txt").exists())
            self.assertFalse(result["published"])
            self.assertEqual((self.staging/"receipt.txt").read_bytes(), RECEIPT)

    def test_ambiguous_component_never_activates_attachment_services_or_creates_stage(self):
        with self.choice("reports/receipt .txt", output="receipt .txt") as plan, \
                patch("zipworkspace._windows_member.attachment_services") as activate, \
                patch.object(plan.dst.api, "new_file") as create:
            with self.assertRaisesRegex(Rejected, "ambiguous"):
                plan.assess_private_attachment(allow_configured_windows_services=True)
            activate.assert_not_called()
            create.assert_not_called()

    def test_equal_length_change_on_writer_close_is_not_submitted_to_save(self):
        with self.choice("reports/receipt.txt", output="receipt.txt") as plan:
            path, open_original = self.staging / "receipt.txt", plan.dst.api.inspected_file
            def substitute_content(parent, name):
                self.assertEqual(name, "receipt.txt")
                # The real primary writer has closed. Simulate a local filter that changes
                # one byte without changing the actual inode, length, origin or permissions.
                with path.open("r+b") as f: f.write(b"X")
                return open_original(parent, name)
            with patch("zipworkspace._windows_member.attachment_services") as service, \
                    patch.object(plan.dst.api, "inspected_file", side_effect=substitute_content), \
                    patch.object(plan.dst.api, "rename_new") as rename:
                with self.assertRaisesRegex(Rejected, "member bytes"):
                    plan.assess_private_attachment(allow_configured_windows_services=True)
                system = service.return_value.__enter__.return_value
                system.check_proposed.assert_called_once()
                system.save_private.assert_not_called()
                rename.assert_not_called()
            self.assertEqual(list(self.dst.iterdir()), [])
