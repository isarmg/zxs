/* Optional x64 test extension; NO volume creation, driver dispatch, mounting or installation.
 * Link this translation unit with winfsp/provider.c and the trusted SDK, not into the application.
 * check() accepts the already-held, fully admitted synthetic tree from test_windows_snapshot.py. */
#include "../../winfsp/provider.h"
#include <string.h>

#define REQUIRE(x) do { if (!(x)) { PyErr_Format(PyExc_AssertionError, \
    "WinFsp callback contract did not hold at source line %d", __LINE__); goto end; } } while (0)

static PyObject *check(PyObject *module, PyObject *source)
{
    FSP_FILE_SYSTEM fs = {0};
    FSP_FILE_SYSTEM_INTERFACE ops;
    FSP_FSCTL_VOLUME_PARAMS params;
    FSP_FSCTL_FILE_INFO fi;
    FSP_FSCTL_VOLUME_INFO vi;
    PVOID root = NULL, folder = NULL, file = NULL, meta = NULL;
    __declspec(align(8)) BYTE buffer[2048] = {0};
    ULONG used = 0;
    const UINT32 open = FILE_OPEN << 24;
    const WCHAR last[] = {0xd840, 0xdc00, L'.', L't', L'x', L't', 0};
    PyObject *result = NULL;
    ZwSnapshotDataInterface(&params, &ops);
    REQUIRE(params.ReadOnlyVolume && params.CaseSensitiveSearch && params.UmFileContextIsUserContext2);
    REQUIRE(ops.Open && ops.Create && ops.Overwrite && !ops.Rename && !ops.SetDelete && !ops.SetSecurity);
    PVOID refused = (PVOID)(UINT_PTR)1;
    REQUIRE(ops.Create(&fs, L"\\new", FILE_CREATE << 24, FILE_WRITE_DATA, 0, NULL, 0,
                       &refused, &fi) == STATUS_MEDIA_WRITE_PROTECTED && !refused);
    REQUIRE(ZwSnapshotBind(&fs, source));
    REQUIRE(!ZwSnapshotBind(&fs, source));
    REQUIRE(ops.GetSecurityByName(&fs, L"\\", NULL, NULL, NULL) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.GetVolumeInfo(&fs, &vi) == STATUS_SUCCESS && vi.TotalSize == 1048582 && !vi.FreeSize);
    REQUIRE(ops.Open(&fs, L"\\", open | FILE_DIRECTORY_FILE, FILE_LIST_DIRECTORY | FILE_TRAVERSE,
                     &root, &fi) == STATUS_SUCCESS && fi.IndexNumber == 1);
    REQUIRE(ops.Open(&fs, L"\\folder", open, FILE_LIST_DIRECTORY, &folder, &fi) == STATUS_SUCCESS);
    REQUIRE(ops.Open(&fs, L"\\folder\\sample.bin", open, FILE_EXECUTE, &file, &fi) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.Open(&fs, L"\\folder\\sample.bin", open | FILE_DELETE_ON_CLOSE, FILE_READ_DATA,
                     &file, &fi) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.Open(&fs, L"\\folder\\sample.bin", open, FILE_WRITE_DATA, &file, &fi) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.Open(&fs, L"\\folder\\sample.bin", open, FILE_READ_ATTRIBUTES, &meta, &fi) == STATUS_SUCCESS);
    REQUIRE(ops.Read(&fs, meta, buffer, 0, 1, &used) == STATUS_ACCESS_DENIED && !used);
    REQUIRE(ops.Open(&fs, L"\\folder\\sample.bin", open, FILE_READ_DATA, &file, &fi) == STATUS_SUCCESS);
    REQUIRE(ops.GetFileInfo(&fs, file, &fi) == STATUS_SUCCESS && fi.FileSize == 1048578);
    REQUIRE(ops.Overwrite(&fs, file, FILE_ATTRIBUTE_NORMAL, TRUE, 0, &fi) == STATUS_MEDIA_WRITE_PROTECTED);
    REQUIRE(ops.Read(&fs, file, buffer, 1048575, 8, &used) == STATUS_SUCCESS && used == 3 &&
            !memcmp(buffer, "ABC", 3));
    REQUIRE(ops.Read(&fs, file, buffer, 1048578, 1, &used) == STATUS_END_OF_FILE && !used);
    REQUIRE(ops.Write(&fs, file, buffer, 0, 1, FALSE, FALSE, &used, &fi) == STATUS_MEDIA_WRITE_PROTECTED && !used);
    REQUIRE(ops.ReadDirectory(&fs, folder, NULL, NULL, buffer, 2, &used) == STATUS_BUFFER_TOO_SMALL && !used);
    REQUIRE(ops.ReadDirectory(&fs, folder, NULL, L"sample.bin", buffer, sizeof buffer, &used) == STATUS_SUCCESS);
    FSP_FSCTL_DIR_INFO *di = (FSP_FSCTL_DIR_INFO *)buffer;
    REQUIRE(di->Size == sizeof *di + sizeof last - sizeof(WCHAR) &&
            !memcmp(di->FileNameBuf, last, sizeof last - sizeof(WCHAR)));
    REQUIRE(used == FSP_FSCTL_DEFAULT_ALIGN_UP(di->Size) + sizeof(UINT16) &&
            *(UINT16 *)(buffer + used - sizeof(UINT16)) == 0);
    REQUIRE(ops.ReadDirectory(&fs, folder, NULL, (PWSTR)last, buffer, 2, &used) == STATUS_SUCCESS &&
            used == 2 && !*(UINT16 *)buffer);
    result = Py_NewRef(Py_None);
end:
    ZwSnapshotUnbind(&fs);
    return result;
}

static BOOL grants(PSECURITY_DESCRIPTOR descriptor, HANDLE client, DWORD access, BOOL allowed)
{
    GENERIC_MAPPING mapping = {FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_GENERIC_EXECUTE, FILE_ALL_ACCESS};
    __declspec(align(8)) BYTE enough[256];
    DWORD bytes = sizeof enough, got = 0;
    BOOL answer = !allowed;
    return AccessCheck(descriptor, client, access, &mapping, (PPRIVILEGE_SET)enough, &bytes, &got, &answer)
        && answer == allowed && (!allowed || (got & access) == access);
}

/* Exercise the exact native callbacks against a real Windows token, without creating a volume. */
static PyObject *check_session(PyObject *module, PyObject *source)
{
    FSP_FILE_SYSTEM fs = {0};
    FSP_FILE_SYSTEM_INTERFACE ops;
    FSP_FSCTL_VOLUME_PARAMS params;
    FSP_FSCTL_FILE_INFO info;
    HANDLE primary = NULL, client = NULL, restricted = NULL, other = NULL;
    __declspec(align(8)) BYTE sd[256], dir[256], sid[SECURITY_MAX_SID_SIZE];
    SIZE_T size = 0, directory_size = sizeof dir;
    DWORD n = sizeof sid;
    UINT32 attrs = 0;
    PVOID context = NULL;
    FSP_FSCTL_TRANSACT_REQ request = {0};
    FSP_FSCTL_TRANSACT_RSP response = {0};
    PyObject *result = NULL;
    ZwSnapshotSessionInterface(&params, &ops);
    fs.Interface = &ops;
    InitializeSRWLock(&fs.OpGuardLock);
    FspFileSystemSetOperationGuardStrategy(&fs, FSP_FILE_SYSTEM_OPERATION_GUARD_STRATEGY_FINE);
    REQUIRE(params.ReadOnlyVolume && params.PersistentAcls && !params.AllowOpenInKernelMode);
    REQUIRE(!ops.SetSecurity && !ops.Rename);
    REQUIRE(ops.GetSecurityByName(&fs, L"\\", NULL, NULL, NULL) == STATUS_ACCESS_DENIED);
    REQUIRE(ZwSnapshotBindSession(&fs, source));
    REQUIRE(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &primary));
    REQUIRE(DuplicateToken(primary, SecurityImpersonation, &client));
    REQUIRE(CreateWellKnownSid(WinNullSid, NULL, sid, &n));
    SID_AND_ATTRIBUTES restrict_to = {sid, 0};
    REQUIRE(CreateRestrictedToken(primary, 0, 0, NULL, 0, NULL, 1, &restrict_to, &restricted));
    REQUIRE(DuplicateToken(restricted, SecurityImpersonation, &other));

    REQUIRE(ops.GetSecurityByName(&fs, L"\\folder\\sample.bin", &attrs, NULL, &size) == STATUS_BUFFER_OVERFLOW);
    REQUIRE(size > 0 && size <= sizeof sd && !(attrs & FILE_ATTRIBUTE_DIRECTORY));
    size = sizeof sd;
    REQUIRE(ops.GetSecurityByName(&fs, L"\\folder\\sample.bin", &attrs, sd, &size) == STATUS_SUCCESS);
    REQUIRE(IsValidSecurityDescriptor(sd));
    REQUIRE(grants(sd, client, FILE_READ_DATA, TRUE));
    REQUIRE(grants(sd, client, WRITE_DAC, FALSE) && grants(sd, client, FILE_EXECUTE, FALSE));
    REQUIRE(grants(sd, client, FILE_WRITE_DATA, FALSE) && grants(sd, other, FILE_READ_DATA, FALSE));
    REQUIRE(ops.GetSecurityByName(&fs, L"\\", &attrs, dir, &directory_size) == STATUS_SUCCESS &&
            (attrs & FILE_ATTRIBUTE_DIRECTORY) && grants(dir, client, FILE_TRAVERSE, TRUE));
    REQUIRE(ops.GetSecurityByName(&fs, L"\\folder\\missing", &attrs, dir, &directory_size) == STATUS_OBJECT_NAME_NOT_FOUND);
    REQUIRE(ops.Open(&fs, L"\\folder\\sample.bin", FILE_OPEN << 24, FILE_READ_DATA, &context, &info) == STATUS_SUCCESS);
    SIZE_T again = sizeof dir;
    REQUIRE(ops.GetSecurity(&fs, context, dir, &again) == STATUS_SUCCESS && again == size && !memcmp(sd, dir, size));

    request.Kind = FspFsctlTransactCreateKind;
    request.Req.Create.CreateOptions = FILE_OPEN << 24;
    request.Req.Create.UserMode = 1;
    request.Req.Create.AccessToken = (UINT_PTR)client;
    REQUIRE(FspFileSystemEnterOperation(&fs, &request, &response) == STATUS_SUCCESS);
    REQUIRE(FspFileSystemLeaveOperation(&fs, &request, &response) == STATUS_SUCCESS);
    request.Req.Create.UserMode = 0;
    REQUIRE(FspFileSystemEnterOperation(&fs, &request, &response) == STATUS_ACCESS_DENIED);
    request.Req.Create.UserMode = 1;
    request.Req.Create.AccessToken = (UINT_PTR)other;
    REQUIRE(FspFileSystemEnterOperation(&fs, &request, &response) == STATUS_ACCESS_DENIED);
    request.Req.Create.AccessToken = (UINT_PTR)client;
    request.Req.Create.CreateOptions |= FILE_DELETE_ON_CLOSE;
    REQUIRE(FspFileSystemEnterOperation(&fs, &request, &response) == STATUS_ACCESS_DENIED);
    result = Py_NewRef(Py_None);
end:
    if (other) CloseHandle(other);
    if (restricted) CloseHandle(restricted);
    if (client) CloseHandle(client);
    if (primary) CloseHandle(primary);
    ZwSnapshotUnbind(&fs);
    return result;
}

static PyMethodDef methods[] = {
    {"check", check, METH_O, "Check actual SDK callbacks; do not mount."},
    {"check_session", check_session, METH_O, "Check a current-session descriptor and SDK guard; do not mount."},
    {NULL}
};
static struct PyModuleDef module = {PyModuleDef_HEAD_INIT, "_test_winfsp_callbacks", NULL, -1, methods};
PyMODINIT_FUNC PyInit__test_winfsp_callbacks(void) { return PyModule_Create(&module); }
