/* SPDX-License-Identifier: GPL-3.0-or-later
 * A mutation uses one originally owned member and one bounded local stream.
 * User bytes and a positive result are different phases; no VFS disk path.
 */
#include "locking/proto.h"
struct zxs_work_state {
	struct smbd_smb2_request *native;
	struct files_struct *fsp;
	struct zxs_read_fsp *member;
	struct aio_req_fsp_link *link;
	struct tevent_fd *watch;
	struct tevent_timer *deadline;
	const uint8_t *input;
	size_t sent, length, received;
	bool input_done, complete, settle;
	uint8_t command;
	uint64_t expected, eof;
	uint8_t reply[ZXS_WORK_REPLY];
	int fd;
};

static bool zxs_work_current(struct smbd_smb2_request *n,struct files_struct *fsp,
			      const struct zxs_read_fsp *member)
{
	uint64_t dev=0,ext=0;
	if(member==NULL || member!=zxs_find_read(fsp) || !member->writable || member->fd<0 ||
	   !zxs_request_folder_current(n,&member->folder) || member->folder.mode!=2 ||
	   n->tcon!=member->tcon || n->session!=member->session ||
	   n->session->global->auth_session_info_seqnum!=member->seq ||
	   fsp->conn!=n->tcon->compat || fsp->vuid!=n->session->global->session_wire_id ||
	   !fsp->fsp_flags.is_fsa || fsp->fsp_flags.is_pathref || !fsp->fsp_flags.can_write ||
	   !fsp->fsp_flags.can_lock || fsp->op==NULL || fsp->op->global==NULL ||
	   zxs_member_held(member->fd)!=0 ||
	   memcmp(n->tcon->zxs_tree_choice->pool,member->member.pool,16)!=0 ||
	   memcmp(member->folder.volume,member->member.volume,16)!=0 ||
	   !change_to_user_and_service(fsp->conn,fsp->vuid) || fsp->conn->read_only) {
		return false;
	}
	for(size_t i=0;i<8;i++){
		dev=(dev<<8)|member->member.pool[i];ext=(ext<<8)|member->member.pool[8+i];
	}
	return fsp->file_id.devid==dev && fsp->file_id.extid==ext &&
	       fsp->file_id.inode==member->member.number;
}
static bool zxs_work_brl(struct files_struct *fsp,uint64_t offset,uint64_t length)
{
	struct lock_struct lock;
	if(fsp->conn->params==NULL || !lp_locking(fsp->conn->params) ||
	   lp_strict_locking(fsp->conn->params)!=true || lp_posix_locking(fsp->conn->params))return false;
	if(length==0)return true;
	init_strict_lock_struct(fsp,fsp->op->global->open_persistent_id,offset,length,WRITE_LOCK,
				lp_posix_cifsu_locktype(fsp),&lock);
	return zxs_strict_lock_check(fsp,&lock);
}

static int zxs_work_release(struct zxs_work_state *s)
{
	TALLOC_FREE(s->watch);TALLOC_FREE(s->deadline);
	if(s->fd>=0){close(s->fd);s->fd=-1;}
	if(s->member!=NULL){
		if(!s->complete && s->member->fd>=0){
			/* A lost completion cannot leave a stale native size next to
			 * mutable storage state. Revoke only this exact FILE peer. */
			shutdown(s->member->fd,SHUT_RDWR);
			close(s->member->fd);s->member->fd=-1;
		}
		s->member->busy=false;s->member=NULL;
	}
	TALLOC_FREE(s->link); /* release the FSP's drain only after its last use */
	return 0;
}

static void zxs_work_finish(struct tevent_req *req,NTSTATUS status)
{
	struct zxs_work_state *s=tevent_req_data(req,struct zxs_work_state);
	zxs_work_release(s);
	if(tevent_req_nterror(req,status))return;
	tevent_req_done(req);
}

static void zxs_work_input_timeout(struct tevent_context *ev,struct tevent_timer *timer,
				   struct timeval current,void *private_data)
{
	struct tevent_req *req=talloc_get_type_abort(private_data,struct tevent_req);
	zxs_work_finish(req,NT_STATUS_IO_TIMEOUT);
}

static void zxs_work_io(struct tevent_context *ev,struct tevent_fd *fde,uint16_t flags,void *private_data)
{
	struct tevent_req *req=talloc_get_type_abort(private_data,struct tevent_req);
	struct zxs_work_state *s=tevent_req_data(req,struct zxs_work_state);
	ssize_t n;uint8_t status=0,extra=0;
	if(!s->input_done){
		if((flags&TEVENT_FD_WRITE)==0)goto reject;
		if(s->sent<s->length){
			n=send(s->fd,s->input+s->sent,s->length-s->sent,MSG_NOSIGNAL|MSG_DONTWAIT);
			if(n<0 && (errno==EAGAIN || errno==EINTR))return;
			if(n<=0)goto reject;
			s->sent+=(size_t)n;
			if(s->sent!=s->length)return;
		}
		if(shutdown(s->fd,SHUT_WR)!=0)goto reject;
		s->input_done=true;
		TALLOC_FREE(s->deadline); /* only input is timed, not a legitimate on-disk FLUSH */
		tevent_fd_set_flags(fde,TEVENT_FD_READ);return;
	}
	if((flags&TEVENT_FD_READ)==0)goto reject;
	const size_t left=sizeof(s->reply)-s->received;
	n=recv(s->fd,left!=0?s->reply+s->received:&extra,left!=0?left:1,MSG_DONTWAIT);
	if(n<0 && (errno==EAGAIN || errno==EINTR))return;
	if(n<0 || (left==0 && n!=0))goto reject;
	if(n!=0){s->received+=(size_t)n;return;}
	if(left!=0 || zxs_work_result(s->reply,&status,&s->eof)!=0)goto reject;
	if(status!=0){
		s->complete=true;
		zxs_work_finish(req,status==ZXS_WORK_BUSY?NT_STATUS_SHARING_VIOLATION:
			status==ZXS_WORK_FULL?NT_STATUS_DISK_FULL:
			status==ZXS_WORK_MALFORMED?NT_STATUS_INVALID_PARAMETER:NT_STATUS_ACCESS_DENIED);
		return;
	}
	if(s->eof!=s->expected || !zxs_work_current(s->native,s->fsp,s->member))goto reject;
	if(s->command==ZXS_WORK_FLUSH || s->settle){s->member->dirty=false;}
	else if((s->command==ZXS_WORK_WRITE && s->length!=0) ||
	        (s->command==ZXS_WORK_EOF && s->eof!=s->member->member.size)){s->member->dirty=true;}
	s->member->member.size=s->eof;
	s->fsp->fsp_name->st.st_ex_size=(off_t)s->eof;
	s->complete=true;
	zxs_work_finish(req,NT_STATUS_OK);return;
reject:
	zxs_work_finish(req,NT_STATUS_ACCESS_DENIED);
}

struct tevent_req *zxs_work_send(TALLOC_CTX *ctx,struct tevent_context *ev,
	struct tevent_req *outer,struct smbd_smb2_request *n,struct files_struct *fsp,
	uint8_t command,uint64_t offset,DATA_BLOB input,bool settle,bool closing)
{
	struct zxs_work_state *s;
	struct tevent_req *req=tevent_req_create(ctx,&s,struct zxs_work_state);
	struct zxs_read_fsp *member=zxs_find_read(fsp);
	uint64_t range_at=offset,range_len=input.length;
	NTSTATUS status=NT_STATUS_ACCESS_DENIED;
	if(req==NULL)return NULL;
	s->fd=-1;talloc_set_destructor(s,zxs_work_release);
	if(!zxs_work_current(n,fsp,member) || closing!=fsp->fsp_flags.closing)goto reject;
	if(member->busy){status=NT_STATUS_SHARING_VIOLATION;goto reject;}
	if(input.length>ZXS_READ_LIMIT || (input.length!=0&&input.data==NULL) ||
	   offset>(uint64_t)INT64_MAX-input.length){status=NT_STATUS_INVALID_PARAMETER;goto reject;}
	s->expected=member->member.size;
	switch(command){
	case ZXS_WORK_WRITE:
		if(closing || (fsp->access_mask & (SEC_FILE_WRITE_DATA|SEC_FILE_APPEND_DATA))==0 ||
		   ((fsp->access_mask&SEC_FILE_WRITE_DATA)==0 && offset!=s->expected))goto reject;
		if(input.length!=0)s->expected=MAX(s->expected,offset+input.length);
		break;
	case ZXS_WORK_EOF:
		if(closing || (fsp->access_mask&SEC_FILE_WRITE_DATA)==0 || input.length!=0 || settle)goto reject;
		range_at=MIN(offset,s->expected);
		range_len=MAX(offset,s->expected)-range_at;
		s->expected=offset;break;
	case ZXS_WORK_FLUSH:
		if(offset!=0 || input.length!=0 || settle ||
		   (fsp->access_mask & (SEC_FILE_WRITE_DATA|SEC_FILE_APPEND_DATA))==0)goto reject;
		break;
	default:goto reject;
	}
	if(!zxs_work_brl(fsp,range_at,range_len)){status=NT_STATUS_FILE_LOCK_CONFLICT;goto reject;}
	s->link=aio_add_req_to_fsp(fsp,outer);
	if(s->link==NULL){status=NT_STATUS_NO_MEMORY;goto reject;}
	talloc_steal(s,s->link);
	s->fd=zxs_work_query(member->fd,command,offset,input.length,settle);
	if(s->fd<0)goto reject;
	member->busy=true;s->native=n;s->fsp=fsp;s->member=member;s->input=input.data;s->length=input.length;
	s->command=command;s->settle=settle;
	s->watch=tevent_add_fd(ev,s,s->fd,TEVENT_FD_WRITE,zxs_work_io,req);
	s->deadline=tevent_add_timer(ev,s,timeval_current_ofs(10,0),zxs_work_input_timeout,req);
	if(s->watch==NULL || s->deadline==NULL){status=NT_STATUS_NO_MEMORY;goto reject;}
	if(smbd_smb2_is_compound(n) && !smbd_smb2_is_last_in_compound(n)){
		smb2_request_set_async_internal(n,true);
	}
	return req;
reject:
	zxs_work_finish(req,status);return tevent_req_post(req,ev);
}

NTSTATUS zxs_work_recv(struct tevent_req *req)
{
	return tevent_req_simple_recv_ntstatus(req);
}
