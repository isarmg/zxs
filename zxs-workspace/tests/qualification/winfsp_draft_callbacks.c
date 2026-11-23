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
static PyMethodDef methods[] = {{"check", check, METH_O, NULL}, {NULL, NULL, 0, NULL}};
static struct PyModuleDef module = {PyModuleDef_HEAD_INIT, "_workspace_winfsp_draft_io", NULL, -1, methods};
PyMODINIT_FUNC PyInit__workspace_winfsp_draft_io(void) { return PyModule_Create(&module); }
