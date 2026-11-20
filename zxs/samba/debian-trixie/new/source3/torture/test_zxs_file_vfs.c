/* SPDX-License-Identifier: GPL-3.0-or-later
 * Source-only fixture. No Samba daemon, user credentials or actual share.
 * Call the native dispatcher used by smbd, not a duplicate deny predicate. */
#include "includes.h"
#include "smbd/smbd.h"
#include "smbd/globals.h"
#include "../lib/util/tevent_ntstatus.h"
#include "../lib/async_req/async_sock.h"
#include <sys/socket.h>
#include "smbd/fd_handle.h"
#include "param/loadparm.h"
#include "lib/substitute.h"
#include "lib/param/loadparm.h"
#include "lib/param/param.h"
#include "smbd/zxs_file_vfs.h"
#include "smbd/zxs_directory.h"
#include "trans2.h"
#include "librpc/gen_ndr/smbXsrv.h"

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
	/* Only inspect two synthetic substitution contexts: never execute
	 * a script. The accepted effective-profile check uses the raw getter. */
	{
		int snum=lp_add_service("zxs-fixture", -1);
		const struct loadparm_substitution *raw=lpcfg_noop_substitution();
		const struct loadparm_substitution *live=loadparm_s3_global_substitution();
		if (snum<0 || !lp_do_parameter(snum,"magic script","%U")) goto done;
		set_current_user_info("","","");
		if (strcmp(lp_magic_script(lp,raw,snum),"%U")!=0 ||
		    strcmp(lp_magic_script(lp,live,snum),"")!=0) goto done;
		set_current_user_info("sample","zxs-smb-client","ZXSLOCAL");
		if (strcmp(lp_magic_script(lp,raw,snum),"%U")!=0 ||
		    strcmp(lp_magic_script(lp,live,snum),"sample")!=0) goto done;
		set_current_user_info("","","");
	}
	ok = true;
done:
	TALLOC_FREE(lp);
	return ok;
}

/* Fixture supplies a synthetic *already selected* tree/endpoint, not a
 * Samba login or SQL decision. The Rust source separately exercises actual F. */
static bool only_the_attached_virtual_root(connection_struct *conn,const char *service)
{
	int sock[2];struct smbXsrv_tcon *t=talloc_zero(conn,struct smbXsrv_tcon);
	struct zxs_smb_tree_cut *s=talloc_zero(t,struct zxs_smb_tree_cut);
	struct smb_filename name={.base_name=discard_const_p(char,ZXS_SMB_VIRTUAL_ROOT)};
	struct smb_filename bad={.base_name=discard_const_p(char,"/etc")};
	struct smb_filename *rp=NULL;bool ok=false;enum timestamp_set_resolution resolution;
	if (t==NULL||s==NULL || !zxs_smb_service_id((const uint8_t *)service,strlen(service),s->share) ||
	    socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,sock)!=0) {return false;}
	conn->tcon=t;conn->connectpath=talloc_strdup(conn,ZXS_SMB_VIRTUAL_ROOT);t->zxs_tree_choice=s;
	s->root_fd=sock[0];s->volume_ordinal=2;s->pool[7]=17;
	if (conn->connectpath==NULL ||
	    smb_vfs_call_connect(conn->vfs_handles,service,"zxs-smb-client")!=0 ||
	    smb_vfs_call_connect(conn->vfs_handles,service,"files.smb")!=-1 ||
	    smb_vfs_call_realpath(conn->vfs_handles,conn,&bad)!=NULL) {goto done;}
	rp=smb_vfs_call_realpath(conn->vfs_handles,conn,&name);
	if (rp==NULL || strcmp(rp->base_name,ZXS_SMB_VIRTUAL_ROOT)!=0 ||
	    smb_vfs_call_stat(conn->vfs_handles,&name)!=0 || !S_ISDIR(name.st.st_ex_mode) ||
	    name.st.st_ex_dev!=17 || name.st.st_ex_ino!=3 ||
	    smb_vfs_call_fs_capabilities(conn->vfs_handles,&resolution)!=0 || resolution!=TIMESTAMP_SET_SECONDS) {goto done;}
	conn->tcon_done=true;
	if (smb_vfs_call_stat(conn->vfs_handles,&name)!=-1 || errno!=EACCES ||
	    vfs_ChDir(conn,&name)!=0 || vfs_ChDir(conn,&bad)!=-1) {goto done;}
	smb_vfs_call_disconnect(conn->vfs_handles);
	ok=s->root_fd<0 && zxs_smb_ctl_live(sock[1])<0;
done:
	zxs_smb_root_release(s);close(sock[1]);conn->tcon_done=false;conn->tcon=NULL;TALLOC_FREE(t);
	return ok;
}
/* Real native record encoder, including ID_BOTH's 64-bit ID, alignment
 * and UTF-16 name. The terminal VFS would return UINT64_MAX for host
 * allocation; that value must never appear in this logical projection. */
static bool native_directory_projection(connection_struct *c)
{
	struct share_params params={.service=GLOBAL_SECTION_SNUM};
	char bytes[4608]={0},*next=bytes,*save=NULL;
	uint64_t at=123,number=UINT64_C(0xfedcba9823456789);
	NTSTATUS status;
	c->params=&params;
	status=zxs_marshal_child(c,c,SMB_FIND_ID_BOTH_DIRECTORY_INFO,"报告",number,4097,
		8,bytes,&next,bytes+sizeof(bytes)-1,&at);
	if(!NT_STATUS_EQUAL(status,STATUS_MORE_ENTRIES)||next!=bytes||at!=123)goto fail;
	status=zxs_marshal_child(c,c,SMB_FIND_ID_BOTH_DIRECTORY_INFO,"报告",number,4097,
		256,bytes,&next,bytes+sizeof(bytes)-1,&at);
	if(!NT_STATUS_IS_OK(status)||at!=0||BVAL(bytes,40)!=4097||BVAL(bytes,48)!=8192||
	    IVAL(bytes,60)!=4||SVAL(bytes,68)!=0||BVAL(bytes,96)!=number||
	    SVAL(bytes,104)!=0x62a5||SVAL(bytes,106)!=0x544a)goto fail;
	save=next;
	status=zxs_marshal_child(c,c,SMB_FIND_FILE_NAMES_INFO,"b",300,0,
		256,bytes,&next,bytes+sizeof(bytes)-1,&at);
	if(!NT_STATUS_IS_OK(status)||at%8!=0||bytes+at<save||SVAL(bytes+at,12)!='b')goto fail;
	c->params=NULL;return true;
fail:
	c->params=NULL;return false;
}
/* The linked, real waiter is canceled through the same terminal function
 * as root CREATE/QUERY. The outer request survives the CLOSE/TDIS walk;
 * the real FSP's AIO list has already shrunk before its callback runs. */
struct cancel_probe {
	struct tevent_context *ev;
	struct tevent_req *wait;
	struct aio_req_fsp_link *link;
};
struct cancel_seen { unsigned callbacks; bool canceled; };
static bool probe_cancel(struct tevent_req *req)
{
	struct cancel_probe *s=tevent_req_data(req,struct cancel_probe);
	return zxs_dir_wait_cancel(req,s->ev,&s->wait,&s->link);
}
static void probe_done(struct tevent_req *req)
{
	struct cancel_seen *o=tevent_req_callback_data_void(req);
	NTSTATUS status=NT_STATUS_OK;
	o->callbacks++;o->canceled=tevent_req_is_nterror(req,&status)&&NT_STATUS_EQUAL(status,NT_STATUS_CANCELLED);
	tevent_req_received(req);TALLOC_FREE(req);
}
static bool native_cancel_walk(connection_struct *conn)
{
	TALLOC_CTX *ctx=talloc_new(conn);
	struct tevent_context *ev=tevent_context_init(ctx);
	struct smbd_server_connection sc={0};
	struct cancel_probe *s;
	struct cancel_seen seen={0};
	struct tevent_req *req;
	files_struct *fsp=talloc_zero(ctx,files_struct);
	int sv[2];bool ok=false;
	if(ctx==NULL || ev==NULL || fsp==NULL || socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,sv)!=0){
		TALLOC_FREE(ctx);return false;
	}
	conn->sconn=&sc;sc.files=fsp;fsp->conn=conn;
	req=tevent_req_create(ctx,&s,struct cancel_probe);
	if(req==NULL)goto out;
	s->ev=ev;s->wait=wait_for_read_send(s,ev,sv[0],false);
	s->link=aio_add_req_to_fsp(fsp,req);
	if(s->wait==NULL || s->link==NULL || fsp->num_aio_requests!=1)goto out;
	talloc_steal(s,s->link);
	tevent_req_set_callback(req,probe_done,&seen);tevent_req_set_cancel_fn(req,probe_cancel);
	if(!tevent_req_cancel(fsp->aio_requests[0]) || fsp->num_aio_requests!=0 ||
	   seen.callbacks!=0 || s->wait!=NULL || s->link!=NULL)goto out;
	if(tevent_loop_once(ev)!=0 || seen.callbacks!=1 || !seen.canceled)goto out;
	ok=true;
out:
	if(!ok && req!=NULL && seen.callbacks==0)TALLOC_FREE(req);
	conn->sconn=NULL;close(sv[0]);close(sv[1]);TALLOC_FREE(ctx);return ok;
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
	if (!native_cancel_walk(conn) || !native_directory_projection(conn) || !only_the_attached_virtual_root(conn,lower)) { return 1; }
	TALLOC_FREE(conn);
	return 0;
}
