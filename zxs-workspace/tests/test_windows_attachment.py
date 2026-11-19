"""Protocol contract only: no COM activation, Shell, filesystem or network here."""
from contextlib import contextmanager
from unittest.mock import patch
import unittest

from zipworkspace import _windows_attachment as attachment
from zipworkspace.model import Rejected
from zipworkspace._windows_output import PinnedParent


class AttachmentContract(unittest.TestCase):
    def test_no_service_without_explicit_opt_in(self):
        for consent in (False, None, "yes", 1):
            # Even on non-Windows, the explicit-grant gate comes first.
            with self.assertRaisesRegex(Rejected, "explicit consent"):
                with attachment.attachment_services(allow_configured_windows_services=consent):
                    self.fail("a service was activated without exact consent")

    def test_only_private_object_may_save_and_each_role_checks_its_own_real_name(self):
        calls = []
        @contextmanager
        def object_for(_):
            role = object()
            exposed = attachment._Attachment.__new__(attachment._Attachment)
            exposed.ptr = role
            exposed._method = lambda slot, *_: lambda ptr, *args: calls.append((role, slot, args)) or 0
            try: yield exposed
            finally: exposed.close()
        with patch.object(attachment, "_object", object_for):
            service = attachment._Services(None)
            service.check_proposed("given.type", "FINAL/given.type", "SOURCE.zxs")
            service.save_private("given.type", "PRIVATE/given.type", "SOURCE.zxs")
        self.assertEqual([slot for _,slot,_ in calls], [6,5,8,9,2, 6,5,8,9,11,2])
        self.assertEqual([args for _,_,args in calls],
                         [("given.type",),("FINAL/given.type",),("SOURCE.zxs",),(),(),
                          ("given.type",),("PRIVATE/given.type",),("SOURCE.zxs",),(),(),()])
        self.assertTrue(all(role is calls[0][0] for role,_,_ in calls[:5]))
        self.assertTrue(all(role is calls[-1][0] for role,_,_ in calls[5:]))
        self.assertIsNot(calls[0][0], calls[-1][0])

    def test_prompt_or_other_nonzero_status_never_proceeds_to_save(self):
        for reply in (1, 2, -2147024891):
            called = []
            @contextmanager
            def object_for(_):
                item = attachment._Attachment.__new__(attachment._Attachment)
                item.ptr = 1
                def method(slot, *_):
                    def call(*_):
                        called.append(slot)
                        return reply if slot == 9 else 0
                    return call
                item._method = method
                try: yield item
                finally: item.close()
            with patch.object(attachment, "_object", object_for), self.assertRaises(Rejected):
                attachment._Services(None).save_private("given.txt", "PRIVATE/given.txt", "SOURCE.zxs")
            self.assertEqual(called, [6,5,8,9,2])
        with self.assertRaises(Rejected): attachment._ok("Save", 1)

    def test_ambiguous_dos_components_are_declined_before_any_native_or_shell_lookup(self):
        # Spellings only; this never attempts to open a device or create one of these names.
        for parents, leaf in ((("plain",), "CON .txt"), (("folder .v1","ok"), "plain.txt"),
                              (("NUL  .txt",), "plain.txt"), (("normal",), "receipt .data")):
            parent = PinnedParent.__new__(PinnedParent)
            parent.parts = parents
            parent.check = lambda: self.fail("ambiguous Shell spelling reached a native call")
            with self.assertRaisesRegex(Rejected, "ambiguous"):
                parent.shell_name(leaf)
