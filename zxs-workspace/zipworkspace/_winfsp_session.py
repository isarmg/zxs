"""Internal Windows qualification only: no public drive or automatic native-extension import."""
from contextlib import contextmanager, ExitStack
import os
from threading import Event, Lock
from typing import NamedTuple

from .model import Rejected
from .windows_snapshot import selected_snapshot


class Observation(NamedTuple):
    outcome: str
    status: int               # Unsigned NTSTATUS for the owner's admission/observation.
    volume: str | None        # Observed through the one original driver control handle.
    created: int              # SDK Create succeeded; the owner has since called Delete on it.
    dispatcher_started: int   # Start returned, not proof of a usable disk.
    same_before_release: int
    stopped_callback: int     # 0: not seen, 1: SDK says requested, 2: independently stopped.
    dispatcher_status: int    # Raw SDK result; an ordinary stop can cancel outstanding requests.

    @classmethod
    def from_native(cls, fields):
        record = cls(*fields)
        if record.outcome in ("stop-requested", "observation-window-ended"):
            # DispatcherStopped(Normally) alone can race an independent failure. Preserve the
            # actual dispatcher result even when our own stop and final identity read succeeded.
            if not (record.created and record.dispatcher_started and record.volume and
                    record.same_before_release and record.status == 0 and record.stopped_callback == 1 and
                    record.dispatcher_status in (0, 0xC0000120)):  # STATUS_SUCCESS or STATUS_CANCELLED
                return record._replace(outcome="dispatcher-stop-unconfirmed")
        return record


class _Attempt:
    def __init__(self, source, authority, actor):
        self._source, self._authority, self._actor = source, authority, actor
        self.node = source.view.commit
        self._lock, self._stop, self._stack = Lock(), Event(), None
        self._used = self._claimed = self._running = self._closing = False
        self._observed, self._native_name = Event(), None

    def _check(self, source):
        if source is not self._source or source.view.commit != self.node or self._authority.caller() != self._actor:
            raise Rejected("not the original selection and ordinary Windows user")
        source.check()

    def cancel(self):
        """True means cancel won before the SDK admission point. Later requests only ask to drain."""
        with self._lock:
            self._stop.set()
            return not self._claimed

    def _want_stop(self):
        return self._stop.is_set()

    def _claim(self, source):
        self._check(source)
        with self._lock:
            if not self._running or self._claimed: raise Rejected("not an active single-use decision")
            if self._closing or self._stop.is_set(): return False
            self._claimed = True
            return True

    def _recheck(self, source):
        self._check(source)
        return not self._stop.is_set()

    def _observe(self, source, name):
        self._check(source)
        with self._lock:
            if not self._claimed: raise Rejected("no original SDK instance")
            if not self._stop.is_set():
                self._native_name = name
                self._observed.set()

    def wait_observation(self, timeout):
        """A past observation, not a drive name or permission to reopen it."""
        if not self._observed.wait(timeout): return None
        with self._lock: return self._native_name

    def run(self, *, allow_installed_driver_start=False):
        """On a dedicated owner thread. Synchronous teardown can wait for the OS."""
        with self._lock:
            if self._used or self._closing: raise Rejected("the original decision has ended")
            self._used = self._running = True
            original, self._stack = self._stack, None  # Worker alone can release the selected handles now.
        try:
            if allow_installed_driver_start is not True:
                raise Rejected("explicit consent to a native system volume and possible installed service start is required")
            if self._stop.is_set():
                return Observation("cancelled-before-create", 0, None, 0, 0, 0, 0, 0)
            self._check(self._source)
            # Only a privately provisioned, independently trusted qualification environment
            # supplies this extension + matching vendor DLL. No loader, package install or fallback.
            from . import _qualification_winfsp_session
            return Observation.from_native(_qualification_winfsp_session.run_once(self._source, self, True))
        finally:
            try: original.close()  # Native owner already drained and returned (also on failure).
            finally:
                with self._lock: self._running = False; self._source = None

    def _close_scope(self):
        with self._lock:
            self._closing = True
            self._stop.set()
            unclaimed, self._stack = self._stack, None
            if unclaimed is not None: self._source = None
        if unclaimed is not None: unclaimed.close()  # An active worker still owns its own stack.


@contextmanager
def selected_qualification(path, *, expected=None):
    """Own one explicit selection and normal-medium user; never look a source up on restart."""
    if os.name != "nt": raise Rejected("this qualification requires Windows")
    from ._windows_access import NativeSecurity
    authority = NativeSecurity()
    actor = authority.caller()
    with ExitStack() as incoming:
        source = incoming.enter_context(selected_snapshot(path, expected=expected))
        choice = _Attempt(source, authority, actor)
        choice._check(source)
        choice._stack = incoming.pop_all()
        try: yield choice
        finally: choice._close_scope()
