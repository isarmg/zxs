/* Private in-process Windows directory mount. Dedicated owning worker, trusted SDK/DLL 2.1,
 * ordinary local logon, one explicit R3 working file; no HTTP, drive mapping or service installation. */
#include "draft_io.h"
#include "disk_identity.h"
#include <stdint.h>
#include <wchar.h>

typedef struct {
    FSP_FILE_SYSTEM_INTERFACE ops; /* never shorter-lived than the original SDK volume */
    volatile LONG stopped;
} DRAFT_HOST;

static VOID stopped(FSP_FILE_SYSTEM *fs, BOOLEAN normal)
{
    DRAFT_HOST *host = CONTAINING_RECORD(fs->Interface, DRAFT_HOST, ops);
    InterlockedExchange(&host->stopped, normal ? 1 : 2);
}

static int current_choice(PyObject *choice)
{
    PyObject *name = PyUnicode_FromString("zipworkspace.desktop.container_windows_mount");
    PyObject *m = name ? PyImport_GetModule(name) : NULL;
    Py_XDECREF(name);
    PyObject *t = m ? PyObject_GetAttrString(m, "WindowsPrivateDraftMount") : NULL;
    Py_XDECREF(m);
    int exact = t && PyType_Check(t) && Py_TYPE(choice) == (PyTypeObject *)t;
    Py_XDECREF(t); return exact;
}

static int mode(PyObject *choice)
{
    PyObject *r = PyObject_CallMethod(choice, "_poll", NULL);
    long n = r && PyLong_CheckExact(r) ? PyLong_AsLong(r) : -1;
    Py_XDECREF(r); return n >= 0 && n <= 2 ? (int)n : -1;
}

static int confirm_gone(PyObject *choice)
{
    PyObject *r = PyObject_CallMethod(choice, "_gone", NULL);
    int ok = r == Py_None; Py_XDECREF(r); return ok;
}

static NTSTATUS link_id(FSP_FILE_SYSTEM *fs, const WCHAR *path, FILE_ID_INFO *original, BOOL first)
{
    FILE_ID_INFO id;
    FILE_ATTRIBUTE_TAG_INFO tag;
    static const BYTE zero[16] = {0};
    WCHAR name[32768];
    if (!fs->MountHandle || !fs->MountPoint || wcscmp(path, fs->MountPoint)) return STATUS_DATA_ERROR;
    DWORD n = GetFinalPathNameByHandleW(fs->MountHandle, name, 32768, VOLUME_NAME_GUID);
    if (!n || n >= 32768 || wcscmp(name, path)) return STATUS_DATA_ERROR;
    if (!GetFileInformationByHandleEx(fs->MountHandle, FileIdInfo, &id, sizeof id) ||
        !GetFileInformationByHandleEx(fs->MountHandle, FileAttributeTagInfo, &tag, sizeof tag))
        return FspNtStatusFromWin32(GetLastError());
    if (!id.VolumeSerialNumber || !memcmp(id.FileId.Identifier, zero, 16) ||
        (tag.FileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY)) !=
            (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) ||
        tag.ReparseTag != IO_REPARSE_TAG_MOUNT_POINT ||
        (!first && (id.VolumeSerialNumber != original->VolumeSerialNumber ||
                    memcmp(id.FileId.Identifier, original->FileId.Identifier, 16))))
        return STATUS_DATA_ERROR;
    if (first) *original = id;
    return STATUS_SUCCESS;
}

static int verify_pair(PyObject *choice, PyObject *plane, FSP_FILE_SYSTEM *fs,
        WCHAR disk[NAME_COUNT], const WCHAR *path, FILE_ID_INFO *id, BOOL first, NTSTATUS *status)
{
    Py_BEGIN_ALLOW_THREADS
    *status = identity(fs, disk, FALSE);
    if (*status == STATUS_SUCCESS) *status = link_id(fs, path, id, first);
    Py_END_ALLOW_THREADS
    if (*status != STATUS_SUCCESS) return 0;
    PyObject *name = PyUnicode_FromWideChar(disk, NAME_COUNT - 1);
    PyObject *fid = PyBytes_FromStringAndSize((const char *)id->FileId.Identifier, 16);
    PyObject *r = name && fid ? PyObject_CallMethod(choice, "_verify", "OOKO", plane, name,
                                                    (unsigned long long)id->VolumeSerialNumber, fid) : NULL;
    int ok = r == Py_None;
    Py_XDECREF(r); Py_XDECREF(fid); Py_XDECREF(name);
    if (!ok) *status = STATUS_DATA_ERROR;
    return ok;
}

/* Caller cannot pass any NT handles, arbitrary filesystem paths, or a replacement transport. */
static PyObject *run(PyObject *module, PyObject *args)
{
    (void)module;
    PyObject *choice, *plane, *service, *directory;
    if (!PyArg_ParseTuple(args, "OOOO", &choice, &plane, &service, &directory)) return NULL;
    if (service != Py_True || directory != Py_True || !current_choice(choice))
        return PyErr_Format(PyExc_PermissionError, "confirm this original Windows directory mount and installed service");
    DRAFT_HOST host = {0}; FSP_FILE_SYSTEM keeper = {0}, *fs = NULL; FSP_FSCTL_VOLUME_PARAMS vp;
    WCHAR disk[NAME_COUNT] = {0}, *path = NULL;
    FILE_ID_INFO mount = {0};
    Py_ssize_t units = 0;
    PyObject *pair = NULL, *sd = NULL;
    NTSTATUS status = STATUS_SUCCESS, dispatcher_status = STATUS_UNSUCCESSFUL;
    BOOL created = FALSE, moved = FALSE, started = FALSE, linked = FALSE, shown = FALSE;
    BOOL normal = FALSE, absent = FALSE, joined = FALSE, accepted = FALSE, release_pending = FALSE;
    const char *reason = "cancelled-before-create";
    pair = PyObject_CallMethod(choice, "_claim", "O", plane);
    if (!pair) return NULL;
    if (pair == Py_None) goto end;
    if (!PyTuple_CheckExact(pair) || PyTuple_GET_SIZE(pair) != 2 ||
        !PyUnicode_CheckExact(PyTuple_GET_ITEM(pair, 0)) || !PyBytes_CheckExact(PyTuple_GET_ITEM(pair, 1)))
        { reason = "invalid-original-mount-choice"; status = STATUS_INVALID_PARAMETER; goto end; }
    path = PyUnicode_AsWideCharString(PyTuple_GET_ITEM(pair, 0), &units);
    sd = PyTuple_GET_ITEM(pair, 1);
    if (!path || units < 80 || units >= 32768 || wcslen(path) != (size_t)units ||
        wcsncmp(path, L"\\\\?\\Volume{", 11) || PyBytes_GET_SIZE(sd) < SECURITY_DESCRIPTOR_MIN_LENGTH ||
        PyBytes_GET_SIZE(sd) > 4096 || !IsValidSecurityDescriptor(PyBytes_AS_STRING(sd)) ||
        GetSecurityDescriptorLength(PyBytes_AS_STRING(sd)) != (DWORD)PyBytes_GET_SIZE(sd))
        { reason = "invalid-original-path-or-acl"; status = STATUS_INVALID_PARAMETER; goto end; }
    /* Preclaim the original before Create can start a service, even before any kernel volume exists. */
    ZwDraftNamespaceInterface(&vp, &host.ops); host.ops.DispatcherStopped = stopped;
    keeper.Interface = &host.ops; keeper.UmFileContextIsUserContext2 = TRUE;
    if (!ZwDraftBindNamespace(&keeper, plane)) { reason = "original-claim-refused"; status = STATUS_ACCESS_DENIED; goto end; }
    if (!ZwDraftRememberLease(&keeper, choice)) {
        reason = "cannot-retain-original-release-lease"; status = STATUS_INSUFFICIENT_RESOURCES; goto end;
    }
    UINT32 version = 0; PWSTR sxs;
    Py_BEGIN_ALLOW_THREADS
    sxs = FspSxsIdent(); status = FspVersion(&version);
    Py_END_ALLOW_THREADS
    if (status != STATUS_SUCCESS || version != ((2u << 16) | 1u) || !sxs || !sxs[0]) {
        reason = "exact-sxs-sdk-required"; status = STATUS_NOT_SUPPORTED; goto end;
    }
    if (mode(choice) != 0) goto end;
    reason = "create-failed";
    Py_BEGIN_ALLOW_THREADS
    status = FspFileSystemCreate(L"" FSP_FSCTL_DISK_DEVICE_NAME, &vp, &host.ops, &fs);
    Py_END_ALLOW_THREADS
    if (status != STATUS_SUCCESS) goto end;
    created = TRUE;
    if (!ZwDraftMoveBinding(&keeper, fs, TRUE)) { reason = "original-bind-refused"; status = STATUS_ACCESS_DENIED; goto end; }
    moved = TRUE;
    Py_BEGIN_ALLOW_THREADS
    status = identity(fs, disk, TRUE);
    Py_END_ALLOW_THREADS
    if (status != STATUS_SUCCESS) { reason = "original-volume-unconfirmed"; goto end; }
    if (mode(choice) != 0) { reason = "cancelled-before-mount"; goto end; }
    Py_BEGIN_ALLOW_THREADS
    status = FspFileSystemStartDispatcher(fs, 2);
    Py_END_ALLOW_THREADS
    if (status != STATUS_SUCCESS) { reason = "dispatcher-start-failed"; goto end; }
    started = TRUE;
    /* Real dispatcher is alive with all requests denied until the original link has been checked. */
    if (mode(choice) != 0) { reason = "cancelled-before-mount"; goto end; }
    PSECURITY_DESCRIPTOR private_sd = PyBytes_AS_STRING(sd);
    Py_BEGIN_ALLOW_THREADS
    status = FspFileSystemSetMountPointEx(fs, path, private_sd);
    Py_END_ALLOW_THREADS
    if (status != STATUS_SUCCESS) { reason = "fresh-directory-mount-failed"; goto end; }
    linked = TRUE;
    if (!verify_pair(choice, plane, fs, disk, path, &mount, TRUE, &status)) { reason = "mount-target-unconfirmed"; goto end; }
    if (mode(choice) != 0) { reason = "cancelled-before-presentation"; goto end; }
    if (!ZwDraftPresent(fs)) { reason = "dispatcher-presentation-refused"; status = STATUS_DEVICE_NOT_READY; goto end; }
    PyObject *advertised = PyObject_CallMethod(choice, "_advertise", NULL);
    shown = advertised == Py_True; Py_XDECREF(advertised);
    if (!shown) { reason = "presentation-abandoned"; goto end; }
    ULONGLONG check_at = GetTickCount64() + 2000;
    for (;;) {
        DWORD wait;
        Py_BEGIN_ALLOW_THREADS
        wait = WaitForSingleObject(fs->DispatcherThread, 100);
        Py_END_ALLOW_THREADS
        if (wait != WAIT_TIMEOUT || InterlockedCompareExchange(&host.stopped, 0, 0)) {
            reason = "dispatcher-lost"; status = STATUS_DEVICE_NOT_READY; break;
        }
        int selected = mode(choice);
        if (selected == 0) {
            if (GetTickCount64() >= check_at) {
                if (!verify_pair(choice, plane, fs, disk, path, &mount, FALSE, &status)) {
                    reason = "original-mount-changed"; break;
                }
                check_at = GetTickCount64() + 2000;
            }
            continue;
        }
        if (selected != 1) { reason = "forced-or-controller-lost"; status = STATUS_CANCELLED; break; }
        ULONG remaining = 0;
        int fenced = ZwDraftFenceIfIdle(fs, &remaining);
        PyObject *r = fenced >= 0 ? PyObject_CallMethod(choice, "_count", "k", (unsigned long)remaining) : NULL;
        int observed = r == Py_None; Py_XDECREF(r);
        if (!observed) { reason = "callback-drain-unconfirmed"; status = STATUS_DATA_ERROR; break; }
        if (!fenced) continue; /* Existing edits/Close continue; never force after a timer. */
        if (!verify_pair(choice, plane, fs, disk, path, &mount, FALSE, &status)) { reason = "original-target-lost"; break; }
        normal = TRUE; reason = "original-drain-observed"; break;
    }
end:
    PyErr_Clear(); /* Native/authority failures below are reported as dispositions, never credentials or paths. */
    if (fs) {
        if (linked) {
            Py_BEGIN_ALLOW_THREADS
            FspFileSystemRemoveMountPoint(fs); /* Directory branch closes only the SDK's own DELETE_ON_CLOSE handle. */
            Py_END_ALLOW_THREADS
            absent = !fs->MountPoint && !fs->MountHandle && confirm_gone(choice);
            if (!absent) { normal = FALSE; reason = "mount-removal-unconfirmed"; status = STATUS_DATA_ERROR; }
            PyErr_Clear();
        }
        if (started) {
            DWORD live = WaitForSingleObject(fs->DispatcherThread, 0);
            if (live != WAIT_TIMEOUT || InterlockedCompareExchange(&host.stopped, 0, 0)) normal = FALSE;
            NTSTATUS same = STATUS_DATA_ERROR;
            Py_BEGIN_ALLOW_THREADS
            FspFileSystemStopDispatcher(fs); /* Owner only, no GIL; joins all dispatch threads. */
            FspFileSystemGetDispatcherResult(fs, &dispatcher_status);
            if (disk[0]) same = identity(fs, disk, FALSE);
            Py_END_ALLOW_THREADS
            joined = TRUE;
            if (same != STATUS_SUCCESS || host.stopped != 1 ||
                (dispatcher_status != STATUS_SUCCESS && dispatcher_status != STATUS_CANCELLED)) normal = FALSE;
        }
        if (moved && !ZwDraftMoveBinding(fs, &keeper, FALSE)) {
            normal = FALSE; reason = "ownership-transfer-unconfirmed";
            if (!ZwDraftAbandonFilePlane(fs)) { release_pending = TRUE; ZwDraftDropAfterJoin(fs); }
            PyErr_Clear();
        }
        Py_BEGIN_ALLOW_THREADS
        FspFileSystemDelete(fs);
        Py_END_ALLOW_THREADS
    }
    if (normal && linked && shown && joined && absent && confirm_gone(choice)) {
        PyObject *permit = PyObject_CallMethod(choice, "_final_choice", NULL);
        BOOL approved = permit == Py_True; Py_XDECREF(permit);
        if (approved) accepted = ZwDraftAcceptDeleted(&keeper); /* Only after deletion, absence and final choice. */
    }
    if (keeper.UserContext && !ZwDraftAbandonFilePlane(&keeper)) {
        /* Retain the *exact* per-binding token for retry, never use the plane's current lease.
         * Even an interrupted losing bind must not retire another owner's draft. */
        PyErr_Clear();
        ZwDraftRememberLease(&keeper, choice);
        release_pending = TRUE; ZwDraftDropAfterJoin(&keeper);
    }
    PyErr_Clear();
    if (accepted) { reason = "original-detached"; status = STATUS_SUCCESS; }
    else if (!strcmp(reason, "original-drain-observed")) { reason = "detach-unconfirmed"; status = STATUS_DATA_ERROR; }
    Py_XDECREF(pair); PyMem_Free(path);
    return Py_BuildValue("{s:s,s:k,s:i,s:i,s:i,s:i,s:i,s:i,s:i,s:k}",
            "outcome", reason, "status", (unsigned long)(UINT32)status, "created", (int)created,
            "started", (int)started, "linked", (int)linked, "presented", (int)shown, "absent", (int)absent,
            "drained", (int)accepted, "local_release_pending", (int)release_pending, "dispatcher_status", (unsigned long)(UINT32)dispatcher_status);
}

static PyObject *consume(PyObject *module, PyObject *args)
{
    PyObject *proof, *plane, *lease;
    (void)module;
    if (!PyArg_ParseTuple(args, "OOO", &proof, &plane, &lease)) return NULL;
    return PyBool_FromLong(ZwDraftConsumeCertificate(proof, plane, lease));
}

static PyMethodDef methods[] = {
    {"run", run, METH_VARARGS, "Run one explicitly selected private directory-mount owner."},
    {"consume_certificate", consume, METH_VARARGS, "Check one short-lived original detach certificate."}, {NULL}
};
static struct PyModuleDef module = {PyModuleDef_HEAD_INIT, "_workspace_winfsp_draft_host", NULL, -1, methods};
PyMODINIT_FUNC PyInit__workspace_winfsp_draft_host(void) { return PyModule_Create(&module); }
