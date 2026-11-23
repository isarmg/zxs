"""Selection regressions using only recorded namespace and birth evidence."""
import sys
from types import SimpleNamespace
import unittest


@unittest.skipUnless(sys.platform == "linux", "Linux submission selection")
class LinuxSubmitSelection(unittest.TestCase):
    def test_ignored_original_name_cannot_bind_to_a_new_inode(self):
        from zipworkspace.container_plan import Baseline, FullBody, IgnoredOriginalNameConflict
        from zipworkspace.linux_edit import NamespaceEvent
        from zipworkspace.linux_submit import OriginalBinding, _select
        from zipworkspace.linux_upload_ignore import UploadIgnore

        original_id, new_id, head = "a" * 32, "b" * 32, "1" * 32
        name, ignored_name = "report.txt", "report.txt.tmp"
        original = OriginalBinding((1, 2), name, Baseline(
            original_id, name, 8, FullBody(7, "c" * 64), selected_node=head, head=head))
        renamed = NamespaceEvent("rename", original.local_identity, name, ignored_name)
        unlinked = NamespaceEvent("unlink", original.local_identity, ignored_name, None)
        new_file = SimpleNamespace(local_identity=(1, 3), path=name)
        proposals = ((new_file.local_identity, new_id),)
        cut = SimpleNamespace(upload_ignore=UploadIgnore(),
                              changed_files=(new_file,), created_files=(new_file,))

        for events in ((renamed,), (renamed, unlinked)):
            with self.subTest(backup_unlinked=len(events) == 2):
                cut.namespace_events = events
                with self.assertRaises(IgnoredOriginalNameConflict) as raised:
                    _select(cut, (original,), proposals, admit_created=False)
                self.assertEqual((name, ignored_name), (raised.exception.path, raised.exception.ignored_path))

                new_file.path = "different.txt"
                chosen, current, deleted, forced, kept = _select(cut, (original,), proposals, admit_created=False)
                self.assertEqual({new_id: new_file}, chosen)
                self.assertEqual({original_id: (name, original.baseline), new_id: ("different.txt", None)}, current)
                self.assertEqual(((), (), frozenset({original_id})), (deleted, forced, kept))
                new_file.path = name
