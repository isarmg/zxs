"""Opt-in synthetic contract on a disposable ORIGINAL Windows fixed-local NTFS scratch directory.

Provide an operator-only scratch with equally protected descendants; no sync jobs, EFS or user data.
The same-volume rename does not itself re-inherit the destination ACL. Tests leave
fresh synthetic cases for their operator to inspect; they perform NO recursive/rebound-name cleanup.
Do not change privilege, sharing policy or OS settings to pass. No UI, external process, or foreign file.
"""
from dataclasses import replace
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace._windows_member import selected_export
from zipworkspace._windows_output import PinnedParent
from zipworkspace.desktop.selection import Selection
from zipworkspace.document import archive_sha, named_document
from zipworkspace.model import Commit, Manifest, Rejected, new_id
from zipworkspace.refs import Node


SCRATCH = os.environ.get("ZIPWORKSPACE_NTFS_EXPORT_SCRATCH")
PAYLOAD = b"the selected subdirectory's ordinary member\0\r\n\x1a\xff\xfe" * 137


@unittest.skipUnless(os.name == "nt" and SCRATCH, "explicit disposable Windows fixed-local NTFS scratch required")
class OriginalMemberExport(unittest.TestCase):
    def setUp(self):
        # Only an operator-owned, quiescent, disposable location appointed for this qualification.
        scratch = PinnedParent(SCRATCH, output=True)
        self.addCleanup(scratch.close)  # Keep every approved ancestor from setup to teardown.
        native = Path(tempfile.mkdtemp(prefix="zw-test-", dir=scratch.chain[-1][2]))
        self.root = Path(SCRATCH) / native.name
        case = PinnedParent(self.root, output=True)
        self.addCleanup(case.close)
        if case.chain[-1][2] != str(native): raise Rejected("the synthetic case changed scope")
        self.dst = self.root / "outer" / "destination"
        self.dst.mkdir(parents=True)
        self.staging = self.root / "staging"
        self.staging.mkdir()
        self.archive = self.root / "loose.zxs"
        files = {"first/same.bin":b"not the chosen file", "second/same.bin":PAYLOAD, "zero":b""}
        manifest = Manifest()
        for name, body in files.items():
            manifest.add(name, False, len(body), hashlib.sha256(body).hexdigest())
        marker = Commit(new_id(), new_id(), 1, new_id(), None, manifest.digest())
        with zipfile.ZipFile(self.archive, "x", compression=zipfile.ZIP_STORED) as z:
            for name, body in files.items(): z.writestr(name, body)
            z.comment = marker.encode()
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
        # Changing source evidence does not silently export an unmarked default stream.
        with open(str(self.archive) + ":Zone.Identifier", "xb") as ads:
            ads.write(b"[ZoneTransfer]\r\nZoneId=3\r\n")
        with self.assertRaises(Rejected), self.choice(selected=Selection.open(self.archive)): pass
        self.assertEqual(list(self.dst.iterdir()), [])

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
                copy(h)
                plan.dst.api.absent(plan.dst.handle, final)
            with patch.object(plan, "_copy", side_effect=actual_file): plan.publish()
        self.assertEqual((self.dst / final).read_bytes(), PAYLOAD)

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
