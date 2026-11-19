/* Internal data-plane interface. Built against the installed WinFsp 2.1 x64 SDK and CPython
 * 3.12+ (GIL-enabled, main interpreter). No loader, mountpoint, service or dispatcher here. */
#pragma once
#include <winfsp/winfsp.h>
#include <Python.h>

void ZwSnapshotDataInterface(FSP_FSCTL_VOLUME_PARAMS *params, FSP_FILE_SYSTEM_INTERFACE *ops);
/* Both calls are made with the GIL. Never bind a second object to a live dispatcher. The
 * caller must keep selected_snapshot() entered, stop and join ALL callbacks, then unbind
 * before ending the selection or interpreter. A Python ref is not a substitute for that scope. */
int ZwSnapshotBind(FSP_FILE_SYSTEM *fs, PyObject *selected_volume);
void ZwSnapshotUnbind(FSP_FILE_SYSTEM *fs);
