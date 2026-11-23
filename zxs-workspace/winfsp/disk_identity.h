/* Original WinFsp disk identity through the actual driver control handle. */
#pragma once
#include <winfsp/winfsp.h>
#include <string.h>

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
