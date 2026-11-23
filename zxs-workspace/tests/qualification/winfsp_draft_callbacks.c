/* Opt-in x64 CPython extension with the actual WinFsp interface. Link draft_io.c and the trusted
 * WinFsp 2.1 SDK. No volume creation, dispatch start, mountpoint, service or GO_ONLINE operation. */
#include "../../winfsp/draft_io.h"
#include <stdint.h>
#include <string.h>

#define REQUIRE(x) do { if (!(x)) { if (!PyErr_Occurred()) PyErr_Format(PyExc_AssertionError, \
    "Windows draft callback qualification failed at line %d", __LINE__); goto out; } } while (0)

static PyObject *check(PyObject *module, PyObject *plane)
{
    FSP_FILE_SYSTEM fs = {0}, competitor = {0};
    FSP_FILE_SYSTEM_INTERFACE ops;
    FSP_FSCTL_FILE_INFO fi;
    FSP_FSCTL_TRANSACT_REQ req = {0};
    FSP_FSCTL_TRANSACT_RSP rsp = {0};
    PVOID r = NULL, w = NULL, a = NULL, refused = NULL;
    ULONG transferred = 99, count = 99;
    BYTE bytes[16] = {0};
    BOOL bound = FALSE;
    PyObject *result = NULL, *exception = NULL;
    const UINT32 existing = FILE_OPEN << 24;
    const PWSTR file = L"\\review.bin";
    (void)module;
    ZwDraftFileInterface(&ops);
    fs.Interface = &ops;
    REQUIRE(ops.Open && ops.Close && ops.Write && ops.Read && ops.SetFileSize && !ops.Rename && !ops.GetStreamInfo);
    REQUIRE(ZwDraftBindFilePlane(&fs, plane)); bound = TRUE;
    REQUIRE(!ZwDraftBindFilePlane(&fs, plane));
    req.Kind = FspFsctlTransactCreateKind;
    REQUIRE(FspFileSystemEnterOperation(&fs, &req, &rsp) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.GetSecurityByName(&fs, file, NULL, NULL, NULL) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.Open(&fs, L"\\Review.bin", existing, FILE_READ_DATA, &refused, &fi)
            == STATUS_OBJECT_NAME_NOT_FOUND && !refused);
    REQUIRE(ops.Open(&fs, L"\\review.bin:Zone.Identifier", existing, FILE_READ_DATA, &refused, &fi)
            == STATUS_OBJECT_NAME_NOT_FOUND && !refused);
    REQUIRE(ops.Open(&fs, file, existing, FILE_EXECUTE, &refused, &fi) == STATUS_ACCESS_DENIED && !refused);
    REQUIRE(ops.Open(&fs, file, existing | FILE_DELETE_ON_CLOSE, FILE_READ_DATA, &refused, &fi)
            == STATUS_ACCESS_DENIED && !refused);
    REQUIRE(ops.Open(&fs, file, existing, FILE_READ_DATA, &r, &fi) == STATUS_SUCCESS && r);
    REQUIRE(fi.FileSize == 4 && fi.IndexNumber == 2 && fi.CreationTime && fi.ChangeTime);
    competitor.Interface = &ops;
    REQUIRE(!ZwDraftBindFilePlane(&competitor, plane)); PyErr_Clear();
    REQUIRE(!competitor.UserContext && !ZwDraftStopNewOpens(&competitor, &count));
    REQUIRE(ops.GetFileInfo(&competitor, r, &fi) == STATUS_INVALID_HANDLE);
    REQUIRE(ops.GetFileInfo(&fs, r, &fi) == STATUS_SUCCESS && fi.FileSize == 4);
    REQUIRE(ops.Open(&fs, file, existing, FILE_WRITE_DATA, &w, &fi) == STATUS_SUCCESS && w && r != w);
    REQUIRE(ops.Open(&fs, file, existing, FILE_APPEND_DATA, &a, &fi) == STATUS_SUCCESS && a && a != w && a != r);
    REQUIRE(ops.Write(&fs, r, (PVOID)"X", 0, 1, FALSE, FALSE, &transferred, &fi) == STATUS_ACCESS_DENIED && !transferred);
    REQUIRE(ops.Read(&fs, w, bytes, 0, 1, &transferred) == STATUS_ACCESS_DENIED && !transferred);
    REQUIRE(ops.Write(&fs, a, (PVOID)"X", 0, 1, FALSE, FALSE, &transferred, &fi) == STATUS_ACCESS_DENIED && !transferred);
    REQUIRE(ops.SetFileSize(&fs, a, 1, FALSE, &fi) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.SetFileSize(&fs, w, 2, TRUE, &fi) == STATUS_NOT_SUPPORTED);
    REQUIRE(ops.Read(&fs, r, bytes, 4, 1, &transferred) == STATUS_END_OF_FILE && !transferred);
    REQUIRE(ops.Write(&fs, w, (PVOID)"XY", 1, 2, FALSE, FALSE, &transferred, &fi) == STATUS_SUCCESS &&
            transferred == 2 && fi.FileSize == 4);
    REQUIRE(ops.Write(&fs, a, (PVOID)"?", UINT64_MAX, 1, TRUE, FALSE, &transferred, &fi) == STATUS_SUCCESS &&
            transferred == 1 && fi.FileSize == 5);
    REQUIRE(ops.Write(&fs, w, (PVOID)"ZLAT", 4, 4, FALSE, TRUE, &transferred, &fi) == STATUS_SUCCESS &&
            transferred == 1 && fi.FileSize == 5);
    REQUIRE(ops.Read(&fs, r, bytes, 0, sizeof bytes, &transferred) == STATUS_SUCCESS &&
            transferred == 5 && !memcmp(bytes, "sXYdZ", 5));
    REQUIRE(ops.SetFileSize(&fs, w, 3, FALSE, &fi) == STATUS_SUCCESS && fi.FileSize == 3);
    REQUIRE(ops.Flush(&fs, NULL, &fi) == STATUS_NOT_SUPPORTED);
    REQUIRE(ops.Flush(&fs, r, &fi) == STATUS_SUCCESS && fi.FileSize == 3);
    REQUIRE(ZwDraftStopNewOpens(&fs, &count) && count == 3);
    REQUIRE(ops.Open(&fs, file, existing, FILE_READ_DATA, &refused, &fi) == STATUS_ACCESS_DENIED && !refused);
    ops.Cleanup(&fs, w, NULL, 0);
    REQUIRE(ops.Write(&fs, w, (PVOID)"Q", 2, 1, FALSE, TRUE, &transferred, &fi) == STATUS_SUCCESS &&
            transferred == 1 && fi.FileSize == 3);
    REQUIRE(ops.Write(&fs, w, (PVOID)"!", 3, 1, FALSE, TRUE, &transferred, &fi) == STATUS_SUCCESS &&
            transferred == 0 && fi.FileSize == 3);
    ops.Close(&fs, a);
    REQUIRE(ops.GetFileInfo(&fs, a, &fi) == STATUS_ACCESS_DENIED);
    ops.Close(&fs, w); ops.Close(&fs, r);
    REQUIRE(ZwDraftStopNewOpens(&fs, &count) && count == 0);
    result = Py_NewRef(Py_None);
out:
    exception = PyErr_GetRaisedException();
    if (competitor.UserContext && !ZwDraftAbandonFilePlane(&competitor)) Py_CLEAR(result);
    if ((bound || fs.UserContext) && !ZwDraftAbandonFilePlane(&fs)) {
        Py_CLEAR(result);
        if (!PyErr_Occurred()) PyErr_SetString(PyExc_RuntimeError, "could not release original file plane");
    }
    if (exception) PyErr_SetRaisedException(exception);
    return result;
}
static BOOL grants(PSECURITY_DESCRIPTOR sd, HANDLE client, DWORD right, BOOL accepted)
{
    GENERIC_MAPPING map = {FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_GENERIC_EXECUTE, FILE_ALL_ACCESS};
    __declspec(align(8)) BYTE privileges[256];
    DWORD n = sizeof privileges, allowed = 0; BOOL answer = !accepted;
    return AccessCheck(sd, client, right, &map, (PPRIVILEGE_SET)privileges, &n, &allowed, &answer) &&
           answer == accepted && (!accepted || (allowed & right) == right);
}

/* Real SDK guards and record writers on a synthetic in-process filesystem only. */
static PyObject *check_namespace(PyObject *module, PyObject *args)
{
    PyObject *plane, *expected, *result = NULL, *exception;
    if (!PyArg_ParseTuple(args, "OO", &plane, &expected)) return NULL;
    if (!PyBytes_CheckExact(expected) || PyBytes_GET_SIZE(expected) <= 0 || PyBytes_GET_SIZE(expected) >= 2048)
        return PyErr_Format(PyExc_ValueError, "expected the original bounded fixture origin stream");
    FSP_FILE_SYSTEM fs = {0};
    FSP_FILE_SYSTEM_INTERFACE ops;
    FSP_FSCTL_VOLUME_PARAMS vp;
    FSP_FSCTL_FILE_INFO fi;
    FSP_FSCTL_TRANSACT_REQ req = {0};
    FSP_FSCTL_TRANSACT_RSP rsp = {0};
    PVOID root = NULL, file = NULL, mark = NULL, refused = NULL;
    HANDLE token = NULL, client = NULL;
    __declspec(align(8)) BYTE b[2048] = {0}, sd[256] = {0};
    SIZE_T sdbytes = sizeof sd, again = sizeof b;
    ULONG used = 0, count = 0;
    const UINT32 existing = FILE_OPEN << 24;
    const WCHAR alias[] = L"review.bin";
    const PWSTR data = L"\\review.bin", zone = L"\\review.bin:Zone.Identifier";
    const WCHAR stream[] = L"Zone.Identifier";
    const WCHAR *other = L"\\review.bin:secret";
    (void)module;
    ZwDraftNamespaceInterface(&vp, &ops);
    REQUIRE(vp.NamedStreams && vp.PersistentAcls && !vp.FileInfoTimeout && !vp.ReadOnlyVolume &&
            !vp.AllowOpenInKernelMode && !vp.HardLinks && !vp.ReparsePoints);
    fs.Interface = &ops; InitializeSRWLock(&fs.OpGuardLock);
    fs.UmFileContextIsUserContext2 = vp.UmFileContextIsUserContext2;
    REQUIRE(!ZwDraftBindFilePlane(&fs, plane));
    REQUIRE(ZwDraftBindNamespace(&fs, plane));
    REQUIRE(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &token));
    REQUIRE(DuplicateToken(token, SecurityImpersonation, &client));
    req.Kind = FspFsctlTransactCreateKind;
    req.Req.Create.AccessToken = (UINT_PTR)client; req.Req.Create.CreateOptions = existing;
    REQUIRE(FspFileSystemEnterOperation(&fs, &req, &rsp) == STATUS_ACCESS_DENIED);
    req.Req.Create.UserMode = 1; req.Req.Create.OpenTargetDirectory = 1;
    REQUIRE(FspFileSystemEnterOperation(&fs, &req, &rsp) == STATUS_ACCESS_DENIED);
    req.Req.Create.OpenTargetDirectory = 0;
    req.Req.Create.HasBackupPrivilege = 1;
    req.Req.Create.CreateOptions = existing | FILE_DIRECTORY_FILE | FILE_OPEN_FOR_BACKUP_INTENT;
    REQUIRE(FspFileSystemEnterOperation(&fs, &req, &rsp) == STATUS_ACCESS_DENIED);
    req.Req.Create.HasBackupPrivilege = 0; req.Req.Create.HasRestorePrivilege = 1;
    REQUIRE(FspFileSystemEnterOperation(&fs, &req, &rsp) == STATUS_ACCESS_DENIED);
    req.Req.Create.HasRestorePrivilege = 0;
    REQUIRE(FspFileSystemEnterOperation(&fs, &req, &rsp) == STATUS_SUCCESS);
    REQUIRE(FspFileSystemLeaveOperation(&fs, &req, &rsp) == STATUS_SUCCESS);
    REQUIRE(ops.GetSecurityByName(&fs, L"\\", NULL, NULL, NULL) == STATUS_SUCCESS);
    REQUIRE(ops.GetSecurityByName(&fs, data, NULL, sd, &sdbytes) == STATUS_SUCCESS && IsValidSecurityDescriptor(sd));
    REQUIRE(grants(sd, client, FILE_GENERIC_READ | FILE_GENERIC_WRITE, TRUE) && grants(sd, client, FILE_EXECUTE, FALSE) &&
            grants(sd, client, DELETE, FALSE) && grants(sd, client, WRITE_DAC, FALSE));
    REQUIRE(ops.GetSecurityByName(&fs, (PWSTR)other, NULL, NULL, NULL) == STATUS_OBJECT_NAME_NOT_FOUND);
    REQUIRE(ops.Open(&fs, L"\\", existing | FILE_DIRECTORY_FILE, FILE_GENERIC_READ | FILE_TRAVERSE,
                     &root, &fi) == STATUS_SUCCESS && (fi.FileAttributes & FILE_ATTRIBUTE_DIRECTORY));
    REQUIRE(ops.ReadDirectory(&fs, root, NULL, NULL, b, 2, &used) == STATUS_BUFFER_TOO_SMALL && !used);
    REQUIRE(ops.ReadDirectory(&fs, root, NULL, NULL, b, sizeof b, &used) == STATUS_SUCCESS);
    FSP_FSCTL_DIR_INFO *di = (FSP_FSCTL_DIR_INFO *)b;
    REQUIRE(di->Size == sizeof *di + sizeof alias - sizeof(WCHAR) && di->FileInfo.IndexNumber == 2 &&
            !memcmp(di->FileNameBuf, alias, sizeof alias - sizeof(WCHAR)));
    REQUIRE(ops.ReadDirectory(&fs, root, NULL, (PWSTR)alias, b, 2, &used) == STATUS_SUCCESS && used == 2 && !*(UINT16 *)b);
    REQUIRE(ops.Open(&fs, data, existing, FILE_EXECUTE, &refused, &fi) == STATUS_ACCESS_DENIED && !refused);
    REQUIRE(ops.Open(&fs, data, existing | FILE_OPEN_FOR_BACKUP_INTENT, FILE_GENERIC_READ, &refused, &fi) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.Open(&fs, data, FILE_OVERWRITE_IF << 24, FILE_GENERIC_WRITE, &refused, &fi) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.Open(&fs, data, (FILE_OPEN_IF << 24) | FILE_WRITE_THROUGH, FILE_GENERIC_WRITE, &refused, &fi) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.Open(&fs, data, FILE_OPEN_IF << 24, FILE_GENERIC_READ | FILE_GENERIC_WRITE, &file, &fi) == STATUS_SUCCESS);
    REQUIRE(ops.GetStreamInfo(&fs, file, b, sizeof(FSP_FSCTL_STREAM_INFO), &used) == STATUS_BUFFER_TOO_SMALL && !used);
    REQUIRE(ops.GetStreamInfo(&fs, file, b, sizeof b, &used) == STATUS_SUCCESS);
    FSP_FSCTL_STREAM_INFO *def = (FSP_FSCTL_STREAM_INFO *)b;
    REQUIRE(def->Size == sizeof *def && def->StreamSize == 4);
    FSP_FSCTL_STREAM_INFO *zs = (FSP_FSCTL_STREAM_INFO *)(b + FSP_FSCTL_DEFAULT_ALIGN_UP(def->Size));
    REQUIRE(zs->Size == sizeof *zs + sizeof stream - sizeof(WCHAR) && zs->StreamSize == (UINT64)PyBytes_GET_SIZE(expected) &&
            !memcmp(zs->StreamNameBuf, stream, sizeof stream - sizeof(WCHAR)));
    REQUIRE(ops.Open(&fs, zone, existing, FILE_WRITE_DATA, &refused, &fi) == STATUS_ACCESS_DENIED && !refused);
    REQUIRE(ops.Open(&fs, zone, existing, FILE_GENERIC_READ, &mark, &fi) == STATUS_SUCCESS && fi.IndexNumber == 2);
    REQUIRE(ops.Read(&fs, mark, b, 0, sizeof b, &used) == STATUS_SUCCESS &&
            used == PyBytes_GET_SIZE(expected) && !memcmp(b, PyBytes_AS_STRING(expected), used));
    REQUIRE(ops.GetSecurity(&fs, mark, b, &again) == STATUS_SUCCESS && IsValidSecurityDescriptor(b));
    REQUIRE(grants(b, client, FILE_READ_DATA, TRUE) && grants(b, client, FILE_WRITE_DATA, FALSE) &&
            grants(b, client, FILE_APPEND_DATA, FALSE) && grants(b, client, FILE_EXECUTE, FALSE));
    REQUIRE(ops.Write(&fs, mark, (PVOID)"X", 0, 1, FALSE, FALSE, &used, &fi) == STATUS_ACCESS_DENIED);
    REQUIRE(ops.Write(&fs, file, (PVOID)"W", 0, 1, FALSE, FALSE, &used, &fi) == STATUS_SUCCESS && used == 1);
    REQUIRE(!ops.SetSecurity && !ops.SetDelete && !ops.Rename && !ops.SetEa && !ops.SetBasicInfo);
    REQUIRE(ZwDraftStopNewOpens(&fs, &count) && count == 3);
    ops.Close(&fs, root); ops.Close(&fs, mark); ops.Close(&fs, file);
    REQUIRE(ZwDraftStopNewOpens(&fs, &count) && !count);
    result = Py_NewRef(Py_None);
out:
    exception = PyErr_GetRaisedException();
    if (client) CloseHandle(client);
    if (token) CloseHandle(token);
    if (fs.UserContext && !ZwDraftAbandonFilePlane(&fs)) Py_CLEAR(result);
    if (exception) PyErr_SetRaisedException(exception);
    return result;
}
static PyMethodDef methods[] = {{"check", check, METH_O, NULL},
    {"check_namespace", check_namespace, METH_VARARGS, NULL}, {NULL, NULL, 0, NULL}};
static struct PyModuleDef module = {PyModuleDef_HEAD_INIT, "_workspace_winfsp_draft_io", NULL, -1, methods};
PyMODINIT_FUNC PyInit__workspace_winfsp_draft_io(void) { return PyModule_Create(&module); }
