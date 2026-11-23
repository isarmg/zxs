/* One selected working file and optional local scratch through WinFsp 2.1. No archive, mount or R3 send. */
#include "draft_io.h"
#include "logon_policy.h"
#include <stdint.h>
#include <limits.h>
#include <stddef.h>
#include <string.h>
#include <wchar.h>

#if !defined(_WIN64) || defined(Py_GIL_DISABLED)
#error This private bridge requires GIL-enabled x64 CPython on Windows
#endif

#define SPAN (1u << 20)
#define SLOT(fc) ((unsigned long long)(UINT_PTR)(fc))

static NTSTATUS open_file(FSP_FILE_SYSTEM *, PWSTR, UINT32, UINT32, PVOID *, FSP_FSCTL_FILE_INFO *);
typedef struct {
    PyObject *plane, *lease;
    SessionPolicy policy;
    AlignedBuffer zone, scratch, add_file;
    DWORD zone_bytes, scratch_bytes, add_file_bytes;
    BOOL ready, scoped, replacement, hidden, fenced, late, real;
    SRWLOCK gate; LONG active;
    DWORD cert_thread; BOOL cert_unused;
} DRAFT_BINDING;

static DRAFT_BINDING *binding(FSP_FILE_SYSTEM *fs)
{
    if (!fs || !fs->Interface || fs->Interface->Open != open_file) return NULL;
    return (DRAFT_BINDING *)fs->UserContext;
}

static PyObject *owner(FSP_FILE_SYSTEM *fs)
{
    DRAFT_BINDING *b = binding(fs);
    return b && b->ready ? b->plane : NULL; /* Valid until the owner joins all native calls. */
}

static NTSTATUS failure(void)
{
    NTSTATUS s = STATUS_IO_DEVICE_ERROR;
    if (PyErr_ExceptionMatches(PyExc_FileNotFoundError)) s = STATUS_OBJECT_NAME_NOT_FOUND;
    else if (PyErr_ExceptionMatches(PyExc_FileExistsError)) s = STATUS_OBJECT_NAME_COLLISION;
    else if (PyErr_ExceptionMatches(PyExc_BlockingIOError)) s = STATUS_SHARING_VIOLATION;
    else if (PyErr_ExceptionMatches(PyExc_NotADirectoryError)) s = STATUS_NOT_A_DIRECTORY;
    else if (PyErr_ExceptionMatches(PyExc_IsADirectoryError)) s = STATUS_FILE_IS_A_DIRECTORY;
    else if (PyErr_ExceptionMatches(PyExc_PermissionError)) s = STATUS_ACCESS_DENIED;
    else if (PyErr_ExceptionMatches(PyExc_EOFError)) s = STATUS_END_OF_FILE;
    else if (PyErr_ExceptionMatches(PyExc_NotImplementedError)) s = STATUS_NOT_SUPPORTED;
    else if (PyErr_ExceptionMatches(PyExc_MemoryError)) s = STATUS_INSUFFICIENT_RESOURCES;
    else if (PyErr_ExceptionMatches(PyExc_ValueError)) s = STATUS_INVALID_PARAMETER;
    PyErr_Clear();
    return s;
}

static void poison(PyObject *plane)
{
    PyObject *r = PyObject_CallMethod(plane, "fault", NULL);
    Py_XDECREF(r);
    PyErr_Clear();
}

static NTSTATUS file_info(PyObject *row, FSP_FSCTL_FILE_INFO *out)
{
    if (!row) return failure();
    if (!PyTuple_CheckExact(row) || 7 != PyTuple_GET_SIZE(row)) return STATUS_DATA_ERROR;
    UINT64 fields[5];
    for (int i = 0; i < 5; i++) {
        PyObject *v = PyTuple_GET_ITEM(row, i);
        if (!PyLong_CheckExact(v)) return STATUS_DATA_ERROR;
        fields[i] = PyLong_AsUnsignedLongLong(v);
        if (PyErr_Occurred()) return failure();
        if (fields[i] > INT64_MAX || (i && !fields[i])) return STATUS_DATA_ERROR;
    }
    if (fields[0] > INT64_MAX - 4095) return STATUS_FILE_TOO_LARGE;
    memset(out, 0, sizeof *out);
    PyObject *k = PyTuple_GET_ITEM(row, 5);
    if (!PyLong_CheckExact(k)) return STATUS_DATA_ERROR;
    long kind = PyLong_AsLong(k);
    if (PyErr_Occurred()) return failure();
    PyObject *id = PyTuple_GET_ITEM(row, 6);
    if (!PyLong_CheckExact(id)) return STATUS_DATA_ERROR;
    UINT64 index = PyLong_AsUnsignedLongLong(id);
    if (PyErr_Occurred()) return failure();
    if (kind < 1 || kind > 4 || (1 == kind ? fields[0] || index != 1 : index < 2 || index > 3) ||
        (kind == 4 && index != 3)) return STATUS_DATA_ERROR;
    out->FileAttributes = 1 == kind ? FILE_ATTRIBUTE_DIRECTORY : FILE_ATTRIBUTE_ARCHIVE;
    out->IndexNumber = index; /* A named stream keeps its actual virtual backing FileIndex. */
    out->FileSize = fields[0];
    out->AllocationSize = (fields[0] + 4095) & ~(UINT64)4095;
    out->CreationTime = fields[1]; out->LastAccessTime = fields[2];
    out->LastWriteTime = fields[3]; out->ChangeTime = fields[4];
    return STATUS_SUCCESS;
}

static const FSP_FSCTL_TRANSACT_REQ *real_request(FSP_FILE_SYSTEM *fs)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b || !b->real || !b->ready || !b->replacement) return NULL;
    FSP_FILE_SYSTEM_OPERATION_CONTEXT *op = FspFileSystemGetOperationContext();
    return op ? op->Request : NULL;
}

static NTSTATUS finish_open(FSP_FILE_SYSTEM *fs, PWSTR name, UINT32 opts, UINT32 granted,
                            BOOL create, BOOL target, BOOL probe, PVOID *ctx, FSP_FSCTL_FILE_INFO *info)
{
    *ctx = NULL;
    PyObject *plane = owner(fs);
    if (!plane) return STATUS_DEVICE_NOT_READY;
    size_t length = name ? wcsnlen_s(name, 274) : 0;
    if (!length || length >= 274) return STATUS_OBJECT_NAME_INVALID;
    PyGILState_STATE gil = PyGILState_Ensure();
    PyObject *path = PyUnicode_FromWideChar(name, (Py_ssize_t)length);
    PyObject *r = !path ? NULL : create ?
        PyObject_CallMethod(plane, "create_scratch", "Okk", path, (unsigned long)opts, (unsigned long)granted) :
        PyObject_CallMethod(plane, "open_file", "OkkOO", path, (unsigned long)opts, (unsigned long)granted,
                            target ? Py_True : Py_False, probe ? Py_True : Py_False);
    Py_XDECREF(path);
    NTSTATUS s = STATUS_SUCCESS;
    if (!r) s = failure();
    else if (!PyTuple_CheckExact(r) || PyTuple_GET_SIZE(r) != 2 ||
             !PyLong_CheckExact(PyTuple_GET_ITEM(r, 0))) { s = STATUS_DATA_ERROR; poison(plane); }
    else {
        UINT64 id = PyLong_AsUnsignedLongLong(PyTuple_GET_ITEM(r, 0));
        if (PyErr_Occurred()) s = failure();
        else if (!id || id > INT64_MAX) s = STATUS_DATA_ERROR;
        else s = file_info(PyTuple_GET_ITEM(r, 1), info);
        if (s == STATUS_SUCCESS) *ctx = (PVOID)(UINT_PTR)id;
        else {
            if (id && id <= INT64_MAX) {
                PyObject *closed = PyObject_CallMethod(plane, "close_file", "K", (unsigned long long)id);
                Py_XDECREF(closed); PyErr_Clear();
            }
            poison(plane);
        }
    }
    Py_XDECREF(r); PyGILState_Release(gil);
    return s;
}

static NTSTATUS open_file(FSP_FILE_SYSTEM *fs, PWSTR name, UINT32 opts, UINT32 granted,
                          PVOID *ctx, FSP_FSCTL_FILE_INFO *info)
{
    const FSP_FSCTL_TRANSACT_REQ *req = real_request(fs);
    BOOL create = req && req->Kind == FspFsctlTransactCreateKind;
    BOOL target = create && req->Req.Create.OpenTargetDirectory;
    BOOL probe = create && !target && (opts & FILE_COMPLETE_IF_OPLOCKED) &&
        FSP_FSCTL_TRANSACT_REQ_TOKEN_PID(req->Req.Create.AccessToken) == GetCurrentProcessId();
    return finish_open(fs, name, opts, granted, FALSE, target, probe, ctx, info);
}

static NTSTATUS information(FSP_FILE_SYSTEM *fs, PVOID ctx, FSP_FSCTL_FILE_INFO *info)
{
    PyObject *plane = owner(fs);
    if (!plane || !ctx) return STATUS_INVALID_HANDLE;
    PyGILState_STATE gil = PyGILState_Ensure();
    PyObject *r = PyObject_CallMethod(plane, "info", "K", SLOT(ctx));
    NTSTATUS s = file_info(r, info);
    Py_XDECREF(r); PyGILState_Release(gil); return s;
}

static NTSTATUS read_file(FSP_FILE_SYSTEM *fs, PVOID ctx, PVOID data, UINT64 offset,
                          ULONG length, PULONG transferred)
{
    *transferred = 0;
    PyObject *plane = owner(fs);
    if (!plane || !ctx) return STATUS_INVALID_HANDLE;
    if (length > SPAN || (!data && length)) return STATUS_INVALID_PARAMETER;
    PyGILState_STATE gil = PyGILState_Ensure();
    PyObject *r = PyObject_CallMethod(plane, "read", "KKk", SLOT(ctx), (unsigned long long)offset,
                                      (unsigned long)length);
    NTSTATUS s = STATUS_SUCCESS;
    if (!r) s = failure();
    else if (!PyBytes_CheckExact(r) || PyBytes_GET_SIZE(r) > length) { s = STATUS_DATA_ERROR; poison(plane); }
    else {
        ULONG count = (ULONG)PyBytes_GET_SIZE(r);
        if (count) memcpy(data, PyBytes_AS_STRING(r), count);
        *transferred = count;
    }
    Py_XDECREF(r); PyGILState_Release(gil); return s;
}

static NTSTATUS write_file(FSP_FILE_SYSTEM *fs, PVOID ctx, PVOID data, UINT64 offset, ULONG length,
                           BOOLEAN append, BOOLEAN constrained, PULONG transferred, FSP_FSCTL_FILE_INFO *info)
{
    *transferred = 0;
    PyObject *plane = owner(fs);
    if (!plane || !ctx) return STATUS_INVALID_HANDLE;
    if (length > SPAN || (!data && length) || append > 1 || constrained > 1)
        return STATUS_INVALID_PARAMETER;
    PyGILState_STATE gil = PyGILState_Ensure();
    PyObject *bytes = PyBytes_FromStringAndSize((const char *)data, length);
    PyObject *r = bytes ? PyObject_CallMethod(plane, "write", "KKOOO", SLOT(ctx), (unsigned long long)offset,
                                              bytes, append ? Py_True : Py_False,
                                              constrained ? Py_True : Py_False) : NULL;
    Py_XDECREF(bytes);
    NTSTATUS s = STATUS_SUCCESS;
    if (!r) s = failure();
    else if (!PyTuple_CheckExact(r) || PyTuple_GET_SIZE(r) != 2 ||
             !PyLong_CheckExact(PyTuple_GET_ITEM(r, 0))) { s = STATUS_DATA_ERROR; poison(plane); }
    else {
        unsigned long count = PyLong_AsUnsignedLong(PyTuple_GET_ITEM(r, 0));
        if (PyErr_Occurred()) s = failure();
        else if (count > length || (!constrained && count != length)) s = STATUS_DATA_ERROR;
        else s = file_info(PyTuple_GET_ITEM(r, 1), info);
        if (s == STATUS_SUCCESS) *transferred = count; else poison(plane);
    }
    Py_XDECREF(r); PyGILState_Release(gil); return s;
}

static NTSTATUS resize_file(FSP_FILE_SYSTEM *fs, PVOID ctx, UINT64 size, BOOLEAN allocation,
                            FSP_FSCTL_FILE_INFO *info)
{
    PyObject *plane = owner(fs);
    if (!plane || !ctx) return STATUS_INVALID_HANDLE;
    if (allocation > 1 || size > INT64_MAX) return STATUS_INVALID_PARAMETER;
    PyGILState_STATE gil = PyGILState_Ensure();
    PyObject *r = PyObject_CallMethod(plane, "resize", "KKO", SLOT(ctx), (unsigned long long)size,
                                      allocation ? Py_True : Py_False);
    NTSTATUS s = file_info(r, info);
    if (r && s != STATUS_SUCCESS) poison(plane);
    Py_XDECREF(r); PyGILState_Release(gil); return s;
}

static NTSTATUS flush_file(FSP_FILE_SYSTEM *fs, PVOID ctx, FSP_FSCTL_FILE_INFO *info)
{
    PyObject *plane = owner(fs);
    if (!plane) return STATUS_DEVICE_NOT_READY;
    if (!ctx) return STATUS_NOT_SUPPORTED; /* NULL denotes a volume flush, not this held file. */
    PyGILState_STATE gil = PyGILState_Ensure();
    PyObject *r = PyObject_CallMethod(plane, "flush", "K", SLOT(ctx));
    NTSTATUS s = file_info(r, info);
    if (r && s != STATUS_SUCCESS) poison(plane);
    Py_XDECREF(r); PyGILState_Release(gil); return s;
}

static VOID cleanup(FSP_FILE_SYSTEM *fs, PVOID ctx, PWSTR name, ULONG flags)
{
    /* Deliberately not final Close: paging/writeback may still arrive for this open. */
    (void)fs; (void)ctx; (void)name; (void)flags;
}

static VOID close_file(FSP_FILE_SYSTEM *fs, PVOID ctx)
{
    PyObject *plane = owner(fs);
    if (!plane) return;
    PyGILState_STATE gil = PyGILState_Ensure();
    PyObject *r = PyObject_CallMethod(plane, "close_file", "K", SLOT(ctx));
    if (!r) { PyErr_Clear(); poison(plane); }
    Py_XDECREF(r); PyGILState_Release(gil);
}

static NTSTATUS deny_security(FSP_FILE_SYSTEM *fs, PVOID ctx, PSECURITY_DESCRIPTOR sd, SIZE_T *size)
{ return STATUS_ACCESS_DENIED; }
static NTSTATUS deny_name(FSP_FILE_SYSTEM *fs, PWSTR name, PUINT32 attr, PSECURITY_DESCRIPTOR sd, SIZE_T *size)
{ return STATUS_ACCESS_DENIED; }
static NTSTATUS deny_create(FSP_FILE_SYSTEM *fs, PWSTR name, UINT32 opts, UINT32 granted, UINT32 attrs,
                            PSECURITY_DESCRIPTOR sd, UINT64 alloc, PVOID *ctx, FSP_FSCTL_FILE_INFO *info)
{ *ctx = NULL; return STATUS_ACCESS_DENIED; }
static NTSTATUS deny_overwrite(FSP_FILE_SYSTEM *fs, PVOID ctx, UINT32 attrs, BOOLEAN replace,
                               UINT64 alloc, FSP_FSCTL_FILE_INFO *info)
{ return STATUS_ACCESS_DENIED; }
static NTSTATUS deny_dispatch(FSP_FILE_SYSTEM *fs, FSP_FSCTL_TRANSACT_REQ *req, FSP_FSCTL_TRANSACT_RSP *rsp)
{ return STATUS_ACCESS_DENIED; }

void ZwDraftFileInterface(FSP_FILE_SYSTEM_INTERFACE *ops)
{
    memset(ops, 0, sizeof *ops);
    ops->GetSecurityByName = deny_name; ops->GetSecurity = deny_security;
    ops->Create = deny_create; ops->Overwrite = deny_overwrite;
    ops->Open = open_file; ops->GetFileInfo = information; ops->Read = read_file;
    ops->Write = write_file; ops->SetFileSize = resize_file; ops->Flush = flush_file;
    ops->Cleanup = cleanup; ops->Close = close_file;
}

/* Scoped callbacks require a second, deliberately different table. */
#include "draft_namespace.inc"

static int bind_file_plane(FSP_FILE_SYSTEM *fs, PyObject *plane, BOOL scoped)
{
    if (!fs || fs->UserContext || !fs->Interface || fs->Interface->Open != open_file || !plane) return 0;
    if (scoped != (fs->Interface->GetSecurityByName == scoped_name) ||
        (scoped && (!fs->UmFileContextIsUserContext2 || fs->UmFileContextIsFullContext ||
                    fs->Interface->GetStreamInfo != scoped_streams || fs->Interface->GetSecurity != scoped_security)))
        return 0;
    BOOL replacement = scoped && fs->Interface->Rename == scoped_rename;
    if (replacement != (fs->Interface->Create == scoped_create)) return 0;
    PyObject *name = PyUnicode_FromString("zipworkspace.desktop.container_windows_fileplane");
    PyObject *loaded = name ? PyImport_GetModule(name) : NULL; /* Existing trusted application module only. */
    Py_XDECREF(name);
    PyObject *type = loaded ? PyObject_GetAttrString(loaded, "WindowsDraftFilePlane") : NULL;
    Py_XDECREF(loaded);
    int exact = type && PyType_Check(type) && Py_TYPE(plane) == (PyTypeObject *)type;
    Py_XDECREF(type);
    if (!exact) return 0;
    /* No accidental driver access via e.g. a kernel-mode create that bypasses an SD query.
     * Direct callbacks on a private synthetic fs remain available to the qualification owner. */
    FspFileSystemSetOperationGuard(fs, deny_dispatch, NULL);
    DRAFT_BINDING *b = PyMem_Calloc(1, sizeof *b);
    if (!b) { PyErr_NoMemory(); return 0; }
    if (scoped && !session_policy(&b->policy, FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                                   FILE_GENERIC_READ | FILE_TRAVERSE)) {
        PyMem_Free(b); PyErr_SetString(PyExc_PermissionError, "ordinary Windows logon required"); return 0;
    }
    if (scoped) {
        PSID system = NULL; BOOL defaulted = FALSE;
        if (!GetSecurityDescriptorOwner(b->policy.file.bytes, &system, &defaulted) || !system ||
            !descriptor(b->policy.sid.bytes, system, FILE_GENERIC_READ, &b->zone, &b->zone_bytes) ||
            (replacement && (!descriptor(b->policy.sid.bytes, system, FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE,
                                          &b->scratch, &b->scratch_bytes) ||
                             !descriptor(b->policy.sid.bytes, system, FILE_GENERIC_READ | FILE_TRAVERSE | FILE_ADD_FILE,
                                         &b->add_file, &b->add_file_bytes)))) {
            PyMem_Free(b); PyErr_SetString(PyExc_PermissionError, "cannot establish the origin-stream descriptor"); return 0;
        }
    }
    InitializeSRWLock(&b->gate);
    b->scoped = scoped;
    b->replacement = replacement;
    b->lease = PyObject_CallNoArgs((PyObject *)&PyBaseObject_Type);
    if (!b->lease) { PyMem_Free(b); return 0; }
    b->plane = Py_NewRef(plane);
    fs->UserContext = b; /* Retain before the claim; never route requests through an unaccepted lease. */
    PyObject *r = PyObject_CallMethod(plane, "bind", "OOO", b->lease, scoped ? Py_True : Py_False,
                                      replacement ? Py_True : Py_False);
    if (!r || r != Py_None) {
        Py_XDECREF(r);
        PyObject *error = PyErr_GetRaisedException();
        ZwDraftAbandonFilePlane(fs); /* If release fails the caller still owns fs->UserContext. */
        if (error) PyErr_SetRaisedException(error);
        return 0;
    }
    b->ready = TRUE;
    if (scoped) FspFileSystemSetOperationGuard(fs, scoped_enter, scoped_leave);
    Py_DECREF(r); return 1;
}

int ZwDraftBindFilePlane(FSP_FILE_SYSTEM *fs, PyObject *plane) { return bind_file_plane(fs, plane, FALSE); }
int ZwDraftBindNamespace(FSP_FILE_SYSTEM *fs, PyObject *plane) { return bind_file_plane(fs, plane, TRUE); }

int ZwDraftStopNewOpens(FSP_FILE_SYSTEM *fs, ULONG *remaining)
{
    PyObject *plane = owner(fs);
    if (!plane || !remaining) return 0;
    PyObject *r = PyObject_CallMethod(plane, "stop_opening", NULL);
    if (!r) return 0;
    ULONG count = PyLong_CheckExact(r) ? PyLong_AsUnsignedLong(r) : ULONG_MAX;
    Py_DECREF(r);
    if (PyErr_Occurred() || count > 1024) { poison(plane); return 0; }
    *remaining = count; return 1;
}

int ZwDraftAbandonFilePlane(FSP_FILE_SYSTEM *fs)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b) return 0;
    b->ready = FALSE;
    PyObject *r = PyObject_CallMethod(b->plane, "abandon", "O", b->lease);
    if (!r || r != Py_None) { Py_XDECREF(r); return 0; } /* Unknown: keep the original reference. */
    Py_DECREF(r); fs->UserContext = NULL;
    Py_DECREF(b->plane); Py_DECREF(b->lease); PyMem_Free(b);
    return 1;
}

/* Only the dedicated native owner transfers a preclaimed lease to the exact SDK instance.
 * The shell cannot access the original working file through the guard before verification. */
int ZwDraftMoveBinding(FSP_FILE_SYSTEM *from, FSP_FILE_SYSTEM *to, BOOL attach)
{
    DRAFT_BINDING *b = binding(from);
    if (!b || !b->scoped || !b->ready || !to || to->UserContext || from->Interface != to->Interface ||
        !from->UmFileContextIsUserContext2 || !to->UmFileContextIsUserContext2 ||
        from->UmFileContextIsFullContext || to->UmFileContextIsFullContext) return 0;
    if (attach) {
        if (from->VolumeHandle || !to->VolumeHandle || to->DispatcherThread || b->real) return 0;
        b->hidden = b->real = TRUE;
        FspFileSystemSetOperationGuard(to, scoped_enter, scoped_leave);
    } else if (!from->VolumeHandle || to->VolumeHandle || !b->real) return 0;
    to->UserContext = b; from->UserContext = NULL; return 1;
}

int ZwDraftPresent(FSP_FILE_SYSTEM *fs)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b || !b->ready || !b->scoped || !b->real || !fs->VolumeHandle || !fs->MountHandle ||
        !fs->DispatcherThread) return 0;
    AcquireSRWLockExclusive(&b->gate);
    BOOL ok = b->hidden && !b->fenced && !b->late;
    if (ok) b->hidden = FALSE;
    ReleaseSRWLockExclusive(&b->gate); return ok;
}

int ZwDraftFenceIfIdle(FSP_FILE_SYSTEM *fs, ULONG *remaining)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b || !b->ready || !b->scoped || !b->real || !ZwDraftStopNewOpens(fs, remaining)) return -1;
    AcquireSRWLockExclusive(&b->gate);
    int result = 0;
    if (b->late || b->active < 0 || b->hidden) result = -1;
    else if (!*remaining && !b->active) { b->fenced = TRUE; result = 1; }
    ReleaseSRWLockExclusive(&b->gate); return result;
}

static const char cert_name[] = "zipworkspace.private.winfsp.deleted.original";
int ZwDraftConsumeCertificate(PyObject *proof, PyObject *plane, PyObject *lease)
{
    if (!PyCapsule_IsValid(proof, cert_name)) return 0;
    DRAFT_BINDING *b = PyCapsule_GetPointer(proof, cert_name);
    if (!b || b->plane != plane || b->lease != lease || !b->cert_unused ||
        b->cert_thread != GetCurrentThreadId()) return 0;
    b->cert_unused = FALSE; return 1;
}

int ZwDraftRememberLease(FSP_FILE_SYSTEM *fs, PyObject *choice)
{
    DRAFT_BINDING *b = binding(fs);
    return b && PyObject_SetAttrString(choice, "_cleanup_lease", b->lease) == 0;
}

void ZwDraftDropAfterJoin(FSP_FILE_SYSTEM *fs)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b) return;
    fs->UserContext = NULL;
    Py_DECREF(b->plane); Py_DECREF(b->lease); PyMem_Free(b);
}

int ZwDraftAcceptDeleted(FSP_FILE_SYSTEM *keeper)
{
    DRAFT_BINDING *b = binding(keeper);
    if (!b || keeper->VolumeHandle || !b->ready || !b->scoped || !b->real ||
        b->hidden || !b->fenced || b->active || b->late) return 0;
    b->ready = FALSE;
    PyObject *proof = PyCapsule_New(b, cert_name, NULL);
    if (!proof) return 0;
    b->cert_thread = GetCurrentThreadId(); b->cert_unused = TRUE;
    PyObject *r = PyObject_CallMethod(b->plane, "_finish_native", "OO", b->lease, proof);
    int ok = r == Py_None && !b->cert_unused;
    b->cert_thread = 0; b->cert_unused = FALSE;
    PyCapsule_SetName(proof, "zipworkspace.expired"); /* Even a retained Python reference cannot be reused. */
    Py_DECREF(proof); Py_XDECREF(r);
    if (ok) ZwDraftDropAfterJoin(keeper);
    return ok;
}
