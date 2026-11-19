"""No default driver access. Real Create below additionally requires explicit synthetic-host consent."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
import io
import os
from pathlib import Path
from threading import Event
import unittest
from unittest.mock import Mock, patch

from zipworkspace._winfsp_session import _Attempt, Observation, selected_qualification
from zipworkspace.model import Rejected

CONSENT = "ALLOW_ONE_SEALED_SYSTEM_VOLUME_AND_POSSIBLE_INSTALLED_WINFSP_SERVICE_START"


class Decisions(unittest.TestCase):
    def choice(self):
        source = Mock()
        actor = Mock()
        actor.caller.return_value = b"original ordinary test principal"
        choice = _Attempt(source, actor, actor.caller())
        choice._stack = ExitStack()
        closed = Mock()
        choice._stack.callback(closed)
        self.addCleanup(choice._close_scope)
        return choice, source, actor, closed

    def test_no_consent_and_cancel_before_native_import_are_terminal(self):
        choice, source, _, closed = self.choice()
        with self.assertRaises(Rejected): choice.run()
        source.check.assert_not_called(); closed.assert_called_once()
        with self.assertRaises(Rejected): choice.run(allow_installed_driver_start=True)
        choice, source, _, closed = self.choice()
        self.assertTrue(choice.cancel())
        result = choice.run(allow_installed_driver_start=True)
        self.assertEqual((result.outcome, result.created, result.volume), ("cancelled-before-create", 0, None))
        source.check.assert_not_called(); closed.assert_called_once()

    def test_caller_or_snapshot_replacement_cannot_claim_driver_call(self):
        c, original, actor, closed = self.choice()
        actor.caller.return_value = b"different"
        with self.assertRaises(Rejected): c.run(allow_installed_driver_start=True)
        closed.assert_called_once()
        c, original, _, _ = self.choice()
        with self.assertRaises(Rejected): c._claim(Mock())
        self.assertFalse(c._claimed)

    def test_discard_during_worker_keeps_original_until_native_drain(self):
        import zipworkspace
        c, original, _, closed = self.choice()
        active, drain = Event(), Event()
        def native(source, controller, permission):
            self.assertTrue(permission)
            self.assertTrue(controller._claim(source))
            with self.assertRaises(Rejected): controller._claim(source)
            active.set()
            if not drain.wait(10): self.fail("test worker did not receive its release")
            self.assertTrue(controller._want_stop())
            closed.assert_not_called()
            return ("stop-requested", 0, r"\Device\Volume{12345678-1234-1234-1234-123456789abc}", 1, 1, 1, 1, 0)
        with patch.object(zipworkspace, "_qualification_winfsp_session", Mock(run_once=native), create=True):
            with ThreadPoolExecutor(1) as pool:
                task = pool.submit(c.run, allow_installed_driver_start=True)
                try:
                    self.assertTrue(active.wait(10))
                    self.assertFalse(c.cancel())  # SDK ownership already passed, not 'never created'.
                    c._close_scope()              # Does not wait, close FDs or stop another volume.
                    closed.assert_not_called()
                finally: drain.set()
                self.assertEqual(task.result().outcome, "stop-requested")
        closed.assert_called_once()

    def test_normally_callback_and_original_name_do_not_hide_dispatcher_error(self):
        name = r"\Device\Volume{12345678-1234-1234-1234-123456789abc}"
        for reason in ("stop-requested", "observation-window-ended"):
            for status in (0, 0xC0000120):
                self.assertEqual(Observation.from_native((reason, 0, name, 1, 1, 1, 1, status)).outcome, reason)
            bad = Observation.from_native((reason, 0, name, 1, 1, 1, 1, 0xC0000185))
            self.assertEqual((bad.outcome, bad.dispatcher_status), ("dispatcher-stop-unconfirmed", 0xC0000185))

    def test_discard_before_worker_releases_once_and_refuses_late_start(self):
        c, _, _, closed = self.choice()
        c._close_scope(); c._close_scope()
        with self.assertRaises(Rejected): c.run(allow_installed_driver_start=True)
        closed.assert_called_once()


@unittest.skipUnless(os.name == "nt" and os.environ.get("ZIP_WORKSPACE_WINFSP_FIXTURE") and
                     os.environ.get("ZIP_WORKSPACE_WINFSP_LIFECYCLE_PERMISSION") == CONSENT,
                     "separate explicit synthetic-volume and installed-service-start permission required")
class NativeSealedLifecycle(unittest.TestCase):
    def test_create_observe_and_release_one_original_without_assigning_a_letter(self):
        # Same known contents as the already existing vendor callback fixture, never a user archive.
        from test_windows_snapshot import synthetic
        with io.BytesIO() as buf: marker = synthetic(buf)
        with selected_qualification(Path(os.environ["ZIP_WORKSPACE_WINFSP_FIXTURE"])) as choice:
            self.assertEqual(choice.node, marker)
            with ThreadPoolExecutor(1) as pool:
                job = pool.submit(choice.run, allow_installed_driver_start=True)
                try: original = choice.wait_observation(20)
                finally: choice.cancel()  # No global volume search, drive removal or forced stop.
                outcome = job.result()
        self.assertRegex(original or "", r"^\\Device\\Volume\{[0-9a-fA-F-]{36}\}$")
        self.assertIsInstance(outcome, Observation)
        self.assertEqual((outcome.outcome, outcome.status, outcome.volume, outcome.created,
                          outcome.dispatcher_started, outcome.same_before_release, outcome.stopped_callback),
                         ("stop-requested", 0, original, 1, 1, 1, 1))
        self.assertIn(outcome.dispatcher_status, (0, 0xC0000120))  # Success or expected queue cancellation only.
        # Neither accepted status proves that the OS removed every name/reference or stopped the service.
