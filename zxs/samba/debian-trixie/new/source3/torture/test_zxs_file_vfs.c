/* SPDX-License-Identifier: GPL-3.0-or-later
 * Source-only fixture. No Samba daemon, user credentials or actual share.
 * Call the native dispatcher used by smbd, not a duplicate deny predicate. */
#include "includes.h"
#include "smbd/smbd.h"
#include "smbd/fd_handle.h"
#include "param/loadparm.h"
#include "lib/param/param.h"
#include "smbd/zxs_file_vfs.h"

/* Use the native globals and parameter setter, not a copy of the
 * documented default. No config file, data share or listener is used. */
static bool ipc_only_pipe_policy(void)
{
	struct loadparm_context *lp = loadparm_init_s3(NULL, loadparm_s3_helpers());
	bool ok = false;
	if (lp == NULL) {
		return false;
	}
	if (!lp_nt_pipe_support() ||
	    zxs_file_vfs_route(1, 1, true, false, false, NULL, lp_nt_pipe_support()) != ZXS_VFS_REJECT ||
	    !lp_do_parameter(-1, "nt pipe support", "yes") || !lp_nt_pipe_support() ||
	    zxs_file_vfs_route(1, 1, true, false, false, NULL, lp_nt_pipe_support()) != ZXS_VFS_REJECT ||
	    !lp_do_parameter(-1, "nt pipe support", "no") || lp_nt_pipe_support() ||
	    zxs_file_vfs_route(1, 1, true, false, false, NULL, lp_nt_pipe_support()) != ZXS_VFS_IPC) {
		goto done;
	}
	ok = true;
done:
	TALLOC_FREE(lp);
	return ok;
}

int main(void)
{
	const char *file[] = {"zxs_file", NULL};
	const char *extra[] = {"zxs_file", "shadow_copy2", NULL};
	const char *path[] = {"/tmp/zxs_file.so", NULL};
	connection_struct *conn = talloc_zero(NULL, connection_struct);
	files_struct fsp = {.conn = conn};
	SMB_STRUCT_STAT untouched = {.st_ex_size = 42};
	uint8_t marker = 0x35, id[16];
	const char lower[] = "zxs-0102030405060708090a0b0c0d0e0fff";
	const char upper[] = "ZxS-0102030405060708090A0B0C0D0E0Fff";
	if (!zxs_smb_service_id((const uint8_t *)upper, sizeof(upper)-1, id) ||
	    id[0] != 1 || id[14] != 15 || id[15] != 255 ||
	    !zxs_smb_direct_service(lower, 0, 0, 0) ||
	    zxs_smb_direct_service(upper, 0, 0, 0) ||
	    zxs_smb_direct_service(lower, 1, 0, 0) ||
	    zxs_smb_direct_service(lower, 0, 1, 0) ||
	    zxs_smb_direct_service(lower, 0, 0, 1) ||
	    zxs_smb_direct_service("zxs-00000000000000000000000000000000", 0, 0, 0) ||
	    zxs_smb_direct_service("IPC$", 0, 0, 0)) {
		return 1;
	}
	if (!ipc_only_pipe_policy() || conn == NULL ||
	    zxs_file_vfs_route(3,1,false,false,false,file,false) != ZXS_VFS_FILE ||
	    zxs_file_vfs_route(1,1,true,false,false,NULL,false) != ZXS_VFS_IPC ||
	    zxs_file_vfs_route(3,1,true,false,false,NULL,false) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(1,1,false,false,false,file,false) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(1,1,true,false,false,file,false) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,false,false,NULL,false) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,false,false,extra,false) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,false,false,path,false) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,false,true,file,false) != ZXS_VFS_REJECT ||
	    zxs_file_vfs_route(3,1,false,true,false,file,false) != ZXS_VFS_REJECT) {
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
	/* Even a zero-length read on the installed business stack goes to
	 * its real PREAD. The unbound terminal denies instead of reporting a
	 * fabricated success. No native disk fd is provided to the FSP. */
	fsp.fh = fd_handle_create(conn);
	if (fsp.fh == NULL ||
	    smb_vfs_call_strict_lock_check(conn->vfs_handles, &fsp, NULL) ||
	    errno != ENOSYS ||
	    read_file(&fsp, NULL, 17, 0) != -1 ||
	    errno != ENOSYS ||
	    read_file(&fsp, (char *)&marker, 17, 1) != -1 ||
	    errno != ENOSYS || marker != 0x35) {
		return 1;
	}
	TALLOC_FREE(conn);
	return 0;
}
