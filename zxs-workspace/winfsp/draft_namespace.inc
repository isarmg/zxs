/* Included only by draft_io.c. The separate frozen-snapshot interface is not writable. */
static PCWSTR request_path(const FSP_FSCTL_TRANSACT_REQ *req, FSP_FSCTL_TRANSACT_BUF b)
{
    const size_t header = offsetof(FSP_FSCTL_TRANSACT_REQ, Buffer);
    if (req->Size < header || req->Size > FSP_FSCTL_TRANSACT_REQ_SIZEMAX || ((b.Offset | b.Size) & 1) ||
        b.Size < 2 * sizeof(WCHAR) || b.Size > 274 * sizeof(WCHAR) ||
        (size_t)b.Offset + b.Size > req->Size - header) return NULL;
    PCWSTR name = (PCWSTR)(req->Buffer + b.Offset);
    size_t count = b.Size / sizeof(WCHAR);
    return name[count - 1] == 0 && wcsnlen_s(name, count) == count - 1 ? name : NULL;
}

static BOOL matching_logon(const DRAFT_BINDING *b, UINT64 token)
{
    AlignedBuffer buffer;
    HANDLE h = FSP_FSCTL_TRANSACT_REQ_TOKEN_HANDLE(token);
    PSID sid = h ? local_logon(h, &buffer) : NULL;
    return sid && EqualSid(sid, b->policy.sid.bytes);
}

static BOOL plain_replace(const FSP_FSCTL_TRANSACT_REQ *req)
{
    return req && req->Kind == FspFsctlTransactSetInformationKind &&
        req->Req.SetInformation.FileInformationClass == 10 && /* FileRenameInformation, never Ex/POSIX. */
        req->Req.SetInformation.UserContext == 0 && req->Req.SetInformation.UserContext2 &&
        req->Req.SetInformation.Info.RenameEx.Flags == 1 && req->Req.SetInformation.Info.Rename.AccessToken;
}

static BOOL permits_name(FSP_FILE_SYSTEM *fs, const char *method, PCWSTR path, UINT64 slot, PCWSTR old)
{
    PyObject *plane = owner(fs);
    if (!plane || !path) return FALSE;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *name = PyUnicode_FromWideChar(path, -1);
    PyObject *prior = old ? PyUnicode_FromWideChar(old, -1) : NULL;
    PyObject *r = !name || (old && !prior) ? NULL : old ?
        PyObject_CallMethod(plane, method, "OKO", name, (unsigned long long)slot, prior) :
        PyObject_CallMethod(plane, method, "O", name);
    BOOL yes = r == Py_True;
    Py_XDECREF(r); Py_XDECREF(prior); Py_XDECREF(name); PyErr_Clear(); PyGILState_Release(g);
    return yes;
}

static NTSTATUS check_enter(FSP_FILE_SYSTEM *fs, FSP_FSCTL_TRANSACT_REQ *req, FSP_FSCTL_TRANSACT_RSP *rsp)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b || !b->scoped || !b->ready || !req || !rsp) return STATUS_ACCESS_DENIED;
    if (FspFsctlTransactCreateKind == req->Kind) {
        UINT32 opts = req->Req.Create.CreateOptions;
        UINT32 disposition = opts >> 24;
        if (!req->Req.Create.UserMode || req->Req.Create.HasBackupPrivilege || req->Req.Create.HasRestorePrivilege ||
            (opts & (FILE_DELETE_ON_CLOSE | FILE_OPEN_BY_FILE_ID)) ||
            ((opts & FILE_OPEN_FOR_BACKUP_INTENT) && disposition != FILE_OPEN) ||
            req->Req.Create.Ea.Size || req->Req.Create.EaIsReparsePoint ||
            !matching_logon(b, req->Req.Create.AccessToken))
            return STATUS_ACCESS_DENIED;
        /* WinFsp 2.1 FspMainFileOpen re-enters with a force-access-checked FILE_OPEN +
         * FILE_OPEN_REPARSE_POINT and FILE_READ_ATTRIBUTES before a named-stream lookup.
         * No ECP identity crosses this ABI. Treat it as a narrowly non-mutating
         * request on an already admitted leaf; this namespace has no reparse data. */
        if (opts & FILE_OPEN_REPARSE_POINT) {
            UINT32 access = req->Req.Create.DesiredAccess | req->Req.Create.GrantedAccess;
            if (!b->real || disposition != FILE_OPEN || req->Req.Create.NamedStream ||
                req->Req.Create.OpenTargetDirectory || req->Req.Create.HasTrailingBackslash ||
                req->Req.Create.SecurityDescriptor.Offset || req->Req.Create.SecurityDescriptor.Size ||
                req->Req.Create.AllocationSize || req->Req.Create.ShareAccess != (FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE) ||
                !(access & FILE_READ_ATTRIBUTES) || (access & ~(FILE_READ_ATTRIBUTES | SYNCHRONIZE)) ||
                (opts & 0xffffff & ~(FILE_OPEN_REPARSE_POINT | FILE_NON_DIRECTORY_FILE |
                                     FILE_SYNCHRONOUS_IO_ALERT | FILE_SYNCHRONOUS_IO_NONALERT)) ||
                !permits_name(fs, "permits_stream_main", request_path(req, req->FileName), 0, NULL))
                return STATUS_ACCESS_DENIED;
        }
        if (req->Req.Create.OpenTargetDirectory || disposition == FILE_CREATE) {
            if (!b->real || !b->replacement || req->Req.Create.NamedStream || req->Req.Create.HasTrailingBackslash ||
                req->Req.Create.SecurityDescriptor.Offset || req->Req.Create.SecurityDescriptor.Size)
                return STATUS_ACCESS_DENIED;
            PCWSTR name = request_path(req, req->FileName);
            if (req->Req.Create.OpenTargetDirectory) {
                if (disposition != FILE_OPEN || !permits_name(fs, "permits_target", name, 0, NULL)) return STATUS_ACCESS_DENIED;
            } else {
                if ((opts & FILE_DIRECTORY_FILE) || req->Req.Create.AllocationSize ||
                    (req->Req.Create.FileAttributes & ~(FILE_ATTRIBUTE_NORMAL | FILE_ATTRIBUTE_ARCHIVE)) ||
                    !permits_name(fs, "scratch_name", name, 0, NULL)) return STATUS_ACCESS_DENIED;
            }
        } else if (disposition != FILE_OPEN && disposition != FILE_OPEN_IF) return STATUS_ACCESS_DENIED;
        /* The SDK still checks desired rights. Only bounded Create/Open callbacks can create contexts. */
    } else if (req->Kind == FspFsctlTransactSetInformationKind) {
        UINT32 kind = req->Req.SetInformation.FileInformationClass;
        if (kind == 13 || kind == 64 || kind == 65) return STATUS_ACCESS_DENIED; /* Never disposition or RenameEx. */
        if (kind == 10) {
            if (!b->real || !b->replacement || !plain_replace(req) ||
                !matching_logon(b, req->Req.SetInformation.Info.Rename.AccessToken)) return STATUS_ACCESS_DENIED;
            PCWSTR old = request_path(req, req->FileName);
            PCWSTR dest = request_path(req, req->Req.SetInformation.Info.Rename.NewFileName);
            if (!old || !permits_name(fs, "permits_target", dest, req->Req.SetInformation.UserContext2, old))
                return STATUS_ACCESS_DENIED;
        }
    } else if (req->Kind == FspFsctlTransactCleanupKind && req->Req.Cleanup.Delete) {
        /* Defensive: a delete must never have been armed. Denying Cleanup is not a rollback. */
        AcquireSRWLockExclusive(&b->gate); b->late = TRUE; ReleaseSRWLockExclusive(&b->gate);
        return STATUS_ACCESS_DENIED;
    }
    return FspFileSystemOpEnter(fs, req, rsp);
}

static NTSTATUS scoped_enter(FSP_FILE_SYSTEM *fs, FSP_FSCTL_TRANSACT_REQ *req, FSP_FSCTL_TRANSACT_RSP *rsp)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b || !b->scoped || !b->ready || !req || !rsp) return STATUS_ACCESS_DENIED;
    AcquireSRWLockExclusive(&b->gate);
    if (b->hidden || b->fenced) {
        if (b->fenced && (req->Kind == FspFsctlTransactWriteKind || req->Kind == FspFsctlTransactSetInformationKind ||
                (req->Kind == FspFsctlTransactFlushBuffersKind && req->Req.FlushBuffers.UserContext2) ||
                (req->Kind == FspFsctlTransactCleanupKind && req->Req.Cleanup.UserContext2) ||
                (req->Kind == FspFsctlTransactCloseKind && req->Req.Close.UserContext2) ||
                (req->Kind == FspFsctlTransactReadKind && req->Req.Read.UserContext2) ||
                (req->Kind == FspFsctlTransactQueryInformationKind && req->Req.QueryInformation.UserContext2) ||
                (req->Kind == FspFsctlTransactQueryDirectoryKind && req->Req.QueryDirectory.UserContext2) ||
                (req->Kind == FspFsctlTransactQueryStreamInformationKind && req->Req.QueryStreamInformation.UserContext2) ||
                (req->Kind == FspFsctlTransactQuerySecurityKind && req->Req.QuerySecurity.UserContext2)))
            b->late = TRUE;
        ReleaseSRWLockExclusive(&b->gate); return STATUS_DEVICE_NOT_READY;
    }
    ++b->active;
    ReleaseSRWLockExclusive(&b->gate);
    NTSTATUS status = check_enter(fs, req, rsp);
    if (!NT_SUCCESS(status)) {
        AcquireSRWLockExclusive(&b->gate); --b->active; ReleaseSRWLockExclusive(&b->gate);
    }
    return status;
}

static NTSTATUS scoped_leave(FSP_FILE_SYSTEM *fs, FSP_FSCTL_TRANSACT_REQ *req, FSP_FSCTL_TRANSACT_RSP *rsp)
{
    DRAFT_BINDING *b = binding(fs);
    NTSTATUS s = FspFileSystemOpLeave(fs, req, rsp);
    AcquireSRWLockExclusive(&b->gate);
    if (!NT_SUCCESS(s) || b->active <= 0) b->late = TRUE;
    --b->active;
    ReleaseSRWLockExclusive(&b->gate);
    return s;
}

static NTSTATUS scoped_capacity(FSP_FILE_SYSTEM *fs, FSP_FSCTL_VOLUME_INFO *info)
{
    static const WCHAR label[] = L"Workspace";
    PyObject *plane = owner(fs);
    if (!plane) return STATUS_DEVICE_NOT_READY;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *row = PyObject_CallMethod(plane, "volume", NULL);
    NTSTATUS s = STATUS_SUCCESS;
    if (!row) s = failure();
    else if (!PyTuple_CheckExact(row) || PyTuple_GET_SIZE(row) != 2 ||
             !PyLong_CheckExact(PyTuple_GET_ITEM(row, 0)) || !PyLong_CheckExact(PyTuple_GET_ITEM(row, 1)))
        s = STATUS_DATA_ERROR;
    else {
        memset(info, 0, sizeof *info);
        info->TotalSize = PyLong_AsUnsignedLongLong(PyTuple_GET_ITEM(row, 0));
        info->FreeSize = PyLong_AsUnsignedLongLong(PyTuple_GET_ITEM(row, 1));
        if (PyErr_Occurred()) s = failure();
        else if (!info->TotalSize || info->TotalSize > INT64_MAX || info->FreeSize > info->TotalSize) s = STATUS_DATA_ERROR;
        else {
            memcpy(info->VolumeLabel, label, sizeof label - sizeof(WCHAR));
            info->VolumeLabelLength = (UINT16)(sizeof label - sizeof(WCHAR));
        }
    }
    Py_XDECREF(row); PyGILState_Release(g); return s;
}

static NTSTATUS scoped_sd(FSP_FILE_SYSTEM *fs, long kind, PSECURITY_DESCRIPTOR sd, SIZE_T *size)
{
    DRAFT_BINDING *b = binding(fs);
    if (!b || !b->ready || !b->scoped) return STATUS_ACCESS_DENIED;
    const AlignedBuffer *chosen;
    SIZE_T required;
    switch (kind) {
    case 1: chosen = &b->policy.directory; required = b->policy.directory_bytes; break;
    case 2: chosen = &b->policy.file; required = b->policy.file_bytes; break;
    case 3: chosen = &b->zone; required = b->zone_bytes; break;
    case 4: chosen = &b->scratch; required = b->scratch_bytes; break;
    case 5: chosen = &b->add_file; required = b->add_file_bytes; break;
    default: return STATUS_DATA_ERROR;
    }
    if (!required) return STATUS_ACCESS_DENIED;
    if (!size) return sd ? STATUS_INVALID_PARAMETER : STATUS_SUCCESS;
    SIZE_T supplied = *size; *size = required;
    if (!sd || supplied < required) return STATUS_BUFFER_OVERFLOW;
    memcpy(sd, chosen->bytes, required);
    return STATUS_SUCCESS;
}

static NTSTATUS scoped_create(FSP_FILE_SYSTEM *fs, PWSTR path, UINT32 opts, UINT32 granted, UINT32 attrs,
                              PSECURITY_DESCRIPTOR sd, UINT64 alloc, PVOID *ctx, FSP_FSCTL_FILE_INFO *info)
{
    *ctx = NULL; (void)sd; /* Native guard rejected supplied descriptors; physical file inherits private NTFS grants. */
    const FSP_FSCTL_TRANSACT_REQ *req = real_request(fs);
    if (!req || req->Kind != FspFsctlTransactCreateKind || req->Req.Create.OpenTargetDirectory ||
        (opts >> 24) != FILE_CREATE || alloc || (attrs & ~(FILE_ATTRIBUTE_NORMAL | FILE_ATTRIBUTE_ARCHIVE)))
        return STATUS_ACCESS_DENIED;
    return finish_open(fs, path, opts, granted, TRUE, FALSE, FALSE, ctx, info);
}

static NTSTATUS scoped_rename(FSP_FILE_SYSTEM *fs, PVOID ctx, PWSTR old, PWSTR dest, BOOLEAN replace)
{
    const FSP_FSCTL_TRANSACT_REQ *req = real_request(fs);
    if (!replace || !ctx || !plain_replace(req) || SLOT(ctx) != req->Req.SetInformation.UserContext2 ||
        old != (PWSTR)request_path(req, req->FileName) ||
        dest != (PWSTR)request_path(req, req->Req.SetInformation.Info.Rename.NewFileName))
        return STATUS_ACCESS_DENIED;
    PyObject *plane = owner(fs);
    if (!plane) return STATUS_DEVICE_NOT_READY;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *before = PyUnicode_FromWideChar(old, -1), *after = PyUnicode_FromWideChar(dest, -1);
    PyObject *r = before && after ? PyObject_CallMethod(plane, "replace", "KOO", SLOT(ctx), before, after) : NULL;
    NTSTATUS s = !r ? failure() : r == Py_None ? STATUS_SUCCESS : STATUS_DATA_ERROR;
    if (s != STATUS_SUCCESS) {
        /* A Python interruption after the pointer change is not a successful native reply. */
        PyObject *rolled = PyObject_CallMethod(plane, "failed_replace_reply", "K", SLOT(ctx));
        if (rolled != Py_None) poison(plane);
        Py_XDECREF(rolled);
    }
    Py_XDECREF(r); Py_XDECREF(before); Py_XDECREF(after); PyGILState_Release(g);
    return s;
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
    const FSP_FSCTL_TRANSACT_REQ *req = real_request(fs);
    /* Only during the original, admitted native operation, not a general DELETE/ADD_FILE grant.
     * WinFsp's synthetic destination check preserves the real dispatch TLS request. */
    if (kind == 1 && req && req->Kind == FspFsctlTransactCreateKind &&
        (req->Req.Create.OpenTargetDirectory || (req->Req.Create.CreateOptions >> 24) == FILE_CREATE)) kind = 5;
    if (kind == 2 && plain_replace(req)) {
        PCWSTR dest = request_path(req, req->Req.SetInformation.Info.Rename.NewFileName);
        if (dest && !wcscmp(dest, path)) kind = 4;
    }
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
    __declspec(align(8)) BYTE packed[1280] = {0};
    ULONG used = 0, complete = 0;
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *m = marker ? PyUnicode_FromWideChar(marker, (Py_ssize_t)n) : Py_NewRef(Py_None);
    PyObject *r = m ? PyObject_CallMethod(plane, "directory", "KO", SLOT(ctx), m) : NULL;
    NTSTATUS s = STATUS_SUCCESS;
    if (!r) s = failure();
    else if (!PyTuple_CheckExact(r) || PyTuple_GET_SIZE(r) > 2) s = STATUS_DATA_ERROR;
    else for (Py_ssize_t i = 0; i < PyTuple_GET_SIZE(r); ++i) {
        __declspec(align(8)) BYTE record[sizeof(FSP_FSCTL_DIR_INFO) + 512] = {0};
        FSP_FSCTL_DIR_INFO *di = (FSP_FSCTL_DIR_INFO *)record;
        PyObject *row = PyTuple_GET_ITEM(r, i);
        if (!PyTuple_CheckExact(row) || PyTuple_GET_SIZE(row) != 2 ||
            !PyUnicode_CheckExact(PyTuple_GET_ITEM(row, 0))) { s = STATUS_DATA_ERROR; break; }
        s = file_info(PyTuple_GET_ITEM(row, 1), &di->FileInfo);
        if (s != STATUS_SUCCESS) break;
        Py_ssize_t units = PyUnicode_AsWideChar(PyTuple_GET_ITEM(row, 0), di->FileNameBuf, 256);
        if (units < 0) { s = failure(); break; }
        if (!units || units > 255) { s = STATUS_DATA_ERROR; break; }
        di->Size = (UINT16)(sizeof *di + units * sizeof(WCHAR));
        if (!FspFileSystemAddDirInfo(di, packed, sizeof packed, &used)) { s = STATUS_DATA_ERROR; break; }
        if (used <= cap) complete = used;
    }
    if (s == STATUS_SUCCESS) {
        if (!FspFileSystemAddDirInfo(NULL, packed, sizeof packed, &used)) s = STATUS_DATA_ERROR;
        else if (used > cap && !complete) s = STATUS_BUFFER_TOO_SMALL;
        else {
            *copied = used <= cap ? used : complete; /* No EOF after a partial page. */
            memcpy(out, packed, *copied);
        }
    }
    Py_XDECREF(r); Py_XDECREF(m); PyGILState_Release(g); return s;
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
    else for (Py_ssize_t i = 0, index = 0; i < PyTuple_GET_SIZE(rows); i++) {
        FSP_FSCTL_FILE_INFO fi;
        PyObject *row = PyTuple_GET_ITEM(rows, i);
        s = file_info(row, &fi);
        if (s != STATUS_SUCCESS) break;
        long kind = PyLong_AsLong(PyTuple_GET_ITEM(row, 5));
        if (i ? (kind != 3 || fi.IndexNumber != (UINT64)index) : (kind != 2 && kind != 4))
            { s = STATUS_DATA_ERROR; break; }
        index = (Py_ssize_t)fi.IndexNumber;
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

void ZwDraftPrepareNamespace(FSP_FSCTL_VOLUME_PARAMS *vp, FSP_FILE_SYSTEM_INTERFACE *ops, BOOL replacement)
{
    ZwDraftFileInterface(ops); memset(vp, 0, sizeof *vp);
    vp->Version = sizeof *vp; vp->SectorSize = 512; vp->SectorsPerAllocationUnit = 8;
    vp->MaxComponentLength = 255; vp->CaseSensitiveSearch = vp->CasePreservedNames = vp->UnicodeOnDisk = 1;
    vp->UmFileContextIsUserContext2 = vp->PersistentAcls = vp->NamedStreams = 1;
    vp->FlushAndPurgeOnCleanup = 1; /* Cache preference, never detach evidence. */
    vp->FileInfoTimeout = 0; vp->AllowOpenInKernelMode = 0;
    ops->GetVolumeInfo = scoped_capacity;
    ops->GetSecurityByName = scoped_name; ops->GetSecurity = scoped_security;
    ops->ReadDirectory = scoped_directory; ops->GetStreamInfo = scoped_streams;
    if (replacement) { ops->Create = scoped_create; ops->Rename = scoped_rename; }
}

void ZwDraftNamespaceInterface(FSP_FSCTL_VOLUME_PARAMS *vp, FSP_FILE_SYSTEM_INTERFACE *ops)
{
    ZwDraftPrepareNamespace(vp, ops, FALSE);
}
