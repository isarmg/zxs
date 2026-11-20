/* Internal data-plane interface. Built against the installed WinFsp 2.1 x64 SDK and CPython
 * 3.12+ (GIL-enabled, main interpreter). No loader, mountpoint, service or dispatcher here. */
#pragma once
#include <winfsp/winfsp.h>
#include <Python.h>

void ZwSnapshotDataInterface(FSP_FSCTL_VOLUME_PARAMS *params, FSP_FILE_SYSTEM_INTERFACE *ops);
/* Internal alternate profile for native access qualification, not a mount entry.
 * Requires the actual ordinary current-session token and both descriptor callbacks. */
void ZwSnapshotSessionInterface(FSP_FSCTL_VOLUME_PARAMS *params, FSP_FILE_SYSTEM_INTERFACE *ops);
int ZwSnapshotBindSession(FSP_FILE_SYSTEM *fs, PyObject *selected_volume);
/* Both calls are made with the GIL. Never bind a second object to a live dispatcher. The
 * caller must keep selected_snapshot() entered, stop and join ALL callbacks, then unbind
 * before ending the selection or interpreter. A Python ref is not a substitute for that scope. */
int ZwSnapshotBind(FSP_FILE_SYSTEM *fs, PyObject *selected_volume);
void ZwSnapshotUnbind(FSP_FILE_SYSTEM *fs);
