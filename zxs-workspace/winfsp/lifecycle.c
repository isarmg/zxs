/* Non-default qualification host for a sealed WinFsp 2.1 DISK instance.
 * Caller: one dedicated owner, conventional GIL-enabled CPython x64 main interpreter.
 * No mount point; a kernel volume is a real system object even without a drive letter. */
#include "provider.h"
#include <string.h>

typedef struct {
    FSP_FILE_SYSTEM_INTERFACE ops;  /* WinFsp retains this address until Delete. */
    volatile LONG stopped;          /* last SDK callback: 1 requested, 2 externally stopped */
} Owner;

static NTSTATUS sealed(FSP_FILE_SYSTEM *fs, FSP_FSCTL_TRANSACT_REQ *request,
                       FSP_FSCTL_TRANSACT_RSP *response)
{ return STATUS_ACCESS_DENIED; }

static VOID stopped(FSP_FILE_SYSTEM *fs, BOOLEAN normally)
{
    Owner *owner = CONTAINING_RECORD(fs->Interface, Owner, ops);
    InterlockedExchange(&owner->stopped, normally ? 1 : 2);
}

static const WCHAR shape[] = L"\\Device\\Volume{00000000-0000-0000-0000-000000000000}";
#define NAME_COUNT (sizeof shape / sizeof(WCHAR))
static BOOLEAN disk_name(const WCHAR *name)
{
    for (SIZE_T i = 0; i < NAME_COUNT; i++) {
        WCHAR ch = name[i];
        if (shape[i] == L'0') {
            if (!((ch >= L'0' && ch <= L'9') || (ch >= L'a' && ch <= L'f') ||
                  (ch >= L'A' && ch <= L'F'))) return FALSE;
        } else if (ch != shape[i]) return FALSE;
    }
    return TRUE;
}

/* The official driver returns a counted, NUL-terminated native name on the control HANDLE.
 * It requires its full output buffer, even though a disk GUID name is much shorter. */
static NTSTATUS identity(FSP_FILE_SYSTEM *fs, WCHAR original[NAME_COUNT], BOOL first)
{
    WCHAR name[FSP_FSCTL_VOLUME_NAME_SIZEMAX / sizeof(WCHAR)] = {0};
    OVERLAPPED io = {0};
    DWORD bytes = 0, error = 0;
    io.hEvent = CreateEventW(NULL, TRUE, FALSE, NULL);
    if (!io.hEvent) return FspNtStatusFromWin32(GetLastError());
    if (!DeviceIoControl(fs->VolumeHandle, FSP_FSCTL_VOLUME_NAME, NULL, 0, name, sizeof name,
                         &bytes, &io)) {
        error = GetLastError();
        if (error == ERROR_IO_PENDING)
            error = GetOverlappedResult(fs->VolumeHandle, &io, &bytes, TRUE) ? 0 : GetLastError();
    }
    CloseHandle(io.hEvent);
    if (error) return FspNtStatusFromWin32(error);
    if (bytes != sizeof shape || !disk_name(name) || memcmp(name, fs->VolumeName, sizeof shape) ||
        (!first && memcmp(name, original, sizeof shape))) return STATUS_DATA_ERROR;
    if (first) memcpy(original, name, sizeof shape);
    return STATUS_SUCCESS;
}

/* Private trusted adapter (not an entry point on the installed program). Never accepts an
 * external native pointer or mount path. 'choice' is the one owner of the selected archive's
 * live scope. Only 'run' calls this, after explicit service-start consent. */
static PyObject *run_once(PyObject *module, PyObject *args)
{
    PyObject *source, *choice, *allow;
    if (!PyArg_ParseTuple(args, "OOO", &source, &choice, &allow)) return NULL;
    if (allow != Py_True) return PyErr_Format(PyExc_PermissionError, "installed service start not approved");
    PyObject *claimed = PyObject_CallMethod(choice, "_claim", "O", source);
    if (!claimed) return NULL;
    int go = claimed == Py_True;
    Py_DECREF(claimed);

    FSP_FILE_SYSTEM *fs = NULL;
    FSP_FSCTL_VOLUME_PARAMS vp;
    Owner owner = {0};
    WCHAR name[NAME_COUNT] = {0};
    NTSTATUS status = STATUS_SUCCESS, dispatch_status = STATUS_SUCCESS;
    BOOL created = FALSE, bound = FALSE, started = FALSE, original_at_release = FALSE;
    const char *reason = "cancelled-before-create";
    if (!go) goto finish;

    /* A legacy non-SxS DLL can search for and start some other installation. Refuse it. */
    UINT32 version = 0;
    PWSTR ident;
    Py_BEGIN_ALLOW_THREADS
    ident = FspSxsIdent();
    status = FspVersion(&version);
    Py_END_ALLOW_THREADS
    if (!NT_SUCCESS(status) || version != ((2u << 16) | 1u) || !ident || !ident[0]) {
        if (NT_SUCCESS(status)) status = STATUS_NOT_SUPPORTED;
        reason = "exact-sxs-sdk-required"; goto finish;
    }
    ZwSnapshotDataInterface(&vp, &owner.ops);
    owner.ops.DispatcherStopped = stopped;
    /* Create can try to start an already-installed driver service, and the kernel may
     * immediately mount the device. Do not call it, nor vendor Preflight, before claim. */
    reason = "create-failed";
    Py_BEGIN_ALLOW_THREADS
    status = FspFileSystemCreate(L"" FSP_FSCTL_DISK_DEVICE_NAME, &vp, &owner.ops, &fs);
    Py_END_ALLOW_THREADS
    if (!NT_SUCCESS(status)) goto finish;
    created = TRUE;
    /* Even a request that would bypass SD lookup cannot reach Open/Read/metadata: the
     * v2.1 dispatcher invokes a data handler only after this per-instance guard succeeds. */
    FspFileSystemSetOperationGuard(fs, sealed, NULL);
    if (!ZwSnapshotBind(fs, source)) { reason = "bind-refused"; status = STATUS_ACCESS_DENIED; goto finish; }
    bound = TRUE;
    Py_BEGIN_ALLOW_THREADS
    status = identity(fs, name, TRUE);
    Py_END_ALLOW_THREADS
    if (!NT_SUCCESS(status)) { reason = "original-volume-unconfirmed"; goto finish; }
    PyObject *admitted = PyObject_CallMethod(choice, "_recheck", "O", source);
    if (!admitted) { PyErr_Clear(); reason = "original-or-user-lost"; status = STATUS_ACCESS_DENIED; goto finish; }
    go = admitted == Py_True;
    Py_DECREF(admitted);
    if (!go) { reason = "cancelled-after-claim"; goto finish; }

    Py_BEGIN_ALLOW_THREADS
    status = FspFileSystemStartDispatcher(fs, 2);
    Py_END_ALLOW_THREADS
    if (!NT_SUCCESS(status)) { reason = "dispatcher-start-failed"; goto finish; }
    started = TRUE;
    Py_BEGIN_ALLOW_THREADS
    status = identity(fs, name, FALSE);
    Py_END_ALLOW_THREADS
    if (!NT_SUCCESS(status)) { reason = "original-volume-unconfirmed"; goto finish; }

    PyObject *id = PyUnicode_FromWideChar(name, NAME_COUNT - 1);
    PyObject *ready = id ? PyObject_CallMethod(choice, "_observe", "OO", source, id) : NULL;
    Py_XDECREF(id);
    if (!ready) { PyErr_Clear(); reason = "original-or-user-lost"; status = STATUS_ACCESS_DENIED; goto finish; }
    Py_DECREF(ready);
    /* Only an opt-in qualification window, not a permanent daemon. 30 seconds bounds the
     * observation window, NOT how long an OS call or draining the driver may take. */
    ULONGLONG until = GetTickCount64() + 30000;
    for (;;) {
        PyObject *stop = PyObject_CallMethod(choice, "_want_stop", NULL);
        if (!stop) { PyErr_Clear(); reason = "controller-unavailable"; status = STATUS_UNSUCCESSFUL; break; }
        BOOL requested = stop == Py_True; Py_DECREF(stop);
        if (requested) { reason = "stop-requested"; break; }
        if (GetTickCount64() >= until) { reason = "observation-window-ended"; break; }
        DWORD wait, error = 0;
        Py_BEGIN_ALLOW_THREADS
        wait = WaitForSingleObject(fs->DispatcherThread, 100);
        if (wait == WAIT_FAILED) error = GetLastError();
        Py_END_ALLOW_THREADS
        if (wait == WAIT_TIMEOUT) continue;
        if (wait == WAIT_OBJECT_0) reason = "dispatcher-ended-independently";
        else { reason = "dispatcher-wait-failed"; status = error ? FspNtStatusFromWin32(error) : STATUS_UNSUCCESSFUL; }
        break;
    }

finish:
    if (fs) {
        NTSTATUS again = STATUS_UNSUCCESSFUL;
        Py_BEGIN_ALLOW_THREADS
        if (started) {
            FspFileSystemStopDispatcher(fs); /* joins; never on a callback or holding the GIL */
            FspFileSystemGetDispatcherResult(fs, &dispatch_status);
        }
        if (name[0]) again = identity(fs, name, FALSE);
        Py_END_ALLOW_THREADS
        original_at_release = name[0] && NT_SUCCESS(again);
        if (name[0] && !original_at_release) {
            if (NT_SUCCESS(status)) status = again;
            reason = "original-volume-unconfirmed";
        }
        if (bound) ZwSnapshotUnbind(fs);
        Py_BEGIN_ALLOW_THREADS
        FspFileSystemDelete(fs); /* Only our control object; no mount point was ever assigned. */
        Py_END_ALLOW_THREADS
    }
    PyObject *identifier = name[0] ? PyUnicode_FromWideChar(name, NAME_COUNT - 1) : Py_NewRef(Py_None);
    if (!identifier) return NULL;
    PyObject *result = Py_BuildValue("(skOiiilk)", reason, (unsigned long)(UINT32)status, identifier,
            (int)created, (int)started, (int)original_at_release, (long)owner.stopped,
            (unsigned long)(UINT32)dispatch_status);
    Py_DECREF(identifier);
    return result;
}

static PyMethodDef methods[] = {
    {"run_once", run_once, METH_VARARGS, "Exercise one sealed local SDK instance; never assign a drive."}, {NULL}
};
static struct PyModuleDef module = {PyModuleDef_HEAD_INIT, "_qualification_winfsp_session", NULL, -1, methods};
PyMODINIT_FUNC PyInit__qualification_winfsp_session(void) { return PyModule_Create(&module); }
