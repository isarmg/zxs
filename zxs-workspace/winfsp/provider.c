/* Actual WinFsp callback signatures from the vendor SDK; no private structure replicas. */
#include "provider.h"
#include <stdint.h>
#include <string.h>

#define V(fs) ((PyObject *)(fs)->UserContext)
#define INDEX(fc) ((unsigned long long)((UINT_PTR)(fc) >> 1))
#define CAN_READ(fc) ((UINT_PTR)(fc) & 1)
#define MAX_READ (16U << 20)
#define MAX_UNITS 240

/* Map exceptions within a callback. No traceback, member data or local paths go to system logs. */
static NTSTATUS exception(void)
{
    NTSTATUS s = STATUS_IO_DEVICE_ERROR;
    if (PyErr_ExceptionMatches(PyExc_FileNotFoundError)) s = STATUS_OBJECT_NAME_NOT_FOUND;
    else if (PyErr_ExceptionMatches(PyExc_NotADirectoryError)) s = STATUS_NOT_A_DIRECTORY;
    else if (PyErr_ExceptionMatches(PyExc_IsADirectoryError)) s = STATUS_FILE_IS_A_DIRECTORY;
    else if (PyErr_ExceptionMatches(PyExc_EOFError)) s = STATUS_END_OF_FILE;
    else if (PyErr_ExceptionMatches(PyExc_ValueError)) s = STATUS_INVALID_PARAMETER;
    else if (PyErr_ExceptionMatches(PyExc_MemoryError)) s = STATUS_INSUFFICIENT_RESOURCES;
    PyErr_Clear();
    return s;
}

static NTSTATUS info_from(PyObject *row, FSP_FSCTL_FILE_INFO *out)
{
    if (!row) return exception();
    if (!PyTuple_CheckExact(row) || PyTuple_GET_SIZE(row) != 3) return STATUS_DATA_ERROR;
    PyObject *id = PyTuple_GET_ITEM(row, 0), *dir = PyTuple_GET_ITEM(row, 1),
             *sz = PyTuple_GET_ITEM(row, 2);
    if (!PyLong_CheckExact(id) || !PyBool_Check(dir) || !PyLong_CheckExact(sz)) return STATUS_DATA_ERROR;
    UINT64 i = PyLong_AsUnsignedLongLong(id);
    if (PyErr_Occurred()) return exception();
    UINT64 n = PyLong_AsUnsignedLongLong(sz);
    if (PyErr_Occurred()) return exception();
    if (i == 0 || i > (UINTPTR_MAX >> 1) || n > UINT64_MAX - 4095 ||
            (dir == Py_True && n)) return STATUS_DATA_ERROR;
    memset(out, 0, sizeof *out);
    out->FileAttributes = FILE_ATTRIBUTE_READONLY | (dir == Py_True ? FILE_ATTRIBUTE_DIRECTORY : 0);
    out->IndexNumber = i;
    out->FileSize = n;
    out->AllocationSize = (n + 4095) & ~(UINT64)4095;
    return STATUS_SUCCESS;
}

static NTSTATUS volume_info(FSP_FILE_SYSTEM *fs, FSP_FSCTL_VOLUME_INFO *info)
{
    if (!V(fs)) return STATUS_DEVICE_NOT_READY;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *n = PyObject_CallMethod(V(fs), "volume_size", NULL);
    NTSTATUS s = STATUS_SUCCESS;
    memset(info, 0, sizeof *info);
    if (n) {
        info->TotalSize = PyLong_AsUnsignedLongLong(n);
        if (PyErr_Occurred()) s = exception();
        else {
            static const WCHAR label[] = L"ZIP Snapshot";
            memcpy(info->VolumeLabel, label, sizeof label - sizeof(WCHAR));
            info->VolumeLabelLength = (UINT16)(sizeof label - sizeof(WCHAR));
        }
    } else s = exception();
    Py_XDECREF(n); PyGILState_Release(g);
    return s;
}

static NTSTATUS open_node(FSP_FILE_SYSTEM *fs, PWSTR name, UINT32 opts, UINT32 granted,
                          PVOID *context, FSP_FSCTL_FILE_INFO *info)
{
    *context = NULL;
    if (!V(fs)) return STATUS_DEVICE_NOT_READY;
    /* WinFsp forwards NtCreateFile's disposition in the upper byte. Never open-if, overwrite,
     * open-by-ID, delete-on-close, or turn GENERIC_ALL/MAXIMUM_ALLOWED into a data capability. */
    if ((opts >> 24) != FILE_OPEN || (opts & (FILE_DELETE_ON_CLOSE | FILE_OPEN_BY_FILE_ID)) ||
            (granted & ~(FILE_GENERIC_READ | FILE_TRAVERSE)))
        return STATUS_ACCESS_DENIED;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *path = PyUnicode_FromWideChar(name, -1);
    PyObject *row = path ? PyObject_CallMethod(V(fs), "lookup", "O", path) : NULL;
    Py_XDECREF(path);
    NTSTATUS s = info_from(row, info);
    if (s == STATUS_SUCCESS) {
        int d = (info->FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
        if (!d && (opts & FILE_DIRECTORY_FILE)) s = STATUS_NOT_A_DIRECTORY;
        else if (d && (opts & FILE_NON_DIRECTORY_FILE)) s = STATUS_FILE_IS_A_DIRECTORY;
        else if (!d && (granted & FILE_EXECUTE)) s = STATUS_ACCESS_DENIED;
        else *context = (PVOID)(UINT_PTR)((info->IndexNumber << 1) | ((granted & FILE_READ_DATA) != 0));
    }
    Py_XDECREF(row); PyGILState_Release(g);
    return s;
}

static NTSTATUS get_info(FSP_FILE_SYSTEM *fs, PVOID context, FSP_FSCTL_FILE_INFO *info)
{
    if (!V(fs)) return STATUS_DEVICE_NOT_READY;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *row = PyObject_CallMethod(V(fs), "info", "K", INDEX(context));
    NTSTATUS s = info_from(row, info);
    Py_XDECREF(row); PyGILState_Release(g);
    return s;
}

static NTSTATUS read_node(FSP_FILE_SYSTEM *fs, PVOID context, PVOID buffer, UINT64 off,
                          ULONG length, PULONG transferred)
{
    *transferred = 0;
    if (!V(fs)) return STATUS_DEVICE_NOT_READY;
    if (!CAN_READ(context)) return STATUS_ACCESS_DENIED;
    if (length > MAX_READ) return STATUS_INVALID_PARAMETER;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *body = PyObject_CallMethod(V(fs), "read", "KKk", INDEX(context),
                                         (unsigned long long)off, (unsigned long)length);
    NTSTATUS s = STATUS_SUCCESS;
    if (!body) s = exception();
    else if (!PyBytes_CheckExact(body) || PyBytes_GET_SIZE(body) > (Py_ssize_t)length) s = STATUS_DATA_ERROR;
    else {
        ULONG n = (ULONG)PyBytes_GET_SIZE(body);
        if (n) memcpy(buffer, PyBytes_AS_STRING(body), n);
        *transferred = n;
    }
    Py_XDECREF(body); PyGILState_Release(g);
    return s;
}

static NTSTATUS read_dir(FSP_FILE_SYSTEM *fs, PVOID context, PWSTR pattern, PWSTR marker,
                         PVOID buffer, ULONG length, PULONG transferred)
{
    (void)pattern; /* FSD applies its pattern; resume by the preceding filename, not a numeric offset. */
    *transferred = 0;
    if (!V(fs)) return STATUS_DEVICE_NOT_READY;
    if (!CAN_READ(context)) return STATUS_ACCESS_DENIED;
    ULONG used = 0;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *mark = marker ? PyUnicode_FromWideChar(marker, -1) : Py_NewRef(Py_None);
    PyObject *it = mark ? PyObject_CallMethod(V(fs), "directory", "KO", INDEX(context), mark) : NULL;
    PyObject *item = NULL;
    NTSTATUS s = STATUS_SUCCESS;
    if (!it) s = exception();
    else if (!PyIter_Check(it)) s = STATUS_DATA_ERROR;
    else for (;;) {
        item = PyIter_Next(it);
        if (!item) {
            if (PyErr_Occurred()) s = exception();
            else if (!FspFileSystemAddDirInfo(NULL, buffer, length, &used) && !used)
                s = STATUS_BUFFER_TOO_SMALL;
            break;
        }
        __declspec(align(8)) BYTE record[sizeof(FSP_FSCTL_DIR_INFO) + sizeof(WCHAR) * (MAX_UNITS+1)];
        memset(record, 0, sizeof record);
        FSP_FSCTL_DIR_INFO *di = (FSP_FSCTL_DIR_INFO *)record;
        if (!PyTuple_CheckExact(item) || PyTuple_GET_SIZE(item) != 2 ||
                !PyUnicode_CheckExact(PyTuple_GET_ITEM(item, 0))) { s = STATUS_DATA_ERROR; break; }
        s = info_from(PyTuple_GET_ITEM(item, 1), &di->FileInfo);
        if (s != STATUS_SUCCESS) break;
        Py_ssize_t n = PyUnicode_AsWideChar(PyTuple_GET_ITEM(item, 0), di->FileNameBuf, MAX_UNITS + 1);
        if (n < 0) { s = exception(); break; }
        if (!n || n > MAX_UNITS) { s = STATUS_DATA_ERROR; break; }
        di->Size = (UINT16)(sizeof *di + n * sizeof(WCHAR));
        ULONG prev = used;
        if (!FspFileSystemAddDirInfo(di, buffer, length, &used)) {
            if (!used) s = STATUS_BUFFER_TOO_SMALL;
            break;  /* There is another real entry; never append an artificial EOF. */
        }
        /* The SDK counts the aligned size, but copies only di->Size bytes. */
        memset((BYTE *)buffer + prev + di->Size, 0, used - prev - di->Size);
        Py_CLEAR(item);
    }
    if (s == STATUS_SUCCESS) {
        PyObject *checked = PyObject_CallMethod(V(fs), "check", NULL);
        if (!checked) s = exception();
        else { Py_DECREF(checked); *transferred = used; }
    }
    Py_XDECREF(item); Py_XDECREF(it); Py_XDECREF(mark); PyGILState_Release(g);
    return s;
}

/* This leaf cannot grant anyone a mounted namespace: no permissive or absent ACL fallback.
 * A future real host must qualify caller scope + attachment provenance before replacing these. */
static NTSTATUS deny_name(FSP_FILE_SYSTEM *fs, PWSTR name, PUINT32 attr,
                          PSECURITY_DESCRIPTOR sd, SIZE_T *size) { return STATUS_ACCESS_DENIED; }
static NTSTATUS deny_security(FSP_FILE_SYSTEM *fs, PVOID fc, PSECURITY_DESCRIPTOR sd, SIZE_T *size)
{ return STATUS_ACCESS_DENIED; }
/* The 2.1 dispatcher requires both handlers even to dispatch a FILE_OPEN. */
static NTSTATUS deny_create(FSP_FILE_SYSTEM *fs, PWSTR name, UINT32 opts, UINT32 granted,
                            UINT32 attrs, PSECURITY_DESCRIPTOR sd, UINT64 alloc,
                            PVOID *context, FSP_FSCTL_FILE_INFO *info)
{ *context = NULL; return STATUS_MEDIA_WRITE_PROTECTED; }
static NTSTATUS deny_overwrite(FSP_FILE_SYSTEM *fs, PVOID fc, UINT32 attrs, BOOLEAN replace,
                               UINT64 alloc, FSP_FSCTL_FILE_INFO *info)
{ return STATUS_MEDIA_WRITE_PROTECTED; }
static NTSTATUS deny_write(FSP_FILE_SYSTEM *fs, PVOID fc, PVOID buf, UINT64 off, ULONG len,
                           BOOLEAN end, BOOLEAN constrained, PULONG bytes, FSP_FSCTL_FILE_INFO *info)
{ *bytes = 0; return STATUS_MEDIA_WRITE_PROTECTED; }

void ZwSnapshotDataInterface(FSP_FSCTL_VOLUME_PARAMS *p, FSP_FILE_SYSTEM_INTERFACE *ops)
{
    memset(p, 0, sizeof *p); memset(ops, 0, sizeof *ops);
    p->Version = sizeof *p; p->SectorSize = 512; p->SectorsPerAllocationUnit = 8;
    p->MaxComponentLength = MAX_UNITS; /* v2.1 validates a difference of WCHAR pointers in src/sys/name.c. */
    p->ReadOnlyVolume = p->CaseSensitiveSearch = p->CasePreservedNames = p->UnicodeOnDisk = 1;
    p->UmFileContextIsUserContext2 = 1; /* constant per-open inode + read access, no native per-open allocation */
    ops->GetSecurityByName = deny_name; ops->GetSecurity = deny_security;
    ops->Create = deny_create; ops->Overwrite = deny_overwrite;
    ops->GetVolumeInfo = volume_info; ops->Open = open_node; ops->GetFileInfo = get_info;
    ops->Read = read_node; ops->ReadDirectory = read_dir; ops->Write = deny_write;
    /* No writable callback or handler for alternate streams, EAs or reparse data. */
}

int ZwSnapshotBind(FSP_FILE_SYSTEM *fs, PyObject *volume)
{
    if (!fs || fs->UserContext || !volume) return 0;
    fs->UserContext = Py_NewRef(volume);
    return 1;
}

void ZwSnapshotUnbind(FSP_FILE_SYSTEM *fs)
{
    PyObject *volume = V(fs);
    fs->UserContext = NULL;
    Py_XDECREF(volume);
}
