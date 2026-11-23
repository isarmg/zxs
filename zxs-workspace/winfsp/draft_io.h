/* Private WinFsp 2.1 x64 file callback table. No volume parameters, mountpoint, service, or installer. */
#pragma once
#include <winfsp/winfsp.h>
#include <Python.h>

void ZwDraftFileInterface(FSP_FILE_SYSTEM_INTERFACE *ops);
/* GIL held. Original one-shot WindowsDraftFilePlane only, before any dispatcher starts.
 * On a failed bind with non-NULL fs->UserContext, do not free fs; finish Abandon after join.
 * For this restricted table all SDK dispatch is denied: direct native qualification only. */
int ZwDraftBindFilePlane(FSP_FILE_SYSTEM *fs, PyObject *plane);
/* GIL held. Stop admission without dropping existing data contexts; count is NOT an OS drain. */
int ZwDraftStopNewOpens(FSP_FILE_SYSTEM *fs, ULONG *remaining);
/* GIL held; MUST already have stopped/joined every native dispatcher/fixture caller.
 * Always abandonment, including zero contexts: never creates an accepted SaveNode cut. */
int ZwDraftAbandonFilePlane(FSP_FILE_SYSTEM *fs);
