/* SPDX-License-Identifier: GPL-3.0-or-later
 * Internal to the fixed private smbd. This is stack routing, not a grant. */
#ifndef ZXS_FILE_VFS_H
#define ZXS_FILE_VFS_H
#if SMB_VFS_INTERFACE_VERSION != 50
#error Re-audit the complete terminal table for this Samba ABI
#endif
enum zxs_vfs_route { ZXS_VFS_REJECT, ZXS_VFS_IPC, ZXS_VFS_FILE };
static inline enum zxs_vfs_route zxs_file_vfs_route(
	int snum, int ipc, bool request_ipc, bool printer, bool wide,
	const char *const *objects)
{
	if (snum < 0 || ipc < 0 || printer || wide ||
	    ((snum == ipc) != request_ipc)) {
		return ZXS_VFS_REJECT;
	}
	if (request_ipc) {
		return (objects == NULL || objects[0] == NULL) ?
			ZXS_VFS_IPC : ZXS_VFS_REJECT;
	}
	return (objects != NULL && objects[0] != NULL &&
		strcmp(objects[0], "zxs_file") == 0 && objects[1] == NULL) ?
		ZXS_VFS_FILE : ZXS_VFS_REJECT;
}
bool zxs_file_vfs_install(struct connection_struct *conn);
#endif
