"""Internal Windows Attachment Services bridge for an explicitly authorized synthetic trial.

A proposed name gets CheckPolicy only; a *separate* object may Save the existing private
file. Neither result is a publication/launch capability. No COM activation on import.
"""
from contextlib import contextmanager
import ctypes as c
import os
from uuid import UUID

from .model import Rejected
from .windows_nt import I32, U32, P, _function


_GUID = U32 * 4
_CLASS = _GUID.from_buffer_copy(UUID("4125dd96-e03a-4103-8f70-e0597d803b9c").bytes_le)
_INTERFACE = _GUID.from_buffer_copy(UUID("73db1241-1e85-4581-8e4f-a81e1d0f8c57").bytes_le)
# Fixed local in-process class, no code download/custom marshalling/activate-as-activator.
_CONTEXT = 0x1 | 0x400 | 0x1000 | 0x8000


def _ok(method, result):
    if result == 0: return
    if method == "CheckPolicy" and result == 1:
        raise Rejected("Windows attachment policy requires a prompt; nothing is authorized")
    raise Rejected(f"Windows attachment service declined {method}: HRESULT {result & 0xffffffff:08x}")


class _Attachment:
    """Only these IAttachmentExecute slots are callable here; no Prompt or Execute wrapper."""
    def __init__(self, ptr):
        self.ptr = ptr
        # IUnknown's three slots precede IAttachmentExecute; stdcall on 32-bit Windows.
        self.vtable = c.cast(ptr, c.POINTER(c.POINTER(P))).contents

    def _method(self, slot, result, *argtypes):
        address = self.vtable[slot]
        if not address: raise Rejected("Windows attachment method unavailable")
        return c.WINFUNCTYPE(result, P, *argtypes)(address)

    def check(self, name, path, referring_archive):
        for slot, label, value in ((6, "SetFileName", name), (5, "SetLocalPath", path),
                                    (8, "SetReferrer", referring_archive)):
            _ok(label, self._method(slot, I32, c.c_wchar_p)(self.ptr, value))
        _ok("CheckPolicy", self._method(9, I32)(self.ptr))

    def save(self): _ok("Save", self._method(11, I32)(self.ptr))
    def close(self): self._method(2, U32)(self.ptr)  # IUnknown::Release, not an HRESULT.


@contextmanager
def _object(create):
    ptr = P()
    _ok("CoCreateInstance", create(c.byref(_CLASS), None, _CONTEXT, c.byref(_INTERFACE), c.byref(ptr)))
    if not ptr.value: raise Rejected("no Windows attachment object was returned")
    item = _Attachment(ptr)
    try: yield item
    finally: item.close()


class _Services:
    def __init__(self, create): self.create = create

    def check_proposed(self, name, proposed, referring_archive):
        # No final file exists; this method cannot ask the service to examine/persist it.
        with _object(self.create) as item: item.check(name, proposed, referring_archive)

    def save_private(self, name, actual, referring_archive):
        with _object(self.create) as item:
            item.check(name, actual, referring_archive)
            item.save()  # May change/quarantine/delete the private file, including on failure.


@contextmanager
def attachment_services(*, allow_configured_windows_services=False):
    if allow_configured_windows_services is not True:
        raise Rejected("explicit consent to configured Windows services is required, including possible remote processing")
    if os.name != "nt": raise Rejected("native Windows Attachment Services unavailable")
    ole = c.WinDLL("ole32.dll", use_last_error=True, winmode=0x800)
    init = _function(ole, "CoInitializeEx", I32, P, U32)
    uninit = _function(ole, "CoUninitialize", None)
    create = _function(ole, "CoCreateInstance", I32, P, P, U32, P, c.POINTER(P))
    rc = init(None, 0x2 | 0x4)  # COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE
    if rc not in (0, 1): _ok("CoInitializeEx", rc)
    try: yield _Services(create)
    finally: uninit()  # S_FALSE also added one apartment initialization on this thread.
