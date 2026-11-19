/* SPDX-License-Identifier: GPL-3.0-or-later
 * Source-only fixture. No Samba daemon, user credentials or actual share.
 * Call the native dispatcher used by smbd, not a duplicate deny predicate. */
#include "includes.h"
#include "smbd/zxs_file_vfs.h"

int main(void)
{
	const char *file[] = {"zxs_file", NULL};
	const char *extra[] = {"zxs_file", "shadow_copy2", NULL};
	const char *path[] = {"/tmp/zxs_file.so", NULL};
	connection_struct *conn = talloc_zero(NULL, connection_struct);
	SMB_STRUCT_STAT untouched = {.st_ex_size = 42};
	uint8_t marker = 0x35;
	if (conn == NULL ||
	    zxs_file_vfs_route(3,1,false,false,false,file) != ZXS_VFS_FILE ||
	    zxs_file_vfs_route(1,1,true,false,false,NULL) != ZXS_VFS_IPC ||
	    zxs_file_vfs_route(3,1,true,false,false,NULL) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(1,1,false,false,false,file) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(1,1,true,false,false,file) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,false,false,NULL) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,false,false,extra) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,false,false,path) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,false,true,file) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,true,false,file) != ZXS_VFS_REJECT) {
		return 1;
	}
	if (zxs_file_vfs_active(conn) || !zxs_file_vfs_install(conn) || conn->vfs_handles == NULL ||
	    conn->vfs_handles->next == NULL || conn->vfs_handles->next->next != NULL ||
	    !zxs_file_vfs_active(conn) || zxs_file_vfs_install(conn)) {
		return 1;
	}
	/* Orthogonal to ShareAccess: if both clients allow all sharing,
	 * their write/append opens still conflict. A plain reader takes no
	 * business writer slot in either ordering. Original independent
	 * ShareAccess checks and byte locks still apply. */
	if (!zxs_data_writer_conflict(true, FILE_WRITE_DATA, FILE_APPEND_DATA) ||
	    !zxs_data_writer_conflict(true, FILE_APPEND_DATA, FILE_WRITE_DATA) ||
	    !zxs_data_writer_conflict(true, FILE_READ_DATA|FILE_WRITE_DATA, FILE_WRITE_DATA) ||
	    zxs_data_writer_conflict(true, FILE_WRITE_DATA, FILE_READ_DATA) ||
	    zxs_data_writer_conflict(true, FILE_READ_DATA, FILE_APPEND_DATA) ||
	    zxs_data_writer_conflict(true, FILE_READ_ATTRIBUTES, FILE_READ_DATA) ||
	    zxs_data_writer_conflict(false, FILE_WRITE_DATA, FILE_WRITE_DATA)) {
		return 1;
	}
	if (smb_vfs_call_connect(conn->vfs_handles,"unused","files.smb") != -1 ||
	    errno != EACCES ||
	    smb_vfs_call_fstat(conn->vfs_handles,NULL,&untouched) != -1 ||
	    errno != ENOSYS || untouched.st_ex_size != 42 ||
	    smb_vfs_call_pread(conn->vfs_handles,NULL,&marker,1,0) != -1 ||
	    errno != ENOSYS || marker != 0x35 ||
	    smb_vfs_call_pwrite(conn->vfs_handles,NULL,&marker,1,0) != -1 ||
	    errno != ENOSYS ||
	    smb_vfs_call_unlinkat(conn->vfs_handles,NULL,NULL,0) != -1 ||
	    errno != ENOSYS) {
		return 1;
	}
	TALLOC_FREE(conn);
	return 0;
}
