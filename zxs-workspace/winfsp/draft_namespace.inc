/* Included only by draft_io.c. The separate frozen-snapshot interface is not writable. */
static NTSTATUS scoped_enter(FSP_FILE_SYSTEM *fs, FSP_FSCTL_TRANSACT_REQ *req, FSP_FSCTL_TRANSACT_RSP *rsp)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b || !b->scoped || !b->ready || !req || !rsp) return STATUS_ACCESS_DENIED;
    if (FspFsctlTransactCreateKind == req->Kind) {
        UINT32 opts = req->Req.Create.CreateOptions;
        if (!req->Req.Create.UserMode || req->Req.Create.OpenTargetDirectory ||
            req->Req.Create.HasBackupPrivilege || req->Req.Create.HasRestorePrivilege ||
            FILE_OPEN != (opts >> 24) || (opts & (FILE_DELETE_ON_CLOSE | FILE_OPEN_BY_FILE_ID |
                       FILE_OPEN_FOR_BACKUP_INTENT | FILE_OPEN_REPARSE_POINT)))
            return STATUS_ACCESS_DENIED;
        AlignedBuffer buffer;
        PSID sid = local_logon(FSP_FSCTL_TRANSACT_REQ_TOKEN_HANDLE(req->Req.Create.AccessToken), &buffer);
        if (!sid || !EqualSid(sid, b->policy.sid.bytes)) return STATUS_ACCESS_DENIED;
        /* The SDK must still evaluate desired rights against our self-relative descriptor.
         * Open then constrains the exact requested member/stream, options and granted rights. */
    }
    return FspFileSystemOpEnter(fs, req, rsp);
}

static NTSTATUS scoped_sd(FSP_FILE_SYSTEM *fs, long kind, PSECURITY_DESCRIPTOR sd, SIZE_T *size)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b || !b->ready || !b->scoped) return STATUS_ACCESS_DENIED;
    if (kind < 1 || kind > 3) return STATUS_DATA_ERROR;
    const AlignedBuffer *chosen = kind == 1 ? &b->policy.directory : kind == 3 ? &b->zone : &b->policy.file;
    SIZE_T required = kind == 1 ? b->policy.directory_bytes : kind == 3 ? b->zone_bytes : b->policy.file_bytes;
    if (!required) return STATUS_ACCESS_DENIED;
    if (!size) return sd ? STATUS_INVALID_PARAMETER : STATUS_SUCCESS;
    SIZE_T supplied = *size; *size = required;
    if (!sd || supplied < required) return STATUS_BUFFER_OVERFLOW;
    memcpy(sd, chosen->bytes, required);
    return STATUS_SUCCESS;
}

static NTSTATUS scoped_name(FSP_FILE_SYSTEM *fs, PWSTR path, PUINT32 attrs, PSECURITY_DESCRIPTOR sd, SIZE_T *size)
{
    PyObject *plane = owner(fs);
    if (!plane) return STATUS_ACCESS_DENIED;
    size_t n = path ? wcsnlen_s(path, 274) : 0;
    if (!n || n >= 274) return STATUS_OBJECT_NAME_INVALID;
    FSP_FSCTL_FILE_INFO fi;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *name = PyUnicode_FromWideChar(path, (Py_ssize_t)n);
    PyObject *r = name ? PyObject_CallMethod(plane, "lookup", "O", name) : NULL;
    NTSTATUS s = file_info(r, &fi);
    long kind = s == STATUS_SUCCESS ? PyLong_AsLong(PyTuple_GET_ITEM(r, 5)) : 0;
    Py_XDECREF(r); Py_XDECREF(name); PyGILState_Release(g);
    if (s != STATUS_SUCCESS) return s;
    if (attrs) *attrs = fi.FileAttributes;
    return scoped_sd(fs, kind, sd, size);
}

static NTSTATUS scoped_security(FSP_FILE_SYSTEM *fs, PVOID ctx, PSECURITY_DESCRIPTOR sd, SIZE_T *size)
{
    PyObject *plane = owner(fs);
    if (!plane || !ctx) return STATUS_ACCESS_DENIED;
    FSP_FSCTL_FILE_INFO fi;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *r = PyObject_CallMethod(plane, "security_info", "K", SLOT(ctx));
    NTSTATUS s = file_info(r, &fi);
    long kind = s == STATUS_SUCCESS ? PyLong_AsLong(PyTuple_GET_ITEM(r, 5)) : 0;
    Py_XDECREF(r); PyGILState_Release(g);
    return s == STATUS_SUCCESS ? scoped_sd(fs, kind, sd, size) : s;
}

static NTSTATUS scoped_directory(FSP_FILE_SYSTEM *fs, PVOID ctx, PWSTR pattern, PWSTR marker,
                                 PVOID out, ULONG cap, PULONG copied)
{
    PyObject *plane = owner(fs);
    *copied = 0; (void)pattern; /* PassQueryDirectoryPattern is false; FSD filters. */
    if (!plane || !ctx) return STATUS_INVALID_HANDLE;
    if (!out && cap) return STATUS_INVALID_PARAMETER;
    size_t n = marker ? wcsnlen_s(marker, 256) : 0;
    if (n >= 256) return STATUS_OBJECT_NAME_INVALID;
    __declspec(align(8)) BYTE packed[sizeof(FSP_FSCTL_DIR_INFO) + 544] = {0};
    ULONG used = 0, first = 0;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *m = marker ? PyUnicode_FromWideChar(marker, (Py_ssize_t)n) : Py_NewRef(Py_None);
    PyObject *r = m ? PyObject_CallMethod(plane, "directory", "KO", SLOT(ctx), m) : NULL;
    NTSTATUS s = STATUS_SUCCESS;
    if (!r) s = failure();
    else if (r != Py_None) {
        __declspec(align(8)) BYTE record[sizeof(FSP_FSCTL_DIR_INFO) + 512] = {0};
        FSP_FSCTL_DIR_INFO *di = (FSP_FSCTL_DIR_INFO *)record;
        if (!PyTuple_CheckExact(r) || PyTuple_GET_SIZE(r) != 2 ||
            !PyUnicode_CheckExact(PyTuple_GET_ITEM(r, 0))) s = STATUS_DATA_ERROR;
        else s = file_info(PyTuple_GET_ITEM(r, 1), &di->FileInfo);
        if (s == STATUS_SUCCESS) {
            Py_ssize_t units = PyUnicode_AsWideChar(PyTuple_GET_ITEM(r, 0), di->FileNameBuf, 256);
            if (units < 0) s = failure();
            else if (!units || units > 255) s = STATUS_DATA_ERROR;
            else {
                di->Size = (UINT16)(sizeof *di + units * sizeof(WCHAR));
                if (!FspFileSystemAddDirInfo(di, packed, sizeof packed, &used)) s = STATUS_DATA_ERROR;
                first = used;
            }
        }
    }
    if (s == STATUS_SUCCESS) {
        if (!FspFileSystemAddDirInfo(NULL, packed, sizeof packed, &used)) s = STATUS_DATA_ERROR;
        else if (used > cap && (!first || first > cap)) s = STATUS_BUFFER_TOO_SMALL;
        else {
            *copied = used <= cap ? used : first; /* Resume at real last entry; never fabricate EOF. */
            memcpy(out, packed, *copied);
        }
    }
    Py_XDECREF(r); Py_XDECREF(m); PyGILState_Release(g);
    return s;
}

static NTSTATUS scoped_streams(FSP_FILE_SYSTEM *fs, PVOID ctx, PVOID out, ULONG cap, PULONG copied)
{
    PyObject *plane = owner(fs);
    *copied = 0;
    if (!plane || !ctx) return STATUS_INVALID_HANDLE;
    if (!out && cap) return STATUS_INVALID_PARAMETER;
    static const WCHAR zone[] = L"Zone.Identifier";
    __declspec(align(8)) BYTE packed[256] = {0};
    ULONG used = 0;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *rows = PyObject_CallMethod(plane, "streams", "K", SLOT(ctx));
    NTSTATUS s = STATUS_SUCCESS;
    if (!rows) s = failure();
    else if (!PyTuple_CheckExact(rows) || PyTuple_GET_SIZE(rows) > 2) s = STATUS_DATA_ERROR;
    else for (Py_ssize_t i = 0; i < PyTuple_GET_SIZE(rows); i++) {
        FSP_FSCTL_FILE_INFO fi;
        PyObject *row = PyTuple_GET_ITEM(rows, i);
        s = file_info(row, &fi);
        if (s != STATUS_SUCCESS) break;
        if (PyLong_AsLong(PyTuple_GET_ITEM(row, 5)) != (i ? 3 : 2)) { s = STATUS_DATA_ERROR; break; }
        __declspec(align(8)) BYTE record[sizeof(FSP_FSCTL_STREAM_INFO) + sizeof zone] = {0};
        FSP_FSCTL_STREAM_INFO *si = (FSP_FSCTL_STREAM_INFO *)record;
        si->Size = (UINT16)(sizeof *si + (i ? sizeof zone - sizeof(WCHAR) : 0));
        si->StreamSize = fi.FileSize; si->StreamAllocationSize = fi.AllocationSize;
        if (i) memcpy(si->StreamNameBuf, zone, sizeof zone - sizeof(WCHAR));
        if (!FspFileSystemAddStreamInfo(si, packed, sizeof packed, &used)) { s = STATUS_DATA_ERROR; break; }
    }
    if (s == STATUS_SUCCESS) {
        if (!FspFileSystemAddStreamInfo(NULL, packed, sizeof packed, &used)) s = STATUS_DATA_ERROR;
        else if (cap < used) s = STATUS_BUFFER_TOO_SMALL; /* Never report a truncated list without its MOTW. */
        else { memcpy(out, packed, used); *copied = used; }
    }
    Py_XDECREF(rows); PyGILState_Release(g); return s;
}

void ZwDraftNamespaceInterface(FSP_FSCTL_VOLUME_PARAMS *vp, FSP_FILE_SYSTEM_INTERFACE *ops)
{
    ZwDraftFileInterface(ops); memset(vp, 0, sizeof *vp);
    vp->Version = sizeof *vp; vp->SectorSize = 512; vp->SectorsPerAllocationUnit = 8;
    vp->MaxComponentLength = 255; vp->CaseSensitiveSearch = vp->CasePreservedNames = vp->UnicodeOnDisk = 1;
    vp->UmFileContextIsUserContext2 = vp->PersistentAcls = vp->NamedStreams = 1;
    vp->FlushAndPurgeOnCleanup = 1; /* Cache preference, never detach evidence. */
    vp->FileInfoTimeout = 0; vp->AllowOpenInKernelMode = 0;
    ops->GetSecurityByName = scoped_name; ops->GetSecurity = scoped_security;
    ops->ReadDirectory = scoped_directory; ops->GetStreamInfo = scoped_streams;
}
