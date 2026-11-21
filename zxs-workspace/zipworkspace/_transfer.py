"""Cancelable verification private to a single, not-yet-published transfer."""
from threading import Event, Lock

from .model import Rejected


class CancelledCopy(Rejected):
    """This copy stopped before making any public name."""


class _CheckedReads:
    def __init__(self, raw, check, on_read=None): self.raw, self.check, self.on_read = raw, check, on_read

    def __getattr__(self, name): return getattr(self.raw, name)

    def read(self, *args):
        self.check()
        data = self.raw.read(*args)
        self.check()
        if self.on_read is not None: self.on_read(len(data))
        return data


class _BeforePublication:
    _cancel_note = "已取消这次目录复制，未发布新的正式名称。"

    def _init_cancellation(self):
        self.cancelled, self._publication_lock, self.publishing = Event(), Lock(), False
        self._progress = ("", 0, None)

    def _phase(self, name, total=None):
        with self._publication_lock: self._progress = (name, 0, total)

    def _bytes_processed(self, amount):
        with self._publication_lock:
            label, done, total = self._progress
            self._progress = label, done + amount, total

    def progress(self):
        with self._publication_lock: return self._progress

    def cancel(self):
        """Return true only if cancellation won before the unique native publication."""
        with self._publication_lock:
            if self.closed or self.publishing: return False
            self.cancelled.set()
            return True

    def _checkpoint(self):
        if self.cancelled.is_set(): raise CancelledCopy(self._cancel_note)

    def _begin_publication(self):
        # No lock spans system I/O. This is the irrevocable commitment to
        # perform the one allowed native publication, not its success.
        with self._publication_lock:
            self._checkpoint()
            self.publishing = True
