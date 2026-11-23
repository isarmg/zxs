/* Private WinFsp 2.1 x64 file callback table. No volume parameters, mountpoint, service, or installer. */
#pragma once
#include <winfsp/winfsp.h>
#include <Python.h>

/* Native ordinary-logon view: root + one exact alias + verified read-only provenance stream.
 * No host, mounted volume, edit completion or normal detach is implied by building an interface. */
void ZwDraftNamespaceInterface(FSP_FSCTL_VOLUME_PARAMS *params, FSP_FILE_SYSTEM_INTERFACE *ops);
int ZwDraftBindNamespace(FSP_FILE_SYSTEM *fs, PyObject *plane);
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

/* Private owner operations only. Move to actual SDK fs prior to dispatch, home after complete join. */
int ZwDraftMoveBinding(FSP_FILE_SYSTEM *from, FSP_FILE_SYSTEM *to, BOOL attach);
int ZwDraftPresent(FSP_FILE_SYSTEM *fs);
int ZwDraftFenceIfIdle(FSP_FILE_SYSTEM *fs, ULONG *remaining); /* -1 fault, 0 still busy, 1 fenced. */
int ZwDraftAcceptDeleted(FSP_FILE_SYSTEM *keeper);
int ZwDraftConsumeCertificate(PyObject *proof, PyObject *plane, PyObject *lease);
int ZwDraftRememberLease(FSP_FILE_SYSTEM *fs, PyObject *choice);
void ZwDraftDropAfterJoin(FSP_FILE_SYSTEM *keeper);
